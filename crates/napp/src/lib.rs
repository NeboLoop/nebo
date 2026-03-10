pub mod hooks;
pub mod manifest;
pub mod napp;
pub mod reader;
pub mod registry;
pub mod role;
pub mod role_loader;
pub mod runtime;
pub mod sandbox;
pub mod signing;
pub mod supervisor;
pub mod version;

pub use manifest::{Manifest, ManifestSignature, QualifiedName};
pub use registry::{Registry, RegistryConfig};
pub use runtime::{Process, Runtime};
pub use signing::{SigningKeyProvider, RevocationChecker};
pub use hooks::{HookDispatcher, HookCaller, HookType};

use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum NappError {
    #[error("manifest error: {0}")]
    Manifest(String),
    #[error("signing error: {0}")]
    Signing(String),
    #[error("extraction error: {0}")]
    Extraction(String),
    #[error("sandbox error: {0}")]
    Sandbox(String),
    #[error("runtime error: {0}")]
    Runtime(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("revoked: {0}")]
    Revoked(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Other(String),
}

/// Install event from NeboLoop (MQTT/WebSocket).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallEvent {
    #[serde(rename = "type")]
    pub event_type: String, // tool_installed, tool_updated, tool_uninstalled, tool_revoked
    pub tool_id: String,
    #[serde(default)]
    pub payload: serde_json::Value,
}

/// Quarantine event emitted when a tool is quarantined.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuarantineEvent {
    pub tool_id: String,
    pub reason: String,
}
