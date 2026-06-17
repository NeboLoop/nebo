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
pub async fn check_all(state: &AppState) -> Result<(), String> {
    // Bail if master auto_update is off
    if let Ok(Some(settings)) = state.store.get_settings() {
        if settings.auto_update == 0 {
            debug!("artifact updates: master auto_update is off, skipping");
            return Ok(());
        }
    }

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

        // Auto-apply where configured
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
    let local_version = agent
        .napp_path
        .as_ref()
        .and_then(|p| {
            let manifest_path = std::path::PathBuf::from(p).join("manifest.json");
            std::fs::read_to_string(manifest_path).ok()
        })
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v["version"].as_str().map(|s| s.to_string()))
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

/// Compare versions using semver. Falls back to string comparison if parsing fails.
fn has_newer_version(local: &str, remote: &str) -> bool {
    match (Version::parse(local), Version::parse(remote)) {
        (Ok(l), Ok(r)) => r > l,
        _ => !remote.is_empty() && remote != local,
    }
}

/// Auto-apply updates for artifacts configured for auto-update.
async fn auto_apply(state: &AppState, api: &comm::api::NeboAIApi) {
    let pending = state.store.list_artifacts_with_updates().unwrap_or_default();
    for artifact in &pending {
        if artifact.auto_update == 0 {
            continue;
        }

        // Atomically claim the update to prevent double-apply
        let claimed = state
            .store
            .claim_artifact_update(&artifact.artifact_id, &artifact.artifact_type)
            .unwrap_or(false);
        if !claimed {
            continue;
        }

        let result = match artifact.artifact_type.as_str() {
            "agent" => apply_agent_update_pub(state, api, &artifact.artifact_id).await,
            "plugin" => apply_plugin_update_pub(state, api, &artifact.artifact_id).await,
            _ => Ok(()),
        };

        match result {
            Ok(()) => {
                // Update local version to match remote
                let _ = state.store.upsert_artifact_update_pref(
                    &artifact.artifact_id,
                    &artifact.artifact_type,
                    &artifact.remote_version,
                );
                state.hub.broadcast(
                    "artifact_update_applied",
                    serde_json::json!({
                        "id": artifact.artifact_id,
                        "type": artifact.artifact_type,
                        "version": artifact.remote_version,
                    }),
                );
                info!(
                    artifact = %artifact.artifact_id,
                    version = %artifact.remote_version,
                    "auto-applied artifact update"
                );
            }
            Err(e) => {
                // Unclaim so user can retry
                let _ = state
                    .store
                    .unclaim_artifact_update(&artifact.artifact_id, &artifact.artifact_type);
                state.hub.broadcast(
                    "artifact_update_failed",
                    serde_json::json!({
                        "id": artifact.artifact_id,
                        "type": artifact.artifact_type,
                        "error": e,
                    }),
                );
                warn!(
                    artifact = %artifact.artifact_id,
                    error = %e,
                    "failed to auto-apply artifact update"
                );
            }
        }

        tokio::time::sleep(STAGGER).await;
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
