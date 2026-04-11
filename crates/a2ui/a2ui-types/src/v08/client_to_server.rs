use serde::{Deserialize, Serialize};

use crate::common::SurfaceId;

/// A single client-to-server event message in v0.8.
/// Must contain exactly one of `user_action` or `error`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientToServerMessage {
    /// Reports a user-initiated action from a component.
    #[serde(rename = "userAction", skip_serializing_if = "Option::is_none")]
    pub user_action: Option<UserAction>,

    /// Reports a client-side error.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<serde_json::Value>,
}

/// A user-initiated action event in v0.8.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserAction {
    /// The name of the action, taken from the component's action.name property.
    pub name: String,

    /// The ID of the surface where the event originated.
    #[serde(rename = "surfaceId")]
    pub surface_id: SurfaceId,

    /// The ID of the component that triggered the event.
    #[serde(rename = "sourceComponentId")]
    pub source_component_id: String,

    /// An ISO 8601 timestamp of when the event occurred.
    pub timestamp: String,

    /// Key-value pairs from the component's action.context, after resolving all data bindings.
    pub context: serde_json::Value,
}
