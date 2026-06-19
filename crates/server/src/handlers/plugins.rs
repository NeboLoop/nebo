//! Plugin handlers — listing installed plugins and authentication.
//!
//! Plugins that require credentials (e.g., GWS needing Google OAuth) declare
//! auth requirements in their manifest. These handlers run the plugin's own
//! auth CLI commands and report status via WebSocket events.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::response::Json;
use tokio::io::AsyncReadExt;
use tracing::{info, warn};

use super::{HandlerResult, to_error_response};
use crate::state::AppState;
use types::NeboError;

/// GET /plugins
///
/// Lists all installed plugins, deduped by slug (highest version wins).
/// Enriches each entry with manifest data (name, description, author, auth info)
/// and DB fields (enabled, signatureStatus) when available.
pub async fn list_plugins(State(state): State<AppState>) -> HandlerResult<serde_json::Value> {
    let installed = state.plugin_store.list_installed();

    // Build DB lookup for enrichment (enabled, signature_status).
    let db_plugins: HashMap<String, db::models::PluginRegistry> = state
        .store
        .list_installed_plugins()
        .unwrap_or_default()
        .into_iter()
        .map(|p| (p.slug.clone(), p))
        .collect();

    // Dedup by slug — list_installed sorts by slug asc, version desc,
    // so first occurrence per slug is the highest version.
    let mut seen = HashMap::new();
    for (slug, version, _binary_path, source) in &installed {
        seen.entry(slug.clone())
            .or_insert_with(|| (version.clone(), *source));
    }

    let mut plugins = Vec::new();
    for (slug, (version, source)) in &seen {
        let manifest = state.plugin_store.get_manifest(slug);
        let (has_auth, auth_label, auth_type, auth_env_vars) = match &manifest {
            Some(m) => match &m.auth {
                Some(auth) => {
                    // Only surface env vars for env-type auth (API keys the user provides).
                    // OAuth plugins have pre-filled client credentials — users never touch those.
                    let env_vars: Vec<String> = if auth.auth_type == "env" {
                        auth.env.keys().cloned().collect()
                    } else {
                        Vec::new()
                    };
                    (true, auth.label.clone(), auth.auth_type.clone(), env_vars)
                }
                None => (false, String::new(), String::new(), Vec::new()),
            },
            None => (false, String::new(), String::new(), Vec::new()),
        };

        // A plugin is multi-account if its auth declares a profile_dir_env
        // (the "resource" credential model — e.g. gws holding several Gmail
        // accounts per agent). The accounts UI filters on this.
        let multi_account = manifest
            .as_ref()
            .and_then(|m| m.auth.as_ref())
            .and_then(|a| a.profile_dir_env.as_ref())
            .is_some();

        let event_count = manifest
            .as_ref()
            .and_then(|m| m.events.as_ref())
            .map(|e| e.len())
            .unwrap_or(0);

        let db_row = db_plugins.get(slug.as_str());
        let enabled = db_row.map(|r| r.is_enabled != 0).unwrap_or(true);
        let sig_status = db_row
            .map(|r| r.signature_status.as_str())
            .unwrap_or("unverified");

        // Inline the setup wizard config when the plugin declares one,
        // so the frontend doesn't have to fetch the manifest separately.
        let setup = manifest
            .as_ref()
            .and_then(|m| m.setup.as_ref())
            .map(|s| serde_json::to_value(s).unwrap_or(serde_json::Value::Null));

        plugins.push(serde_json::json!({
            "slug": slug,
            "version": version.to_string(),
            "name": manifest.as_ref().map(|m| m.name.as_str()).unwrap_or(slug.as_str()),
            "description": manifest.as_ref().map(|m| m.description.as_str()).unwrap_or(""),
            "author": manifest.as_ref().map(|m| m.author.as_str()).unwrap_or(""),
            "hasAuth": has_auth,
            "authLabel": auth_label,
            "authType": auth_type,
            "authEnvVars": auth_env_vars,
            "hasEvents": event_count > 0,
            "eventCount": event_count,
            "source": source,
            "enabled": enabled,
            "signatureStatus": sig_status,
            "setup": setup,
            "multiAccount": multi_account,
        }));
    }

    plugins.sort_by(|a, b| {
        a["slug"]
            .as_str()
            .unwrap_or("")
            .cmp(b["slug"].as_str().unwrap_or(""))
    });

    // Enrich with stored API key status for plugins with env vars
    for plugin in &mut plugins {
        let slug = plugin["slug"].as_str().unwrap_or("");
        let env_vars = plugin["authEnvVars"]
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect::<Vec<_>>())
            .unwrap_or_default();
        if !env_vars.is_empty() {
            let settings = state
                .store
                .list_plugin_settings_by_slug(slug)
                .unwrap_or_default();
            let all_set = env_vars.iter().all(|var| {
                settings.iter().any(|s| s.setting_key == *var && !s.setting_value.is_empty())
            });
            plugin["authKeysSet"] = serde_json::json!(all_set);
        }
    }

    let total = plugins.len();
    Ok(Json(serde_json::json!({
        "plugins": plugins,
        "total": total,
    })))
}

