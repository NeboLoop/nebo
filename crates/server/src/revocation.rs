//! A hub revocation (`tool_revoked`): NeboAI withdrew an artifact, and every
//! bot that installed it turns it off.
//!
//! The notice names the hub's artifact id. Each kind of install records that
//! id its own way, so the revoked item is found by it across every kind: an
//! employee or app is the agent row with that id, a skill is the folder whose
//! `.artifact_id` (or manifest `id`) is it, a store plugin is the one whose
//! manifest `id` is it, and a connector is the integrations recorded with it.
//! Before this the bot looked the id up only among running legacy tool
//! processes, keyed by manifest id, so nothing was ever found and a revoked
//! skill, employee, app or plugin stayed live.
//!
//! Turning off keeps everything the owner has: an employee or app is
//! deactivated (its worker and sidecar stop, its chats, memories and data
//! stay), a skill gets the quarantine marker and is no longer loaded, a
//! plugin's versions are quarantined (binary removed, data and accounts
//! kept) and it is disabled, and a connector's integrations are disabled
//! (credentials kept). The owner is told once, in plain words.

use std::path::{Path, PathBuf};

use tracing::{info, warn};

use crate::state::AppState;

/// What a revoked artifact id names here.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Found {
    /// `(id, name)` of the employee or app.
    pub employee: Option<(String, String)>,
    /// Skill folders carrying the id.
    pub skill_dirs: Vec<PathBuf>,
    /// Plugin slugs whose manifest carries the id.
    pub plugins: Vec<String>,
    /// MCP integration ids recorded with the id.
    pub connectors: Vec<String>,
}

impl Found {
    pub fn is_empty(&self) -> bool {
        self.employee.is_none() && self.skill_dirs.is_empty() && self.plugins.is_empty() && self.connectors.is_empty()
    }
}

/// Find everything installed here that `artifact_id` names. `skill_roots`
/// are the folders skills install into (marketplace and user).
pub(crate) fn find(
    store: &db::Store,
    plugins: &napp::plugin::PluginStore,
    skill_roots: &[PathBuf],
    artifact_id: &str,
) -> Found {
    let mut found = Found {
        employee: store.get_agent(artifact_id).ok().flatten().map(|a| (a.id, a.name)),
        ..Default::default()
    };
    for root in skill_roots {
        skill_dirs_with_id(root, artifact_id, 0, &mut found.skill_dirs);
    }
    for root in [plugins.plugins_dir(), plugins.user_plugins_dir()] {
        let Ok(entries) = std::fs::read_dir(root) else { continue };
        for entry in entries.flatten() {
            let Some(slug) = entry.file_name().to_str().map(str::to_owned) else { continue };
            if found.plugins.contains(&slug) {
                continue;
            }
            if plugins.get_manifest(&slug).is_some_and(|m| m.id == artifact_id) {
                found.plugins.push(slug);
            }
        }
    }
    found.connectors = store
        .list_mcp_integrations_by_artifact(artifact_id)
        .unwrap_or_default()
        .into_iter()
        .map(|i| i.id)
        .collect();
    found
}

/// Skill folders under `dir` whose install recorded `artifact_id`: the
/// `.artifact_id` sidecar every skill install writes, or an `id` in its
/// manifest.json (the two places a skill uninstall reads it from). Skills sit
/// at most a scope, a name and a version deep (`@acme/leads/1.0.0`).
fn skill_dirs_with_id(dir: &Path, artifact_id: &str, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 0 && records_id(dir, artifact_id) {
        out.push(dir.to_path_buf());
        return;
    }
    if depth >= 3 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && !entry.file_name().to_string_lossy().starts_with('.') {
            skill_dirs_with_id(&path, artifact_id, depth + 1, out);
        }
    }
}

