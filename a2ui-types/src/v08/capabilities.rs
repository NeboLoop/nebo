use serde::{Deserialize, Serialize};

/// Client capabilities for v0.8, sent in A2A message metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClientCapabilities {
    /// The URIs of catalogs supported by the client.
    #[serde(rename = "supportedCatalogIds")]
    pub supported_catalog_ids: Vec<String>,

    /// Inline catalog definitions (only if the agent declares `acceptsInlineCatalogs: true`).
    #[serde(rename = "inlineCatalogs", skip_serializing_if = "Option::is_none")]
    pub inline_catalogs: Option<Vec<InlineCatalog>>,
}

/// An inline catalog definition provided by the client in v0.8.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InlineCatalog {
    /// Unique identifier for this catalog.
    #[serde(rename = "catalogId")]
    pub catalog_id: String,

    /// Component definitions, keyed by component type name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub components: Option<serde_json::Value>,

    /// Style definitions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub styles: Option<serde_json::Value>,
}