/// POST /plugins/{slug}/toggle
///
/// Toggles a plugin's enabled state. Refreshes the plugin tool definition so the
/// LLM sees the updated set of active plugins.
pub async fn toggle_plugin(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let current = state
        .store
        .get_plugin_by_slug(&slug)
        .map_err(to_error_response)?;
    let was_enabled = current.map(|r| r.is_enabled != 0).unwrap_or(true);
    state
        .store
        .set_plugin_enabled(&slug, !was_enabled)
        .map_err(to_error_response)?;
    state.tools.refresh_definition("plugin").await;
    Ok(Json(serde_json::json!({
        "slug": slug,
        "enabled": !was_enabled,
    })))
}

/// POST /plugins/{slug}/auth/login
///
/// Spawns the plugin's auth login command in the background. Returns immediately.
/// Broadcasts `plugin_auth_complete` or `plugin_auth_error` via WebSocket when done.
pub async fn auth_login(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let (binary_path, auth) = state
        .plugin_store
        .get_auth_info(&slug)
        .ok_or_else(|| to_error_response(NeboError::NotFound))?;
    spawn_plugin_login(
        state,
        slug,
        auth.commands.login.clone(),
        binary_path,
        auth.label.clone(),
        None,
    );
    Ok(Json(serde_json::json!({ "started": true })))
}

/// One account's login context for a multi-account ("resource") plugin.
/// When passed to `spawn_plugin_login`, the login runs with the plugin's
/// `profile_dir_env` pointed at `config_dir`, and on success the
/// (agent, plugin, account) → config_dir mapping is recorded.
struct LoginProfile {
    agent_id: String,
    account_label: String,
    /// The plugin's profile_dir_env name (e.g. GOOGLE_WORKSPACE_CLI_CONFIG_DIR).
    env_name: String,
    config_dir: String,
}

/// POST /plugins/{slug}/accounts/login
///
/// Per-account login for multi-account plugins. Body:
///   { "agentId": "...", "accountLabel": "work@acme.com" }
/// Allocates an isolated config dir for this (agent, account), runs the
/// plugin's login pointed at it, and records the profile on success.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountLoginRequest {
    pub agent_id: String,
    pub account_label: String,
}

pub async fn auth_login_account(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    Json(req): Json<AccountLoginRequest>,
) -> HandlerResult<serde_json::Value> {
    let (binary_path, auth) = state
        .plugin_store
        .get_auth_info(&slug)
        .ok_or_else(|| to_error_response(NeboError::NotFound))?;

    let env_name = auth.profile_dir_env.clone().ok_or_else(|| {
        to_error_response(NeboError::Internal(format!(
            "plugin '{slug}' does not support multiple accounts (no profile_dir_env declared)"
        )))
    })?;

    // Allocate an isolated, sanitized config dir for this (agent, account).
    let config_dir = plugin_profile_dir(&req.agent_id, &slug, &req.account_label);
    if let Err(e) = std::fs::create_dir_all(&config_dir) {
        return Err(to_error_response(NeboError::Internal(format!(
            "failed to create profile dir: {e}"
        ))));
    }

    spawn_plugin_login(
        state,
        slug,
        auth.commands.login.clone(),
        binary_path,
        auth.label.clone(),
        Some(LoginProfile {
            agent_id: req.agent_id,
            account_label: req.account_label,
            env_name,
            config_dir: config_dir.to_string_lossy().into_owned(),
        }),
    );
    Ok(Json(serde_json::json!({ "started": true })))
}

/// Per-(agent, plugin, account) credential directory. Lives under the Nebo
/// data dir so it's isolated from the global `~/.config/<plugin>` default.
fn plugin_profile_dir(agent_id: &str, slug: &str, account_label: &str) -> std::path::PathBuf {
    let sanitize = |s: &str| -> String {
        s.chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' })
            .collect()
    };
    let base = config::data_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    base.join("nebo")
        .join("plugin-profiles")
        .join(sanitize(agent_id))
        .join(sanitize(slug))
        .join(sanitize(account_label))
}

