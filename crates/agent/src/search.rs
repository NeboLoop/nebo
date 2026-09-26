use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use ai::EmbeddingProvider;
use db::Store;
use tracing::{debug, warn};
use turbovec::IdMapIndex;

/// A search result with merged score.
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub memory_id: Option<i64>,
    pub chunk_id: Option<i64>,
    pub key: String,
    pub value: String,
    pub namespace: String,
    pub score: f64,
    pub source: String,
}

/// Configuration for hybrid search.
#[derive(Debug, Clone)]
pub struct SearchConfig {
    pub limit: usize,
    pub min_score: f64,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            limit: 20,
            min_score: 0.3,
        }
    }
}

/// Perform hybrid search combining FTS5 text search and vector similarity.
/// When a `vector_index` holding vectors for `user_id` is provided, uses
/// TurboVec ANN search instead of brute-force cosine scan over all embeddings.
pub async fn hybrid_search(
    store: &Arc<Store>,
    embedding_provider: Option<&dyn EmbeddingProvider>,
    query: &str,
    user_id: &str,
    config: &SearchConfig,
    vector_index: Option<Arc<VectorIndex>>,
) -> Vec<SearchResult> {
    let query_class = classify_query(query);
    // Embed the query up front: fusion weights depend on whether the vector
    // leg actually runs. A single-leg (FTS-only) search must not be scaled by
    // its fusion weight — that pushed every real match under any floor, which
    // is why recall historically ran floorless (min_score 0) and injected
    // whatever scored best, relevant or not.
    let query_vec: Option<Vec<f32>> = match embedding_provider {
        Some(provider) => match provider.embed(&[query.to_string()]).await {
            Ok(vecs) if !vecs.is_empty() => Some(vecs[0].clone()),
            Ok(_) => {
                debug!("empty embedding result for query");
                None
            }
            Err(e) => {
                warn!(error = %e, "vector search embedding failed, using text-only");
                None
            }
        },
        None => None,
    };
    let (vector_weight, text_weight) = if query_vec.is_some() {
        adaptive_weights(&query_class)
    } else {
        (0.0, 1.0)
    };

    let mut merged: HashMap<String, SearchResult> = HashMap::new();
    // Strongest UNWEIGHTED component per candidate (raw cosine or raw
    // normalized BM25). The relevance FILTER runs on this; the fused weighted
    // score only ORDERS. Filtering on the fused score structurally floored
    // any memory missing one leg — an unembedded memory tops out at
    // text_weight (0.2 on long queries) no matter how exact its text match.
    let mut strength: HashMap<String, f64> = HashMap::new();
    let fts_limit = (config.limit * 3) as i64;

    // 1. FTS5 on memories table — across the read-scope chain so an
    // agent-scoped search also surfaces owner-level facts.
    let scope_chain = crate::memory::memory_scope_chain(user_id);
    if let Ok(fts_results) = store.search_memories_fts(query, &scope_chain, fts_limit) {
        for (memory_id, rank) in &fts_results {
            let raw = normalize_bm25(*rank);
            let norm_score = raw * text_weight;
            if let Ok(Some(mem)) = store.get_memory(*memory_id) {
                let merge_key = format!("mem:{}", memory_id);
                let st = strength.entry(merge_key.clone()).or_insert(0.0);
                *st = st.max(raw);
                let entry = merged.entry(merge_key).or_insert_with(|| SearchResult {
                    memory_id: Some(*memory_id),
                    chunk_id: None,
                    key: mem.key.clone(),
                    value: mem.value.clone(),
                    namespace: mem.namespace.clone(),
                    score: 0.0,
                    source: "fts_memory".to_string(),
                });
                entry.score += norm_score;
            }
        }
    }

    // 2. FTS5 on memory_chunks table (0.6x dampening for session chunks)
    if let Ok(chunk_results) = store.search_chunks_fts(query, user_id, fts_limit) {
        for (chunk_id, rank) in &chunk_results {
            if let Ok(Some((_, memory_id, text, source))) = store.get_memory_chunk(*chunk_id) {
                let dampening = if source.as_deref() == Some("session") {
                    0.6
                } else {
                    1.0
                };
                let raw = normalize_bm25(*rank) * dampening;
                let norm_score = raw * text_weight;

                // Merge by memory_id if available, else by chunk_id
                let merge_key = if let Some(mid) = memory_id {
                    format!("mem:{}", mid)
                } else {
                    format!("chunk:{}", chunk_id)
                };
                let st = strength.entry(merge_key.clone()).or_insert(0.0);
                *st = st.max(raw);

                let entry = merged.entry(merge_key).or_insert_with(|| {
                    // Try to load the parent memory for key/namespace
                    let (key, value, namespace) = if let Some(mid) = memory_id {
                        store
                            .get_memory(mid)
                            .ok()
                            .flatten()
                            .map(|m| (m.key, m.value, m.namespace))
                            .unwrap_or_else(|| {
                                ("chunk".to_string(), text.clone(), "unknown".to_string())
                            })
                    } else {
                        (
                            "session_chunk".to_string(),
                            text.clone(),
                            "session".to_string(),
                        )
                    };

                    SearchResult {
                        memory_id,
                        chunk_id: Some(*chunk_id),
                        key,
                        value,
                        namespace,
                        score: 0.0,
                        source: "fts_chunk".to_string(),
                    }
                });
                entry.score += norm_score;
            }
        }
    }

    // 3. Vector search (when the query embedded above)
    if let Some(query_vec) = &query_vec {
        // Fast path: ANN search via TurboVec. Off the runtime workers — the
        // index lock can be held by a scope sync (see VectorIndex::sync_scope).
        let ann_hits = match vector_index {
            Some(index) => {
                let (uid, q, k) = (user_id.to_string(), query_vec.clone(), config.limit * 3);
                tokio::task::spawn_blocking(move || index.search(&uid, &q, k))
                    .await
                    .ok()
                    .flatten()
            }
            None => None,
        };
        if let Some(hits) = ann_hits {
            for (score, id) in hits {
                let sim = score as f64;
                if sim < config.min_score {
                    continue;
                }
                let chunk_id = id as i64;
                merge_vector_hit(store, &mut merged, &mut strength, chunk_id, sim, vector_weight);
            }
        } else {
            // Brute-force fallback: load all embeddings and cosine scan
            let model = embedding_provider
                .map(|p| p.id().to_string())
                .unwrap_or_default();
            if let Ok(all_embeddings) = store.get_all_embeddings_by_user(user_id, &model) {
                for (chunk_id, blob) in &all_embeddings {
                    let stored_vec = ai::bytes_to_f32(blob);
                    let sim = cosine_similarity(query_vec, &stored_vec);
                    if sim < config.min_score {
                        continue;
                    }
                    merge_vector_hit(store, &mut merged, &mut strength, *chunk_id, sim, vector_weight);
                }
            }
        }
    }

    // Collect, filter on component STRENGTH (raw, unweighted), order by the
    // fused score, take top N.
    let mut results: Vec<SearchResult> = merged
        .into_iter()
        .filter(|(k, _)| strength.get(k).copied().unwrap_or(0.0) >= config.min_score)
        .map(|(_, r)| r)
        .collect();
    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results.truncate(config.limit);
    results
}

