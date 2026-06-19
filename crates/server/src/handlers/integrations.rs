use axum::extract::{Path, Query, State};
use axum::response::Json;
use rand::RngCore;
use sha2::{Digest, Sha256};
use tracing::{info, warn};

use super::{HandlerResult, to_error_response};
use crate::state::AppState;

/// Slugify an integration name for use in tool naming.
/// e.g. "monument.sh" → "monument_sh", "My GitHub" → "my_github"
fn slugify_name(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect::<String>()
        .trim_matches('_')
        .to_string()
}

/// True if an integration's `metadata` carries a stdio launch spec (a non-empty
/// `command`). The one predicate for "is this a stdio server" — used by every
/// connect path so stdio integrations (which have no `server_url`) aren't skipped.
pub(crate) fn metadata_is_stdio(metadata: Option<&str>) -> bool {
    metadata
        .and_then(|m| serde_json::from_str::<serde_json::Value>(m).ok())
        .and_then(|v| {
            v.get("command")
                .and_then(|c| c.as_str())
                .map(|s| !s.is_empty())
        })
        .unwrap_or(false)
}

/// One MCP server parsed from a standard config block, normalized to our
/// `mcp_integrations` columns.
pub(crate) struct ParsedMcpServer {
    pub name: String,
    pub server_type: String,    // "http" | "sse" | "stdio"
    pub server_url: Option<String>,
    pub auth_type: String,      // "none" | "oauth" | "api_key"
    pub metadata: Option<String>, // JSON: {command,args,env} for stdio, {headers} for remote
}

/// Parse a standard MCP server config block (Claude Desktop `mcpServers`,
/// VS Code `servers`) into one `ParsedMcpServer` per entry. This is the single
/// parser for the standard format — shared by the settings "paste config" path
/// and by connector (CONN-) code redemption, so the two can't drift.
///
/// Each entry is either stdio (`{command, args?, env?}`) or remote
/// (`{type?: "http"|"sse", url, headers?}`). Unknown shapes are skipped.
pub(crate) fn parse_mcp_servers_block(v: &serde_json::Value) -> Vec<ParsedMcpServer> {
    let Some(map) = v
        .get("mcpServers")
        .or_else(|| v.get("servers"))
        .and_then(|m| m.as_object())
    else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for (name, block) in map {
        if let Some(command) = block.get("command").and_then(|c| c.as_str()) {
            // stdio server — store the launch spec in metadata.
            let metadata = serde_json::json!({
                "command": command,
                "args": block.get("args").cloned().unwrap_or_else(|| serde_json::json!([])),
                "env": block.get("env").cloned().unwrap_or_else(|| serde_json::json!({})),
            })
            .to_string();
            out.push(ParsedMcpServer {
                name: name.clone(),
                server_type: "stdio".to_string(),
                server_url: None,
                auth_type: "none".to_string(),
                metadata: Some(metadata),
            });
        } else if let Some(url) = block.get("url").and_then(|u| u.as_str()) {
            // remote server — `type` defaults to http; carry any headers in metadata.
            let server_type = block
                .get("type")
                .and_then(|t| t.as_str())
                .unwrap_or("http")
                .to_string();
            let auth_type = block
                .get("authType")
                .and_then(|a| a.as_str())
                .unwrap_or("none")
                .to_string();
            let metadata = block
                .get("headers")
                .map(|h| serde_json::json!({ "headers": h }).to_string());
            out.push(ParsedMcpServer {
                name: name.clone(),
                server_type,
                server_url: Some(url.to_string()),
                auth_type,
                metadata,
            });
        }
    }
    out
}

/// Create local `mcp_integrations` from a standard config block, then connect
/// the ones that can connect without user interaction (stdio + no-auth remote;
/// OAuth servers wait for the user to authorize). Returns the created rows.
/// Used by both the settings paste-import and connector code redemption.
pub(crate) async fn create_integrations_from_block(
    state: &AppState,
    block: &serde_json::Value,
) -> Result<Vec<db::models::McpIntegration>, types::NeboError> {
    let servers = parse_mcp_servers_block(block);
    let mut created = Vec::new();
    for s in servers {
        let id = uuid::Uuid::new_v4().to_string();
        let integration = state.store.create_mcp_integration(
            &id,
            &s.name,
            &s.server_type,
            s.server_url.as_deref(),
            &s.auth_type,
            s.metadata.as_deref(),
        )?;
        created.push(integration);
    }
    // Connect everything that doesn't need an OAuth round-trip.
    sync_bridge(state).await;
    Ok(created)
}