/// Shared background login flow used by both global and per-account login.
/// `profile = Some(..)` injects that account's config dir into the plugin's
/// profile_dir_env and records the profile mapping on success.
fn spawn_plugin_login(
    state: AppState,
    slug: String,
    login_command: String,
    binary_path: std::path::PathBuf,
    label: String,
    profile: Option<LoginProfile>,
) {
    let hub = state.hub.clone();
    let slug_owned = slug.clone();
    let store_for_restart = state.store.clone();
    let workers_for_restart = state.agent_workers.clone();
    let plugin_store_for_auth = state.plugin_store.clone();
    let tools_for_refresh = state.tools.clone();
    let profile_store = state.store.clone();

    info!(plugin = %slug, account = ?profile.as_ref().map(|p| &p.account_label), "starting plugin auth login");

    hub.broadcast(
        "plugin_auth_started",
        serde_json::json!({ "plugin": &slug, "label": &label }),
    );

    // Spawn background task — auth login may take minutes (user authorizes in browser).
    // gws writes the OAuth URL to stderr, so we read both streams and open the URL
    // with open::that(), mirroring how onboarding opens the browser.
    let plugin_store_clone = state.plugin_store.clone();
    tokio::spawn(async move {
        let runtime = napp::PluginRuntime::new(&slug_owned, binary_path, plugin_store_clone);
        let mut cmd = runtime.command(&login_command);
        // Per-account: point the plugin at this account's isolated config dir
        // so its login/token/refresh all land there, not the global default.
        if let Some(ref p) = profile {
            cmd.env(&p.env_name, &p.config_dir);
        }
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                warn!(plugin = %slug_owned, error = %e, "plugin auth login command failed");
                hub.broadcast(
                    "plugin_auth_error",
                    serde_json::json!({
                        "plugin": &slug_owned,
                        "error": e.to_string(),
                    }),
                );
                return;
            }
        };

        // Read stderr lines for OAuth URLs — gws writes to stderr, not stdout.
        let stderr_handle = child.stderr.take();
        let stdout_handle = child.stdout.take();
        let slug_for_stderr = slug_owned.clone();
        let hub_for_stderr = hub.clone();

        // Shared flag: once either stream opens a URL, the other skips.
        let url_opened = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let url_opened_stderr = url_opened.clone();
        let url_opened_stdout = url_opened.clone();

        // Read stderr/stdout in chunks, scanning for OAuth URLs. When found,
        // broadcast via WebSocket so the frontend can open the browser.
        let stderr_task = tokio::spawn(async move {
            let mut all = String::new();
            let mut opened = false;
            if let Some(mut stream) = stderr_handle {
                let mut buf = [0u8; 4096];
                loop {
                    opened = opened || url_opened_stderr.load(std::sync::atomic::Ordering::Relaxed);
                    let has_candidate = !opened && has_url_candidate(&all);
                    let read_result = if has_candidate {
                        match tokio::time::timeout(Duration::from_secs(1), stream.read(&mut buf))
                            .await
                        {
                            Ok(r) => r,
                            Err(_) => {
                                // Timeout — no more data coming, treat URL as complete.
                                if !url_opened_stderr.load(std::sync::atomic::Ordering::Relaxed) {
                                    if let Some(url) = extract_url(&all, true) {
                                        open_auth_url(&slug_for_stderr, &url, &hub_for_stderr);
                                        url_opened_stderr
                                            .store(true, std::sync::atomic::Ordering::Relaxed);
                                        opened = true;
                                    }
                                }
                                continue;
                            }
                        }
                    } else {
                        stream.read(&mut buf).await
                    };
                    match read_result {
                        Ok(0) => break,
                        Ok(n) => {
                            let chunk = String::from_utf8_lossy(&buf[..n]);
                            info!(plugin = %slug_for_stderr, chunk = %chunk, "plugin auth stderr");
                            all.push_str(&chunk);
                            if !opened
                                && !url_opened_stderr.load(std::sync::atomic::Ordering::Relaxed)
                            {
                                if let Some(url) = extract_url(&all, false) {
                                    open_auth_url(&slug_for_stderr, &url, &hub_for_stderr);
                                    url_opened_stderr
                                        .store(true, std::sync::atomic::Ordering::Relaxed);
                                    opened = true;
                                }
                            }
                        }
                        Err(_) => break,
                    }
                }
            }
            all
        });

        let slug_for_stdout = slug_owned.clone();
        let hub_for_stdout = hub.clone();
        let stdout_task = tokio::spawn(async move {
            let mut all = String::new();
            let mut opened = false;
            if let Some(mut stream) = stdout_handle {
                let mut buf = [0u8; 4096];
                loop {
                    opened = opened || url_opened_stdout.load(std::sync::atomic::Ordering::Relaxed);
                    let has_candidate = !opened && has_url_candidate(&all);
                    let read_result = if has_candidate {
                        match tokio::time::timeout(Duration::from_secs(1), stream.read(&mut buf))
                            .await
                        {
                            Ok(r) => r,
                            Err(_) => {
                                if !url_opened_stdout.load(std::sync::atomic::Ordering::Relaxed) {
                                    if let Some(url) = extract_url(&all, true) {
                                        open_auth_url(&slug_for_stdout, &url, &hub_for_stdout);
                                        url_opened_stdout
                                            .store(true, std::sync::atomic::Ordering::Relaxed);
                                        opened = true;
                                    }
                                }
                                continue;
                            }
                        }
                    } else {
                        stream.read(&mut buf).await
                    };
                    match read_result {
                        Ok(0) => break,
                        Ok(n) => {
                            let chunk = String::from_utf8_lossy(&buf[..n]);
                            info!(plugin = %slug_for_stdout, chunk = %chunk, "plugin auth stdout");
                            all.push_str(&chunk);
                            if !opened
                                && !url_opened_stdout.load(std::sync::atomic::Ordering::Relaxed)
                            {
                                if let Some(url) = extract_url(&all, false) {
                                    open_auth_url(&slug_for_stdout, &url, &hub_for_stdout);
                                    url_opened_stdout
                                        .store(true, std::sync::atomic::Ordering::Relaxed);
                                    opened = true;
                                }
                            }
                        }
                        Err(_) => break,
                    }
                }
            }
            all
        });

        let (stderr_output, stdout_output) = tokio::join!(stderr_task, stdout_task);
        let all_stderr = stderr_output.unwrap_or_default();
        let all_stdout = stdout_output.unwrap_or_default();

        match child.wait().await {
            Ok(status) if status.success() => {
                info!(plugin = %slug_owned, "plugin auth login succeeded");

                // Per-account: record the (agent, plugin, account) → config_dir
                // mapping now that the account's tokens exist in that dir.
                if let Some(ref p) = profile {
                    let id = format!("{}:{}:{}", p.agent_id, slug_owned, p.account_label);
                    if let Err(e) = profile_store.upsert_plugin_account_profile(
                        &id,
                        &p.agent_id,
                        &slug_owned,
                        &p.account_label,
                        &p.config_dir,
                    ) {
                        warn!(plugin = %slug_owned, error = %e, "failed to record account profile");
                    }
                }

                hub.broadcast(
                    "plugin_auth_complete",
                    serde_json::json!({
                        "plugin": &slug_owned,
                        "account": profile.as_ref().map(|p| p.account_label.clone()),
                    }),
                );

                // Update in-memory auth cache so getAgent reflects the change instantly
                plugin_store_for_auth.update_auth_status(&slug_owned).await;
                // Readiness may have changed — refresh plugin tool definition
                tools_for_refresh.refresh_definition("plugin").await;

                // Restart agent workers that depend on this plugin
                let store_r = store_for_restart.clone();
                let workers_r = workers_for_restart.clone();
                let slug_r = slug_owned.clone();
                tokio::spawn(async move {
                    if let Ok(agents) = store_r.list_agents(1000, 0) {
                        for agent in &agents {
                            if agent.is_enabled == 0 {
                                continue;
                            }
                            if let Ok(bindings) = store_r.list_agent_workflows(&agent.id) {
                                let uses_plugin = bindings.iter().any(|b| {
                                    b.trigger_type == "watch" && b.trigger_config.contains(&slug_r)
                                });
                                if uses_plugin {
                                    let notif_id = format!("auth-required:{}:{}", agent.id, slug_r);
                                    let _ = store_r.delete_notification(&notif_id, "");
                                    info!(
                                        agent = %agent.id,
                                        plugin = %slug_r,
                                        "restarting agent worker after plugin auth"
                                    );
                                    workers_r.start_agent(&agent.id, &agent.name, None).await;
                                }
                            }
                        }
                    }
                });
            }
            Ok(_status) => {
                let error = if all_stderr.trim().is_empty() {
                    all_stdout.trim().to_string()
                } else {
                    all_stderr.trim().to_string()
                };
                warn!(plugin = %slug_owned, error = %error, "plugin auth login failed");
                hub.broadcast(
                    "plugin_auth_error",
                    serde_json::json!({
                        "plugin": &slug_owned,
                        "error": error,
                    }),
                );
            }
            Err(e) => {
                warn!(plugin = %slug_owned, error = %e, "plugin auth login command failed");
                hub.broadcast(
                    "plugin_auth_error",
                    serde_json::json!({
                        "plugin": &slug_owned,
                        "error": e.to_string(),
                    }),
                );
            }
        }
    });
}

