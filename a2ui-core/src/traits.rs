use async_trait::async_trait;

use a2ui_types::common::CatalogId;
use a2ui_types::v09::catalog::Catalog;

/// Metadata about a catalog available from the provider.
#[derive(Debug, Clone)]
pub struct CatalogInfo {
    /// The unique catalog identifier.
    pub catalog_id: CatalogId,

    /// Optional human-readable description.
    pub description: Option<String>,
}

/// Trait for providing catalogs to the A2UI library.
///
/// Implemented by the agent harness or a plugin. The library calls into this
/// trait to discover available catalogs and retrieve their definitions.
pub trait CatalogProvider: Send + Sync {
    /// Returns metadata for all catalogs this provider can supply.
    fn available_catalogs(&self) -> Vec<CatalogInfo>;

    /// Returns the full typed catalog for the given ID, or `None` if not found.
    fn get_catalog(&self, id: &CatalogId) -> Option<Catalog>;

    /// Returns the raw JSON Schema for the given catalog ID.
    /// Used for validation and LLM prompt injection.
    fn get_catalog_schema(&self, id: &CatalogId) -> Option<serde_json::Value>;
}

/// Trait for sending messages to the A2UI client.
///
/// Implemented by the agent harness or transport layer. The library serializes
/// messages and hands them to this trait for delivery.
#[async_trait]
pub trait ClientTransport: Send + Sync {
    /// Send a serialized server-to-client message to the client.
    ///
    /// The `msg` parameter is the JSON-serialized bytes of a single A2UI message.
    async fn send_to_client(
        &self,
        msg: &[u8],
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
}
