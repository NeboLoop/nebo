//! Background marketplace artifact update checker.
//!
//! Periodically polls NeboAI for version updates to installed agents, skills, and plugins.
//! Respects per-type and per-artifact auto-update preferences. Staggers between API calls
//! to avoid overwhelming the NeboAI API.

use std::time::Duration;

use semver::Version;
use tracing::{debug, info, warn};

use crate::codes::build_api_client;
use crate::state::AppState;

const BOOT_DELAY: Duration = Duration::from_secs(60);
const STAGGER: Duration = Duration::from_secs(2);
const DEFAULT_INTERVAL_HOURS: u64 = 6;

/// Spawn the artifact update background loop.
pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        tokio::time::sleep(BOOT_DELAY).await;
        loop {
            let interval_hours = state
                .store
                .get_artifact_update_settings()
                .map(|s| s.check_interval_hours as u64)
                .unwrap_or(DEFAULT_INTERVAL_HOURS);

            if let Err(e) = check_all(&state).await {
                warn!("artifact update check failed: {e}");
            }

            tokio::time::sleep(Duration::from_secs(interval_hours * 3600)).await;
        }
    });
}

/// Manually trigger an update check (called from HTTP handler).
///
/// Checking is ALWAYS performed (so the user can be notified of available
/// updates) regardless of the master `settings.auto_update` flag. Detection is
/// free; *applying* is what requires consent — a detected update is auto-applied
/// only when its per-artifact `auto_update` is on (opt-in), otherwise it's
/// surfaced via notification + the Updates panel for the user to approve.
pub async fn check_all(state: &AppState) -> Result<(), String> {
    let prefs = state
        .store
        .get_artifact_update_settings()
        .map_err(|e| e.to_string())?;

    let api = match build_api_client(state) {
        Ok(api) => api,
        Err(e) => {
            debug!("artifact updates: not connected to NeboAI ({e}), skipping");
            return Ok(());
        }
    };

    let mut updates_found: Vec<serde_json::Value> = Vec::new();

    // Check agents
    if prefs.agents {
        let agents = state.store.list_agents(1000, 0).unwrap_or_default();
        for agent in agents.iter().filter(|a| a.kind.is_some()) {
            let kind = agent.kind.as_deref().unwrap_or("");
            if kind.is_empty() {
                continue;
            }
            tokio::time::sleep(STAGGER).await;
            if let Some(update) = check_agent(state, &api, &agent).await {
                updates_found.push(update);
            }
        }
    }

    // Check plugins
    if prefs.plugins {
        let plugins = state.store.list_installed_plugins().unwrap_or_default();
        for plugin in &plugins {
            if plugin.slug.is_empty() {
                continue;
            }
            tokio::time::sleep(STAGGER).await;
            if let Some(update) = check_plugin(state, &api, plugin).await {
                updates_found.push(update);
            }
        }
    }

    // Check skills. Skills live on disk, but their marketplace id + installed
    // version were recorded in artifact_update_prefs at install time — that's the
    // enumeration source (the id is what get_skill needs).
    if prefs.skills {
        let skill_prefs: Vec<_> = state
            .store
            .list_artifact_update_prefs()
            .unwrap_or_default()
            .into_iter()
            .filter(|p| p.artifact_type == "skill" && !p.artifact_id.is_empty())
            .collect();
        let skills_dir = config::nebo_dir().ok().map(|d| d.join("skills"));
        for pref in &skill_prefs {
            tokio::time::sleep(STAGGER).await;
            // The version on disk is the truth: installs recorded 1.0.0 for
            // every skill whose package has no manifest.json, and a 0.x skill
            // then looked newer than every update and never got one.
            let installed = skills_dir.as_deref().and_then(|d| installed_skill_version(d, &pref.artifact_id));
            let local = match installed {
                Some(v) if v != pref.local_version => {
                    let _ = state.store.upsert_artifact_update_pref(&pref.artifact_id, "skill", &v);
                    v
                }
                Some(v) => v,
                None => pref.local_version.clone(),
            };
            if let Some(update) = check_by_artifact_id(state, &api, "skill", &pref.artifact_id, &local).await {
                updates_found.push(update);
            }
        }
    }

    // Check connectors (marketplace MCP connections). Like skills, the CONN-
    // install path records the artifact id + version in artifact_update_prefs,
    // and the marketplace serves connector versions from the same detail
    // endpoint.
    if prefs.connectors {
        let connector_prefs: Vec<_> = state
            .store
            .list_artifact_update_prefs()
            .unwrap_or_default()
            .into_iter()
            .filter(|p| p.artifact_type == "connector" && !p.artifact_id.is_empty())
            .collect();
        for pref in &connector_prefs {
            tokio::time::sleep(STAGGER).await;
            if let Some(update) = check_by_artifact_id(
                state,
                &api,
                "connector",
                &pref.artifact_id,
                &pref.local_version,
            )
            .await
            {
                updates_found.push(update);
            }
        }
    }

    // Broadcast summary if any updates found
    if !updates_found.is_empty() {
        info!(
            "artifact updates: {} update(s) available",
            updates_found.len()
        );
        state.hub.broadcast(
            "artifact_updates_available",
            serde_json::json!({
                "count": updates_found.len(),
                "updates": updates_found,
            }),
        );

        // Persistent notify-and-approve nudge (bell + toast), in addition to the
        // live event above. Deduped per (artifact, target version) so the same
        // pending update doesn't re-notify every check; clears naturally once the
        // user updates (the row's update_available flips off). Auto-update
        // artifacts are skipped — they apply silently below, no nudge needed.
        notify_updates_available(state);

        // Auto-apply only the artifacts the user opted into (per-artifact flag).
        auto_apply(state, &api).await;
    } else {
        debug!("artifact updates: all artifacts up to date");
    }

    Ok(())
}

