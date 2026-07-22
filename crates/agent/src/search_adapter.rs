use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use db::Store;
use tools::bot_tool::{HybridSearchResult, HybridSearcher, MemoryEmbedder};
use tracing::{debug, info};
use turbovec::IdMapIndex;

use crate::search;

/// Process-wide per-user TurboVec index cache. Module-level (not per-adapter)
/// so the ONE chunk+embed pathway (`memory::embed_memories`) can invalidate a
/// user's entry when it persists new vectors — a per-instance cache went stale
/// within a server lifetime, making freshly stored memories invisible to
/// vector recall until restart. One Store per process, so keying by
/// memory-scope user_id is unambiguous (tests use unique user ids).
fn index_cache() -> &'static RwLock<HashMap<String, Arc<IdMapIndex>>> {
    static CACHE: OnceLock<RwLock<HashMap<String, Arc<IdMapIndex>>>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Drop a user's cached vector index so the next search rebuilds it from the
/// DB. Called by `memory::embed_memories` — the single point every write path
/// (auto-extraction, explicit tool store, flush, backfill) funnels through.
/// Rebuilds are cheap: TurboVec quantization is data-oblivious (no training),
/// so a rebuild is one SQLite scan of the user's embeddings.
pub fn invalidate_index(user_id: &str) {
    if let Ok(mut map) = index_cache().write() {
        if map.remove(user_id).is_some() {
            debug!(user_id, "invalidated TurboVec index after embed");
        }
    }
}

/// Boot pre-warm so the first chat's recall pays neither cold cost:
/// 1. one embedding call to spin up the provider (a cold provider — local
///    model load / gateway spin-up — dominated the observed ~19s first
///    recall; steady-state calls are ~hundreds of ms), and
/// 2. eager per-user ANN index builds for every user with stored embeddings
///    (otherwise built lazily inside the first search).
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
    let mut built = 0usize;
    for user_id in users {
        if let Some(index) = search::load_vector_index(store, &user_id, model) {
            if let Ok(mut map) = index_cache().write() {
                map.insert(user_id, Arc::new(index));
                built += 1;
            }
        }
    }
    info!(indexes = built, "vector index prewarm complete");
}

/// Adapter that bridges agent::search::hybrid_search to the HybridSearcher trait
/// defined in the tools crate, avoiding circular dependencies.
/// Lazy-loads a TurboVec index per user_id on first search (boot prewarm
/// usually gets there first); the shared cache is refreshed on embed writes.
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

    fn get_or_load_index(&self, user_id: &str) -> Option<Arc<IdMapIndex>> {
        // Fast path: index already loaded
        if let Ok(map) = index_cache().read() {
            if let Some(idx) = map.get(user_id) {
                return Some(idx.clone());
            }
        }

        // Slow path: load from DB
        let model = self.embedding_provider.as_ref()?.id().to_string();
        let index = search::load_vector_index(&self.store, user_id, &model)?;
        let index = Arc::new(index);

        if let Ok(mut map) = index_cache().write() {
            debug!(user_id, "cached TurboVec index");
            map.insert(user_id.to_string(), index.clone());
        }

        Some(index)
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

            let index = self.get_or_load_index(user_id);
            let index_ref = index.as_deref();

            let results = search::hybrid_search(
                &self.store,
                provider_ref,
                query,
                user_id,
                &config,
                index_ref,
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
        let path = std::env::temp_dir().join(format!(
            "nebo-index-freshness-test-{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
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

        let _ = std::fs::remove_file(&path);
    }
}