/// Fix legacy integrations that were saved as "stdio" but have HTTP URLs.
fn fix_server_type(state: &AppState) {
    if let Ok(integrations) = state.store.list_mcp_integrations() {
        for i in &integrations {
            if i.server_type == "stdio" {
                if let Some(ref url) = i.server_url {
                    if url.starts_with("http://") || url.starts_with("https://") {
                        let _ = state
                            .store
                            .update_mcp_integration(&i.id, None, None, None, None, None);
                        // Direct SQL update for server_type since update_mcp_integration doesn't expose it
                        let _ = state.store.set_mcp_server_type(&i.id, "http");
                    }
                }
            }
        }
    }
}

/// Re-sync the MCP bridge after integration changes.
/// Uses per-integration connect with proper token resolution instead of sync_all
/// (which passes None for tokens and would fail for OAuth integrations).
async fn sync_bridge(state: &AppState) {
    let integrations = match state.store.list_mcp_integrations() {
        Ok(i) => i,
        Err(e) => {
            warn!("failed to load MCP integrations for sync: {}", e);
            return;
        }
    };

    // Disconnect integrations that are no longer enabled
    let enabled_ids: std::collections::HashSet<&str> = integrations
        .iter()
        .filter(|i| i.is_enabled.unwrap_or(0) != 0)
        .map(|i| i.id.as_str())
        .collect();
    for conn_id in state.bridge.connected_ids().await {
        if !enabled_ids.contains(conn_id.as_str()) {
            state.bridge.disconnect(&conn_id).await;
        }
    }

    // Connect enabled integrations with proper token handling
    for i in &integrations {
        if i.is_enabled.unwrap_or(0) == 0 {
            continue;
        }
        // Remote servers need a URL; stdio servers carry a command in metadata.
        let server_url = i.server_url.clone().unwrap_or_default();
        if server_url.is_empty() && !metadata_is_stdio(i.metadata.as_deref()) {
            continue;
        }
        if i.auth_type == "oauth" && i.connection_status.is_none() {
            continue;
        }
        let access_token = match tools::mcp_tool::resolve_mcp_token(
            &state.store,
            state.bridge.client(),
            i,
        )
        .await
        {
            tools::mcp_tool::TokenResolution::Ready(t) => t,
            tools::mcp_tool::TokenResolution::NeedsReauth => {
                // Don't connect with a stale token (it would 401 and drop). Surface
                // needs_reauth; the proactive refresher will retry on its next tick.
                let _ = state.store.set_mcp_connection_status(&i.id, "needs_reauth", 0);
                warn!(name = %i.name, "MCP token needs reauth during sync — skipping connect");
                continue;
            }
        };
        let tool_prefix = slugify_name(&i.name);
        match state
            .bridge
            .connect(
                &i.id,
                &tool_prefix,
                &server_url,
                access_token.as_deref(),
                i.metadata.as_deref(),
            )
            .await
        {
            Ok(tools_list) => {
                let _ = state.store.set_mcp_connection_status(
                    &i.id,
                    "connected",
                    tools_list.len() as i64,
                );
            }
            Err(e) => {
                let _ = state.store.set_mcp_connection_status(&i.id, "error", 0);
                warn!(name = %i.name, error = %e, "MCP reconnect failed during sync");
            }
        }
    }
}

/// Spawn a background task that keeps OAuth MCP tokens fresh — refreshing any token
/// expiring within the window so it never reaches expiry (and never 401s / drops a
/// server on reconnect or restart). This is the "fresh 100% of the time" guarantee:
/// renew proactively rather than reactively at connect time. Runs on an interval.
pub fn spawn_mcp_token_refresher(state: AppState) {
    const REFRESH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(300); // 5 min
    const REFRESH_WINDOW_SECS: i64 = 900; // renew when < 15 min to expiry
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(REFRESH_INTERVAL);
        loop {
            tick.tick().await; // first tick fires immediately, so we also catch startup
            let integrations = match state.store.list_mcp_integrations() {
                Ok(v) => v,
                Err(e) => {
                    warn!(error = %e, "token refresher: failed to list MCP integrations");
                    continue;
                }
            };
            for i in &integrations {
                if i.auth_type != "oauth" || i.is_enabled.unwrap_or(0) == 0 {
                    continue;
                }
                let cred = match state.store.get_mcp_credential_full(&i.id, "oauth_token") {
                    Ok(Some(c)) => c,
                    _ => continue,
                };
                if cred.refresh_token.is_some()
                    && tools::mcp_tool::token_expires_within(cred.expires_at, REFRESH_WINDOW_SECS)
                {
                    match tools::mcp_tool::refresh_mcp_token(
                        &state.store,
                        state.bridge.client(),
                        &i.id,
                    )
                    .await
                    {
                        Ok(_) => info!(name = %i.name, "proactively refreshed MCP OAuth token"),
                        Err(e) => {
                            warn!(name = %i.name, error = %e, "proactive MCP token refresh failed")
                        }
                    }
                }
            }
        }
    });
}