/// Check ONE installed artifact against the marketplace, right now, through
/// the same per-type checkers the periodic sweep uses — so a product page
/// can show "Update to …" the moment a newer version exists instead of
/// waiting up to an interval for the sweep to notice. `key` is the slug for
/// plugins and the marketplace id for everything else (what the sweep keys
/// the pref row on, and what apply-update looks the row up by).
pub async fn check_one(state: &AppState, artifact_type: &str, key: &str) -> Option<serde_json::Value> {
    let api = build_api_client(state).ok()?;
    match artifact_type {
        "plugin" => {
            let plugin = state
                .store
                .list_installed_plugins()
                .ok()?
                .into_iter()
                .find(|p| p.slug == key)?;
            check_plugin(state, &api, &plugin).await
        }
        "agent" => {
            let agent = state.store.get_agent(key).ok().flatten()?;
            check_agent(state, &api, &agent).await
        }
        _ => {
            let pref = state
                .store
                .list_artifact_update_prefs()
                .ok()?
                .into_iter()
                .find(|p| p.artifact_type == artifact_type && p.artifact_id == key)?;
            check_by_artifact_id(state, &api, artifact_type, key, &pref.local_version).await
        }
    }
}

async fn check_agent(
    state: &AppState,
    api: &comm::api::NeboAIApi,
    agent: &db::models::Agent,
) -> Option<serde_json::Value> {
    // The installed version comes from the recorded pref — the SAME source `apply`
    // writes — so a successful update converges and this check stops re-flagging it.
    // (Reading the loader's on-disk manifest directly was the re-detection bug: it
    // never converged to the applied version, unlike plugins/skills/connectors which
    // read the store `apply` updates.) Fall back to the loader only to SEED a legacy
    // agent with no pref row yet; the backfill below then records it.
    let local_version = match state
        .store
        .get_artifact_update_pref(&agent.id, "agent")
        .ok()
        .flatten()
        .map(|p| p.local_version)
        .filter(|v| !v.is_empty())
    {
        Some(v) => v,
        None => state
            .agent_loader
            .get_by_name(&agent.name)
            .await
            .and_then(|a| a.version)
            .unwrap_or_default(),
    };

    if local_version.is_empty() {
        return None;
    }

    // Use get_skill (agents are queried via /skills/{id} endpoint)
    match api.get_skill(&agent.id).await {
        Ok(detail) => {
            let remote = &detail.item.version;
            if remote.is_empty() {
                return None;
            }
            if has_newer_version(&local_version, remote) {
                // Backfill the pref for agents installed before update tracking
                // existed (no row to UPDATE otherwise), then mark the remote version.
                let _ = state
                    .store
                    .upsert_artifact_update_pref(&agent.id, "agent", &local_version);
                let _ = state.store.set_artifact_remote_version(
                    &agent.id,
                    "agent",
                    remote,
                    true,
                    &agent.name,
                );
                return Some(serde_json::json!({
                    "id": agent.id,
                    "name": agent.name,
                    "type": "agent",
                    "localVersion": local_version,
                    "remoteVersion": remote,
                }));
            }
        }
        Err(e) => {
            debug!(agent = %agent.id, error = %e, "agent update check failed");
        }
    }
    None
}

