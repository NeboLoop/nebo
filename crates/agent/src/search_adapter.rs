use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use db::Store;
use tools::bot_tool::{HybridSearchResult, HybridSearcher, MemoryEmbedder};
use tracing::{debug, info};

use crate::search::{self, VectorIndex};

/// Process-wide TurboVec indexes, ONE per embedding model, each shared by
/// every memory scope and updated in place for the process lifetime (see
/// [`VectorIndex`]). Module-level (not per-adapter) so the ONE chunk+embed
/// pathway (`memory::embed_memories`) can mark a scope stale when it persists
/// new vectors — a per-instance cache went stale within a server lifetime,
/// making freshly stored memories invisible to vector recall until restart.
fn index_cache() -> &'static RwLock<HashMap<String, Arc<VectorIndex>>> {
    static CACHE: OnceLock<RwLock<HashMap<String, Arc<VectorIndex>>>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

/// The long-lived index for `model`, created empty on first use.
fn model_index(model: &str) -> Arc<VectorIndex> {
    if let Ok(map) = index_cache().read() {
        if let Some(idx) = map.get(model) {
            return idx.clone();
        }
    }
    let mut map = index_cache()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    map.entry(model.to_string()).or_default().clone()
}

/// Tell the cached indexes that a scope's vectors changed in the DB, so its
/// next search reconciles them in place (adds new chunk ids, removes deleted
/// ones — never a rebuild). Called by `memory::embed_memories` — the single
/// point every write path (auto-extraction, explicit tool store, flush,
/// backfill) funnels through — and by every delete path.
pub fn invalidate_index(user_id: &str) {
    if let Ok(map) = index_cache().read() {
        for idx in map.values() {
            idx.mark_stale(user_id);
        }
    }
}

/// Load (first use) or reconcile (stale) `user_id`'s scope of the `model`
/// index. Blocking SQLite scan + quantization, so off the runtime workers: run
/// inline it pins a runtime worker, and on a single-worker runtime (1-vCPU
/// cloud pod) that starves the HTTP accept loop until the liveness probe
/// kills the healthy server (2026-07-22 incident).
async fn sync_scope(store: &Arc<Store>, model: &str, user_id: &str) -> Arc<VectorIndex> {
    let index = model_index(model);
    let (idx, store, uid, model) = (
        index.clone(),
        store.clone(),
        user_id.to_string(),
        model.to_string(),
    );
    let _ = tokio::task::spawn_blocking(move || idx.sync_scope(&store, &uid, &model)).await;
    index
}

/// Boot pre-warm so the first chat's recall pays neither cold cost:
/// 1. one embedding call to spin up the provider (a cold provider — local
///    model load / gateway spin-up — dominated the observed ~19s first
///    recall; steady-state calls are ~hundreds of ms), and
/// 2. an eager load of every scope with stored embeddings into the model's
///    ANN index (otherwise loaded lazily inside the scope's first search).
/// Called after the boot backfill completes — including when the backfill had
/// nothing to do.
pub async fn prewarm(store: &Arc<Store>, provider: &dyn ai::EmbeddingProvider) {
    // Unique text each boot: the DB-backed embedding cache would otherwise
    // short-circuit the call and never touch (warm) the actual provider.
    let warmup = vec![format!(
        "nebo boot warmup {}",
        chrono::Utc::now().timestamp_millis()
    )];
    if let Err(e) = provider.embed(&warmup).await {
        debug!(error = %e, "embedding provider warmup failed");
    }

    let model = provider.id();
    let users = match store.list_embedding_user_ids(model) {
        Ok(u) => u,
        Err(e) => {
            debug!(error = %e, "index prewarm: failed to list embedding users");
            return;
        }
    };
    let scopes = users.len();
    for user_id in users {
        sync_scope(store, model, &user_id).await;
    }
    info!(scopes, "vector index prewarm complete");
}

