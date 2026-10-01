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
//!
//! Only what is still on is turned off, so a revoke is quiet the second
//! time. That is what lets the startup sweep ([`spawn_sweep`]) run through
//! the same path: a revoke drained to a bot older than #543 was lost, and
//! revoke did nothing on 0.16.5 in any shape, so once per start the bot asks
//! the hub what it has withdrawn and revokes each one here.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tracing::{debug, info, warn};

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

/// Find everything installed here, and still on, that `artifact_id` names:
/// an enabled employee or app, a skill not quarantined, a plugin with a
/// version not quarantined, an enabled connector. `skill_roots` are the
/// folders skills install into (marketplace and user).
pub(crate) fn find(
    store: &db::Store,
    plugins: &napp::plugin::PluginStore,
    skill_roots: &[PathBuf],
    artifact_id: &str,
) -> Found {
    let mut found = Found {
        employee: store
            .get_agent(artifact_id)
            .ok()
            .flatten()
            .filter(|a| a.is_enabled != 0)
            .map(|a| (a.id, a.name)),
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
            if plugins.get_manifest(&slug).is_some_and(|m| m.id == artifact_id) && plugin_on(plugins, &slug) {
                found.plugins.push(slug);
            }
        }
    }
    found.connectors = store
        .list_mcp_integrations_by_artifact(artifact_id)
        .unwrap_or_default()
        .into_iter()
        .filter(|i| i.is_enabled != Some(0))
        .map(|i| i.id)
        .collect();
    found
}

/// A plugin is on while any installed version of it is not quarantined.
fn plugin_on(plugins: &napp::plugin::PluginStore, slug: &str) -> bool {
    [plugins.plugins_dir(), plugins.user_plugins_dir()].into_iter().any(|root| {
        std::fs::read_dir(root.join(slug)).is_ok_and(|entries| {
            entries.flatten().any(|e| e.path().is_dir() && !e.path().join(".quarantined").exists())
        })
    })
}