async fn check_plugin(
    state: &AppState,
    api: &comm::api::NeboAIApi,
    plugin: &db::models::PluginRegistry,
) -> Option<serde_json::Value> {
    let local_version = &plugin.version;
    if local_version.is_empty() {
        return None;
    }

    let platform = current_platform();
    match api.get_plugin::<napp::plugin::PluginManifest>(&plugin.slug, &platform).await {
        Ok(manifest) => {
            let remote = &manifest.version;
            if remote.is_empty() {
                return None;
            }
            if has_newer_version(local_version, remote) {
                let _ = state.store.set_artifact_remote_version(
                    &plugin.slug,
                    "plugin",
                    remote,
                    true,
                    &plugin.name,
                );
                return Some(serde_json::json!({
                    "id": plugin.slug,
                    "name": plugin.name,
                    "type": "plugin",
                    "localVersion": local_version,
                    "remoteVersion": remote,
                }));
            }
            // Not newer: say so, or a remote version recorded before an
            // install keeps offering an "update" backwards. Seen live: gmail
            // 0.1.4 installed, the row still said remote 0.1.2 from August,
            // and Settings offered "0.1.4 → 0.1.2".
            let _ = state.store.set_artifact_remote_version(&plugin.slug, "plugin", remote, false, &plugin.name);
        }
        Err(e) => {
            debug!(plugin = %plugin.slug, error = %e, "plugin update check failed");
        }
    }
    None
}

/// Check one artifact whose version is served by the marketplace's skill
/// detail endpoint (`get_skill` also serves agents and connectors) against the
/// locally recorded version from `artifact_update_prefs`. Shared by the skill
/// and connector checkers — they differ only in `artifact_type`.
async fn check_by_artifact_id(
    state: &AppState,
    api: &comm::api::NeboAIApi,
    artifact_type: &str,
    artifact_id: &str,
    local_version: &str,
) -> Option<serde_json::Value> {
    if local_version.is_empty() {
        return None;
    }
    match api.get_skill(artifact_id).await {
        Ok(detail) => {
            let remote = &detail.item.version;
            if remote.is_empty() {
                return None;
            }
            if has_newer_version(local_version, remote) {
                let _ = state.store.set_artifact_remote_version(
                    artifact_id,
                    artifact_type,
                    remote,
                    true,
                    &detail.item.name,
                );
                return Some(serde_json::json!({
                    "id": artifact_id,
                    "name": detail.item.name,
                    "type": artifact_type,
                    "localVersion": local_version,
                    "remoteVersion": remote,
                }));
            }
            // Not newer: clear what an earlier check recorded (see check_plugin).
            let _ = state.store.set_artifact_remote_version(artifact_id, artifact_type, remote, false, &detail.item.name);
        }
        Err(e) => {
            debug!(artifact = %artifact_id, artifact_type, error = %e, "update check failed");
        }
    }
    None
}

