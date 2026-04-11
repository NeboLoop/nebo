use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Schema for attaching the client data model to transport metadata.
/// Placed in the `a2uiClientDataModel` field of the metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientDataModel {
    /// Protocol version — always `"v0.9"`.
    pub version: String,

    /// A map of surface IDs to their current data models.
    pub surfaces: HashMap<String, serde_json::Value>,
}
