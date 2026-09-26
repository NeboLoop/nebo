pub mod agent;
pub mod agent_loader;
pub mod app_data;
pub mod child_guard;
pub mod hooks;
pub mod manifest;
pub mod napp;
pub mod pack;
pub mod plugin;
pub mod plugin_runtime;
pub mod reader;
pub mod registry;
pub mod runtime;
pub mod sandbox;
pub mod sealed;
pub mod signing;
pub mod supervisor;
#[cfg(all(unix, any(test, feature = "test-sidecar")))]
pub mod test_sidecar;
pub mod user_agent;
pub mod version;

pub use agent_loader::{AgentFsEvent, AgentLoader, AgentSource, LoadedAgent};
pub use hooks::{HookCaller, HookDispatcher, HookType, register_plugin_hooks};
pub use manifest::{Manifest, ManifestSignature, QualifiedName};
pub use pack::{
    Pack, PackEntry, PackError, PackLaw, PackLayer, PackQuestion, PackRule, PackStandard,
    commit_change, copy_tree, load_pack, scan_packs, unified_diff, watch_packs,
};
pub use registry::{Registry, RegistryConfig};
pub use user_agent::{AgentPackage, AppFields, read_ui_files, write_user_agent};
pub use runtime::{Process, Runtime};
pub use plugin_runtime::PluginRuntime;
pub use signing::{RevocationChecker, SigningKeyProvider, builtin_verifying_key};

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
    #[error("plugin '{0}' not found")]
    PluginNotFound(String),
    #[error("plugin '{plugin}' has no binary for platform '{platform}'")]
    PluginPlatformUnavailable { plugin: String, platform: String },
    #[error("plugin download failed: {0}")]
    PluginDownloadFailed(String),
    #[error("plugin manifest validation failed: {0}")]
    PluginValidation(String),
    #[error("{0}")]
    Other(String),
}

impl NappError {
    /// A launch that failed this way fails the same way every time until the
    /// package itself changes: its program is missing, the system refuses to
    /// run it, or its manifest is damaged. Retrying cannot help; reinstalling
    /// can. Everything else (a slow start, a crash, a busy disk) may pass on
    /// the next try.
    pub fn is_permanent(&self) -> bool {
        matches!(self, NappError::NotFound(_) | NappError::Sandbox(_) | NappError::Manifest(_))
    }
}

/// Install event from NeboAI (MQTT/WebSocket).
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