/// Adapter that bridges agent::search::hybrid_search to the HybridSearcher trait
/// defined in the tools crate, avoiding circular dependencies.
/// Lazy-loads a user_id's scope into the model's TurboVec index on first
/// search (boot prewarm usually gets there first); embed and delete writes
/// mark the scope stale and its next search reconciles it in place.
pub struct HybridSearchAdapter {
    store: Arc<Store>,
    embedding_provider: Option<Arc<dyn ai::EmbeddingProvider>>,
}

impl HybridSearchAdapter {
    pub fn new(
        store: Arc<Store>,
        embedding_provider: Option<Arc<dyn ai::EmbeddingProvider>>,
    ) -> Self {
        Self {
            store,
            embedding_provider,
        }
    }

    async fn get_or_load_index(&self, user_id: &str) -> Option<Arc<VectorIndex>> {
        let model = self.embedding_provider.as_ref()?.id();
        Some(sync_scope(&self.store, model, user_id).await)
    }
}

impl HybridSearcher for HybridSearchAdapter {
    fn search<'a>(
        &'a self,
        query: &'a str,
        user_id: &'a str,
        limit: usize,
        min_score: Option<f64>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<HybridSearchResult>> + Send + 'a>>
    {
        Box::pin(async move {
            let mut config = search::SearchConfig {
                limit,
                ..Default::default()
            };
            if let Some(floor) = min_score {
                config.min_score = floor;
            }

            let provider_ref: Option<&dyn ai::EmbeddingProvider> =
                self.embedding_provider.as_deref();

            let index = self.get_or_load_index(user_id).await;

            let results = search::hybrid_search(
                &self.store,
                provider_ref,
                query,
                user_id,
                &config,
                index,
            )
            .await;

            results
                .into_iter()
                .map(|r| HybridSearchResult {
                    memory_id: r.memory_id,
                    key: r.key,
                    value: r.value,
                    namespace: r.namespace,
                    score: r.score,
                })
                .collect()
        })
    }
}

/// Adapter that bridges the tools crate's [`MemoryEmbedder`] hook to the ONE
/// chunk+embed pathway (`memory::embed_memories_async`), so explicit memory-tool
/// stores get the same background embedding treatment as automatic extraction.
/// Only constructed when an embedding provider exists (see server wiring).
pub struct MemoryEmbedAdapter {
    store: Arc<Store>,
    embedding_provider: Arc<dyn ai::EmbeddingProvider>,
}

impl MemoryEmbedAdapter {
    pub fn new(store: Arc<Store>, embedding_provider: Arc<dyn ai::EmbeddingProvider>) -> Self {
        Self {
            store,
            embedding_provider,
        }
    }
}