/// GET /plugins/{slug}/accounts?agentId=<id>
///
/// List the accounts an agent has connected for a multi-account plugin.
/// Used by the UI ("add another account") and surfaced to the agent so it
/// knows valid `--account` values. Returns account labels + which is primary
/// (never the credentials themselves — those live in the plugin's config dir).
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListAccountsQuery {
    pub agent_id: String,
}

pub async fn list_plugin_accounts(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    Query(q): Query<ListAccountsQuery>,
) -> HandlerResult<serde_json::Value> {
    let profiles = state
        .store
        .list_plugin_account_profiles(&q.agent_id, &slug)
        .map_err(to_error_response)?;
    let accounts: Vec<serde_json::Value> = profiles
        .iter()
        .map(|p| {
            serde_json::json!({
                "accountLabel": p.account_label,
                "isPrimary": p.is_primary,
                "needsReauth": p.needs_reauth,
            })
        })
        .collect();
    Ok(Json(serde_json::json!({ "accounts": accounts })))
}

/// POST /plugins/{slug}/auth/logout
///
/// Runs the plugin's auth logout command. Returns immediately.
pub async fn auth_logout(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let (binary_path, auth) = state
        .plugin_store
        .get_auth_info(&slug)
        .ok_or_else(|| to_error_response(NeboError::NotFound))?;

    let logout_cmd = auth.commands.logout.as_deref().ok_or_else(|| {
        to_error_response(NeboError::Validation(
            "plugin has no auth logout command".into(),
        ))
    })?;

    let runtime = napp::PluginRuntime::new(&slug, binary_path, state.plugin_store.clone());
    let mut cmd = runtime.command(logout_cmd);

    let output = cmd
        .output()
        .await
        .map_err(|e| to_error_response(NeboError::Internal(e.to_string())))?;

    if output.status.success() {
        info!(plugin = %slug, "plugin auth logout succeeded");
        // Update in-memory auth cache so getAgent reflects the change instantly
        state.plugin_store.update_auth_status(&slug).await;
        state.tools.refresh_definition("plugin").await;
        Ok(Json(serde_json::json!({ "success": true })))
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        warn!(plugin = %slug, error = %stderr, "plugin auth logout failed");
        Err(to_error_response(NeboError::Internal(format!(
            "logout failed: {}",
            stderr
        ))))
    }
}

/// Remove a plugin and all its versions from disk + DB registry, and unregister
/// its hooks. The ONE canonical plugin-removal path — shared by the settings
/// DELETE /plugins/{slug} handler and the marketplace uninstall flow, so both
/// uninstall a plugin identically (CODE_AUDITOR Rule 8). Disk removal is the
/// critical path; the DB delete is best-effort.
pub fn remove_plugin_by_slug(state: &AppState, slug: &str) -> Result<(), NeboError> {
    state
        .plugin_store
        .remove(slug)
        .map_err(|e| NeboError::Internal(e.to_string()))?;

    if let Err(e) = state.store.delete_installed_plugin(slug) {
        warn!(plugin = %slug, error = %e, "failed to delete plugin from DB registry");
    }

    state.hooks.unregister_app(slug);
    info!(plugin = %slug, "plugin removed");
    Ok(())
}

