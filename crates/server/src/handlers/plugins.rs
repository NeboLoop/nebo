//! Plugin handlers — listing installed plugins and authentication.
//!
//! Plugins that require credentials (e.g., GWS needing Google OAuth) declare
//! auth requirements in their manifest. These handlers run the plugin's own
//! auth CLI commands and report status via WebSocket events.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

/// Bound on a plugin's `setup.command` run from the install flow. Generous —
/// setup can download models or seed data — but finite: this executes inside
/// an HTTP handler, and an unbounded child both hangs the request and leaks
/// the process when the client gives up.
const SETUP_COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// Bound on a plugin's per-account `logout` run during disconnect. Logout is
/// where server-side release happens (revoking a token, returning a number);
/// disconnect must still finish when the plugin or network is broken.
const ACCOUNT_LOGOUT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Frontend route of the Plugins settings page (the reconnect destination in
/// owner-facing notices).
const PLUGINS_SETTINGS_PATH: &str = "/settings/plugins";

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

    // Pending updates (slug → newer version) so the list can show an "update
    // available" badge, keyed off the same artifact_update_prefs the Updates panel
    // and the background checker use.
    let pending_updates: HashMap<String, String> = state
        .store
        .list_artifacts_with_updates()
        .unwrap_or_default()
        .into_iter()
        .filter(|a| a.artifact_type == "plugin")
        .map(|a| (a.artifact_id, a.remote_version))
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
                    // env-type auth: every key is a user-provided credential.
                    // Other types (oauth_cli): keys are publisher-prefilled client
                    // credentials users never touch — unless the publisher shipped
                    // them empty, in which case the user must supply their own
                    // (per the manifest's auth.help instructions), so surface
                    // exactly the empty ones as input fields.
                    let env_vars: Vec<String> = if auth.auth_type == "env" {
                        auth.env.keys().cloned().collect()
                    } else {
                        auth.env
                            .iter()
                            .filter(|(_, v)| v.is_empty())
                            .map(|(k, _)| k.clone())
                            .collect()
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
            "updateAvailable": pending_updates.get(slug.as_str()),
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
    /// The exact resource the account should attach ("+18015551234" for a
    /// phone line) — chosen by the user in the connect modal so which line
    /// lands on which agent is never an invisible default. Empty = the
    /// plugin's own default behavior.
    account_number: String,
    /// The plugin's profile_dir_env name (e.g. GOOGLE_WORKSPACE_CLI_CONFIG_DIR).
    env_name: String,
    config_dir: String,
}

/// POST /plugins/{slug}/accounts/login
///
/// Login from a card or the agent settings, which always ask for an account
/// on behalf of one agent. Body:
///   { "agentId": "...", "accountLabel": "work@acme.com" }
/// A multi-account plugin (one that declares `profile_dir_env`) gets an
/// isolated config dir for this (agent, account); the login runs pointed at
/// it and the profile is recorded on success. A single-account plugin has
/// nowhere to keep a second set of credentials, so its one shared login
/// runs instead, the same thing `auth/login` does. Refusing it with a 500
/// left the QuickBooks connect card reading "Sign-in didn't complete" on
/// every click (2026-09-04) when no sign-in had been started at all.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountLoginRequest {
    pub agent_id: String,
    pub account_label: String,
    /// Optional resource selector (a phone line's E.164) — see LoginProfile.
    #[serde(default)]
    pub account_number: String,
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

    let profile = login_profile(auth.profile_dir_env.clone(), &slug, req);
    if let Some(p) = &profile {
        // Allocate an isolated, sanitized config dir for this (agent, account).
        if let Err(e) = std::fs::create_dir_all(&p.config_dir) {
            return Err(to_error_response(NeboError::Internal(format!(
                "failed to create profile dir: {e}"
            ))));
        }
    } else {
        tracing::info!(slug, "single-account plugin: running its shared login for the account card");
    }
    let per_account = profile.is_some();
    spawn_plugin_login(
        state,
        slug,
        auth.commands.login.clone(),
        binary_path,
        auth.label.clone(),
        profile,
    );
    Ok(Json(serde_json::json!({ "started": true, "perAccount": per_account })))
}