fn records_id(dir: &Path, artifact_id: &str) -> bool {
    if std::fs::read_to_string(dir.join(".artifact_id")).is_ok_and(|id| id.trim() == artifact_id) {
        return true;
    }
    std::fs::read_to_string(dir.join("manifest.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .is_some_and(|v| v.get("id").and_then(|i| i.as_str()) == Some(artifact_id))
}

/// Mark a skill folder quarantined; the loader skips it from now on.
pub(crate) fn quarantine_skill(dir: &Path, reason: &str) -> std::io::Result<()> {
    std::fs::write(dir.join(tools::skills::QUARANTINE_MARKER), reason)
}

/// Quarantine every installed version of a plugin (its binary is removed,
/// its manifest, data and accounts stay).
pub(crate) fn quarantine_plugin(plugins: &napp::plugin::PluginStore, slug: &str, reason: &str) {
    for root in [plugins.plugins_dir(), plugins.user_plugins_dir()] {
        let Ok(entries) = std::fs::read_dir(root.join(slug)) else { continue };
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                if let Some(version) = entry.file_name().to_str() {
                    plugins.quarantine(slug, version, reason);
                }
            }
        }
    }
}

const REASON: &str = "revoked by NeboAI";

/// Turn off what a `tool_revoked` notice names, and tell the owner.
pub(crate) async fn revoke(state: &AppState, event: napp::InstallEvent) {
    let artifact_id = event.tool_id.clone();
    let mut skill_roots = Vec::new();
    for dir in [config::nebo_dir(), config::user_dir()].into_iter().flatten() {
        skill_roots.push(dir.join("skills"));
    }
    let found = find(&state.store, &state.plugin_store, &skill_roots, &artifact_id);

    // Legacy tool processes, keyed by manifest id.
    if let Err(e) = state.napp_registry.handle_install_event(event.clone()).await {
        warn!(artifact = %artifact_id, error = %e, "revoke: legacy tool registry");
    }
    if found.is_empty() {
        info!(artifact = %artifact_id, "revoke: nothing installed here carries this id");
        return;
    }

    if let Some((id, name)) = &found.employee {
        // The one deactivation (persisted off, worker and sidecar stopped,
        // off the owner's loop), the same as the owner's own Deactivate.
        let _ = crate::handlers::agents::deactivate_agent(
            axum::extract::State(state.clone()),
            axum::extract::Path(id.clone()),
        )
        .await;
        warn!(artifact = %artifact_id, employee = %name, "revoked: employee deactivated");
    }
    if !found.skill_dirs.is_empty() {
        for dir in &found.skill_dirs {
            if let Err(e) = quarantine_skill(dir, REASON) {
                warn!(artifact = %artifact_id, dir = %dir.display(), error = %e, "revoke: skill not quarantined");
            }
        }
        state.skill_loader.reload_from_disk().await;
        warn!(artifact = %artifact_id, "revoked: skill quarantined");
    }
    for slug in &found.plugins {
        quarantine_plugin(&state.plugin_store, slug, REASON);
        let _ = state.store.set_plugin_enabled(slug, false);
        state.hooks.unregister_app(slug);
        // A running copy keeps its old binary open: stop the shared bridge
        // and restart the workers using it, as a plugin update does.
        state.agent_workers.shared_bridges().stop(slug).await;
        for (agent_id, agent_name) in crate::codes::find_agents_using_plugin(&state.store, slug).await {
            state.agent_workers.start_agent(&agent_id, &agent_name, None).await;
        }
        warn!(artifact = %artifact_id, plugin = %slug, "revoked: plugin quarantined");
    }
    if !found.plugins.is_empty() {
        state.tools.refresh_plugin_tools().await;
        state.skill_loader.reload_from_disk().await;
    }
    for id in &found.connectors {
        let _ = state.store.update_mcp_integration(id, None, None, None, Some(false), None);
        state.bridge.disconnect(id).await;
        warn!(artifact = %artifact_id, integration = %id, "revoked: connector disabled");
    }

    tell_owner(state, &artifact_id, &event.payload, &found);
}