/// Cosine similarity between two f32 vectors.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }

    let mut dot = 0.0f64;
    let mut norm_a = 0.0f64;
    let mut norm_b = 0.0f64;

    for (x, y) in a.iter().zip(b.iter()) {
        let x = *x as f64;
        let y = *y as f64;
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }

    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom == 0.0 {
        0.0
    } else {
        (dot / denom).clamp(-1.0, 1.0)
    }
}

/// Normalize BM25 rank (negative, lower = better) to 0..1 (higher = better).
pub fn normalize_bm25(rank: f64) -> f64 {
    // BM25 ranks are negative in SQLite FTS5; more negative = better match.
    // Map so a STRONGER match scores HIGHER: |rank| / (1 + |rank|).
    // The old 1/(1+|rank|) inverted the ordering — the weakest FTS hit
    // outscored the strongest, so recall surfaced the LEAST relevant memory
    // and the memory tool ranked its text results backwards.
    let a = rank.abs();
    a / (1.0 + a)
}

/// Query classification for adaptive weighting.
#[derive(Debug)]
enum QueryClass {
    ShortProperNoun,
    ShortGeneric,
    Medium,
    Long,
}

/// Classify a query by length and content.
fn classify_query(query: &str) -> QueryClass {
    let words: Vec<&str> = query.split_whitespace().collect();
    let word_count = words.len();

    // Check for proper nouns (capitalized words that aren't sentence-start)
    let has_proper_nouns = words
        .iter()
        .skip(1)
        .any(|w| w.chars().next().is_some_and(|c| c.is_uppercase()));

    // Also check first word if it's all caps or clearly a name
    let first_proper = words
        .first()
        .is_some_and(|w| w.len() > 1 && w.chars().all(|c| c.is_uppercase()));

    if word_count <= 3 && (has_proper_nouns || first_proper) {
        QueryClass::ShortProperNoun
    } else if word_count <= 3 {
        QueryClass::ShortGeneric
    } else if word_count <= 8 {
        QueryClass::Medium
    } else {
        QueryClass::Long
    }
}