/// The version a marketplace skill is installed at, read from disk. An
/// install lands in `skills/<slug>/<version>/` with a `.artifact_id` sidecar,
/// so the folder's name is the marketplace version; the newest folder
/// carrying `artifact_id` wins. `None` when no folder carries it.
pub(crate) fn installed_skill_version(skills_dir: &std::path::Path, artifact_id: &str) -> Option<String> {
    let mut newest: Option<Version> = None;
    for slug in std::fs::read_dir(skills_dir).ok()?.flatten() {
        let folders: Vec<_> = std::fs::read_dir(slug.path()).into_iter().flatten().flatten().map(|d| d.path()).collect();
        // One skill's folder holds its versions side by side. Any of them
        // carrying the id makes them all this skill's: an update's folder
        // written without the id is still the newest version installed.
        let ours = folders.iter().any(|p| {
            std::fs::read_to_string(p.join(".artifact_id")).is_ok_and(|id| id.trim() == artifact_id)
        });
        if !ours {
            continue;
        }
        for path in &folders {
            let Some(version) = path.file_name().and_then(|n| n.to_str()).and_then(|n| Version::parse(n).ok()) else {
                continue;
            };
            if path.is_dir() && newest.as_ref().is_none_or(|n| version > *n) {
                newest = Some(version);
            }
        }
    }
    newest.map(|v| v.to_string())
}

/// Compare versions using semver. Falls back to string comparison if parsing fails.
fn has_newer_version(local: &str, remote: &str) -> bool {
    match (Version::parse(local), Version::parse(remote)) {
        (Ok(l), Ok(r)) => r > l,
        _ => !remote.is_empty() && remote != local,
    }
}

/// Create a persistent, deduped "update available" notification for each pending
/// update the user must approve (i.e. NOT auto-update). The bell + toast come
/// from the canonical `owner_notify::emit` persist+broadcast pathway;
/// the deterministic id keyed on the target version means a given pending update
/// notifies once, not every check.
pub(crate) fn notify_updates_available(state: &AppState) {
    let pending = state.store.list_artifacts_with_updates().unwrap_or_default();
    for a in &pending {
        if a.auto_update != 0 {
            continue; // applied silently — no approval nudge
        }
        let notif_id = format!(
            "artifact-update:{}:{}:{}",
            a.artifact_type, a.artifact_id, a.remote_version
        );
        let title = "Update available".to_string();
        // Lead with the display name — "plugin 0.2.2 → 0.2.3" is ambiguous the
        // moment more than one plugin is installed. Type only when no name.
        let display = a.name.as_deref().unwrap_or(&a.artifact_type);
        let body = format!(
            "{} {} → {} is available. Review it in Settings → Updates.",
            display, a.local_version, a.remote_version
        );
        // Settings → Updates, at this package, with its Update button.
        let action_url = tools::owner_notify::link::update(&a.artifact_id);
        let n = tools::owner_notify::OwnerNotification {
            id: &notif_id,
            kind: "info",
            title: &title,
            body: Some(&body),
            action_url: Some(&action_url),
            agent_id: None,
            loud: false,
        };
        tools::owner_notify::emit(&state.store, Some(&|ev, payload| state.hub.broadcast(ev, payload)), &n);
        // Mirror to the owner's web inbox (informational — no action
        // buttons; applying an update stays a bot-UI decision). The hub
        // upsert on the id keeps the every-check re-push idempotent.
        crate::codes::push_inbox(state, n.hub_item(serde_json::json!({})));
    }
}

/// Auto-apply updates for artifacts the user opted into (per-artifact flag).
async fn auto_apply(state: &AppState, api: &comm::api::NeboAIApi) {
    let pending = state.store.list_artifacts_with_updates().unwrap_or_default();
    for artifact in &pending {
        if artifact.auto_update == 0 {
            continue; // notify-and-approve: user applies manually
        }
        // Atomically claim to prevent double-apply (manual apply races the loop).
        let claimed = state
            .store
            .claim_artifact_update(&artifact.artifact_id, &artifact.artifact_type)
            .unwrap_or(false);
        if !claimed {
            continue;
        }
        apply_claimed_update(state, api, artifact).await;
        tokio::time::sleep(STAGGER).await;
    }
}

