use serde::{Deserialize, Serialize};

use super::catalog::Catalog;

// ---------------------------------------------------------------------------
// Client Capabilities
// ---------------------------------------------------------------------------

/// Client capabilities for v0.9, sent in A2A message metadata under `a2uiClientCapabilities`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClientCapabilities {
    /// The capabilities structure for v0.9.
    #[serde(rename = "v0.9")]
    pub v09: ClientCapabilitiesV09,
}

/// The inner capabilities object for v0.9.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClientCapabilitiesV09 {
    /// URIs of component and function catalogs supported by the client.
    #[serde(rename = "supportedCatalogIds")]
    pub supported_catalog_ids: Vec<String>,

    /// Inline catalog definitions (only if the agent declares `acceptsInlineCatalogs: true`).
    #[serde(rename = "inlineCatalogs", skip_serializing_if = "Option::is_none")]
    pub inline_catalogs: Option<Vec<Catalog>>,
}

// ---------------------------------------------------------------------------
// Server Capabilities
// ---------------------------------------------------------------------------

/// Server capabilities for v0.9, advertised via Agent Card or transport initialization.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerCapabilities {
    /// The server capabilities structure for v0.9.
    #[serde(rename = "v0.9")]
    pub v09: ServerCapabilitiesV09,
}

/// The inner server capabilities object for v0.9.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerCapabilitiesV09 {
    /// Catalog IDs that the server can generate UI for.
    #[serde(
        rename = "supportedCatalogIds",
        skip_serializing_if = "Option::is_none"
    )]
    pub supported_catalog_ids: Option<Vec<String>>,

    /// Whether the server accepts inline catalogs from the client.
    #[serde(
        rename = "acceptsInlineCatalogs",
        skip_serializing_if = "Option::is_none"
    )]
    pub accepts_inline_catalogs: Option<bool>,
}
