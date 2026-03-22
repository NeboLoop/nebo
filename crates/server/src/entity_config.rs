//! Entity config resolution — layer per-entity overrides onto global defaults.

use std::collections::HashMap;

use db::models::{EntityConfig, Setting};
use serde::{Deserialize, Serialize};

/// Fully resolved config for an entity, with inheritance applied.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedEntityConfig {
    pub entity_type: String,
    pub entity_id: String,
    pub heartbeat_enabled: bool,
    pub heartbeat_interval_minutes: i64,
    pub heartbeat_content: String,
    pub heartbeat_window: Option<(String, String)>,
    pub permissions: HashMap<String, bool>,
    pub resource_grants: HashMap<String, String>,
    pub model_preference: Option<String>,
    pub personality_snippet: Option<String>,
    /// Which fields are overridden (not inherited) — for UI display.
    pub overrides: HashMap<String, bool>,
    /// Allowed filesystem paths — restricts file writes and shell to these directories.
    #[serde(default)]
    pub allowed_paths: Vec<String>,
}

/// Resolve entity config by layering overrides on global defaults.
///
/// - `entity` — per-entity row (may be None if no row exists)
/// - `settings` — global settings row
/// - `global_permissions` — parsed from user_profiles.tool_permissions
/// - `heartbeat_md` — contents of HEARTBEAT.md file
pub fn resolve(
    entity_type: &str,
    entity_id: &str,
    entity: Option<&EntityConfig>,
    settings: &Setting,
    global_permissions: &HashMap<String, bool>,
    heartbeat_md: &str,
) -> ResolvedEntityConfig {
    let mut overrides = HashMap::new();

    // Heartbeat enabled
    let heartbeat_enabled = match entity.and_then(|e| e.heartbeat_enabled) {
        Some(v) => {
            overrides.insert("heartbeatEnabled".into(), true);
            v != 0
        }
        None => settings.heartbeat_interval_minutes > 0,
    };

    // Heartbeat interval
    let heartbeat_interval_minutes = match entity.and_then(|e| e.heartbeat_interval_minutes) {
        Some(v) => {
            overrides.insert("heartbeatIntervalMinutes".into(), true);
            v
        }
        None => settings.heartbeat_interval_minutes,
    };

    // Heartbeat content
    let heartbeat_content = match entity.and_then(|e| e.heartbeat_content.clone()) {
        Some(v) => {
            overrides.insert("heartbeatContent".into(), true);
            v
        }
        None => heartbeat_md.to_string(),
    };

    // Heartbeat window
    let heartbeat_window = match (
        entity.and_then(|e| e.heartbeat_window_start.clone()),
        entity.and_then(|e| e.heartbeat_window_end.clone()),
    ) {
        (Some(start), Some(end)) => {
            overrides.insert("heartbeatWindow".into(), true);
            Some((start, end))
        }
        _ => None,
    };

    // Permissions: start from global, overlay entity-specific
    let mut permissions = global_permissions.clone();
    if let Some(entity_perms_json) = entity.and_then(|e| e.permissions.as_deref()) {
        if let Ok(entity_perms) =
            serde_json::from_str::<HashMap<String, bool>>(entity_perms_json)
        {
            overrides.insert("permissions".into(), true);
            for (k, v) in entity_perms {
                permissions.insert(k, v);
            }
        }
    }

    // Resource grants: default all to "inherit", overlay entity-specific
    let mut resource_grants: HashMap<String, String> = HashMap::new();
    resource_grants.insert("screen".into(), "inherit".into());
    resource_grants.insert("browser".into(), "inherit".into());
    if let Some(grants_json) = entity.and_then(|e| e.resource_grants.as_deref()) {
        if let Ok(grants) = serde_json::from_str::<HashMap<String, String>>(grants_json) {
            overrides.insert("resourceGrants".into(), true);
            for (k, v) in grants {
                resource_grants.insert(k, v);
            }
        }
    }

    // Model preference
    let model_preference = match entity.and_then(|e| e.model_preference.clone()) {
        Some(v) if !v.is_empty() => {
            overrides.insert("modelPreference".into(), true);
            Some(v)
        }
        _ => None,
    };

    // Personality snippet
    let personality_snippet = match entity.and_then(|e| e.personality_snippet.clone()) {
        Some(v) if !v.is_empty() => {
            overrides.insert("personalitySnippet".into(), true);
            Some(v)
        }
        _ => None,
    };

    // Allowed paths: restrict file/shell to these directories
    let allowed_paths = entity
        .and_then(|e| e.allowed_paths.clone())
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or_default();
    if !allowed_paths.is_empty() {
        overrides.insert("allowedPaths".into(), true);
    }

    ResolvedEntityConfig {
        entity_type: entity_type.to_string(),
        entity_id: entity_id.to_string(),
        heartbeat_enabled,
        heartbeat_interval_minutes,
        heartbeat_content,
        heartbeat_window,
        permissions,
        resource_grants,
        model_preference,
        personality_snippet,
        overrides,
        allowed_paths,
    }
}

/// Convenience: resolve entity config for chat dispatch.
/// Returns None if resolution fails (best-effort — chat proceeds without overrides).
pub fn resolve_for_chat(
    store: &db::Store,
    entity_type: &str,
    entity_id: &str,
) -> Option<ResolvedEntityConfig> {
    let settings = store.get_settings().ok()?.unwrap_or_else(|| Setting {
        id: 1,
        autonomous_mode: 0,
        auto_approve_read: 0,
        auto_approve_write: 0,
        auto_approve_bash: 0,
        heartbeat_interval_minutes: 0,
        comm_enabled: 0,
        comm_plugin: String::new(),
        developer_mode: 0,
        auto_update: 1,
        updated_at: 0,
    });

    let global_permissions: HashMap<String, bool> = store
        .get_user_profile()
        .ok()
        .flatten()
        .and_then(|p| p.tool_permissions)
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default();

    let heartbeat_md = config::data_dir()
        .ok()
        .map(|d| std::fs::read_to_string(d.join("HEARTBEAT.md")).unwrap_or_default())
        .unwrap_or_default();

    let entity = store.get_entity_config(entity_type, entity_id).ok().flatten();

    Some(resolve(
        entity_type,
        entity_id,
        entity.as_ref(),
        &settings,
        &global_permissions,
        &heartbeat_md,
    ))
}