/// Spawn a background task that keeps OAuth *plugin* accounts fresh — the same
/// "fresh 100% of the time" guarantee as the MCP refresher, but for multi-account
/// plugins (e.g. Google Workspace). A plugin's access token auto-refreshes when
/// the plugin runs, but an account that goes unused can drift to expiry, and a
/// revoked/lapsed refresh token (Workspace RAPT reauth) silently dies until the
/// next call. So we periodically run each connected account's manifest-declared
/// auth `status` command (which exercises the token) and, when it reports an
/// unrecoverable failure, mark the account `needs_reauth` + fire ONE notification
/// so the user can reconnect — instead of discovering it mid-task.
pub fn spawn_plugin_token_refresher(state: AppState) {
    const REFRESH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1800); // 30 min
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(REFRESH_INTERVAL);
        loop {
            tick.tick().await; // first tick fires immediately, so we also catch startup
            let profiles = match state.store.list_all_plugin_account_profiles() {
                Ok(v) => v,
                Err(e) => {
                    warn!(error = %e, "plugin token refresher: failed to list account profiles");
                    continue;
                }
            };
            for p in &profiles {
                // Only plugins that declare BOTH a per-account config dir
                // (profile_dir_env) and an auth status command can be checked
                // per account; others are single-account / have no health probe.
                let Some(manifest) = state.plugin_store.get_manifest(&p.plugin_slug) else {
                    continue;
                };
                let Some(auth) = manifest.auth else { continue };
                if auth.commands.status.is_none() {
                    continue;
                }
                let Some(env_name) = auth.profile_dir_env.as_deref() else {
                    continue;
                };

                // `None` = inconclusive (probe couldn't run / was reaped): leave the
                // account's current state untouched and retry next tick — never let
                // an unknown raise a false alarm or clear a real one.
                let Some(healthy) = state
                    .plugin_store
                    .check_auth_for_profile(&p.plugin_slug, env_name, &p.config_dir)
                    .await
                else {
                    continue;
                };

                if healthy {
                    // Recovered (or still fine) — clear any prior reauth flag so a
                    // future failure notifies again.
                    if p.needs_reauth {
                        if let Err(e) = state.store.set_plugin_account_reauth(&p.id, false) {
                            warn!(error = %e, "failed to clear plugin reauth flag");
                        } else {
                            info!(plugin = %p.plugin_slug, account = %p.account_label, "plugin account token recovered");
                        }
                    }
                    continue;
                }

                // Unhealthy: token couldn't be refreshed. Flag it for the badge.
                if let Err(e) = state.store.set_plugin_account_reauth(&p.id, true) {
                    warn!(error = %e, "failed to set plugin reauth flag");
                }
                // Notify exactly once per unhealthy spell.
                if !p.reauth_notified {
                    notify_plugin_needs_reauth(&state, p);
                    let _ = state.store.mark_plugin_account_reauth_notified(&p.id);
                }
                warn!(plugin = %p.plugin_slug, account = %p.account_label, "plugin account needs reconnect");
            }
        }
    });
}

/// Fire the one-time "reconnect this account" notification (bell + toast) and
/// broadcast it, mirroring the canonical proactive-notification pathway.
fn notify_plugin_needs_reauth(state: &AppState, p: &db::PluginAccountProfile) {
    let user_id = state.store.ensure_local_user_id().unwrap_or_default();
    // Fresh id per occurrence: the `reauth_notified` flag (reset on recovery) is
    // the once-per-spell guard, so a unique id lets a *future* expiry notify again
    // rather than being suppressed by a stale, already-read notification.
    let notif_id = uuid::Uuid::new_v4().to_string();
    let title = format!("Reconnect {}", p.account_label);
    let body = format!(
        "{}'s connection to {} expired. Reconnect it in the agent's Connected Accounts.",
        p.account_label, p.plugin_slug
    );
    let action_url = format!("/{}/settings/accounts", p.agent_id);
    if let Err(e) = state.store.create_notification(
        &notif_id,
        &user_id,
        "warning",
        &title,
        Some(&body),
        Some(&action_url),
        None,
    ) {
        warn!(error = %e, "failed to create plugin reauth notification");
        return;
    }
    state.hub.broadcast(
        "notification_created",
        serde_json::json!({
            "id": notif_id,
            "type": "warning",
            "title": title,
            "body": body,
            "actionUrl": action_url,
            "readAt": null,
            "createdAt": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        }),
    );
}