impl MemoryEmbedder for MemoryEmbedAdapter {
    fn embed(&self, namespace: &str, key: &str, user_id: &str) {
        crate::memory::embed_memories_async(
            self.store.clone(),
            self.embedding_provider.clone(),
            vec![(namespace.to_string(), key.to_string())],
            user_id.to_string(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Same 8-dim unit vector for every text: any query has cosine similarity
    /// 1.0 with any stored chunk, so the vector path finds everything the
    /// index contains — and ONLY what the index contains.
    struct ConstEmbedder;

    #[async_trait::async_trait]
    impl ai::EmbeddingProvider for ConstEmbedder {
        fn id(&self) -> &str {
            "const-test-embed"
        }
        fn dimensions(&self) -> usize {
            8
        }
        async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, ai::ProviderError> {
            Ok(texts.iter().map(|_| vec![1.0; 8]).collect())
        }
    }

    /// Regression test for the same-process staleness bug: a memory stored and
    /// embedded AFTER the user's index was cached must be findable by vector
    /// search without a server restart. The query shares no words with the
    /// stored value, so FTS cannot mask an index miss — only the vector path
    /// (through the cached index) can find it, exactly the live failure mode
    /// ("storage locker code" recalled only after restart).
    #[tokio::test]
    async fn test_embed_refreshes_cached_index_same_process() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index-freshness-test.db");
        let store = Arc::new(Store::new(&path.to_string_lossy()).unwrap());
        // Unique user id: the index cache is process-global, shared across tests.
        let user_id = "index-freshness-u1";
        let provider: Arc<dyn ai::EmbeddingProvider> = Arc::new(ConstEmbedder);
        let adapter = HybridSearchAdapter::new(store.clone(), Some(provider.clone()));

        // Store + embed memory A through the canonical pathway, then search so
        // the user's TurboVec index gets built and cached (containing only A).
        store
            .upsert_memory("tacit/general", "fact/alpha", "alpha content", None, None, user_id)
            .unwrap();
        crate::memory::embed_memories(
            &store,
            provider.as_ref(),
            &[("tacit/general".to_string(), "fact/alpha".to_string())],
            user_id,
        )
        .await;
        let results = adapter.search("alpha", user_id, 10, Some(0.0)).await;
        assert!(!results.is_empty(), "seed search should find memory A");

        // Store + embed memory B AFTER the index was cached.
        store
            .upsert_memory(
                "tacit/general",
                "fact/locker",
                "storage locker code 4417-echo-9",
                None,
                None,
                user_id,
            )
            .unwrap();
        crate::memory::embed_memories(
            &store,
            provider.as_ref(),
            &[("tacit/general".to_string(), "fact/locker".to_string())],
            user_id,
        )
        .await;
        let b = store
            .get_memory_by_key_and_user("tacit/general", "fact/locker", user_id)
            .unwrap()
            .unwrap();

        // Deliberately non-matching FTS query: only the vector index can
        // surface B. A stale cached index reproduces the pre-fix miss.
        let results = adapter.search("zzzq qqzz wwvv", user_id, 10, Some(0.0)).await;
        assert!(
            results.iter().any(|r| r.memory_id == Some(b.id)),
            "same-process vector search must see the newly embedded memory \
             (stale index cache); got: {results:?}"
        );
    }

    /// Same constant vector for any text, under a caller-chosen model id so a
    /// test owns its model's process-wide index.
    struct NamedEmbedder(&'static str);

    #[async_trait::async_trait]
    impl ai::EmbeddingProvider for NamedEmbedder {
        fn id(&self) -> &str {
            self.0
        }
        fn dimensions(&self) -> usize {
            8
        }
        async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, ai::ProviderError> {
            Ok(texts.iter().map(|_| vec![1.0; 8]).collect())
        }
    }

    async fn store_and_embed(
        store: &Arc<Store>,
        provider: &dyn ai::EmbeddingProvider,
        user_id: &str,
        key: &str,
    ) -> Vec<u64> {
        store
            .upsert_memory("tacit/general", key, &format!("{key} value"), None, None, user_id)
            .unwrap();
        crate::memory::embed_memories(
            store,
            provider,
            &[("tacit/general".to_string(), key.to_string())],
            user_id,
        )
        .await;
        let mem = store
            .get_memory_by_key_and_user("tacit/general", key, user_id)
            .unwrap()
            .unwrap();
        store
            .get_all_embeddings_by_user(user_id, provider.id())
            .unwrap()
            .into_iter()
            .filter(|(chunk_id, _)| {
                store
                    .get_memory_chunk(*chunk_id)
                    .ok()
                    .flatten()
                    .is_some_and(|(_, mid, _, _)| mid == Some(mem.id))
            })
            .map(|(chunk_id, _)| chunk_id as u64)
            .collect()
    }

    fn ann_ids(index: &VectorIndex, user_id: &str) -> Vec<u64> {
        index
            .search(user_id, &[1.0; 8], 100)
            .unwrap_or_default()
            .into_iter()
            .map(|(_, id)| id)
            .collect()
    }

    /// The rotation matrix lives inside the index instance, so "built once"
    /// means ONE instance per model for the whole process: loading many
    /// scopes and embedding after each load must keep serving from the same
    /// instance. The old per-scope cache dropped and rebuilt an index (and its
    /// 1536x1536 QR rotation) on every load and after every embed.
    #[tokio::test]
    async fn test_one_index_instance_across_scope_loads_and_embeds() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(&dir.path().join("once.db").to_string_lossy()).unwrap());
        let provider: Arc<dyn ai::EmbeddingProvider> = Arc::new(NamedEmbedder("rotation-once-test"));
        let adapter = HybridSearchAdapter::new(store.clone(), Some(provider.clone()));

        let first = store_and_embed(&store, provider.as_ref(), "once-u0", "fact/seed").await;
        adapter.search("seed", "once-u0", 10, Some(0.0)).await;
        let index = model_index(provider.id());
        assert_eq!(ann_ids(&index, "once-u0"), first);

        let mut scope_ids = Vec::new();
        for n in 1..=5 {
            let user = format!("once-u{n}");
            let ids = store_and_embed(&store, provider.as_ref(), &user, "fact/a").await;
            adapter.search("zzzq", &user, 10, Some(0.0)).await;
            let more = store_and_embed(&store, provider.as_ref(), &user, "fact/b").await;
            adapter.search("zzzq", &user, 10, Some(0.0)).await;
            assert!(
                Arc::ptr_eq(&index, &model_index(provider.id())),
                "scope load / embed #{n} replaced the model's index (rotation rebuilt)"
            );
            scope_ids.push((user, [ids, more].concat()));
        }

        // Scopes share the instance but never see each other's vectors.
        for (user, ids) in &scope_ids {
            let mut got = ann_ids(&index, user);
            got.sort_unstable();
            let mut want = ids.clone();
            want.sort_unstable();
            assert_eq!(got, want, "scope {user} must return exactly its own chunks");
        }
    }

    /// Embed → the cached instance serves the new chunk without a reload;
    /// delete → the chunk is gone from the index, not just filtered later.
    #[tokio::test]
    async fn test_index_updates_in_place_on_embed_and_delete() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::new(&dir.path().join("incr.db").to_string_lossy()).unwrap());
        let provider: Arc<dyn ai::EmbeddingProvider> = Arc::new(NamedEmbedder("incremental-test"));
        let adapter = HybridSearchAdapter::new(store.clone(), Some(provider.clone()));
        let user = "incr-u1";