/// Apply ONE already-claimed pending update: dispatch by type, then on success
/// bump the local version + log history + broadcast applied; on failure unclaim
/// (so the user can retry) + log history + broadcast failed. This is the SINGLE
/// apply core shared by the auto-update loop and the manual apply endpoint
/// (CODE_AUDITOR Rule 8) so the two can't drift in what "apply" means.
pub(crate) async fn apply_claimed_update(
    state: &AppState,
    api: &comm::api::NeboAIApi,
    artifact: &db::models::ArtifactUpdatePref,
) {
    let id = &artifact.artifact_id;
    let atype = &artifact.artifact_type;
    match apply_by_type(state, api, atype, id).await {
        Ok(()) => {
            let _ = state
                .store
                .upsert_artifact_update_pref(id, atype, &artifact.remote_version);
            let _ = state.store.record_artifact_update_history(
                id,
                atype,
                artifact.name.as_deref().unwrap_or(""),
                &artifact.local_version,
                &artifact.remote_version,
                "applied",
                "",
            );
            state.hub.broadcast(
                "artifact_update_applied",
                serde_json::json!({
                    "id": id,
                    "type": atype,
                    "version": artifact.remote_version,
                }),
            );
            info!(artifact = %id, version = %artifact.remote_version, "applied artifact update");
        }
        Err(e) => {
            let _ = state.store.unclaim_artifact_update(id, atype);
            let _ = state.store.record_artifact_update_history(
                id,
                atype,
                artifact.name.as_deref().unwrap_or(""),
                &artifact.local_version,
                &artifact.remote_version,
                "failed",
                &e,
            );
            state.hub.broadcast(
                "artifact_update_failed",
                serde_json::json!({ "id": id, "type": atype, "error": e }),
            );
            warn!(artifact = %id, error = %e, "failed to apply artifact update");
        }
    }
}

/// Put the marketplace's current package of one installed artifact in place,
/// by type, keeping what the owner set on it. `id` is what the update row is
/// keyed on: the slug for a plugin, the marketplace id for everything else.
async fn apply_by_type(state: &AppState, api: &comm::api::NeboAIApi, artifact_type: &str, id: &str) -> Result<(), String> {
    match artifact_type {
        "agent" => apply_agent_update_pub(state, api, id).await,
        "plugin" => apply_plugin_update_pub(state, api, id).await,
        "skill" => apply_skill_update_pub(state, api, id).await,
        "connector" => apply_connector_update_pub(state, api, id).await,
        other => Err(format!("updates for '{other}' artifacts aren't supported yet")),
    }
}

/// What a hub update notice (`tool_updated`) does to an installed artifact.
#[derive(Debug, PartialEq)]
pub(crate) enum NoticeStep {
    /// Not tracked here: the install pathway, as for any install event.
    Install,
    /// Already current: the notice's version was applied after it was sent,
    /// or the notice is older than what is installed.
    Skip,
    /// A newer version: the same as one the update checker finds. Applied
    /// now when the owner turned on automatic updates for it, otherwise
    /// offered in Settings → Updates for the owner's yes.
    Offer,
    /// The same version, rebuilt in place (a private or loop artifact's
    /// edit, which needs no version bump): put in place now.
    Refresh,
}

/// The step for a notice of `notice` when `local` is installed. `applied_at`
/// is when an update to `notice` was last applied here, `sent_at` when the
/// hub sent the notice (both unix seconds).
pub(crate) fn notice_step(local: Option<&str>, notice: &str, applied_at: Option<i64>, sent_at: Option<i64>) -> NoticeStep {
    let Some(local) = local.filter(|v| !v.is_empty()) else {
        return NoticeStep::Install;
    };
    if notice.is_empty() {
        return NoticeStep::Install;
    }
    if notice == local {
        return match (applied_at, sent_at) {
            (Some(applied), Some(sent)) if applied >= sent => NoticeStep::Skip,
            _ => NoticeStep::Refresh,
        };
    }
    if has_newer_version(local, notice) {
        NoticeStep::Offer
    } else {
        NoticeStep::Skip
    }
}

