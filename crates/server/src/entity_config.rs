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
    /// The employee's folders (its folder rules).
    #[serde(default)]
    pub allowed_paths: Vec<String>,
    /// Whether this entity is pinned in the sidebar.
    #[serde(default)]
    pub pinned: bool,
    /// Whether this entity supports multiple concurrent chats.
    #[serde(default)]
    pub multi_chat: bool,
    /// Self-improvement mode: "auto" | "staged" | "off". None = off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub learning_mode: Option<String>,
}

/// An entity's permissions in the shapes today's settings pages show,
/// read from its rules (the one store). The employee Permissions page and
/// company defaults (WP2.15) replace these shapes.
#[derive(Debug, Clone, Default)]
pub struct PermissionView {
    /// Capability → allowed, company defaults overlaid by the employee's own.
    pub permissions: HashMap<String, bool>,
    /// "screen" / "browser" → "deny" | "inherit".
    pub resource_grants: HashMap<String, String>,
    /// The employee's folders.
    pub allowed_paths: Vec<String>,
    /// Which of the three the employee overrides.
    pub own_permissions: bool,
    pub own_grants: bool,
}

/// The rule scope an entity's permissions live at: the main assistant's are
/// the company defaults, an employee's are its own; a channel has none.
fn scope_of(entity_type: &str, entity_id: &str) -> Option<types::permissions::Scope> {
    match entity_type {
        "agent" => Some(types::permissions::Scope::Employee(entity_id.to_string())),
        "main" => Some(types::permissions::Scope::Company),
        _ => None,
    }
}

/// The screen keys a "screen: deny" setting denies, and the browser family.
const SCREEN_KEY: &str = "desktop_click";
const BROWSER_KEY: &str = "browser_*";
const SCREEN_KEYS: &[&str] = &[
    "desktop_click",
    "desktop_move_mouse",
    "desktop_key",
    "desktop_type",
    "desktop_scroll",
    "desktop_drag",
    "desktop_paste",
    "window*",
    "ui*",
    "menu*",
    "dialog*",
    "space*",
    "shortcut*",
];

/// The capability toggles one scope's rules state: a capability rule with
/// no field, allow or deny. A capability no rule names is on.
fn toggles_in(rules: &[types::permissions::Rule]) -> HashMap<String, bool> {
    rules
        .iter()
        .filter(|r| r.field.is_none())
        .filter_map(|r| match &r.key {
            types::permissions::RuleKey::Capability(c) => Some((c.clone(), r.effect == types::permissions::Effect::Allow)),
            _ => None,
        })
        .collect()
}

/// The company's capability toggles, every listed capability named.
pub fn company_toggles(store: &db::Store) -> HashMap<String, bool> {
    let rules = store.permission_rules_in(&types::permissions::Scope::Company).unwrap_or_default();
    let mut toggles: HashMap<String, bool> =
        tools::capabilities::CAPABILITIES.iter().map(|c| (c.key.to_string(), true)).collect();
    toggles.extend(toggles_in(&rules));
    toggles
}

/// The permission view of one entity.
pub fn permission_view(store: &db::Store, entity_type: &str, entity_id: &str) -> PermissionView {
    let mut view = PermissionView { permissions: company_toggles(store), ..Default::default() };
    view.resource_grants.insert("screen".into(), "inherit".into());
    view.resource_grants.insert("browser".into(), "inherit".into());
    let Some(scope @ types::permissions::Scope::Employee(_)) = scope_of(entity_type, entity_id) else {
        return view;
    };
    let own = store.permission_rules_in(&scope).unwrap_or_default();
    let toggles = toggles_in(&own);
    view.own_permissions = !toggles.is_empty();
    view.permissions.extend(toggles);
    let denied = |key: &str| {
        own.iter().any(|r| {
            r.effect == types::permissions::Effect::Deny
                && r.key == types::permissions::RuleKey::Tool(key.to_string())
        })
    };
    for (grant, key) in [("screen", SCREEN_KEY), ("browser", BROWSER_KEY)] {
        if denied(key) {
            view.resource_grants.insert(grant.into(), "deny".into());
            view.own_grants = true;
        }
    }
    view.allowed_paths = types::permissions::folders_of(&own, &[])
        .into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    view
}

