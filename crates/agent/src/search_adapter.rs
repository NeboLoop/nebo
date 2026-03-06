use std::sync::Arc;

use db::Store;
use tools::bot_tool::{HybridSearchResult, HybridSearcher};

use crate::search;

/// Adapter that bridges agent::search::hybrid_search to the HybridSearcher trait
/// defined in the tools crate, avoiding circular dependencies.
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

            let results =
                search::hybrid_search(&self.store, provider_ref, query, user_id, &config).await;

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