/// DELETE /plugins/{slug}
///
/// Removes a plugin and all its versions from disk and DB registry.
pub async fn remove_plugin(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> HandlerResult<serde_json::Value> {
    remove_plugin_by_slug(&state, &slug).map_err(to_error_response)?;
    Ok(Json(serde_json::json!({ "message": "Plugin removed" })))
}

/// GET /plugins/{slug}/dependents
///
/// Lists all installed skills and agents that depend on this plugin.
/// Used by the frontend to determine whether a plugin can be safely removed.
pub async fn list_dependents(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> HandlerResult<serde_json::Value> {
    // Skills that declare this plugin as a dependency, excluding skills bundled
    // inside the plugin's own directory (those are part of the plugin itself).
    let all_skills = state.skill_loader.list().await;
    let plugin_skills_prefix = format!("/plugins/{}/", slug);
    let skill_dependents: Vec<serde_json::Value> = all_skills
        .iter()
        .filter(|s| s.plugins.iter().any(|p| p.name == slug))
        .filter(|s| {
            // Exclude skills whose source_path is inside the plugin directory
            if let Some(ref path) = s.source_path {
                !path.to_string_lossy().contains(&plugin_skills_prefix)
            } else {
                true
            }
        })
        .map(|s| {
            serde_json::json!({
                "name": s.name,
                "description": s.description,
                "type": "skill",
            })
        })
        .collect();

    // Agents that declare this plugin in requires.plugins or use it in a Watch trigger.
    let all_agents = state.agent_loader.list().await;
    let agent_dependents: Vec<serde_json::Value> = all_agents
        .iter()
        .filter(|a| {
            if let Some(cfg) = &a.config {
                let in_requires = cfg.requires.plugins.iter().any(|p| p.contains(&slug));
                let in_triggers = cfg.workflows.values().any(|w| {
                    matches!(&w.trigger, napp::agent::AgentTrigger::Watch { plugin, .. } if plugin == &slug)
                });
                in_requires || in_triggers
            } else {
                false
            }
        })
        .map(|a| {
            serde_json::json!({
                "name": a.agent_def.name,
                "description": a.agent_def.description,
                "type": "agent",
            })
        })
        .collect();

    let total = skill_dependents.len() + agent_dependents.len();
    Ok(Json(serde_json::json!({
        "skills": skill_dependents,
        "agents": agent_dependents,
        "total": total,
    })))
}

/// Check if a plugin is authenticated.
///
/// Returns `None` if the plugin has no auth config or no status command,
/// `Some(true)` if authenticated, `Some(false)` if not.
pub(crate) async fn check_plugin_auth(
    plugin_store: &std::sync::Arc<napp::plugin::PluginStore>,
    slug: &str,
) -> Option<bool> {
    let (_binary_path, auth) = plugin_store.get_auth_info(slug)?;
    // None = no status command → nothing to check (caller treats as "no auth needed").
    auth.commands.status.as_deref()?;
    // Delegate the decision to the one canonical check (rich interpretation, cached).
    Some(plugin_store.check_auth_lazy(slug).await)
}

/// GET /plugins/{slug}/auth/status
///
/// Returns `{ "authenticated": bool }`. The decision is computed by the one
/// canonical check (`PluginStore::check_auth_now` → `run_auth_status_check`),
/// which interprets reporter-style status output (explicit boolean / "none"
/// credential signals) rather than the raw exit code, and refreshes the cache.
pub async fn auth_status(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> HandlerResult<serde_json::Value> {
    // 404 only when the plugin isn't installed at all.
    if state.plugin_store.get_auth_info(&slug).is_none() {
        return Err(to_error_response(NeboError::NotFound));
    }
    let authenticated = state.plugin_store.check_auth_now(&slug).await;
    Ok(Json(serde_json::json!({ "authenticated": authenticated })))
}

/// GET /plugins/events
///
/// Lists all declared events across all installed plugins.
pub async fn list_all_plugin_events(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let installed = state.plugin_store.list_installed();

    // Dedup by slug (highest version wins).
    let mut seen = HashMap::new();
    for (slug, version, _binary_path, _source) in &installed {
        seen.entry(slug.clone()).or_insert_with(|| version.clone());
    }

    let mut events = Vec::new();
    for slug in seen.keys() {
        if let Some(event_defs) = state.plugin_store.get_events(slug) {
            for ev in &event_defs {
                events.push(serde_json::json!({
                    "plugin": slug,
                    "name": ev.name,
                    "source": format!("{}.{}", slug, ev.name),
                    "description": ev.description,
                    "multiplexed": ev.multiplexed,
                }));
            }
        }
    }

    events.sort_by(|a, b| {
        a["source"]
            .as_str()
            .unwrap_or("")
            .cmp(b["source"].as_str().unwrap_or(""))
    });

    let total = events.len();
    Ok(Json(serde_json::json!({
        "events": events,
        "total": total,
    })))
}

/// GET /plugins/{slug}/events
///
/// Lists declared events for a specific plugin.
pub async fn list_plugin_events(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let event_defs = state.plugin_store.get_events(&slug).unwrap_or_default();

    let events: Vec<serde_json::Value> = event_defs
        .iter()
        .map(|ev| {
            serde_json::json!({
                "name": ev.name,
                "source": format!("{}.{}", slug, ev.name),
                "description": ev.description,
                "multiplexed": ev.multiplexed,
            })
        })
        .collect();

    let total = events.len();
    Ok(Json(serde_json::json!({
        "plugin": slug,
        "events": events,
        "total": total,
    })))
}

/// GET /plugins/{slug}/config
///
/// Returns the plugin's config schema merged with stored values.
/// Secret values are redacted in the response.
pub async fn get_plugin_config(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let manifest = state
        .plugin_store
        .get_manifest(&slug)
        .ok_or_else(|| to_error_response(NeboError::NotFound))?;

    let schema = manifest
        .capabilities
        .as_ref()
        .map(|c| &c.config_schema[..])
        .unwrap_or(&[]);

    let stored = state
        .store
        .list_plugin_settings_by_slug(&slug)
        .unwrap_or_default();

    let stored_map: HashMap<String, (String, bool)> = stored
        .into_iter()
        .map(|s| (s.setting_key, (s.setting_value, s.is_secret != 0)))
        .collect();

    let fields: Vec<serde_json::Value> = schema
        .iter()
        .map(|field| {
            let (value, is_secret) = stored_map
                .get(&field.key)
                .cloned()
                .unwrap_or_else(|| (field.default.clone().unwrap_or_default(), field.secret));
            let display_value = if is_secret && !value.is_empty() {
                "********".to_string()
            } else {
                value
            };
            serde_json::json!({
                "key": field.key,
                "label": field.label,
                "description": field.description,
                "fieldType": field.field_type,
                "default": field.default,
                "required": field.required,
                "secret": field.secret,
                "options": field.options,
                "value": display_value,
            })
        })
        .collect();

    Ok(Json(serde_json::json!({
        "plugin": slug,
        "config": fields,
    })))
}

/// PUT /plugins/{slug}/config
///
/// Replaces all config values for a plugin. Validates against the schema.
pub async fn set_plugin_config(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    Json(body): Json<HashMap<String, String>>,
) -> HandlerResult<serde_json::Value> {
    let manifest = state
        .plugin_store
        .get_manifest(&slug)
        .ok_or_else(|| to_error_response(NeboError::NotFound))?;

    let schema = manifest
        .capabilities
        .as_ref()
        .map(|c| &c.config_schema[..])
        .unwrap_or(&[]);

    // Validate required fields
    for field in schema {
        if field.required && !body.contains_key(&field.key) {
            return Err(to_error_response(NeboError::Validation(format!(
                "missing required config field: {}",
                field.key
            ))));
        }
    }

    // Collect allowed env var keys from auth.env (any auth type)
    let auth_env_keys: HashSet<&str> = manifest
        .auth
        .as_ref()
        .map(|a| a.env.keys().map(|k| k.as_str()).collect())
        .unwrap_or_default();

    let schema_map: HashMap<&str, &napp::plugin::PluginConfigField> =
        schema.iter().map(|f| (f.key.as_str(), f)).collect();

    // Store each value (keys declared in schema OR auth.env)
    for (key, value) in &body {
        if let Some(field) = schema_map.get(key.as_str()) {
            if let Err(e) = state
                .store
                .upsert_plugin_setting_by_slug(&slug, key, value, field.secret)
            {
                warn!(plugin = %slug, key = %key, error = %e, "failed to save plugin config");
                return Err(to_error_response(NeboError::Internal(e.to_string())));
            }
        } else if auth_env_keys.contains(key.as_str()) {
            // Auth env vars are always secrets
            if let Err(e) = state
                .store
                .upsert_plugin_setting_by_slug(&slug, key, value, true)
            {
                warn!(plugin = %slug, key = %key, error = %e, "failed to save plugin env var");
                return Err(to_error_response(NeboError::Internal(e.to_string())));
            }
        }
    }

    // Update in-memory env var cache so plugin commands get the new values immediately
    for (key, value) in &body {
        if auth_env_keys.contains(key.as_str()) || schema_map.contains_key(key.as_str()) {
            state.plugin_store.set_env_var(&slug, key, value);
        }
    }

    // Readiness may have changed — refresh plugin tool definition
    state.tools.refresh_definition("plugin").await;

    info!(plugin = %slug, keys = body.len(), "updated plugin config");
    Ok(Json(serde_json::json!({ "success": true })))
}

/// GET /plugins/{slug}/diagnostics
///
/// Returns the diagnostic timeline for a plugin (install, verification, runtime events).
pub async fn get_diagnostics(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let diags = state.plugin_store.get_diagnostics(&slug);
    let entries: Vec<serde_json::Value> = diags
        .iter()
        .map(|d| {
            serde_json::json!({
                "level": d.level,
                "phase": d.phase,
                "message": d.message,
                "timestamp": d.timestamp,
            })
        })
        .collect();

    Ok(Json(serde_json::json!({
        "plugin": slug,
        "diagnostics": entries,
        "total": entries.len(),
    })))
}

/// ANY /plugins/{slug}/api/{*path}
///
/// Proxy handler for plugin-declared HTTP routes (e.g., OAuth callbacks, webhooks).
/// Matches the request path and method against the plugin's `capabilities.routes[]`.
pub async fn proxy_plugin_route(
    State(state): State<AppState>,
    Path((slug, path)): Path<(String, String)>,
    method: axum::http::Method,
    body: axum::body::Bytes,
) -> HandlerResult<serde_json::Value> {
    let manifest = state
        .plugin_store
        .get_manifest(&slug)
        .ok_or_else(|| to_error_response(NeboError::NotFound))?;

    let caps = manifest
        .capabilities
        .as_ref()
        .ok_or_else(|| to_error_response(NeboError::NotFound))?;

    // Find matching route by path and method
    let route_def = caps
        .routes
        .iter()
        .find(|r| {
            let r_path = r.path.trim_start_matches('/');
            r_path == path && r.method.eq_ignore_ascii_case(method.as_str())
        })
        .ok_or_else(|| to_error_response(NeboError::NotFound))?;

    let binary = state
        .plugin_store
        .resolve(&slug, "*")
        .ok_or_else(|| to_error_response(NeboError::Internal("plugin binary not found".into())))?;

    let runtime = napp::PluginRuntime::new(&slug, binary, state.plugin_store.clone())
        .with_permissions();
    let timeout = runtime.effective_timeout(Duration::from_secs(30));
    let mut cmd = runtime.command(&route_def.command);
    cmd.stdin(std::process::Stdio::piped());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| to_error_response(NeboError::Internal(format!("spawn: {}", e))))?;

    // Write request body to stdin
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        let _ = stdin.write_all(&body).await;
        drop(stdin);
    }

    let output = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| to_error_response(NeboError::Internal("route handler timed out".into())))?
        .map_err(|e| to_error_response(NeboError::Internal(e.to_string())))?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        // Try to parse as JSON, otherwise return as raw text
        match serde_json::from_str::<serde_json::Value>(&stdout) {
            Ok(json) => Ok(Json(json)),
            Err(_) => Ok(Json(serde_json::json!({ "output": stdout.trim() }))),
        }
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(to_error_response(NeboError::Internal(format!(
            "route handler failed: {}",
            stderr.trim()
        ))))
    }
}