/// One notice per revoked artifact, in plain words.
fn tell_owner(state: &AppState, artifact_id: &str, payload: &serde_json::Value, found: &Found) {
    let name = payload
        .get("name")
        .and_then(|v| v.as_str())
        .filter(|n| !n.is_empty())
        .map(str::to_owned)
        .or_else(|| found.employee.as_ref().map(|(_, n)| n.clone()))
        .or_else(|| found.plugins.first().cloned())
        .unwrap_or_else(|| "An item you installed".to_string());
    let (title, body) = notice_words(&name);
    let id = format!("revoked:{artifact_id}");
    let action_url = match (&found.employee, found.plugins.is_empty()) {
        (Some((agent_id, _)), _) => Some(tools::owner_notify::link::employee(agent_id)),
        (None, false) => Some(tools::owner_notify::link::plugins()),
        _ => None,
    };
    let n = tools::owner_notify::OwnerNotification {
        id: &id,
        kind: "warning",
        title: &title,
        body: Some(&body),
        action_url: action_url.as_deref(),
        agent_id: None,
        loud: true,
    };
    tools::owner_notify::emit(&state.store, Some(&|ev, p| state.hub.broadcast(ev, p)), &n);
    crate::codes::push_inbox(state, n.hub_item(serde_json::json!({})));
}

/// The owner's words for a revocation.
fn notice_words(name: &str) -> (String, String) {
    (
        format!("{name} is turned off"),
        format!(
            "NeboAI withdrew {name} from the marketplace, so it is turned off here. Everything it saved is kept."
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, db::Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = db::Store::new(dir.path().join("test.db").to_str().unwrap()).unwrap();
        (dir, store)
    }

    // The hub names a revoked artifact by its id. Every kind of install
    // records that id its own way, and each is found by it.
    #[test]
    fn a_revoked_artifact_is_found_by_its_hub_id_whatever_its_kind() {
        let (tmp, store) = store();
        let id = "5b0c1f0e-0000-4000-8000-000000000001";

        // An employee or app: the agent row carries the id.
        store.create_agent(id, Some("AGNT-1"), "Lead Finder", "", "", "{}", None, None).unwrap();

        // A skill, versioned under a scope, with the install's sidecar.
        let skills = tmp.path().join("skills");
        let skill = skills.join("@acme").join("leads").join("1.0.0");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(skill.join("SKILL.md"), "---\nname: leads\n---\n").unwrap();
        std::fs::write(skill.join(".artifact_id"), format!("{id}\n")).unwrap();
        let other = skills.join("notes");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join(".artifact_id"), "someone-else").unwrap();

        // A store plugin: its manifest's id.
        let installed = tmp.path().join("plugins");
        let user = tmp.path().join("user-plugins");
        let v = installed.join("lead-api").join("0.2.0");
        std::fs::create_dir_all(&v).unwrap();
        std::fs::write(
            v.join("plugin.json"),
            format!(r#"{{"id":"{id}","slug":"lead-api","name":"Lead API","version":"0.2.0","platforms":{{}}}}"#),
        )
        .unwrap();
        let plugins = napp::plugin::PluginStore::new(installed.clone(), user, None);

        // A connector: integrations recorded with the id.
        store
            .create_mcp_integration("int-1", "Leads MCP", "http", Some("https://x.example"), "none", None, Some(id))
            .unwrap();

        let found = find(&store, &plugins, &[skills.clone()], id);
        assert_eq!(found.employee, Some((id.to_string(), "Lead Finder".to_string())));
        assert_eq!(found.skill_dirs, vec![skill.clone()]);
        assert_eq!(found.plugins, vec!["lead-api".to_string()]);
        assert_eq!(found.connectors, vec!["int-1".to_string()]);

        assert!(find(&store, &plugins, &[skills.clone()], "not-installed").is_empty());

        // Quarantined, a skill is no longer loaded and its files stay; a
        // plugin version loses its binary and keeps its manifest.
        quarantine_skill(&skill, REASON).unwrap();
        assert!(tools::skills::is_quarantined(&skill));
        assert!(skill.join("SKILL.md").exists());
        quarantine_plugin(&plugins, "lead-api", REASON);
        assert!(v.join(".quarantined").exists() && v.join("plugin.json").exists());
    }

    #[test]
    fn the_owner_is_told_plainly() {
        let (title, body) = notice_words("Lead Finder");
        assert_eq!(title, "Lead Finder is turned off");
        assert!(body.contains("withdrew Lead Finder") && body.contains("Everything it saved is kept"), "{body}");
        for word in ["restart", "offline", "sleep", "flap", "may be"] {
            assert!(!body.to_lowercase().contains(word), "{body}");
        }
    }
}
