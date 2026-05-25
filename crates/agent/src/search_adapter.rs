use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use db::Store;
use tools::bot_tool::{HybridSearchResult, HybridSearcher};
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
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<HybridSearchResult>> + Send + 'a>>
    {
        Box::pin(async move {
            let config = search::SearchConfig {
                limit,
                ..Default::default()
            };

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
                    key: r.key,
                    value: r.value,
                    namespace: r.namespace,
                    score: r.score,
                })
                .collect()
        })
    }
}
