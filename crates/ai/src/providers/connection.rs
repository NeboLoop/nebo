//! A provider as one connection: one key the owner added, addressed by its
//! own id (`<kind>@<auth profile id>`), so two keys of one vendor — a company
//! key and a personal key — are told apart in a model string
//! (`anthropic@<profile>/claude-…`). Everything else is the wrapped provider's.

use std::sync::Arc;

use async_trait::async_trait;

use crate::types::{ChatRequest, EventReceiver, Provider, ProviderError};

/// The id a connection's provider answers to: `<kind>@<profile id>`.
pub fn connection_id(kind: &str, profile_id: &str) -> String {
    format!("{kind}@{profile_id}")
}

/// A provider under its connection's id.
pub struct Connection {
    id: String,
    profile_id: String,
    inner: Arc<dyn Provider>,
}

impl Connection {
    pub fn new(kind: &str, profile_id: &str, inner: Arc<dyn Provider>) -> Self {
        Self { id: connection_id(kind, profile_id), profile_id: profile_id.to_string(), inner }
    }
}

#[async_trait]
impl Provider for Connection {
    fn id(&self) -> &str {
        &self.id
    }
    fn display_name(&self) -> &str {
        self.inner.display_name()
    }
    fn profile_id(&self) -> &str {
        &self.profile_id
    }
    fn handles_tools(&self) -> bool {
        self.inner.handles_tools()
    }
    fn retryable(&self) -> bool {
        self.inner.retryable()
    }
    fn cancel_is_async(&self) -> bool {
        self.inner.cancel_is_async()
    }
    fn supports_vision(&self) -> bool {
        self.inner.supports_vision()
    }
    async fn stream(&self, req: &ChatRequest) -> Result<EventReceiver, ProviderError> {
        self.inner.stream(req).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_connection_answers_to_its_own_id() {
        let inner: Arc<dyn Provider> = Arc::new(crate::OllamaProvider::new("http://localhost:11434".into(), "m".into()));
        let c = Connection::new("ollama", "p1", inner);
        assert_eq!(c.id(), "ollama@p1");
        assert_eq!(c.profile_id(), "p1");
        assert_eq!(connection_id("anthropic", "abc"), "anthropic@abc");
    }
}
