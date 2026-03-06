use std::collections::HashMap;
use std::sync::Arc;

use ai::EmbeddingProvider;
use db::Store;
use tracing::{debug, warn};

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
pub async fn hybrid_search(
    store: &Arc<Store>,
    embedding_provider: Option<&dyn EmbeddingProvider>,
    query: &str,
    user_id: &str,
    config: &SearchConfig,
) -> Vec<SearchResult> {
    let query_class = classify_query(query);
    let (vector_weight, text_weight) = adaptive_weights(&query_class);

    let mut merged: HashMap<String, SearchResult> = HashMap::new();
    let fts_limit = (config.limit * 3) as i64;

    // 1. FTS5 on memories table
    if let Ok(fts_results) = store.search_memories_fts(query, user_id, fts_limit) {
        for (memory_id, rank) in &fts_results {
            let norm_score = normalize_bm25(*rank) * text_weight;
            if let Ok(Some(mem)) = store.get_memory(*memory_id) {
                let merge_key = format!("mem:{}", memory_id);
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
                let norm_score = normalize_bm25(*rank) * text_weight * dampening;

                // Merge by memory_id if available, else by chunk_id
                let merge_key = if let Some(mid) = memory_id {
                    format!("mem:{}", mid)
                } else {
                    format!("chunk:{}", chunk_id)
                };

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
                        ("session_chunk".to_string(), text.clone(), "session".to_string())
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

    // 3. Vector search (if embedding provider available)
    if let Some(provider) = embedding_provider {
        match provider.embed(&[query.to_string()]).await {
            Ok(query_vecs) if !query_vecs.is_empty() => {
                let query_vec = &query_vecs[0];
                let model = provider.id().to_string();

                if let Ok(all_embeddings) = store.get_all_embeddings_by_user(user_id, &model) {
                    for (chunk_id, blob) in &all_embeddings {
                        let stored_vec = ai::bytes_to_f32(blob);
                        let sim = cosine_similarity(query_vec, &stored_vec);
                        if sim < config.min_score {
                            continue;
                        }

                        let vector_score = sim * vector_weight;

                        if let Ok(Some((_, memory_id, text, _source))) =
                            store.get_memory_chunk(*chunk_id)
                        {
                            let merge_key = if let Some(mid) = memory_id {
                                format!("mem:{}", mid)
                            } else {
                                format!("chunk:{}", chunk_id)
                            };

                            let entry = merged.entry(merge_key).or_insert_with(|| {
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
                                    source: "vector".to_string(),
                                }
                            });
                            entry.score += vector_score;
                        }
                    }
                }
            }
            Ok(_) => debug!("empty embedding result for query"),
            Err(e) => warn!(error = %e, "vector search embedding failed, using text-only"),
        }
    }

    // Collect, filter by min_score, sort by score DESC, take top N
    let mut results: Vec<SearchResult> = merged
        .into_values()
        .filter(|r| r.score >= config.min_score)
        .collect();
    results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
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
    // BM25 ranks are negative in SQLite FTS5; more negative = better match
    // Convert: score = 1 / (1 + abs(rank))
    1.0 / (1.0 + rank.abs())
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
        assert!((normalize_bm25(0.0) - 1.0).abs() < 1e-6);
        // Negative rank => between 0 and 1
        let score = normalize_bm25(-5.0);
        assert!(score > 0.0 && score < 1.0);
        // More negative = better = higher score
        assert!(normalize_bm25(-10.0) < normalize_bm25(-5.0));
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
}