/// Merge a single vector search hit into the results map.
fn merge_vector_hit(
    store: &Arc<Store>,
    merged: &mut HashMap<String, SearchResult>,
    strength: &mut HashMap<String, f64>,
    chunk_id: i64,
    raw_sim: f64,
    vector_weight: f64,
) {
    let vector_score = raw_sim * vector_weight;
    if let Ok(Some((_, memory_id, text, _source))) = store.get_memory_chunk(chunk_id) {
        let merge_key = if let Some(mid) = memory_id {
            format!("mem:{}", mid)
        } else {
            format!("chunk:{}", chunk_id)
        };
        let st = strength.entry(merge_key.clone()).or_insert(0.0);
        *st = st.max(raw_sim);

        let entry = merged.entry(merge_key).or_insert_with(|| {
            let (key, value, namespace) = if let Some(mid) = memory_id {
                store
                    .get_memory(mid)
                    .ok()
                    .flatten()
                    .map(|m| (m.key, m.value, m.namespace))
                    .unwrap_or_else(|| ("chunk".to_string(), text.clone(), "unknown".to_string()))
            } else {
                (
                    "session_chunk".to_string(),
                    text.clone(),
                    "session".to_string(),
                )
            };

            SearchResult {
                memory_id,
                chunk_id: Some(chunk_id),
                key,
                value,
                namespace,
                score: 0.0,
                source: "vector".to_string(),
            }
        });
        entry.score += vector_score;
    }
}

/// Threads the vector-index math may use. Building an index's rotation
/// matrix is a 1536x1536 QR decomposition (faer + gemm through rayon); on
/// rayon's global pool it took every core, and at machine load 94 the process
/// froze 22-49 s with rayon workers spinning — the tokio runtime starved, the
/// hub's 3 s tunnel probe failed and the tunnel reset (2026-09-26). Ceiling:
/// index work gets at most TWO cores, whatever the machine has.
pub const INDEX_THREADS: usize = 2;

/// The ONE pool every TurboVec call runs in (encode, rotation, search), so
/// faer/gemm/turbovec see `rayon::current_num_threads() == INDEX_THREADS`.
/// Dedicated rather than capping the global pool: other crates (the skills
/// loader) use rayon's global pool for their own work.
pub fn index_pool() -> &'static rayon::ThreadPool {
    static POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();
    POOL.get_or_init(|| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(INDEX_THREADS)
            .thread_name(|i| format!("nebo-vecindex-{i}"))
            .build()
            .expect("build vector index thread pool")
    })
}

/// ONE long-lived TurboVec index per embedding model, shared by every memory
/// scope (user_id). TurboVec builds its rotation matrix once per index
/// instance and keeps it for the index's lifetime, so a single instance that
/// is only ever updated in place pays that build once per process. The old
/// per-scope indexes, dropped and rebuilt after every embed, rebuilt it on
/// every load (31 scopes at boot, again after each new memory).
///
/// The DB stays the source of truth: a scope is loaded from it on first use,
/// and a scope marked stale (`mark_stale`, after any embed or delete) is
/// reconciled against it on its next use — only the changed chunk ids are
/// added or removed, the index itself is never rebuilt.
///
/// Every method but `mark_stale` blocks: call them from `spawn_blocking`.
pub struct VectorIndex {
    inner: RwLock<ScopedIndex>,
    stale: Mutex<HashSet<String>>,
}