/// GET /api/v1/integrations
pub async fn list_integrations(State(state): State<AppState>) -> HandlerResult<serde_json::Value> {
    // Auto-fix legacy "stdio" records that have HTTP URLs
    fix_server_type(&state);
    let integrations = state
        .store
        .list_mcp_integrations()
        .map_err(to_error_response)?;
    Ok(Json(serde_json::json!({"integrations": integrations})))
}

/// POST /api/v1/integrations
pub async fn create_integration(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    // Standard MCP config block (Claude Desktop `mcpServers` / VS Code `servers`):
    // create one integration per server entry and connect them. Same parser the
    // connector (CONN-) code path uses.
    if body.get("mcpServers").is_some() || body.get("servers").is_some() {
        let created = create_integrations_from_block(&state, &body)
            .await
            .map_err(to_error_response)?;
        return Ok(Json(serde_json::json!({
            "integrations": created,
            "total": created.len(),
        })));
    }

    let name = body["name"]
        .as_str()
        .ok_or_else(|| to_error_response(types::NeboError::Validation("name required".into())))?;
    let server_url = body["serverUrl"].as_str();
    // Infer server_type from URL if not explicitly provided
    let server_type = body["serverType"].as_str().unwrap_or_else(|| {
        if let Some(url) = server_url {
            if url.starts_with("http://") || url.starts_with("https://") {
                "http"
            } else {
                "stdio"
            }
        } else {
            "stdio"
        }
    });
    let auth_type = body["authType"].as_str().unwrap_or("none");
    let metadata = body.get("metadata").map(|v| v.to_string());

    let id = uuid::Uuid::new_v4().to_string();
    let integration = state
        .store
        .create_mcp_integration(
            &id,
            name,
            server_type,
            server_url,
            auth_type,
            metadata.as_deref(),
        )
        .map_err(to_error_response)?;

    Ok(Json(serde_json::json!({"integration": integration})))
}