/// The update row an installed artifact is tracked by (`artifact_type`,
/// key) and its installed version, found from the hub's artifact id. `None`
/// when it is not installed here or its type has no update path.
async fn tracked(state: &AppState, artifact_id: &str, artifact_type: &str, slug: &str) -> Option<(&'static str, String, String)> {
    match artifact_type {
        // An app is an employee with a page; both are tracked as "agent".
        // The installed version is read as `check_agent` reads it: the
        // recorded row, else the loaded package (an employee installed
        // before update tracking).
        "agent" | "app" => {
            let agent = state.store.get_agent(artifact_id).ok().flatten()?;
            let recorded = state
                .store
                .get_artifact_update_pref(artifact_id, "agent")
                .ok()
                .flatten()
                .map(|p| p.local_version)
                .filter(|v| !v.is_empty());
            let version = match recorded {
                Some(v) => v,
                None => state.agent_loader.get_by_name(&agent.name).await.and_then(|a| a.version).unwrap_or_default(),
            };
            Some(("agent", artifact_id.to_string(), version))
        }
        "plugin" => {
            let plugin = state.store.list_installed_plugins().ok()?.into_iter().find(|p| p.slug == slug)?;
            Some(("plugin", plugin.slug, plugin.version))
        }
        "skill" | "connector" => {
            let t = if artifact_type == "skill" { "skill" } else { "connector" };
            let pref = state.store.get_artifact_update_pref(artifact_id, t).ok().flatten()?;
            Some((t, artifact_id.to_string(), pref.local_version))
        }
        _ => None,
    }
}

/// A hub update notice (`tool_updated`) for an artifact installed here.
/// Returns `false` when the artifact is not tracked here and the caller
/// installs it the way any install event is installed.
///
/// An update keeps what the owner set on the artifact: it goes through the
/// same apply core as Settings → Updates, never a clean reinstall (which
/// deleted the employee row with its settings and schedules), and grants
/// nothing new; a permission the new version adds is asked for when the
/// employee first needs it, as for any update. A newer version follows the
/// artifact's automatic-updates preference (off by default: offered for the
/// owner's yes); the same version rebuilt in place is put in place now; a
/// notice already applied, or older than what is installed, does nothing.
pub(crate) async fn on_update_notice(
    state: &AppState,
    api: &comm::api::NeboAIApi,
    event: &napp::InstallEvent,
    detail: &comm::api_types::SkillDetail,
) -> bool {
    let item = &detail.item;
    let artifact_type = event
        .payload
        .get("artifact_type")
        .and_then(|v| v.as_str())
        .or(item.artifact_type.as_deref())
        .unwrap_or("skill");
    let Some((row_type, key, local)) = tracked(state, &event.tool_id, artifact_type, &item.slug).await else {
        return false;
    };
    let notice = event
        .payload
        .get("version")
        .and_then(|v| v.as_str())
        .filter(|v| !v.is_empty())
        .unwrap_or(&item.version)
        .to_string();
    let sent_at = event
        .payload
        .get("updated_at")
        .and_then(|v| v.as_str())
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        .map(|t| t.timestamp());
    let applied_at = state.store.artifact_update_applied_at(&key, row_type, &notice).ok().flatten();
    let name = event.payload.get("name").and_then(|v| v.as_str()).unwrap_or(&item.name).to_string();
    match notice_step(Some(&local), &notice, applied_at, sent_at) {
        NoticeStep::Install => false,
        NoticeStep::Skip => {
            debug!(artifact = %key, local = %local, notice = %notice, "update notice: already current");
            true
        }
        NoticeStep::Offer => {
            let _ = state.store.upsert_artifact_update_pref(&key, row_type, &local);
            let _ = state.store.set_artifact_remote_version(&key, row_type, &notice, true, &name);
            let pref = state.store.get_artifact_update_pref(&key, row_type).ok().flatten();
            match pref {
                Some(pref) if pref.auto_update != 0 => {
                    if state.store.claim_artifact_update(&key, row_type).unwrap_or(false) {
                        apply_claimed_update(state, api, &pref).await;
                    }
                }
                _ => notify_updates_available(state),
            }
            true
        }
        NoticeStep::Refresh => {
            let _ = state.store.upsert_artifact_update_pref(&key, row_type, &local);
            let (status, detail) = match apply_by_type(state, api, row_type, &key).await {
                Ok(()) => {
                    state.hub.broadcast(
                        "artifact_update_applied",
                        serde_json::json!({ "id": key, "type": row_type, "version": local }),
                    );
                    info!(artifact = %key, version = %local, "applied an in-place rebuild");
                    ("applied", String::new())
                }
                Err(e) => {
                    state.hub.broadcast(
                        "artifact_update_failed",
                        serde_json::json!({ "id": key, "type": row_type, "error": e }),
                    );
                    warn!(artifact = %key, error = %e, "in-place rebuild not applied");
                    ("failed", e)
                }
            };
            let _ = state
                .store
                .record_artifact_update_history(&key, row_type, &name, &local, &local, status, &detail);
            true
        }
    }
}