struct ScopedIndex {
    /// 4-bit quantization, 8x compression. Dimension is locked by the first
    /// vector added.
    index: IdMapIndex,
    /// Chunk ids per memory scope. Chunk ids are global primary keys, so
    /// scopes never collide inside the shared index.
    scopes: HashMap<String, HashSet<u64>>,
}

impl Default for VectorIndex {
    fn default() -> Self {
        Self {
            inner: RwLock::new(ScopedIndex {
                index: IdMapIndex::new_lazy(4),
                scopes: HashMap::new(),
            }),
            stale: Mutex::new(HashSet::new()),
        }
    }
}

impl VectorIndex {
    /// Mark a scope's vectors as changed in the DB. Cheap and non-blocking;
    /// the reconcile happens on the scope's next `sync_scope`.
    pub fn mark_stale(&self, user_id: &str) {
        if let Ok(mut stale) = self.stale.lock() {
            stale.insert(user_id.to_string());
        }
    }

    /// Bring `user_id`'s slice of the index in line with the DB: a no-op when
    /// the scope is loaded and not stale, otherwise one scan of the scope's
    /// embeddings and an in-place add/remove of the differences.
    pub fn sync_scope(&self, store: &Store, user_id: &str, model: &str) {
        let was_stale = self
            .stale
            .lock()
            .map(|mut s| s.remove(user_id))
            .unwrap_or(false);
        let Ok(mut inner) = self.inner.write() else {
            return;
        };
        if !was_stale && inner.scopes.contains_key(user_id) {
            return;
        }
        // The DB read happens under the write lock so two syncs of one scope
        // can never apply snapshots out of order.
        let rows = match store.get_all_embeddings_by_user(user_id, model) {
            Ok(rows) => rows,
            Err(e) => {
                warn!(user_id, error = %e, "vector index: failed to read embeddings");
                if was_stale {
                    self.mark_stale(user_id);
                }
                return;
            }
        };
        let scoped: &mut ScopedIndex = &mut inner;
        index_pool().install(|| scoped.reconcile(user_id, rows));
    }

    /// Top-`k` (score, chunk_id) hits within `user_id`'s scope, or `None`
    /// when the scope holds no vectors (callers fall back to a brute scan).
    pub fn search(&self, user_id: &str, query: &[f32], k: usize) -> Option<Vec<(f32, u64)>> {
        let inner = self.inner.read().ok()?;
        if inner.index.dim_opt() != Some(query.len()) {
            return None;
        }
        let allow: Vec<u64> = inner.scopes.get(user_id)?.iter().copied().collect();
        if allow.is_empty() {
            return None;
        }
        let index = &inner.index;
        let (scores, ids) =
            index_pool().install(|| index.search_with_allowlist(query, k, Some(&allow)));
        Some(scores.into_iter().zip(ids).collect())
    }
}

impl ScopedIndex {
    fn reconcile(&mut self, user_id: &str, rows: Vec<(i64, Vec<u8>)>) {
        let mut scope = self.scopes.remove(user_id).unwrap_or_default();
        let db_ids: HashSet<u64> = rows.iter().map(|(id, _)| *id as u64).collect();

        let gone: Vec<u64> = scope.difference(&db_ids).copied().collect();
        for id in &gone {
            self.index.remove(*id);
            scope.remove(id);
        }

        // Batch every new vector into one add. The dimension checks happen
        // here because turbovec records ids before validating the dimension.
        let mut ids = Vec::new();
        let mut flat = Vec::new();
        let mut dims = self.index.dim_opt();
        for (chunk_id, blob) in &rows {
            let id = *chunk_id as u64;
            if scope.contains(&id) {
                continue;
            }
            let vec = ai::bytes_to_f32(blob);
            // TurboVec needs a non-zero multiple of 8; the first usable
            // vector locks the dimension.
            let usable = match dims {
                Some(d) => vec.len() == d,
                None if !vec.is_empty() && vec.len() % 8 == 0 => {
                    dims = Some(vec.len());
                    true
                }
                None => false,
            };
            if !usable {
                warn!(chunk_id, dims = vec.len(), "vector index: skipping vector with unusable dimension");
                continue;
            }
            ids.push(id);
            flat.extend_from_slice(&vec);
        }
        let added = ids.len();
        if let Some(d) = dims.filter(|_| added > 0) {
            match self.index.add_with_ids_2d(&flat, d, &ids) {
                Ok(()) => scope.extend(ids),
                Err(e) => warn!(user_id, error = ?e, "vector index: failed to add vectors"),
            }
        }

        debug!(
            user_id,
            added,
            removed = gone.len(),
            scope_vectors = scope.len(),
            total_vectors = self.index.len(),
            "synced TurboVec index scope"
        );
        self.scopes.insert(user_id.to_string(), scope);
    }
}