        let a = store_and_embed(&store, provider.as_ref(), user, "fact/alpha").await;
        adapter.search("alpha", user, 10, Some(0.0)).await;
        let index = model_index(provider.id());
        assert_eq!(ann_ids(&index, user), a);

        let b = store_and_embed(&store, provider.as_ref(), user, "fact/locker").await;
        let results = adapter.search("zzzq qqzz", user, 10, Some(0.0)).await;
        assert!(Arc::ptr_eq(&index, &model_index(provider.id())), "embed must not reload");
        let got = ann_ids(&index, user);
        assert!(b.iter().all(|id| got.contains(id)), "new chunk missing: {got:?} vs {b:?}");
        assert!(a.iter().all(|id| got.contains(id)));
        let b_mem = store
            .get_memory_by_key_and_user("tacit/general", "fact/locker", user)
            .unwrap()
            .unwrap();
        assert!(results.iter().any(|r| r.memory_id == Some(b_mem.id)));

        // Delete the way the memory handler does, then let the next search
        // reconcile the scope.
        store.delete_memory(b_mem.id).unwrap();
        store.delete_chunks_for_memory(b_mem.id).unwrap();
        invalidate_index(user);
        adapter.search("zzzq qqzz", user, 10, Some(0.0)).await;
        assert!(Arc::ptr_eq(&index, &model_index(provider.id())), "delete must not reload");
        assert_eq!(ann_ids(&index, user), a, "deleted chunk must leave the index");
    }
}