/// Skill folders under `dir` whose install recorded `artifact_id`: the
/// `.artifact_id` sidecar every skill install writes, or an `id` in its
/// manifest.json (the two places a skill uninstall reads it from), and not
/// quarantined already. Skills sit at most a scope, a name and a version
/// deep (`@acme/leads/1.0.0`).
fn skill_dirs_with_id(dir: &Path, artifact_id: &str, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 0 && records_id(dir, artifact_id) {
        if !tools::skills::is_quarantined(dir) {
            out.push(dir.to_path_buf());
        }
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
        debug!(artifact = %artifact_id, "revoke: nothing on here carries this id");
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

/// How long after a start the sweep asks the hub, plus up to as long again
/// at random, so bots started together do not all ask at one moment.
const SWEEP_AFTER_START: Duration = Duration::from_secs(60);

/// Once per start (an upgrade is a start), sweep for what the hub withdrew
/// while this bot could not hear it. In the background: nothing waits on
/// it, and a failure is logged and dropped.
pub(crate) fn spawn_sweep(state: AppState) {
    tokio::spawn(async move {
        tokio::time::sleep(SWEEP_AFTER_START + comm::reconnect::random_up_to(SWEEP_AFTER_START)).await;
        // Only the running copy that holds the bot changes anything.
        comm::lease::process().granted_or_unleased().await;
        while crate::codes::neboai_token(&state).is_none() {
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
        sweep(&state).await;
    });
}

/// Ask the hub what it has withdrawn from the marketplace and revoke each
/// through [`revoke`], which turns off what is on here and tells the owner.
/// Nothing withdrawn is on here: nothing happens and nothing is shown.
pub(crate) async fn sweep(state: &AppState) {
    let api = match crate::codes::build_api_client(state) {
        Ok(api) => api,
        Err(e) => {
            debug!(error = %e, "revocation sweep: not connected to NeboAI");
            return;
        }
    };
    let withdrawn = match api.list_revocations().await {
        Ok(list) => list,
        Err(e) => {
            info!(error = %e, "revocation sweep: the hub's withdrawn list could not be read; nothing swept");
            return;
        }
    };
    for item in withdrawn {
        let event = napp::InstallEvent {
            event_type: "tool_revoked".to_string(),
            tool_id: item.id,
            payload: serde_json::json!({ "name": item.name }),
        };
        revoke(state, event).await;
    }
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
        assert!(find(&store, &plugins, &[skills.clone()], id).skill_dirs.is_empty(), "a quarantined skill is off");
        assert!(skill.join("SKILL.md").exists());
        quarantine_plugin(&plugins, "lead-api", REASON);
        assert!(v.join(".quarantined").exists() && v.join("plugin.json").exists());
        assert!(find(&store, &plugins, &[skills.clone()], id).plugins.is_empty(), "a quarantined plugin is off");
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

    /// What one proof installs of a kind, as each kind's install records it.
    enum Installed {
        Employee(String),
        Skill(PathBuf),
        Plugin(PathBuf),
        Connector(String),
    }

    /// Put an item of `kind` with hub id `id` in place on the proof server
    /// the way its install leaves it.
    fn install(state: &AppState, kind: &str, id: &str, name: &str) -> Installed {
        match kind {
            "employee" | "app" => {
                state.store.create_agent(id, None, name, "", "", "{}", None, None).unwrap();
                if kind == "app" {
                    state.store.set_agent_app_fields(id, true, None, None, None).unwrap();
                }
                Installed::Employee(id.to_string())
            }
            "skill" => {
                let dir = config::nebo_dir().unwrap().join("skills").join(name.to_lowercase().replace(' ', "-"));
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(dir.join("SKILL.md"), format!("---\nname: {name}\ndescription: {name}\n---\n\nSteps.\n")).unwrap();
                std::fs::write(dir.join(".artifact_id"), id).unwrap();
                Installed::Skill(dir)
            }
            "plugin" => {
                let slug = name.to_lowercase().replace(' ', "-");
                let v = state.plugin_store.plugins_dir().join(&slug).join("1.0.0");
                std::fs::create_dir_all(&v).unwrap();
                std::fs::write(
                    v.join("plugin.json"),
                    serde_json::json!({ "id": id, "slug": slug, "name": name, "version": "1.0.0", "platforms": {} }).to_string(),
                )
                .unwrap();
                Installed::Plugin(v)
            }
            _ => {
                let integration = format!("int-{id}");
                state
                    .store
                    .create_mcp_integration(&integration, name, "http", Some("https://x.example"), "none", None, Some(id))
                    .unwrap();
                Installed::Connector(integration)
            }
        }
    }

    fn is_on(state: &AppState, item: &Installed) -> bool {
        match item {
            Installed::Employee(id) => state.store.get_agent(id).unwrap().unwrap().is_enabled != 0,
            Installed::Skill(dir) => !tools::skills::is_quarantined(dir),
            Installed::Plugin(v) => !v.join(".quarantined").exists(),
            Installed::Connector(id) => state.store.get_mcp_integration(id).unwrap().unwrap().is_enabled != Some(0),
        }
    }

    fn remove(state: &AppState, item: &Installed) {
        match item {
            Installed::Employee(id) => {
                let _ = state.store.delete_agent(id);
            }
            Installed::Skill(dir) => {
                let _ = std::fs::remove_dir_all(dir);
            }
            Installed::Plugin(v) => {
                let _ = std::fs::remove_dir_all(v.parent().unwrap());
            }
            Installed::Connector(id) => {
                let _ = state.store.delete_mcp_integration(id);
            }
        }
    }

    /// The ids of the revoke notices the owner was shown for `tag`'s items.
    fn notices(rx: &mut tokio::sync::broadcast::Receiver<crate::handlers::ws::HubEvent>, tag: &str) -> Vec<String> {
        let mut ids = Vec::new();
        loop {
            let ev = match rx.try_recv() {
                Ok(ev) => ev,
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(_) => break,
            };
            let id = ev.payload["id"].as_str().unwrap_or("");
            if ev.event_type == "notification" && id.starts_with(&format!("revoked:{tag}")) {
                ids.push(id.to_string());
            }
        }
        ids.sort();
        ids
    }

    const KINDS: [&str; 5] = ["employee", "app", "skill", "plugin", "connector"];

    // A revoke drained to a bot older than #543 was lost, and none worked on
    // 0.16.5. The sweep asks the hub what it has withdrawn and turns each
    // kind off through the one revoke, with its notice; what the hub did
    // not withdraw stays on. A second sweep finds nothing on and is quiet.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_sweep_turns_off_what_the_hub_withdrew_and_leaves_the_rest_on() {
        use crate::staffed_proof::{hub_lists, session};
        let nebo = session().await;
        let state = nebo.state.clone();
        let profile = uuid::Uuid::new_v4().to_string();
        state
            .store
            .create_auth_profile(&profile, "NeboAI", "neboai", "proof-token", None, None, 0, 1, Some("token"), None)
            .unwrap();
        let tag = uuid::Uuid::new_v4().simple().to_string()[..8].to_string();

        let mut items = Vec::new();
        for kind in KINDS {
            for withdrawn in [true, false] {
                let id = format!("{tag}-{kind}-{}", if withdrawn { "gone" } else { "kept" });
                let name = format!("Sweep {kind} {tag} {}", if withdrawn { "gone" } else { "kept" });
                hub_lists(&id, Some((&name, kind, "1.0.0", withdrawn)));
                items.push((id.clone(), withdrawn, install(&state, kind, &id, &name)));
            }
        }

        let mut rx = state.hub.subscribe();
        sweep(&state).await;
        for (id, withdrawn, item) in &items {
            assert_eq!(is_on(&state, item), !withdrawn, "{id}");
        }
        let mut expected: Vec<String> =
            items.iter().filter(|(_, w, _)| *w).map(|(id, _, _)| format!("revoked:{id}")).collect();
        expected.sort();
        assert_eq!(notices(&mut rx, &tag), expected, "one notice per withdrawn item");

        sweep(&state).await;
        assert!(notices(&mut rx, &tag).is_empty(), "nothing still on: the second sweep is quiet");

        for (id, _, item) in &items {
            remove(&state, item);
            hub_lists(id, None);
        }
        state.skill_loader.reload_from_disk().await;
        state.store.delete_auth_profile(&profile).unwrap();
    }

    // An app and a connector, sent the hub's legacy shapes
    // (`skillUpdated` / `skillRevoked` with `skillId`) as #543 reads them:
    // the update is checked against what is installed and offered, keeping
    // the install; the revoke turns each off.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_app_and_a_connector_hear_the_legacy_update_and_revoke() {
        use crate::staffed_proof::{hub_lists, session};
        let nebo = session().await;
        let state = nebo.state.clone();
        let profile = uuid::Uuid::new_v4().to_string();
        state
            .store
            .create_auth_profile(&profile, "NeboAI", "neboai", "proof-token", None, None, 0, 1, Some("token"), None)
            .unwrap();
        let tag = uuid::Uuid::new_v4().simple().to_string()[..8].to_string();

        for (kind, row_type) in [("app", "agent"), ("connector", "connector")] {
            let id = format!("{tag}-{kind}");
            let name = format!("Legacy {kind} {tag}");
            hub_lists(&id, Some((&name, kind, "1.1.0", false)));
            let item = install(&state, kind, &id, &name);
            state.store.upsert_artifact_update_pref(&id, row_type, "1.0.0").unwrap();

            let updated = napp::InstallEvent::parse(
                &serde_json::json!({
                    "type": "skillUpdated", "skillId": id, "skillName": name, "version": "1.1.0",
                    "artifactType": kind, "permissionsAdded": [], "permissionsRemoved": [],
                    "updatedAt": chrono::Utc::now().to_rfc3339(),
                })
                .to_string(),
            )
            .unwrap();
            crate::handle_comm_install_event(&state, updated).await.expect("the update notice");
            let pref = state.store.get_artifact_update_pref(&id, row_type).unwrap().unwrap();
            assert_eq!((pref.local_version.as_str(), pref.remote_version.as_str()), ("1.0.0", "1.1.0"), "{kind}");
            assert_eq!(pref.update_available, 1, "{kind}: offered");
            assert!(is_on(&state, &item), "{kind}: the update keeps it installed and on");

            let revoked = napp::InstallEvent::parse(
                &serde_json::json!({
                    "type": "skillRevoked", "skillId": id, "skillName": name, "artifactType": kind,
                    "reason": "Revoked by admin", "revokedAt": chrono::Utc::now().to_rfc3339(),
                })
                .to_string(),
            )
            .unwrap();
            crate::handle_comm_install_event(&state, revoked).await.expect("the revoke notice");
            assert!(!is_on(&state, &item), "{kind}: the revoke turns it off");

            remove(&state, &item);
            let _ = state.store.delete_artifact_update_pref(&id, row_type);
            hub_lists(&id, None);
        }
        state.store.delete_auth_profile(&profile).unwrap();
    }
}