pub(crate) async fn apply_agent_update_pub(
    state: &AppState,
    api: &comm::api::NeboAIApi,
    agent_id: &str,
) -> Result<(), String> {
    let agent = state
        .store
        .get_agent(agent_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("agent {} not found", agent_id))?;

    let kind = agent.kind.as_deref().unwrap_or("");
    tools::persist_agent_from_api(api, agent_id, &agent.name, kind, &state.store)
        .await
        .map(|_| ())?;

    // Reload agent loader to pick up new version from filesystem
    state.agent_loader.load_all().await;

    // Rematerialize agent_workflows from the freshly persisted frontmatter.
    // The workflow_manager fires bindings from these ROWS, not the agent
    // config — without this, an agent update ships new workflow definitions
    // that never run until someone hand-upserts the rows (nebo #57; bit every
    // single Vivid deploy).
    if let Ok(Some(updated)) = state.store.get_agent(agent_id) {
        if !updated.frontmatter.is_empty() {
            match napp::agent::parse_agent_config(&updated.frontmatter) {
                Ok(config) => crate::sync_agent_workflows(&state.store, agent_id, &config),
                Err(e) => tracing::warn!(
                    agent_id,
                    error = %e,
                    "agent update applied but frontmatter unparseable — workflows NOT rematerialized"
                ),
            }
        }
    }

    // App sidecars: the version dir just changed. Refresh the app's DB paths from the
    // freshly-loaded filesystem (reconcile_app_fields), then stop + relaunch a running
    // sidecar so it runs the NEW binary rather than the swapped-out old one.
    if state
        .store
        .get_agent(agent_id)
        .ok()
        .flatten()
        .map(|a| a.is_app.unwrap_or(0) != 0)
        .unwrap_or(false)
    {
        crate::codes::reconcile_app_fields(state).await;
        if let Ok(Some(app)) = state.store.get_agent(agent_id) {
            crate::app_lifecycle::relaunch(state, &app).await;
        }
    }

    // Lifecycle event: agent updated to a new version.
    state.emit_lifecycle(
        "agent.updated",
        serde_json::json!({ "agent_id": agent_id, "version": agent.kind }),
        format!("update:agent:{agent_id}"),
    );
    Ok(())
}

pub(crate) async fn apply_plugin_update_pub(
    state: &AppState,
    api: &comm::api::NeboAIApi,
    slug: &str,
) -> Result<(), String> {
    // Updating a plugin is just re-installing its latest version. Delegate to the
    // ONE plugin-install core so binary resolution, real sha256/signature DB
    // registration, skill-watcher pausing, and tool/hook re-registration can't
    // drift from the install path (CODE_AUDITOR Rule 8). The previous inline copy
    // skipped plugin_store.remove(), the loader cycle, tool/hook re-register, and
    // wrote empty binary_path/hash into the registry.
    let name = state
        .store
        .list_installed_plugins()
        .ok()
        .and_then(|ps| ps.into_iter().find(|p| p.slug == slug).map(|p| p.name))
        .unwrap_or_else(|| slug.to_string());
    crate::codes::fetch_and_install_plugin(state, api, slug, &name, None)
        .await
        .map_err(|e| e.to_string())
}

pub(crate) async fn apply_skill_update_pub(
    state: &AppState,
    api: &comm::api::NeboAIApi,
    skill_id: &str,
) -> Result<(), String> {
    // Re-persist the skill at its latest version through the SAME core the
    // install path uses (persist_skill_from_api fetches the detail itself), then
    // cold-reload the loader so the new content is live (Rule 8 — no drift from
    // install). `name` is only a dir fallback; the API detail's slug wins.
    tools::persist_skill_from_api(api, skill_id, skill_id, "", Some(&state.store)).await?;
    state.skill_loader.reload_from_disk().await;
    Ok(())
}