/// Adaptive weights: (vector_weight, text_weight) based on query class.
fn adaptive_weights(class: &QueryClass) -> (f64, f64) {
    match class {
        QueryClass::ShortProperNoun => (0.35, 0.65),
        QueryClass::ShortGeneric => (0.45, 0.55),
        QueryClass::Medium => (0.70, 0.30),
        QueryClass::Long => (0.80, 0.20),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity_identical() {
        let a = vec![1.0, 2.0, 3.0];
        let sim = cosine_similarity(&a, &a);
        assert!((sim - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        let sim = cosine_similarity(&a, &b);
        assert!(sim.abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_opposite() {
        let a = vec![1.0, 0.0];
        let b = vec![-1.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim + 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_empty() {
        let a: Vec<f32> = vec![];
        let b: Vec<f32> = vec![];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }

    #[test]
    fn test_cosine_similarity_mismatched_len() {
        let a = vec![1.0, 2.0];
        let b = vec![1.0];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }

    #[test]
    fn test_normalize_bm25() {
        // Rank of 0 => score 1.0
        // rank 0 = no match strength → floor of the scale, not the ceiling.
        assert!(normalize_bm25(0.0).abs() < 1e-6);
        // Negative rank => between 0 and 1
        let score = normalize_bm25(-5.0);
        assert!(score > 0.0 && score < 1.0);
        // More negative = better = higher score
        // More negative (better BM25 match) must score HIGHER.
        assert!(normalize_bm25(-10.0) > normalize_bm25(-5.0));
    }

    #[test]
    fn test_classify_short_proper_noun() {
        let class = classify_query("John Smith");
        assert!(matches!(class, QueryClass::ShortProperNoun));
    }

    #[test]
    fn test_classify_short_generic() {
        let class = classify_query("favorite color");
        assert!(matches!(class, QueryClass::ShortGeneric));
    }

    #[test]
    fn test_classify_medium() {
        let class = classify_query("what is the user's favorite programming language");
        assert!(matches!(class, QueryClass::Medium));
    }

    #[test]
    fn test_classify_long() {
        let class = classify_query(
            "tell me everything you know about the user's work history and career goals and aspirations for the future",
        );
        assert!(matches!(class, QueryClass::Long));
    }

    #[test]
    fn test_adaptive_weights_sum_to_one() {
        for class in [
            QueryClass::ShortProperNoun,
            QueryClass::ShortGeneric,
            QueryClass::Medium,
            QueryClass::Long,
        ] {
            let (v, t) = adaptive_weights(&class);
            assert!((v + t - 1.0).abs() < 1e-6);
        }
    }

    /// Index math runs in a pool that can never use more than INDEX_THREADS
    /// cores — faer/gemm size their work by `rayon::current_num_threads()`
    /// and every parallel iterator inside `install` stays in the pool.
    #[test]
    fn test_index_pool_caps_parallelism() {
        use rayon::prelude::*;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let active = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        let threads = index_pool().install(|| {
            (0..64).into_par_iter().for_each(|_| {
                let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                std::thread::sleep(std::time::Duration::from_millis(2));
                active.fetch_sub(1, Ordering::SeqCst);
            });
            rayon::current_num_threads()
        });
        assert_eq!(threads, INDEX_THREADS);
        assert!(
            peak.load(Ordering::SeqCst) <= INDEX_THREADS,
            "index work ran on {} threads at once",
            peak.load(Ordering::SeqCst)
        );
    }
}