/// Open an OAuth URL: broadcast it to the frontend via WebSocket so the
/// frontend can call `window.open()`.
fn open_auth_url(slug: &str, url: &str, hub: &super::ws::ClientHub) {
    info!(plugin = %slug, url = %url, "broadcasting plugin OAuth URL to frontend");
    hub.broadcast(
        "plugin_auth_url",
        serde_json::json!({
            "plugin": slug,
            "url": url,
        }),
    );
}

/// Returns true if the text contains a URL-like token that `extract_url(text, false)`
/// would skip because it's the last token without trailing whitespace.
fn has_url_candidate(text: &str) -> bool {
    let words: Vec<&str> = text.split_whitespace().collect();
    if let Some(last) = words.last() {
        let trimmed = last.trim_matches(|c: char| c == '"' || c == '\'' || c == '<' || c == '>');
        (trimmed.starts_with("https://") || trimmed.starts_with("http://"))
            && !text.ends_with(char::is_whitespace)
    } else {
        false
    }
}

/// Extract the first HTTP(S) URL from accumulated output text.
///
/// When `complete` is false (streaming), only returns a URL that is followed by
/// more text or trailing whitespace — this avoids matching a partial URL that is
/// still being written. When `complete` is true (after EOF or timeout), the last
/// token is accepted unconditionally since no more data is expected.
fn extract_url(text: &str, complete: bool) -> Option<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    for (i, word) in words.iter().enumerate() {
        let trimmed = word.trim_matches(|c: char| c == '"' || c == '\'' || c == '<' || c == '>');
        if trimmed.starts_with("https://") || trimmed.starts_with("http://") {
            let is_last = i == words.len() - 1;
            if complete || !is_last || text.ends_with(char::is_whitespace) {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

/// GET /plugins/{slug}/help — list help docs from the plugin's help/ directory.
pub async fn get_plugin_help(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let docs = state.plugin_store.list_help_docs(&slug);
    let entries: Vec<serde_json::Value> = docs
        .into_iter()
        .map(|(name, content)| {
            serde_json::json!({
                "name": name,
                "content": content,
            })
        })
        .collect();
    Ok(Json(serde_json::json!({ "docs": entries })))
}

/// POST /plugins/{slug}/help/chat — open an interactive help chat session.
///
/// Creates a dedicated help session with plugin docs as context, seeds it
/// with an assistant greeting, and returns the session key + chat ID.
/// The frontend embeds a mini chat in the setup modal and sends follow-up
/// messages via WebSocket using the returned session key.
pub async fn start_help_chat(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    let agent_id = body["agentId"]
        .as_str()
        .unwrap_or("assistant")
        .to_string();

    // Load plugin manifest for name
    let plugin_name = state
        .plugin_store
        .get_manifest(&slug)
        .map(|m| m.name)
        .unwrap_or_else(|| slug.clone());

    // Load help docs
    let docs = state.plugin_store.list_help_docs(&slug);

    // Also load inline help from auth config
    let auth_help_text = state
        .plugin_store
        .get_manifest(&slug)
        .and_then(|m| m.auth)
        .and_then(|a| a.help)
        .and_then(|h| h.text);

    // Build system context from all help sources
    let mut system_parts = vec![format!(
        "You are a setup assistant for the {} plugin. \
         Help the user configure and connect this plugin. \
         Be concise and guide them step by step. \
         If they ask something outside the scope of this plugin, \
         politely redirect them to the main chat.",
        plugin_name
    )];

    if let Some(text) = &auth_help_text {
        system_parts.push(format!("## Quick Setup\n{text}"));
    }

    for (name, content) in &docs {
        system_parts.push(format!("## {name}\n{content}"));
    }

    let system_context = system_parts.join("\n\n");

    // Create a dedicated help session so the context stays isolated.
    // Don't rotate — the default chat (keyed by session name) is used for
    // both storage and retrieval, keeping get_session_messages compatible.
    let session_key =
        agent::keyparser::build_agent_session_key(&agent_id, &format!("help:{slug}"));

    let session = state
        .runner
        .sessions()
        .get_or_create(&session_key, "")
        .map_err(to_error_response)?;

    // Only seed if this is a fresh session (no messages yet).
    let existing = state
        .runner
        .sessions()
        .get_messages(&session.id)
        .unwrap_or_default();

    if existing.is_empty() {
        // Inject the help docs as a system message so every follow-up turn
        // has context, then add an assistant greeting.
        let _ = state.runner.sessions().append_message(
            &session.id,
            "system",
            &system_context,
            None,
            None,
            None,
        );

        let greeting = format!(
            "Hi! I'm here to help you set up **{}**. What would you like to know?",
            plugin_name
        );
        let _ = state.runner.sessions().append_message(
            &session.id,
            "assistant",
            &greeting,
            None,
            None,
            None,
        );
    }

    Ok(Json(serde_json::json!({
        "sessionKey": session_key,
        "agentId": agent_id,
    })))
}

/// POST /plugins/{slug}/setup
///
/// Execute a `Generate` step from an artifact's setup wizard. Reads the
/// plugin's `setup.steps[stepIndex]` (must be a `Generate` step), runs
/// its command with `{{key}}` placeholders in args substituted from the
/// `values` map, and returns the resulting stdout. Stderr is captured
/// and returned only if the command fails.
///
/// Request body:
///   { "stepIndex": <number>, "values": { "<key>": "<value>", ... } }
///
/// Response (success):
///   { "ok": true, "output": "<stdout>", "outputFormat": "<yaml|json|...>" }
///
/// Response (failure):
///   { "ok": false, "error": "<message>", "stderr": "<stderr>", "exitCode": <n> }
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupRunRequest {
    pub step_index: usize,
    #[serde(default)]
    pub values: HashMap<String, String>,
}

pub async fn plugin_setup_run(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    Json(req): Json<SetupRunRequest>,
) -> HandlerResult<serde_json::Value> {
    let binary_path = state
        .plugin_store
        .resolve(&slug, "*")
        .ok_or_else(|| to_error_response(NeboError::NotFound))?;

    let manifest = state
        .plugin_store
        .get_manifest(&slug)
        .ok_or_else(|| to_error_response(NeboError::NotFound))?;

    let setup = manifest.setup.as_ref().ok_or_else(|| {
        to_error_response(NeboError::Internal(format!(
            "plugin '{slug}' has no setup wizard declared"
        )))
    })?;

    let step = setup.steps.get(req.step_index).ok_or_else(|| {
        to_error_response(NeboError::Internal(format!(
            "step index {} out of range (have {} steps)",
            req.step_index,
            setup.steps.len()
        )))
    })?;

    let (command, args, output_format) = match step {
        napp::plugin::ArtifactSetupStep::Generate {
            command,
            args,
            output_format,
            ..
        } => (command.clone(), args.clone(), output_format.clone()),
        _ => {
            return Err(to_error_response(NeboError::Internal(format!(
                "step {} is not a Generate step",
                req.step_index
            ))));
        }
    };

    // Substitute {{key}} placeholders in args from the values map.
    // Missing keys leave the placeholder intact — the binary's own
    // validation surfaces the error, which is more informative than
    // a generic "missing key" here.
    let substituted_args: Vec<String> = args
        .into_iter()
        .map(|arg| substitute_placeholders(&arg, &req.values))
        .collect();

    info!(
        plugin = %slug,
        command = %command,
        "running plugin setup-generate"
    );

    // Run the command. Capture stdout + stderr. Setup commands are
    // synchronous render operations — no need for the URL-extraction
    // dance auth_login does.
    let runtime = napp::PluginRuntime::new(&slug, binary_path, state.plugin_store.clone());
    let mut cmd = runtime.command(&command);
    for a in &substituted_args {
        cmd.arg(a);
    }
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let output = cmd.output().await.map_err(|e| {
        to_error_response(NeboError::Internal(format!(
            "failed to spawn setup command: {e}"
        )))
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        return Ok(Json(serde_json::json!({
            "ok": false,
            "error": "command exited non-zero",
            "stderr": stderr,
            "exitCode": output.status.code().unwrap_or(-1),
        })));
    }

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    Ok(Json(serde_json::json!({
        "ok": true,
        "output": stdout,
        "outputFormat": output_format,
    })))
}

/// Replace `{{key}}` placeholders in `template` with values from `vars`.
/// Keys with no matching value are left in place — the called binary
/// surfaces the error, which is more informative than a generic miss here.
fn substitute_placeholders(template: &str, vars: &HashMap<String, String>) -> String {
    let mut out = template.to_string();
    for (key, value) in vars {
        let needle = format!("{{{{{}}}}}", key);
        out = out.replace(&needle, value);
    }
    out
}
