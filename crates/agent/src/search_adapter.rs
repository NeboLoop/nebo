use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use db::Store;
use tools::bot_tool::{HybridSearchResult, HybridSearcher, MemoryEmbedder};
use tracing::debug;
use turbovec::IdMapIndex;

use crate::search;

/// Adapter that bridges agent::search::hybrid_search to the HybridSearcher trait
/// defined in the tools crate, avoiding circular dependencies.
/// Lazy-loads a TurboVec index per user_id on first search.
pub struct HybridSearchAdapter {
    store: Arc<Store>,
    embedding_provider: Option<Arc<dyn ai::EmbeddingProvider>>,
    indexes: RwLock<HashMap<String, Arc<IdMapIndex>>>,
}

impl HybridSearchAdapter {
    pub fn new(
        store: Arc<Store>,
        embedding_provider: Option<Arc<dyn ai::EmbeddingProvider>>,
    ) -> Self {
        Self {
            store,
            embedding_provider,
            indexes: RwLock::new(HashMap::new()),
        }
    }

    fn get_or_load_index(&self, user_id: &str) -> Option<Arc<IdMapIndex>> {
        // Fast path: index already loaded
        if let Ok(map) = self.indexes.read() {
            if let Some(idx) = map.get(user_id) {
                return Some(idx.clone());
            }
        }

        // Slow path: load from DB
        let model = self.embedding_provider.as_ref()?.id().to_string();
        let index = search::load_vector_index(&self.store, user_id, &model)?;
        let index = Arc::new(index);

        if let Ok(mut map) = self.indexes.write() {
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
