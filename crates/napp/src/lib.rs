pub mod agent;
pub mod agent_loader;
pub mod app_data;
pub mod app_view;
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
#[cfg(any(test, feature = "test-signing"))]
pub mod test_signing;
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
pub use user_agent::{AgentPackage, AppFields, free_agent_dir, read_ui_files, write_user_agent};
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

impl InstallEvent {
    /// An "installs" message as an install event. Until neboloop #428
    /// (2026-10-01) the hub sent update and revoke notices as
    /// `{"type":"skillUpdated"|"skillRevoked","skillId",...}`, and notices
    /// queued for an offline bot before then keep that shape when they are
    /// drained; they are read as `tool_updated` / `tool_revoked` with the
    /// fields renamed, so none is lost.
    pub fn parse(content: &str) -> Option<Self> {
        if let Ok(event) = serde_json::from_str::<Self>(content) {
            return Some(event);
        }
        let v: serde_json::Value = serde_json::from_str(content).ok()?;
        let event_type = match v.get("type")?.as_str()? {
            "skillUpdated" => "tool_updated",
            "skillRevoked" => "tool_revoked",
            _ => return None,
        };
        let tool_id = v.get("skillId")?.as_str()?.to_string();
        let mut payload = serde_json::Map::new();
        for (old, new) in [
            ("skillName", "name"),
            ("version", "version"),
            ("permissionsAdded", "permissions_added"),
            ("permissionsRemoved", "permissions_removed"),
            ("updatedAt", "updated_at"),
            ("reason", "reason"),
            ("revokedAt", "revoked_at"),
        ] {
            if let Some(value) = v.get(old) {
                payload.insert(new.to_string(), value.clone());
            }
        }
        Some(Self { event_type: event_type.to_string(), tool_id, payload: serde_json::Value::Object(payload) })
    }
}

#[cfg(test)]
mod install_event_tests {
    use super::InstallEvent;

    #[test]
    fn the_install_event_shape_reads_as_it_is() {
        let e = InstallEvent::parse(
            r#"{"type":"tool_updated","tool_id":"a1","payload":{"name":"CRM","version":"0.1.3","artifact_type":"app"},"signature":"s","key_id":"k"}"#,
        )
        .unwrap();
        assert_eq!((e.event_type.as_str(), e.tool_id.as_str()), ("tool_updated", "a1"));
        assert_eq!(e.payload["version"], "0.1.3");
    }

    // Notices queued before neboloop #428 drain in the old shape.
    #[test]
    fn a_notice_queued_before_428_is_still_read() {
        let up = InstallEvent::parse(
            r#"{"type":"skillUpdated","skillId":"a1","skillName":"CRM","version":"0.1.3","permissionsAdded":["network"],"permissionsRemoved":[],"updatedAt":"2026-09-30T10:00:00Z"}"#,
        )
        .unwrap();
        assert_eq!((up.event_type.as_str(), up.tool_id.as_str()), ("tool_updated", "a1"));
        assert_eq!(up.payload["name"], "CRM");
        assert_eq!(up.payload["version"], "0.1.3");
        assert_eq!(up.payload["permissions_added"][0], "network");
        assert_eq!(up.payload["updated_at"], "2026-09-30T10:00:00Z");

        let gone = InstallEvent::parse(
            r#"{"type":"skillRevoked","skillId":"a2","skillName":"Leads","reason":"malware","revokedAt":"2026-09-30T10:00:00Z"}"#,
        )
        .unwrap();
        assert_eq!((gone.event_type.as_str(), gone.tool_id.as_str()), ("tool_revoked", "a2"));
        assert_eq!(gone.payload["reason"], "malware");

        assert!(InstallEvent::parse(r#"{"type":"something_else","skillId":"a3"}"#).is_none());
        assert!(InstallEvent::parse("not json").is_none());
    }
}

/// Quarantine event emitted when a tool is quarantined.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuarantineEvent {
    pub tool_id: String,
    pub reason: String,
}