/// Write a settings page's permission edits as rules, the owner's own, and
/// take them out of `patch` (`permissions`, `resourceGrants`,
/// `allowedPaths`, `operationPolicy`). A locked rule refuses the edit.
pub fn apply_permission_patch(
    store: &db::Store,
    entity_type: &str,
    entity_id: &str,
    patch: &mut serde_json::Value,
) -> Result<(), types::permissions::RuleError> {
    use types::permissions::{Effect, MoneyLimit, Rule, RuleField, RuleKey, RuleSource, Writer};
    let Some(obj) = patch.as_object_mut() else { return Ok(()) };
    let taken: Vec<(String, serde_json::Value)> = ["permissions", "resourceGrants", "allowedPaths", "operationPolicy"]
        .iter()
        .filter_map(|k| obj.remove(*k).map(|v| (k.to_string(), v)))
        .collect();
    let Some(scope) = scope_of(entity_type, entity_id) else { return Ok(()) };
    // A value may arrive as JSON or as a JSON string (the old column form).
    let parse = |v: serde_json::Value| -> serde_json::Value {
        match v {
            serde_json::Value::String(s) => serde_json::from_str(&s).unwrap_or(serde_json::Value::Null),
            other => other,
        }
    };
    let rule = |key: RuleKey, field: Option<RuleField>, effect: Effect, money: Option<MoneyLimit>| Rule {
        id: uuid::Uuid::new_v4().to_string(),
        scope: scope.clone(),
        key,
        field,
        effect,
        money,
        source: RuleSource::Owner,
        locked: false,
        created_at: chrono::Utc::now().timestamp(),
    };
    let own = store.permission_rules_in(&scope).map_err(|e| types::permissions::RuleError::Store(e.to_string()))?;
    let remove_where = |pred: &dyn Fn(&Rule) -> bool| -> Result<(), types::permissions::RuleError> {
        for r in own.iter().filter(|r| !r.locked && pred(r)) {
            store.remove_permission_rule(&r.id, &Writer::Owner)?;
        }
        Ok(())
    };
    for (key, value) in taken {
        let value = parse(value);
        match key.as_str() {
            "permissions" => {
                let toggles: HashMap<String, bool> = serde_json::from_value(value).unwrap_or_default();
                for (cap, on) in toggles.into_iter().filter(|(c, _)| c != "chat") {
                    let effect = if on { Effect::Allow } else { Effect::Deny };
                    store.write_permission_rule(&rule(RuleKey::Capability(cap), None, effect, None), &Writer::Owner)?;
                }
            }
            "resourceGrants" => {
                let grants: HashMap<String, String> = serde_json::from_value(value).unwrap_or_default();
                for (grant, keys) in [("screen", SCREEN_KEYS), ("browser", &[BROWSER_KEY][..])] {
                    let Some(setting) = grants.get(grant) else { continue };
                    if setting == "deny" {
                        for k in keys {
                            store.write_permission_rule(&rule(RuleKey::Tool(k.to_string()), None, Effect::Deny, None), &Writer::Owner)?;
                        }
                    } else {
                        remove_where(&|r: &Rule| {
                            r.effect == Effect::Deny
                                && r.field.is_none()
                                && matches!(&r.key, RuleKey::Tool(k) if keys.contains(&k.as_str()))
                        })?;
                    }
                }
            }
            "allowedPaths" => {
                let folders: Vec<String> = serde_json::from_value(value).unwrap_or_default();
                remove_where(&|r: &Rule| {
                    matches!(&r.field, Some(RuleField::Folder(p)) if !folders.contains(&p.to_string_lossy().into_owned()))
                })?;
                for folder in folders {
                    store.write_permission_rule(
                        &rule(RuleKey::Capability("file".into()), Some(RuleField::Folder(folder.into())), Effect::Allow, None),
                        &Writer::Owner,
                    )?;
                }
            }
            "operationPolicy" => {
                let operations: HashMap<String, serde_json::Value> = value
                    .get("operations")
                    .cloned()
                    .and_then(|o| serde_json::from_value(o).ok())
                    .unwrap_or_default();
                let suffixes: Vec<String> = operations.keys().map(|op| tools::plugin_tool::port_suffix(op)).collect();
                remove_where(&|r: &Rule| {
                    matches!(&r.key, RuleKey::Operation(op) if !suffixes.contains(op)) && r.field.is_none()
                })?;
                for (op, setting) in operations {
                    let (access, bounds) = match &setting {
                        serde_json::Value::String(a) => (a.clone(), None),
                        other => (
                            other.get("access").and_then(|a| a.as_str()).unwrap_or("approval").to_string(),
                            other.get("bounds").cloned(),
                        ),
                    };
                    let effect = match access.as_str() {
                        "always" => Effect::Allow,
                        "blocked" => Effect::Deny,
                        _ => Effect::Ask,
                    };
                    let money = bounds.filter(|_| effect == Effect::Allow).map(|b| MoneyLimit {
                        per_action_cents: b.get("max_amount_cents").and_then(|v| v.as_i64()),
                        per_day_cents: b.get("per_day_cents").and_then(|v| v.as_i64()),
                        per_day_count: b.get("per_day_count").and_then(|v| v.as_i64()),
                        per_counterparty_day_cents: b.get("per_counterparty_day_cents").and_then(|v| v.as_i64()),
                    });
                    let key = RuleKey::Operation(tools::plugin_tool::port_suffix(&op));
                    store.write_permission_rule(&rule(key, None, effect, money), &Writer::Owner)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Resolve entity config by layering overrides on global defaults.
///
/// - `entity` — per-entity row (may be None if no row exists)
/// - `settings` — global settings row
/// - `view` — the entity's permissions, read from its rules
/// - `heartbeat_md` — contents of HEARTBEAT.md file
pub fn resolve(
    entity_type: &str,
    entity_id: &str,
    entity: Option<&EntityConfig>,
    settings: &Setting,
    view: PermissionView,
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

    // Permissions, resource grants and folders: from the rules.
    let PermissionView { permissions, resource_grants, allowed_paths, own_permissions, own_grants } = view;
    if own_permissions {
        overrides.insert("permissions".into(), true);
    }
    if own_grants {
        overrides.insert("resourceGrants".into(), true);
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

    if !allowed_paths.is_empty() {
        overrides.insert("allowedPaths".into(), true);
    }

    // Pinned
    let pinned = entity
        .and_then(|e| e.pinned)
        .map(|v| v != 0)
        .unwrap_or(false);

    // Multi-chat
    let multi_chat = entity
        .and_then(|e| e.multi_chat)
        .map(|v| v != 0)
        .unwrap_or(false);

    // Self-improvement mode (per-employee only; no global layer).
    let learning_mode = entity.and_then(|e| e.learning_mode.clone());
    if learning_mode.is_some() {
        overrides.insert("learningMode".into(), true);
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
        pinned,
        multi_chat,
        learning_mode,
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
        auto_install_deps: 0,
        auto_approve_read: 0,
        auto_approve_write: 0,
        auto_approve_bash: 0,
        heartbeat_interval_minutes: 0,
        comm_enabled: 0,
        comm_plugin: String::new(),
        developer_mode: 0,
        auto_update: 1,
        full_access: 0,
        guardrails: serde_json::json!({}),
        updated_at: 0,
    });

    let heartbeat_md = config::data_dir()
        .ok()
        .map(|d| std::fs::read_to_string(d.join("HEARTBEAT.md")).unwrap_or_default())
        .unwrap_or_default();

    let entity = store
        .get_entity_config(entity_type, entity_id)
        .ok()
        .flatten();

    Some(resolve(
        entity_type,
        entity_id,
        entity.as_ref(),
        &settings,
        permission_view(store, entity_type, entity_id),
        &heartbeat_md,
    ))
}