pub(crate) async fn apply_connector_update_pub(
    state: &AppState,
    api: &comm::api::NeboAIApi,
    connector_id: &str,
) -> Result<(), String> {
    // A connector's manifest IS its MCP config block — fetch the latest and
    // reconcile the installed integrations through the ONE sync routine, which
    // updates rows in place so stored credentials survive (Rule 8 — same
    // parser/creation core the install path uses).
    let detail = api
        .get_skill(connector_id)
        .await
        .map_err(|e| format!("fetch connector {connector_id}: {e}"))?;
    let raw = detail
        .manifest
        .ok_or_else(|| format!("connector {connector_id} has no MCP config"))?;
    // The block may arrive as a JSON object or a JSON-encoded string (same as
    // the CONN- install path).
    let block = match raw {
        serde_json::Value::String(s) => serde_json::from_str(&s)
            .map_err(|e| format!("connector config is not valid JSON: {e}"))?,
        other => other,
    };
    crate::handlers::integrations::sync_integrations_from_block(state, connector_id, &block)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn current_platform() -> String {
    let arch = std::env::consts::ARCH;
    let os = std::env::consts::OS;
    let arch_str = match arch {
        "aarch64" => "arm64",
        "x86_64" => "amd64",
        _ => arch,
    };
    format!("{}-{}", os, arch_str)
}

#[cfg(test)]
mod notice_tests {

    /// A skill's installed version is read from its folder: the newest
    /// version folder that carries its id, never another skill's, and none
    /// when no folder carries it (the 1.0.0 every manifest-less skill was
    /// recorded as hid every 0.x update).
    #[test]
    fn a_skills_installed_version_is_its_newest_folder() {
        let root = std::env::temp_dir().join(format!("nebo-skillver-{}", uuid::Uuid::new_v4()));
        let put = |slug: &str, version: &str, id: &str| {
            let dir = root.join(slug).join(version);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(".artifact_id"), id).unwrap();
        };
        put("app-studio", "0.1.0", "studio-id");
        put("app-studio", "0.1.3", "studio-id\n");
        put("deep-research", "2.0.0", "other-id");
        assert_eq!(super::installed_skill_version(&root, "studio-id").as_deref(), Some("0.1.3"));
        // An update's folder written without the id is still the newest
        // version of that skill (live: the same 0.1.3 -> 0.1.4 applied on
        // every check).
        std::fs::create_dir_all(root.join("app-studio").join("0.1.4")).unwrap();
        assert_eq!(super::installed_skill_version(&root, "studio-id").as_deref(), Some("0.1.4"));
        assert_eq!(super::installed_skill_version(&root, "missing").as_deref(), None);
        assert!(super::has_newer_version("0.1.3", "0.1.4"), "the stub's update now reaches it");
        let _ = std::fs::remove_dir_all(&root);
    }

    use super::{notice_step, NoticeStep};

    // A hub update notice moves an installed artifact only when it brings
    // something: a newer version is offered (or applied, with automatic
    // updates on), the same version rebuilt in place is put in place once,
    // and a repeat or an older notice does nothing. Before this every
    // notice reinstalled unconditionally.
    #[test]
    fn an_update_notice_is_checked_against_what_is_installed() {
        assert_eq!(notice_step(None, "0.2.0", None, None), NoticeStep::Install, "not here: the install path");
        assert_eq!(notice_step(Some("0.1.0"), "0.2.0", None, Some(100)), NoticeStep::Offer);
        assert_eq!(notice_step(Some("0.2.0"), "0.1.0", None, Some(100)), NoticeStep::Skip, "an older notice");
        assert_eq!(notice_step(Some("0.2.0"), "0.2.0", None, Some(100)), NoticeStep::Refresh, "rebuilt in place");
        assert_eq!(notice_step(Some("0.2.0"), "0.2.0", Some(150), Some(100)), NoticeStep::Skip, "applied after it was sent");
        assert_eq!(notice_step(Some("0.2.0"), "0.2.0", Some(50), Some(100)), NoticeStep::Refresh, "an edit after the last apply");
    }
}