/// GET /api/v1/integrations/:id
pub async fn get_integration(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let integration = state
        .store
        .get_mcp_integration(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;
    Ok(Json(serde_json::json!(integration)))
}

/// PUT /api/v1/integrations/:id
pub async fn update_integration(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    state
        .store
        .update_mcp_integration(
            &id,
            body["name"].as_str(),
            body["serverUrl"].as_str(),
            body["authType"].as_str(),
            body["isEnabled"].as_bool(),
            body.get("metadata").map(|v| v.to_string()).as_deref(),
        )
        .map_err(to_error_response)?;

    // Sync bridge to reflect changes
    sync_bridge(&state).await;

    let updated = state
        .store
        .get_mcp_integration(&id)
        .map_err(to_error_response)?;
    Ok(Json(serde_json::json!(updated)))
}

/// DELETE /api/v1/integrations/:id
pub async fn delete_integration(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    // Disconnect before deleting
    state.bridge.disconnect(&id).await;
    state
        .store
        .delete_mcp_integration(&id)
        .map_err(to_error_response)?;
    Ok(Json(serde_json::json!({"success": true})))
}

/// POST /api/v1/integrations/:id/test
pub async fn test_integration(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let integration = state
        .store
        .get_mcp_integration(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    let server_url = integration.server_url.clone().unwrap_or_default();
    let is_stdio = metadata_is_stdio(integration.metadata.as_deref())
        || integration.server_type == "stdio";

    // stdio servers can't be probed without launching — report config validity only.
    if server_url.is_empty() {
        let (success, message) = if is_stdio {
            (true, "Configuration looks valid (stdio server starts on demand)".to_string())
        } else {
            (false, "No server URL configured".to_string())
        };
        return Ok(Json(serde_json::json!({
            "success": success,
            "integration": integration.name,
            "message": message,
        })));
    }

    // Real test: resolve the token (refresh if expired) and do the authenticated MCP
    // connect — exactly what production uses — instead of an unauthenticated GET that
    // calls a 401 "reachable". Status mirrors reality: connected ⇒ success, auth/
    // protocol failure ⇒ failure with the actual reason.
    let access_token = match tools::mcp_tool::resolve_mcp_token(
        &state.store,
        state.bridge.client(),
        &integration,
    )
    .await
    {
        tools::mcp_tool::TokenResolution::Ready(t) => t,
        tools::mcp_tool::TokenResolution::NeedsReauth => {
            let _ = state.store.set_mcp_connection_status(&id, "needs_reauth", 0);
            return Ok(Json(serde_json::json!({
                "success": false,
                "needsReauth": true,
                "integration": integration.name,
                "message": "Authentication expired — re-authorize this server (click the key / Reconnect).",
            })));
        }
    };

    let tool_prefix = slugify_name(&integration.name);
    let (success, message) = match state
        .bridge
        .connect(
            &id,
            &tool_prefix,
            &server_url,
            access_token.as_deref(),
            integration.metadata.as_deref(),
        )
        .await
    {
        Ok(tools_list) => {
            let n = tools_list.len();
            let _ = state.store.set_mcp_connection_status(&id, "connected", n as i64);
            (true, format!("Connected — {} tools available", n))
        }
        Err(e) => {
            let _ = state.store.set_mcp_connection_status(&id, "error", 0);
            (false, format!("Connection failed: {}", e))
        }
    };

    Ok(Json(serde_json::json!({
        "success": success,
        "integration": integration.name,
        "message": message,
    })))
}

/// POST /api/v1/integrations/:id/connect — connect to the MCP server and register its tools.
pub async fn connect_integration(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let integration = state
        .store
        .get_mcp_integration(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    let server_url = integration.server_url.as_deref().unwrap_or("");
    // Remote servers need a URL; stdio servers instead carry a command in metadata.
    let is_stdio = metadata_is_stdio(integration.metadata.as_deref());
    if server_url.is_empty() && !is_stdio {
        return Err(to_error_response(types::NeboError::Validation(
            "No server URL or stdio command configured".into(),
        )));
    }

    // Resolve the OAuth token (refreshing if expired). On failure, surface
    // "needs reauth" instead of connecting with a stale token that would 401.
    let access_token = match tools::mcp_tool::resolve_mcp_token(
        &state.store,
        state.bridge.client(),
        &integration,
    )
    .await
    {
        tools::mcp_tool::TokenResolution::Ready(t) => t,
        tools::mcp_tool::TokenResolution::NeedsReauth => {
            let _ = state.store.set_mcp_connection_status(&id, "needs_reauth", 0);
            return Ok(Json(serde_json::json!({
                "success": false,
                "needsReauth": true,
                "message": "Authentication expired — re-authorize this server (click the key / Reconnect).",
            })));
        }
    };

    // Use integration name (slugified) as server_type for tool naming
    // e.g. "monument.sh" → "monument_sh" → tools named mcp__monument_sh__comment
    let tool_prefix = slugify_name(&integration.name);

    // Try to connect and list tools
    match state
        .bridge
        .connect(
            &id,
            &tool_prefix,
            server_url,
            access_token.as_deref(),
            integration.metadata.as_deref(),
        )
        .await
    {
        Ok(tools) => {
            let tool_count = tools.len();
            // Update connection status in DB. Also re-enable the integration: a
            // successful connect is an explicit user action, and the connect_all
            // sync disconnects (and refuses to reconnect) any is_enabled=0 server.
            // Without this, reconnecting a deactivated server has no lasting
            // effect — it drops again on the next sync.
            let _ = state.store.update_mcp_integration(
                &id,
                None,
                None,
                None,
                Some(true),
                Some(&serde_json::json!({"connection_status": "connected", "tool_count": tool_count}).to_string()),
            );
            // Also update connection_status column directly
            let _ = state
                .store
                .set_mcp_connection_status(&id, "connected", tool_count as i64);

            let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
            Ok(Json(serde_json::json!({
                "success": true,
                "message": format!("Connected — {} tools registered", tool_count),
                "toolCount": tool_count,
                "tools": tool_names,
            })))
        }
        Err(e) => {
            warn!(
                integration = %id,
                server_url = server_url,
                error = %e,
                "MCP connect failed"
            );
            let err_msg = format!("Connection failed: {}", e);
            let _ = state.store.set_mcp_connection_status(&id, "error", 0);
            // Also persist the error message for display
            let _ = state.store.update_mcp_integration(
                &id,
                None,
                None,
                None,
                None,
                Some(&serde_json::json!({"last_error": &err_msg}).to_string()),
            );
            Ok(Json(serde_json::json!({
                "success": false,
                "message": err_msg,
            })))
        }
    }
}

/// POST /api/v1/integrations/:id/reauthenticate — clear stored credentials and restart OAuth.
/// For OAuth integrations whose tokens have fully expired (refresh failed too).
pub async fn reauthenticate_integration(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let integration = state
        .store
        .get_mcp_integration(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    if integration.auth_type != "oauth" {
        return Err(to_error_response(types::NeboError::Validation(
            "Reauthenticate is only for OAuth integrations".into(),
        )));
    }

    // Disconnect from bridge
    state.bridge.disconnect(&id).await;
    let _ = state
        .store
        .set_mcp_connection_status(&id, "disconnected", 0);

    // Clear stored OAuth credentials so a fresh flow can start
    let _ = state.store.delete_mcp_credentials(&id, "oauth_token");
    let _ = state.store.clear_mcp_oauth_state(&id);

    // Start a new OAuth flow (same as get_oauth_url but returns the URL directly)
    let server_url = integration
        .server_url
        .as_deref()
        .ok_or_else(|| to_error_response(types::NeboError::Validation("No server URL".into())))?;

    let metadata = state
        .bridge
        .client()
        .discover_oauth(server_url)
        .await
        .map_err(|e| {
            to_error_response(types::NeboError::Internal(format!(
                "OAuth discovery failed: {e}"
            )))
        })?;

    info!(
        integration = %id,
        "MCP reauthenticate: discovered OAuth metadata"
    );

    let redirect_uri = format!(
        "http://localhost:{}/api/v1/integrations/oauth/callback",
        state.config.port
    );
    let (client_id, client_secret) = if let Some(ref reg_endpoint) = metadata.registration_endpoint
    {
        match do_client_registration(&state, reg_endpoint, &redirect_uri).await {
            Ok((cid, csec)) => (cid, csec),
            Err(e) => {
                warn!("DCR failed during reauth, using fallback: {e}");
                (format!("nebo-agent-{}", id), None)
            }
        }
    } else {
        (format!("nebo-agent-{}", id), None)
    };

    let code_verifier = generate_code_verifier();
    let code_challenge = compute_code_challenge(&code_verifier);
    let oauth_state = generate_state();

    let encrypted_verifier = state
        .bridge
        .client()
        .encrypt_token(&code_verifier)
        .map_err(|e| to_error_response(types::NeboError::Internal(format!("encrypt: {e}"))))?;
    let encrypted_secret =
        match &client_secret {
            Some(s) => Some(state.bridge.client().encrypt_token(s).map_err(|e| {
                to_error_response(types::NeboError::Internal(format!("encrypt: {e}")))
            })?),
            None => None,
        };

    state
        .store
        .set_mcp_oauth_state(
            &id,
            &oauth_state,
            &encrypted_verifier,
            &client_id,
            encrypted_secret.as_deref(),
            &metadata.authorization_endpoint,
            &metadata.token_endpoint,
        )
        .map_err(to_error_response)?;

    let auth_url = format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&state={}&code_challenge={}&code_challenge_method=S256&scope=mcp:full",
        metadata.authorization_endpoint,
        urlencoding::encode(&client_id),
        urlencoding::encode(&redirect_uri),
        urlencoding::encode(&oauth_state),
        urlencoding::encode(&code_challenge),
    );

    info!(integration = %id, "MCP reauthenticate: OAuth URL generated");

    Ok(Json(serde_json::json!({
        "authUrl": auth_url,
    })))
}

// ── PKCE helpers (RFC 7636) ─────────────────────────────────────────

fn generate_code_verifier() -> String {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

fn compute_code_challenge(verifier: &str) -> String {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let hash = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(hash)
}

fn generate_state() -> String {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let mut buf = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

// ── OAuth flow ──────────────────────────────────────────────────────

/// GET /api/v1/integrations/:id/oauth-url — start OAuth flow.
/// Discovers OAuth metadata, does Dynamic Client Registration, generates PKCE,
/// stores flow state, and returns the authorization URL.
pub async fn get_oauth_url(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let integration = state
        .store
        .get_mcp_integration(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    let server_url = integration
        .server_url
        .as_deref()
        .ok_or_else(|| to_error_response(types::NeboError::Validation("No server URL".into())))?;

    // 1. Discover OAuth metadata
    let metadata = state
        .bridge
        .client()
        .discover_oauth(server_url)
        .await
        .map_err(|e| {
            to_error_response(types::NeboError::Internal(format!(
                "OAuth discovery failed: {e}"
            )))
        })?;

    info!(
        integration = %id,
        auth_endpoint = %metadata.authorization_endpoint,
        token_endpoint = %metadata.token_endpoint,
        registration = ?metadata.registration_endpoint,
        "discovered MCP OAuth metadata"
    );

    // 2. Dynamic Client Registration (if supported)
    let redirect_uri = format!(
        "http://localhost:{}/api/v1/integrations/oauth/callback",
        state.config.port
    );
    let (client_id, client_secret) = if let Some(ref reg_endpoint) = metadata.registration_endpoint
    {
        match do_client_registration(&state, reg_endpoint, &redirect_uri).await {
            Ok((cid, csec)) => (cid, csec),
            Err(e) => {
                warn!("DCR failed, using fallback client_id: {e}");
                (format!("nebo-agent-{}", id), None)
            }
        }
    } else {
        (format!("nebo-agent-{}", id), None)
    };

    // 3. Generate PKCE
    let code_verifier = generate_code_verifier();
    let code_challenge = compute_code_challenge(&code_verifier);
    let oauth_state = generate_state();

    // 4. Encrypt and store flow state in DB
    let encrypted_verifier = state
        .bridge
        .client()
        .encrypt_token(&code_verifier)
        .map_err(|e| to_error_response(types::NeboError::Internal(format!("encrypt: {e}"))))?;
    let encrypted_secret =
        match &client_secret {
            Some(s) => Some(state.bridge.client().encrypt_token(s).map_err(|e| {
                to_error_response(types::NeboError::Internal(format!("encrypt: {e}")))
            })?),
            None => None,
        };

    state
        .store
        .set_mcp_oauth_state(
            &id,
            &oauth_state,
            &encrypted_verifier,
            &client_id,
            encrypted_secret.as_deref(),
            &metadata.authorization_endpoint,
            &metadata.token_endpoint,
        )
        .map_err(to_error_response)?;

    // 5. Build authorization URL
    let auth_url = format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&state={}&code_challenge={}&code_challenge_method=S256&scope=mcp:full",
        metadata.authorization_endpoint,
        urlencoding::encode(&client_id),
        urlencoding::encode(&redirect_uri),
        urlencoding::encode(&oauth_state),
        urlencoding::encode(&code_challenge),
    );

    info!(integration = %id, "MCP OAuth URL generated");

    Ok(Json(serde_json::json!({
        "authUrl": auth_url,
    })))
}

/// Dynamic Client Registration (RFC 7591).
async fn do_client_registration(
    _state: &AppState,
    registration_endpoint: &str,
    redirect_uri: &str,
) -> Result<(String, Option<String>), String> {
    let body = serde_json::json!({
        "client_name": "Nebo Agent",
        "redirect_uris": [redirect_uri],
        "token_endpoint_auth_method": "none",
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "scope": "mcp:full offline_access"
    });

    let resp = reqwest::Client::new()
        .post(registration_endpoint)
        .json(&body)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("DCR request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("DCR returned {status}: {text}"));
    }

    #[derive(serde::Deserialize)]
    struct DcrResponse {
        client_id: String,
        client_secret: Option<String>,
    }

    let dcr: DcrResponse = resp.json().await.map_err(|e| format!("DCR decode: {e}"))?;
    info!(client_id = %dcr.client_id, "MCP Dynamic Client Registration complete");
    Ok((dcr.client_id, dcr.client_secret))
}

/// OAuth callback query params.
#[derive(serde::Deserialize)]
pub struct OAuthCallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// GET /api/v1/integrations/oauth/callback — handle OAuth redirect from MCP server.
/// Exchanges authorization code for tokens, stores them, connects, and returns
/// a self-closing HTML page (the browser tab can be closed).
pub async fn oauth_callback(
    State(state): State<AppState>,
    Query(params): Query<OAuthCallbackQuery>,
) -> axum::response::Html<String> {
    // Handle error from OAuth server
    if let Some(ref err) = params.error {
        let desc = params.error_description.as_deref().unwrap_or(err);
        warn!("MCP OAuth error: {desc}");
        return oauth_result_page(false, &format!("OAuth error: {desc}"));
    }

    let code = params.code.as_deref().unwrap_or("");
    let oauth_state = params.state.as_deref().unwrap_or("");

    if code.is_empty() || oauth_state.is_empty() {
        return oauth_result_page(false, "Missing authorization code or state parameter");
    }

    // 1. Look up integration by state
    let integration = match state
        .store
        .get_mcp_integration_by_oauth_state(oauth_state)
        .unwrap_or(None)
    {
        Some(i) => i,
        None => {
            warn!("MCP OAuth callback: no integration found for state");
            return oauth_result_page(false, "Invalid OAuth state — integration not found");
        }
    };

    let token_endpoint = integration.oauth_token_endpoint.as_deref().unwrap_or("");
    let client_id = integration.oauth_client_id.as_deref().unwrap_or("");

    if token_endpoint.is_empty() || client_id.is_empty() {
        return oauth_result_page(false, "Missing OAuth configuration");
    }

    // 2. Decrypt PKCE verifier
    let encrypted_verifier = integration.oauth_pkce_verifier.as_deref().unwrap_or("");
    let code_verifier = match state.bridge.client().decrypt_token(encrypted_verifier) {
        Ok(v) => v,
        Err(e) => {
            warn!("MCP OAuth: failed to decrypt verifier: {e}");
            return oauth_result_page(false, "Failed to decrypt PKCE verifier");
        }
    };

    // Decrypt client_secret if present
    let client_secret = integration
        .oauth_client_secret
        .as_deref()
        .and_then(|enc| state.bridge.client().decrypt_token(enc).ok());

    let redirect_uri = format!(
        "http://localhost:{}/api/v1/integrations/oauth/callback",
        state.config.port
    );

    // 3. Exchange code for tokens
    let tokens = match exchange_mcp_code(
        token_endpoint,
        code,
        &code_verifier,
        &redirect_uri,
        client_id,
        client_secret.as_deref(),
    )
    .await
    {
        Ok(t) => t,
        Err(e) => {
            warn!("MCP OAuth token exchange failed: {e}");
            return oauth_result_page(false, &format!("Token exchange failed: {e}"));
        }
    };

    info!(integration = %integration.id, "MCP OAuth token exchange successful");

    // 4. Encrypt and store tokens
    let encrypted_access = match state.bridge.client().encrypt_token(&tokens.access_token) {
        Ok(v) => v,
        Err(e) => return oauth_result_page(false, &format!("Encryption failed: {e}")),
    };
    let encrypted_refresh = tokens
        .refresh_token
        .as_deref()
        .and_then(|rt| state.bridge.client().encrypt_token(rt).ok());

    let expires_at = tokens.expires_in.map(|secs| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
            + secs
    });

    let _ = state.store.store_mcp_credentials(
        &integration.id,
        "oauth_token",
        &encrypted_access,
        encrypted_refresh.as_deref(),
        expires_at,
        tokens.scope.as_deref(),
    );

    // 5. Clear flow state
    let _ = state.store.clear_mcp_oauth_state(&integration.id);

    // 6. Connect immediately with the new token
    let server_url = integration.server_url.as_deref().unwrap_or("");
    let tool_prefix = slugify_name(&integration.name);
    if !server_url.is_empty() {
        match state
            .bridge
            .connect(
                &integration.id,
                &tool_prefix,
                server_url,
                Some(&tokens.access_token),
                // OAuth servers are always remote HTTP — no stdio launch spec.
                None,
            )
            .await
        {
            Ok(tools) => {
                let _ = state.store.set_mcp_connection_status(
                    &integration.id,
                    "connected",
                    tools.len() as i64,
                );
                info!(integration = %integration.id, tools = tools.len(), "MCP connected after OAuth");
                return oauth_result_page(
                    true,
                    &format!("Connected — {} tools registered", tools.len()),
                );
            }
            Err(e) => {
                warn!("MCP connect after OAuth failed: {e}");
                // Tokens stored — set as disconnected, user can retry
                let _ = state
                    .store
                    .set_mcp_connection_status(&integration.id, "disconnected", 0);
                return oauth_result_page(
                    true,
                    "Authorized — click Connect in Nebo to finish setup",
                );
            }
        }
    }

    oauth_result_page(true, "Authorized — click Connect in Nebo to finish setup")
}

/// Render the post-OAuth page for an MCP integration connect attempt.
/// Delegates to the shared branded auth page so every auth result looks the same.
fn oauth_result_page(success: bool, message: &str) -> axum::response::Html<String> {
    let heading = if success { "All set" } else { "Couldn't connect" };
    super::auth_page::auth_result_page(success, heading, message)
}

#[derive(serde::Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default)]
    scope: Option<String>,
}

/// Exchange an OAuth authorization code for tokens at the MCP server's token endpoint.
async fn exchange_mcp_code(
    token_endpoint: &str,
    code: &str,
    code_verifier: &str,
    redirect_uri: &str,
    client_id: &str,
    client_secret: Option<&str>,
) -> Result<TokenResponse, String> {
    let mut params = vec![
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("client_id", client_id),
        ("code_verifier", code_verifier),
    ];
    if let Some(secret) = client_secret {
        params.push(("client_secret", secret));
    }

    let resp = reqwest::Client::new()
        .post(token_endpoint)
        .form(&params)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("token request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("token endpoint returned {status}: {text}"));
    }

    resp.json::<TokenResponse>()
        .await
        .map_err(|e| format!("decode token response: {e}"))
}

/// GET /api/v1/integrations/registry
///
/// The catalog of installable MCP servers. It is intentionally empty here:
/// servers are published as `connector` artifacts in the loop (NeboLoop) and
/// installed via the connector (CONN-) path, so the desktop never ships a
/// hardcoded list. (The previous built-in list pointed at server URLs that
/// don't exist — fake entries, removed.) Until the loop catalog is wired in,
/// users add servers via "Custom Server" (a real URL) or a pasted config block.
pub async fn list_registry() -> HandlerResult<serde_json::Value> {
    Ok(Json(serde_json::json!({ "registry": [] })))
}

/// GET /api/v1/integrations/tools
pub async fn list_tools(State(state): State<AppState>) -> HandlerResult<serde_json::Value> {
    // Return all registered tools (built-in + MCP)
    let tool_defs = state.tools.list().await;
    let tools: Vec<serde_json::Value> = tool_defs
        .iter()
        .map(|t| {
            serde_json::json!({
                "name": t.name,
                "description": t.description,
                "source": "builtin",
                "inputSchema": t.input_schema,
            })
        })
        .collect();
    Ok(Json(serde_json::json!({"tools": tools})))
}
