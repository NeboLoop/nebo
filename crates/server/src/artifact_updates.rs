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
        for pref in &skill_prefs {
            tokio::time::sleep(STAGGER).await;
            if let Some(update) =
                check_skill(state, &api, &pref.artifact_id, &pref.local_version).await
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

async fn check_agent(
    state: &AppState,
    api: &comm::api::NeboAIApi,
    agent: &db::models::Agent,
) -> Option<serde_json::Value> {
    // The installed version comes from the loader (reads agent.json off disk), not
    // the DB napp_path manifest — code-installed agents have no napp_path, so that
    // path read empty and the agent was never checked.
    let local_version = state
        .agent_loader
        .get_by_name(&agent.name)
        .await
        .and_then(|a| a.version)
        .unwrap_or_default();

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
    match api.get_plugin(&plugin.slug, &platform).await {
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
                );
                return Some(serde_json::json!({
                    "id": plugin.slug,
                    "name": plugin.name,
                    "type": "plugin",
                    "localVersion": local_version,
                    "remoteVersion": remote,
                }));
            }
        }
        Err(e) => {
            debug!(plugin = %plugin.slug, error = %e, "plugin update check failed");
        }
    }
    None
}

async fn check_skill(
    state: &AppState,
    api: &comm::api::NeboAIApi,
    skill_id: &str,
    local_version: &str,
) -> Option<serde_json::Value> {
    if local_version.is_empty() {
        return None;
    }
    match api.get_skill(skill_id).await {
        Ok(detail) => {
            let remote = &detail.item.version;
            if remote.is_empty() {
                return None;
            }
            if has_newer_version(local_version, remote) {
                let _ = state
                    .store
                    .set_artifact_remote_version(skill_id, "skill", remote, true);
                return Some(serde_json::json!({
                    "id": skill_id,
                    "name": detail.item.name,
                    "type": "skill",
                    "localVersion": local_version,
                    "remoteVersion": remote,
                }));
            }
        }
        Err(e) => {
            debug!(skill = %skill_id, error = %e, "skill update check failed");
        }
    }
    None
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
/// from the canonical `create_notification_if_not_exists` + broadcast pathway;
/// the deterministic id keyed on the target version means a given pending update
/// notifies once, not every check.
fn notify_updates_available(state: &AppState) {
    let pending = state.store.list_artifacts_with_updates().unwrap_or_default();
    let user_id = state.store.ensure_local_user_id().unwrap_or_default();
    for a in &pending {
        if a.auto_update != 0 {
            continue; // applied silently — no approval nudge
        }
        let notif_id = format!(
            "artifact-update:{}:{}:{}",
            a.artifact_type, a.artifact_id, a.remote_version
        );
        let title = "Update available".to_string();
        let body = format!(
            "{} {} → {} is available. Review it in Settings → Updates.",
            a.artifact_type, a.local_version, a.remote_version
        );
        let action_url = "/settings/updates".to_string();
        if state
            .store
            .create_notification_if_not_exists(
                &notif_id,
                &user_id,
                "info",
                &title,
                Some(&body),
                Some(&action_url),
                None,
            )
            .is_ok()
        {
            state.hub.broadcast(
                "notification_created",
                serde_json::json!({
                    "id": notif_id,
                    "type": "info",
                    "title": title,
                    "body": body,
                    "actionUrl": action_url,
                    "readAt": null,
                }),
            );
        }
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
    let result = match atype.as_str() {
        "agent" => apply_agent_update_pub(state, api, id).await,
        "plugin" => apply_plugin_update_pub(state, api, id).await,
        "skill" => apply_skill_update_pub(state, api, id).await,
        other => Err(format!("updates for '{other}' artifacts aren't supported yet")),
    };
    match result {
        Ok(()) => {
            let _ = state
                .store
                .upsert_artifact_update_pref(id, atype, &artifact.remote_version);
            let _ = state.store.record_artifact_update_history(
                id,
                atype,
                "",
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
                "",
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
    crate::codes::fetch_and_install_plugin(state, api, slug, &name)
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