/// The per-account context for a login, or `None` when the plugin keeps one
/// shared set of credentials (no `profile_dir_env`), in which case the login
/// runs the way `auth/login` runs it.
fn login_profile(profile_dir_env: Option<String>, slug: &str, req: AccountLoginRequest) -> Option<LoginProfile> {
    let env_name = profile_dir_env?;
    let config_dir = plugin_profile_dir(&req.agent_id, slug, &req.account_label);
    Some(LoginProfile {
        agent_id: req.agent_id,
        account_label: req.account_label,
        account_number: req.account_number,
        env_name,
        config_dir: config_dir.to_string_lossy().into_owned(),
    })
}

/// Per-(agent, plugin, account) credential directory. Lives under the Nebo
/// data dir so it's isolated from the global `~/.config/<plugin>` default.
/// The path shape is owned by config::plugin_account_dir — channel bridges
/// scan its parent (config::plugin_profiles_root), so the two must agree.
fn plugin_profile_dir(agent_id: &str, slug: &str, account_label: &str) -> std::path::PathBuf {
    config::plugin_account_dir(agent_id, slug, account_label)
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
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
    let event_bus = state.event_bus.clone();

    info!(plugin = %slug, account = ?profile.as_ref().map(|p| &p.account_label), "starting plugin auth login");

    hub.broadcast(
        "plugin_auth_started",
        serde_json::json!({ "plugin": &slug, "label": &label }),
    );

    // Spawn background task — auth login may take minutes (user authorizes in browser).
    // gws writes the OAuth URL to stderr, so we read both streams and open the URL
    // with open::that(), mirroring how onboarding opens the browser.
    let plugin_store_clone = state.plugin_store.clone();
    let neboai_api_url = state.config.neboai.api_url.clone();
    tokio::spawn(async move {
        let runtime = napp::PluginRuntime::new(&slug_owned, binary_path, plugin_store_clone);
        let mut cmd = runtime.command(&login_command);
        // Per-account: point the plugin at this account's isolated config dir
        // so its login/token/refresh all land there, not the global default.
        // The login also gets this Nebo's own address and the agent it acts
        // for — some logins are server-side flows through the local API (the
        // phone plugin's "login" provisions a number via the cloud) rather
        // than third-party OAuth.
        // Unconditional: a GLOBAL (no-profile) login needs the local API too —
        // hub-managed installs exchange their auth code through it. Gating this
        // on the per-account profile silently broke exactly the default path.
        for (key, value) in napp::plugin::plugin_base_env() {
            cmd.env(key, value);
        }
        if let Some(ref p) = profile {
            cmd.env(&p.env_name, &p.config_dir);
            cmd.env("NEBO_AGENT_ID", &p.agent_id);
            // The account's display label ("Front Desk") — server-side logins
            // pass it upstream so the same name identifies the account
            // everywhere (e.g. a phone line's label on neboai.com).
            cmd.env("NEBO_ACCOUNT_LABEL", &p.account_label);
            if !p.account_number.is_empty() {
                cmd.env("NEBO_ACCOUNT_NUMBER", &p.account_number);
            }
        }
        // Cloud bots (NEBOAI_PUBLIC_OAUTH=1, set by the provisioner): the user's
        // browser can't reach the pod's loopback listener, so hand the plugin
        // the ONE public redirect + an opaque state and register the pending
        // auth for the hub-relayed callback (crate::plugin_oauth). Desktop
        // never sets the env, so this pathway can't affect it.
        if crate::plugin_oauth::public_oauth_enabled() {
            match config::read_bot_id().filter(|id| !id.is_empty()) {
                Some(bot_id) => match crate::plugin_oauth::begin(&bot_id) {
                    Ok(relay) => {
                        info!(plugin = %slug_owned, port = relay.port, "plugin auth: using public OAuth redirect via hub relay");
                        cmd.env("NEBO_OAUTH_REDIRECT_URI", crate::plugin_oauth::PUBLIC_REDIRECT_URI);
                        cmd.env("NEBO_OAUTH_STATE", &relay.state);
                        cmd.env("NEBO_OAUTH_PORT", relay.port.to_string());
                        // The https relay redirect only exists on the plugin's
                        // CLOUD (Web-application) OAuth client — the Desktop
                        // client in the manifest cannot register it, so an
                        // authorize URL built with the manifest's client id
                        // dies at Google with redirect_uri_mismatch. Ask the
                        // hub which client id backs the relay (public
                        // identifier; the secret stays hub-side). Best-effort:
                        // plugins with one client for both modes (or BYO
                        // setups) work without it.
                        let cloud_client = match (
                            config::read_bot_id(),
                            profile_store.list_all_active_auth_profiles_by_provider("neboai"),
                        ) {
                            (Some(bot_id), Ok(profiles)) if !profiles.is_empty() => {
                                let api = comm::api::NeboAIApi::new(
                                    neboai_api_url.clone(),
                                    bot_id,
                                    profiles[0].api_key.clone(),
                                );
                                api.plugin_oauth_client(&slug_owned).await.ok()
                            }
                            _ => None,
                        };
                        match cloud_client {
                            Some(client_id) => {
                                cmd.env("NEBO_OAUTH_CLIENT_ID", client_id);
                            }
                            None => {
                                warn!(plugin = %slug_owned, "plugin auth: no cloud client id from hub; using manifest client id");
                            }
                        }
                    }
                    Err(e) => {
                        warn!(plugin = %slug_owned, error = %e, "plugin auth: failed to set up public OAuth relay; falling back to loopback redirect");
                    }
                },
                None => {
                    warn!(plugin = %slug_owned, "plugin auth: NEBOAI_PUBLIC_OAUTH set but no bot_id configured; falling back to loopback redirect");
                }
            }
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
                    // A successful reconnect means the token is healthy again —
                    // clear needs_reauth NOW so the "Expired" badge disappears
                    // immediately, instead of lingering until the next refresher
                    // tick. (reauth_notified is reset too, so a future expiry re-notifies.)
                    let _ = profile_store.set_plugin_account_reauth(&id, false);

                    // Lifecycle event: a specific account was connected. Per-account onboarding
                    // workflows subscribe via an `event` trigger on "account.connected" and read
                    // account/config_dir/agent_id from _event_payload.
                    event_bus.emit(tools::events::Event {
                        source: "account.connected".to_string(),
                        payload: serde_json::json!({
                            "plugin": &slug_owned,
                            "account_label": &p.account_label,
                            "config_dir": &p.config_dir,
                            "agent_id": &p.agent_id,
                        }),
                        origin: format!("auth:plugin:{slug_owned}"),
                        timestamp: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs(),
                    });
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

/// GET /plugins/oauth/relay?code=...&state=...
///
/// Hub-relayed OAuth callback (crate::plugin_oauth). The hub forwards Google's
/// redirect into this bot's tunnel; the single-use nonce inside `state` is the
/// auth (the route is public — Google's redirect carries no session), and the
/// loopback port is resolved from the pending registry, never from the request.
pub async fn oauth_relay(
    axum::extract::RawQuery(raw_query): axum::extract::RawQuery,
) -> (axum::http::StatusCode, Json<serde_json::Value>) {
    let raw_query = raw_query.unwrap_or_default();
    let (status, message) = crate::plugin_oauth::relay_request(&raw_query).await;
    (
        axum::http::StatusCode::from_u16(status)
            .unwrap_or(axum::http::StatusCode::INTERNAL_SERVER_ERROR),
        Json(serde_json::json!({ "status": status, "message": message })),
    )
}

/// POST /plugins/oauth/token
///
/// Relay hop for hub-held OAuth client secrets. A plugin whose manifest ships
/// no client secret performs its token exchange here instead of directly with
/// the provider: this handler forwards the body to the hub with the bot's JWT
/// (the plugin never holds hub credentials), and the hub — which alone knows
/// the secret and the provider's token endpoint — returns the provider token
/// JSON verbatim. No logic beyond attaching identity lives here, and the
/// response body is NEVER logged: it contains live user tokens.
///
/// Trust model matches the sibling plugin routes: the server binds loopback,
/// so callers are local processes this Nebo spawned.
pub async fn oauth_token(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> (axum::http::StatusCode, Json<serde_json::Value>) {
    let err = |status: axum::http::StatusCode, msg: String| {
        (status, Json(serde_json::json!({ "error": msg })))
    };

    let Some(bot_id) = config::read_bot_id() else {
        return err(
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "not connected to NeboAI — this plugin's OAuth app is managed by the hub, which requires a connected instance (or set the plugin's client secret locally to use your own OAuth app)"
                .to_string(),
        );
    };
    let profile = match state.store.list_all_active_auth_profiles_by_provider("neboai") {
        Ok(profiles) => match profiles.into_iter().next() {
            Some(p) => p,
            None => {
                return err(
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    "not connected to NeboAI — redeem a NEBO code first".to_string(),
                )
            }
        },
        Err(e) => {
            return err(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to query auth profiles: {e}"),
            )
        }
    };

    let api = comm::api::NeboAIApi::new(
        state.config.neboai.api_url.clone(),
        bot_id,
        profile.api_key.clone(),
    );
    match api.plugin_oauth_token(&body).await {
        Ok(tokens) => (axum::http::StatusCode::OK, Json(tokens)),
        // The hub's error text is safe to surface (it never echoes secrets),
        // and the plugin needs it verbatim to tell an expired grant from a
        // config problem.
        Err(e) => err(axum::http::StatusCode::BAD_GATEWAY, e.to_string()),
    }
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

/// DELETE /plugins/{slug}/accounts?agentId=<id>&accountLabel=<label>
///
/// Disconnect one account from an agent for a multi-account plugin: remove the
/// (agent, plugin, account) profile mapping and delete its credential directory
/// (the plugin owns the tokens there). Idempotent.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DisconnectAccountQuery {
    pub agent_id: String,
    pub account_label: String,
}

pub async fn disconnect_plugin_account(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    Query(q): Query<DisconnectAccountQuery>,
) -> HandlerResult<serde_json::Value> {
    disconnect_account(&state, &slug, &q.agent_id, &q.account_label)
        .await
        .map_err(to_error_response)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// Disconnect one (agent, plugin, account): the plugin's own logout against
/// that account's config dir, then the profile row and the credential dir.
/// The ONE account-disconnect path — the settings UI (handler above) and a
/// provider-side revocation relayed by the hub (`revoke_plugin_auth`) both
/// land here, so the two can never drift.
async fn disconnect_account(
    state: &AppState,
    slug: &str,
    agent_id: &str,
    account_label: &str,
) -> Result<(), NeboError> {
    // Run the plugin's own logout against this account's config dir BEFORE
    // deleting it — logout is where server-side release happens (revoking an
    // OAuth token, returning a phone number). Deleting the dir first would
    // orphan whatever the account held. Best-effort with a bound: disconnect
    // must still succeed when the plugin or network is broken.
    let dir = plugin_profile_dir(agent_id, slug, account_label);
    if dir.is_dir()
        && let Some((binary_path, auth)) = state.plugin_store.get_auth_info(slug)
        && let (Some(logout_cmd), Some(env_name)) =
            (auth.commands.logout.as_deref(), auth.profile_dir_env.as_deref())
    {
        let runtime = napp::PluginRuntime::new(slug, binary_path, state.plugin_store.clone());
        let mut cmd = runtime.command(logout_cmd);
        cmd.env(env_name, &dir);
        // Same locals a login gets — a logout that releases something
        // server-side (a phone number) goes back through the local API.
        for (key, value) in napp::plugin::plugin_base_env() {
            cmd.env(key, value);
        }
        cmd.env("NEBO_AGENT_ID", agent_id);
        match tokio::time::timeout(ACCOUNT_LOGOUT_TIMEOUT, cmd.output()).await {
            Ok(Ok(out)) if out.status.success() => {}
            Ok(Ok(out)) => {
                warn!(plugin = %slug, account = %account_label,
                    stderr = %String::from_utf8_lossy(&out.stderr),
                    "account logout command failed; disconnecting anyway");
            }
            Ok(Err(e)) => {
                warn!(plugin = %slug, error = %e, "account logout could not run; disconnecting anyway");
            }
            Err(_) => {
                warn!(plugin = %slug, account = %account_label,
                    "account logout timed out; disconnecting anyway");
            }
        }
    }

    state
        .store
        .delete_plugin_account_profile(agent_id, slug, account_label)?;
    // Remove the account's credential directory so disconnect is a real removal.
    if dir.is_dir() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    info!(plugin = %slug, account = %account_label, "disconnected plugin account");

    // Lifecycle event: a specific account was disconnected — symmetric with
    // "account.connected". Workflows subscribe via an `event` trigger on this source.
    state.emit_lifecycle(
        "account.disconnected",
        serde_json::json!({
            "plugin": slug,
            "account_label": account_label,
            "agent_id": agent_id,
        }),
        format!("disconnect:plugin:{slug}"),
    );

    Ok(())
}

/// POST /plugins/{slug}/auth/logout
///
/// Runs the plugin's auth logout command. Returns immediately.
pub async fn auth_logout(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> HandlerResult<serde_json::Value> {
    logout_plugin(&state, &slug).await.map_err(to_error_response)?;
    Ok(Json(serde_json::json!({ "success": true })))
}

/// Run a plugin's global auth logout command (single-account plugins keep
/// their credentials in the plugin's own profile). `NotFound` when the plugin
/// has no auth block, `Validation` when it declares no logout command. The
/// ONE global-logout path, shared by the settings handler above and
/// `revoke_plugin_auth`.
async fn logout_plugin(state: &AppState, slug: &str) -> Result<(), NeboError> {
    let (binary_path, auth) = state
        .plugin_store
        .get_auth_info(slug)
        .ok_or(NeboError::NotFound)?;

    let logout_cmd = auth
        .commands
        .logout
        .as_deref()
        .ok_or_else(|| NeboError::Validation("plugin has no auth logout command".into()))?;

    let runtime = napp::PluginRuntime::new(slug, binary_path, state.plugin_store.clone());
    let mut cmd = runtime.command(logout_cmd);

    let output = cmd
        .output()
        .await
        .map_err(|e| NeboError::Internal(e.to_string()))?;

    if output.status.success() {
        info!(plugin = %slug, "plugin auth logout succeeded");
        // Update in-memory auth cache so getAgent reflects the change instantly
        state.plugin_store.update_auth_status(slug).await;
        state.tools.refresh_definition("plugin").await;
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        warn!(plugin = %slug, error = %stderr, "plugin auth logout failed");
        Err(NeboError::Internal(format!("logout failed: {}", stderr)))
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

    // Drop the artifact-update-tracking row (plugin prefs are keyed by slug) so
    // the update checker doesn't keep polling an uninstalled plugin.
    let _ = state.store.delete_artifact_update_pref(slug, "plugin");

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
    let all_skills = state.skill_loader.list(None).await;
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
    // Kill the handler process if the timeout below drops the wait — without
    // this a timed-out route leaks the child forever (the orphan class from
    // PluginRuntime::run_capture's doc comment).
    cmd.kill_on_drop(true);

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
        types::keyparser::build_agent_session_key(&agent_id, &format!("help:{slug}"));

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
    // Bounded via run_capture — this was an UNBOUNDED cmd.output() in an HTTP
    // handler: a setup command that hung held the request forever and, once
    // the client gave up, the process leaked.
    let mut args = napp::plugin_runtime::split_command(&command);
    args.extend(substituted_args.iter().cloned());
    let output = runtime
        .run_capture_args(&args, SETUP_COMMAND_TIMEOUT)
        .await
        .map_err(|e| {
            to_error_response(NeboError::Internal(format!("setup command failed: {e}")))
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

// ── Provider-side revocation (relayed by the hub) ────────────────────────

/// Metadata `kind` the hub puts on a bot-stream delivery when a provider
/// (Intuit, Google, ...) revoked a plugin account's authorization on ITS side.
/// Contract agreed with the hub (all values are strings):
/// `kind=plugin_auth_revoked`, `slug`, `account_label`, optional `realm_id`,
/// optional `provider` (display name for the notice).
pub(crate) const PLUGIN_AUTH_REVOKED_KIND: &str = "plugin_auth_revoked";

/// Owner-visible notice wording when the provider name is not on the wire.
const UNKNOWN_PROVIDER_LABEL: &str = "the provider";

/// A revocation the hub relayed for one (plugin, account).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PluginAuthRevoked {
    pub slug: String,
    pub account_label: String,
    pub realm_id: Option<String>,
    pub provider: Option<String>,
}

/// Recognize a `plugin_auth_revoked` delivery. Stream-agnostic: keyed on the
/// metadata `kind` alone, so the hub may send it on any bot stream. `None`
/// for every other message, and for a malformed revoke (no slug or account):
/// a disconnect the desktop cannot target is logged, never guessed.
pub(crate) fn parse_plugin_auth_revoked(msg: &comm::CommMessage) -> Option<PluginAuthRevoked> {
    if msg.metadata.get("kind").map(String::as_str) != Some(PLUGIN_AUTH_REVOKED_KIND) {
        return None;
    }
    let field = |key: &str| {
        msg.metadata
            .get(key)
            .map(|v| v.trim())
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };
    let (Some(slug), Some(account_label)) = (field("slug"), field("account_label")) else {
        warn!(
            msg_id = %msg.id,
            keys = ?msg.metadata.keys().collect::<Vec<_>>(),
            "plugin_auth_revoked without slug/account_label; ignoring"
        );
        return None;
    };
    Some(PluginAuthRevoked {
        slug,
        account_label,
        realm_id: field("realm_id"),
        provider: field("provider"),
    })
}

/// Apply a provider-side revocation locally: drop the (plugin, account)
/// credentials through the SAME disconnect path the settings UI uses, then
/// tell the owner in one line. Best-effort throughout: the remote token is
/// already dead, so every local failure is logged and the notice still fires.
pub(crate) async fn revoke_plugin_auth(state: &AppState, revoked: &PluginAuthRevoked) {
    let slug = revoked.slug.as_str();
    let label = revoked.account_label.as_str();
    info!(
        plugin = %slug,
        account = %label,
        realm_id = ?revoked.realm_id,
        "plugin authorization revoked on the provider's side; disconnecting locally"
    );

    // Every agent that holds this account as a per-account profile.
    let holders: Vec<db::PluginAccountProfile> = state
        .store
        .list_all_plugin_account_profiles()
        .unwrap_or_else(|e| {
            warn!(plugin = %slug, error = %e, "revoke: could not list account profiles");
            Vec::new()
        })
        .into_iter()
        .filter(|p| p.plugin_slug == slug && p.account_label == label)
        .collect();

    if holders.is_empty() {
        // Single-account plugin: its credentials live in the plugin's own
        // global profile, cleared by its logout command.
        match logout_plugin(state, slug).await {
            Ok(()) => info!(plugin = %slug, "revoke: plugin logged out"),
            Err(NeboError::NotFound) | Err(NeboError::Validation(_)) => {
                info!(plugin = %slug, account = %label, "revoke: no local account matched and the plugin has no logout command; nothing to clear")
            }
            Err(e) => warn!(plugin = %slug, error = %e, "revoke: plugin logout failed"),
        }
    }
    for p in &holders {
        match disconnect_account(state, slug, &p.agent_id, label).await {
            Ok(()) => info!(plugin = %slug, account = %label, agent = %p.agent_id, "revoke: account disconnected"),
            Err(e) => warn!(plugin = %slug, account = %label, agent = %p.agent_id, error = %e, "revoke: account disconnect failed"),
        }
    }

    // The plugin tool's auth view must reflect the loss right away.
    state.plugin_store.update_auth_status(slug).await;
    state.tools.refresh_definition("plugin").await;

    let display = state
        .plugin_store
        .get_manifest(slug)
        .map(|m| m.name)
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| slug.to_string());
    let provider = revoked
        .provider
        .as_deref()
        .unwrap_or(UNKNOWN_PROVIDER_LABEL);
    let title = format!("{display} was disconnected");
    let body = format!(
        "{display} was disconnected from {provider}'s side; reconnect in Settings, Plugins when you are ready."
    );
    // Fresh id per occurrence so a later revoke notifies again instead of
    // being folded into an already-read row (same rule as the reauth notice).
    let notif_id = uuid::Uuid::new_v4().to_string();
    let agent_id = holders.first().map(|p| p.agent_id.as_str());
    tools::owner_notify::emit(
        &state.store,
        Some(&|ev, payload| state.hub.broadcast(ev, payload)),
        &tools::owner_notify::OwnerNotification {
            id: &notif_id,
            kind: "warning",
            title: &title,
            body: Some(&body),
            action_url: Some(PLUGINS_SETTINGS_PATH),
            agent_id,
            loud: true,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req() -> AccountLoginRequest {
        AccountLoginRequest {
            agent_id: "agent-1".into(),
            account_label: "work@acme.com".into(),
            account_number: String::new(),
        }
    }

    #[test]
    fn a_single_account_plugin_runs_its_shared_login_instead_of_refusing() {
        assert!(login_profile(None, "quickbooks", req()).is_none());
    }

    fn hub_delivery(kind: &str, extra: &[(&str, &str)]) -> comm::CommMessage {
        let mut metadata = HashMap::new();
        metadata.insert("kind".to_string(), kind.to_string());
        for (k, v) in extra {
            metadata.insert((*k).to_string(), (*v).to_string());
        }
        comm::CommMessage {
            id: "01J0000000000000000000ABCD".into(),
            from: "hub".into(),
            to: String::new(),
            topic: "account".into(),
            conversation_id: "conv-1".into(),
            msg_type: comm::CommMessageType::Message,
            content: r#"{"text":""}"#.into(),
            metadata,
            timestamp: 0,
            human_injected: false,
            human_id: None,
            task_id: None,
            correlation_id: None,
            task_status: None,
            artifacts: vec![],
            error: None,
            attachments: vec![],
        }
    }

    /// The hub contract: kind=plugin_auth_revoked + slug + account_label,
    /// realm_id and provider optional. Mutation check: dropping the `kind`
    /// guard makes the second assertion fail; dropping the slug requirement
    /// makes the third fail.
    #[test]
    fn plugin_auth_revoked_is_recognized_from_a_hub_delivery() {
        let msg = hub_delivery(
            PLUGIN_AUTH_REVOKED_KIND,
            &[
                ("slug", "quickbooks"),
                ("account_label", "Acme Co"),
                ("realm_id", "9130357"),
                ("provider", "Intuit"),
            ],
        );
        assert_eq!(
            parse_plugin_auth_revoked(&msg),
            Some(PluginAuthRevoked {
                slug: "quickbooks".into(),
                account_label: "Acme Co".into(),
                realm_id: Some("9130357".into()),
                provider: Some("Intuit".into()),
            })
        );

        let stop = hub_delivery("stop", &[("slug", "quickbooks"), ("account_label", "Acme Co")]);
        assert_eq!(parse_plugin_auth_revoked(&stop), None, "other kinds are not revokes");

        let no_slug = hub_delivery(PLUGIN_AUTH_REVOKED_KIND, &[("account_label", "Acme Co")]);
        assert_eq!(parse_plugin_auth_revoked(&no_slug), None, "a revoke without a slug cannot be targeted");

        let minimal = hub_delivery(PLUGIN_AUTH_REVOKED_KIND, &[("slug", "gws"), ("account_label", " work@acme.com ")]);
        let parsed = parse_plugin_auth_revoked(&minimal).expect("optional fields may be absent");
        assert_eq!(parsed.account_label, "work@acme.com", "labels are trimmed");
        assert_eq!(parsed.realm_id, None);
        assert_eq!(parsed.provider, None);
    }

    #[test]
    fn a_multi_account_plugin_gets_its_own_profile_dir() {
        let p = login_profile(Some("GWS_CONFIG_DIR".into()), "gws", req()).expect("profile");
        assert_eq!(p.env_name, "GWS_CONFIG_DIR");
        assert!(p.config_dir.contains("gws"), "{}", p.config_dir);
        assert_eq!(p.agent_id, "agent-1");
    }
}
