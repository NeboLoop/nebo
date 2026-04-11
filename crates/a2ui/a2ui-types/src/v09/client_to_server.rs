use serde::{Deserialize, Serialize};

use crate::common::SurfaceId;

/// A single client-to-server message in v0.9.
/// Must contain `version` and exactly one of `action` or `error`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientToServerMessage {
    /// Protocol version — always `"v0.9"`.
    pub version: String,

    /// Reports a user-initiated action from a component.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<ActionMessage>,

    /// Reports a client-side error.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorMessage>,
}

/// A user-initiated action event in v0.9.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionMessage {
    /// The name of the action.
    pub name: String,

    /// The ID of the surface where the action originated.
    #[serde(rename = "surfaceId")]
    pub surface_id: SurfaceId,

    /// The ID of the component that triggered the action.
    #[serde(rename = "sourceComponentId")]
    pub source_component_id: String,

    /// An ISO 8601 timestamp of when the event occurred.
    pub timestamp: String,

    /// Key-value pairs from the component's action context, after resolving data bindings.
    pub context: serde_json::Value,
}

/// A client-side error report in v0.9.
/// Supports both `VALIDATION_FAILED` and generic errors.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorMessage {
    /// The error code (e.g., `"VALIDATION_FAILED"`).
    pub code: String,

    /// The ID of the surface where the error occurred.
    #[serde(rename = "surfaceId")]
    pub surface_id: SurfaceId,

    /// A short description of the error.
    pub message: String,

    /// For `VALIDATION_FAILED`: the JSON pointer to the field that failed validation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}
