use axum::extract::{Path, Query, State};
use axum::response::Json;
use rand::RngCore;
use sha2::{Digest, Sha256};
use tracing::{info, warn};

use crate::state::AppState;
use super::{to_error_response, HandlerResult};

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

/// Fix legacy integrations that were saved as "stdio" but have HTTP URLs.
fn fix_server_type(state: &AppState) {
    if let Ok(integrations) = state.store.list_mcp_integrations() {
        for i in &integrations {
            if i.server_type == "stdio" {
                if let Some(ref url) = i.server_url {
                    if url.starts_with("http://") || url.starts_with("https://") {
                        let _ = state.store.update_mcp_integration(
                            &i.id, None, None, None, None,
                            None,
                        );
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
    let enabled_ids: std::collections::HashSet<&str> = integrations.iter()
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
        let server_url = match &i.server_url {
            Some(u) if !u.is_empty() => u.clone(),
            _ => continue,
        };
        if i.auth_type == "oauth" && i.connection_status.is_none() {
            continue;
        }
        let access_token = if i.auth_type == "oauth" {
            match state.store.get_mcp_credential_full(&i.id, "oauth_token") {
                Ok(Some(cred)) => {
                    if tools::mcp_tool::is_token_expired(cred.expires_at) && cred.refresh_token.is_some() {
                        match tools::mcp_tool::refresh_mcp_token(&state.store, state.bridge.client(), &i.id).await {
                            Ok(new_token) => Some(new_token),
                            Err(e) => {
                                warn!(name = %i.name, error = %e, "MCP token refresh failed during sync");
                                state.bridge.client().decrypt_token(&cred.credential_value).ok()
                            }
                        }
                    } else {
                        state.bridge.client().decrypt_token(&cred.credential_value).ok()
                    }
                }
                _ => None,
            }
        } else {
            None
        };
        let tool_prefix = slugify_name(&i.name);
        match state.bridge.connect(&i.id, &tool_prefix, &server_url, access_token.as_deref()).await {
            Ok(tools_list) => {
                let _ = state.store.set_mcp_connection_status(&i.id, "connected", tools_list.len() as i64);
            }
            Err(e) => {
                let _ = state.store.set_mcp_connection_status(&i.id, "error", 0);
                warn!(name = %i.name, error = %e, "MCP reconnect failed during sync");
            }
        }
    }
}

/// GET /api/v1/integrations
pub async fn list_integrations(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    // Auto-fix legacy "stdio" records that have HTTP URLs
    fix_server_type(&state);
    let integrations = state.store.list_mcp_integrations().map_err(to_error_response)?;
    Ok(Json(serde_json::json!({"integrations": integrations})))
}

/// POST /api/v1/integrations
pub async fn create_integration(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
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
        .create_mcp_integration(&id, name, server_type, server_url, auth_type, metadata.as_deref())
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

    let updated = state.store.get_mcp_integration(&id).map_err(to_error_response)?;
    Ok(Json(serde_json::json!(updated)))
}

/// DELETE /api/v1/integrations/:id
pub async fn delete_integration(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    // Disconnect before deleting
    state.bridge.disconnect(&id).await;
    state.store.delete_mcp_integration(&id).map_err(to_error_response)?;
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

    // If a URL is provided, always try to reach it regardless of server_type
    let (success, message) = if let Some(ref url) = integration.server_url {
        if url.starts_with("http://") || url.starts_with("https://") {
            match reqwest::Client::new()
                .get(url.as_str())
                .timeout(std::time::Duration::from_secs(10))
                .send()
                .await
            {
                Ok(resp) => (true, format!("Server reachable (HTTP {})", resp.status())),
                Err(e) => (false, format!("Cannot reach server: {}", e)),
            }
        } else {
            (true, "Configuration looks valid".to_string())
        }
    } else {
        match integration.server_type.as_str() {
            "stdio" => (true, "Configuration looks valid (stdio server will be started on demand)".to_string()),
            _ => (false, "No server URL configured".to_string()),
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
    if server_url.is_empty() {
        return Err(to_error_response(types::NeboError::Validation(
            "No server URL configured".into(),
        )));
    }

    // Get stored OAuth token, refreshing if expired
    let access_token = if integration.auth_type == "oauth" {
        match state.store.get_mcp_credential_full(&id, "oauth_token") {
            Ok(Some(cred)) => {
                if tools::mcp_tool::is_token_expired(cred.expires_at) && cred.refresh_token.is_some() {
                    // Token expired — try to refresh before connecting
                    info!(integration = %id, "MCP token expired on connect, attempting refresh");
                    match tools::mcp_tool::refresh_mcp_token(&state.store, state.bridge.client(), &id).await {
                        Ok(new_token) => Some(new_token),
                        Err(e) => {
                            warn!(integration = %id, error = %e, "MCP token refresh on connect failed");
                            // Fall through with possibly-expired token
                            state.bridge.client().decrypt_token(&cred.credential_value).ok()
                        }
                    }
                } else {
                    state.bridge.client().decrypt_token(&cred.credential_value).ok()
                }
            }
            _ => None,
        }
    } else {
        None
    };

    // Use integration name (slugified) as server_type for tool naming
    // e.g. "monument.sh" → "monument_sh" → tools named mcp__monument_sh__comment
    let tool_prefix = slugify_name(&integration.name);

    // Try to connect and list tools
    match state
        .bridge
        .connect(&id, &tool_prefix, server_url, access_token.as_deref())
        .await
    {
        Ok(tools) => {
            let tool_count = tools.len();
            // Update connection status in DB
            let _ = state.store.update_mcp_integration(
                &id,
                None,
                None,
                None,
                None,
                Some(&serde_json::json!({"connection_status": "connected", "tool_count": tool_count}).to_string()),
            );
            // Also update connection_status column directly
            let _ = state.store.set_mcp_connection_status(&id, "connected", tool_count as i64);

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
                &id, None, None, None, None,
                Some(&serde_json::json!({"last_error": &err_msg}).to_string()),
            );
            Ok(Json(serde_json::json!({
                "success": false,
                "message": err_msg,
            })))
        }
    }
}

// ── PKCE helpers (RFC 7636) ─────────────────────────────────────────

fn generate_code_verifier() -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

fn compute_code_challenge(verifier: &str) -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    let hash = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(hash)
}

fn generate_state() -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
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
        .map_err(|e| to_error_response(types::NeboError::Internal(format!("OAuth discovery failed: {e}"))))?;

    info!(
        integration = %id,
        auth_endpoint = %metadata.authorization_endpoint,
        token_endpoint = %metadata.token_endpoint,
        registration = ?metadata.registration_endpoint,
        "discovered MCP OAuth metadata"
    );

    // 2. Dynamic Client Registration (if supported)
    let redirect_uri = format!("http://localhost:{}/api/v1/integrations/oauth/callback", state.config.port);
    let (client_id, client_secret) = if let Some(ref reg_endpoint) = metadata.registration_endpoint {
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
    let encrypted_secret = match &client_secret {
        Some(s) => Some(
            state
                .bridge
                .client()
                .encrypt_token(s)
                .map_err(|e| to_error_response(types::NeboError::Internal(format!("encrypt: {e}"))))?
        ),
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
    let integration = match state.store.get_mcp_integration_by_oauth_state(oauth_state).unwrap_or(None) {
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

    let redirect_uri = format!("http://localhost:{}/api/v1/integrations/oauth/callback", state.config.port);

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
            .connect(&integration.id, &tool_prefix, server_url, Some(&tokens.access_token))
            .await
        {
            Ok(tools) => {
                let _ = state.store.set_mcp_connection_status(
                    &integration.id,
                    "connected",
                    tools.len() as i64,
                );
                info!(integration = %integration.id, tools = tools.len(), "MCP connected after OAuth");
                return oauth_result_page(true, &format!("Connected — {} tools registered", tools.len()));
            }
            Err(e) => {
                warn!("MCP connect after OAuth failed: {e}");
                // Tokens stored — set as disconnected, user can retry
                let _ = state.store.set_mcp_connection_status(&integration.id, "disconnected", 0);
                return oauth_result_page(true, "Authorized — click Connect in Nebo to finish setup");
            }
        }
    }

    oauth_result_page(true, "Authorized — click Connect in Nebo to finish setup")
}

/// Render a simple HTML page that auto-closes the browser tab.
fn oauth_result_page(success: bool, message: &str) -> axum::response::Html<String> {
    let (icon, color) = if success { ("✓", "#22c55e") } else { ("✗", "#ef4444") };
    axum::response::Html(format!(r#"<!DOCTYPE html>
<html><head><title>Nebo — OAuth</title>
<style>body{{font-family:system-ui;background:#1e1e2e;color:#cdd6f4;display:flex;align-items:center;justify-content:center;height:100vh;margin:0}}
.box{{text-align:center;padding:2rem}}.icon{{font-size:3rem;color:{color}}}.msg{{margin-top:1rem;font-size:1.1rem;opacity:0.8}}.hint{{margin-top:1.5rem;font-size:0.85rem;opacity:0.5}}</style>
</head><body><div class="box"><div class="icon">{icon}</div><div class="msg">{message}</div>
<div class="hint">You can close this tab and return to Nebo.</div></div>
<script>setTimeout(function(){{window.close()}},3000)</script></body></html>"#))
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
pub async fn list_registry() -> HandlerResult<serde_json::Value> {
    // Built-in list of known MCP servers that users can install
    Ok(Json(serde_json::json!({
        "registry": [
            {
                "name": "filesystem",
                "description": "Read, write, and manage files on the local filesystem",
                "serverType": "stdio",
                "command": "npx",
                "args": ["-y", "@modelcontextprotocol/server-filesystem"]
            },
            {
                "name": "brave-search",
                "description": "Web search via Brave Search API",
                "serverType": "stdio",
                "command": "npx",
                "args": ["-y", "@modelcontextprotocol/server-brave-search"]
            },
            {
                "name": "github",
                "description": "GitHub repository management",
                "serverType": "stdio",
                "command": "npx",
                "args": ["-y", "@modelcontextprotocol/server-github"]
            },
            {
                "name": "sqlite",
                "description": "SQLite database operations",
                "serverType": "stdio",
                "command": "npx",
                "args": ["-y", "@modelcontextprotocol/server-sqlite"]
            },
            {
                "name": "memory",
                "description": "Knowledge graph-based persistent memory",
                "serverType": "stdio",
                "command": "npx",
                "args": ["-y", "@modelcontextprotocol/server-memory"]
            }
        ]
    })))
}

/// GET /api/v1/integrations/tools
pub async fn list_tools(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
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
