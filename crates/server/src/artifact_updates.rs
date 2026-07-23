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
                check_by_artifact_id(state, &api, "skill", &pref.artifact_id, &pref.local_version)
                    .await
            {
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
        }
        Err(e) => {
            debug!(artifact = %artifact_id, artifact_type, error = %e, "update check failed");
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
        "connector" => apply_connector_update_pub(state, api, id).await,
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
