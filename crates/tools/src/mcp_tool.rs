use std::sync::Arc;

use tracing::{info, warn};

use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

/// McpTool enumerates connected MCP servers. It is the discovery verb for MCP —
/// `mcp(action: "list")` — NOT a call path. Each MCP tool is exposed to the model as
/// its own proxy tool (`mcp__<server>__<tool>`) carrying the server's real input
/// schema, so the model calls it with correct arguments. Those proxies are the single
/// canonical call pathway (see `call_mcp_tool` + `McpProxyTool` in registry.rs).
pub struct McpTool {
    bridge: Arc<mcp::Bridge>,
}

/// Check if a stored OAuth token is expired (with 60s buffer).
pub fn is_token_expired(expires_at: Option<i64>) -> bool {
    match expires_at {
        Some(exp) => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            now >= (exp - 60)
        }
        None => false, // no expiry info = assume valid
    }
}

/// Refresh an MCP integration's OAuth token. Orchestrates DB read → decrypt → HTTP refresh →
/// encrypt → DB write → session update. Returns the new plaintext access_token.
pub async fn refresh_mcp_token(
    store: &db::Store,
    client: &mcp::McpClient,
    integration_id: &str,
) -> Result<String, String> {
    // 1. Read OAuth config from integration row
    let oauth_config = store
        .get_mcp_oauth_config(integration_id)
        .map_err(|e| format!("read oauth config: {e}"))?
        .ok_or("no oauth config found")?;

    let token_endpoint = oauth_config
        .oauth_token_endpoint
        .ok_or("no token_endpoint on integration")?;
    let client_id = oauth_config
        .oauth_client_id
        .ok_or("no client_id on integration")?;

    // Decrypt client_secret if present
    let client_secret = oauth_config
        .oauth_client_secret
        .as_deref()
        .and_then(|enc| client.decrypt_token(enc).ok());

    // 2. Read credential with refresh_token
    let cred = store
        .get_mcp_credential_full(integration_id, "oauth_token")
        .map_err(|e| format!("read credential: {e}"))?
        .ok_or("no credential found")?;

    let encrypted_refresh = cred.refresh_token.ok_or("no refresh_token stored")?;
    let refresh_token = client
        .decrypt_token(&encrypted_refresh)
        .map_err(|e| format!("decrypt refresh_token: {e}"))?;

    // 3. Call refresh endpoint
    let result = client
        .refresh_token(
            &token_endpoint,
            &client_id,
            client_secret.as_deref(),
            &refresh_token,
        )
        .await
        .map_err(|e| format!("refresh request failed: {e}"))?;

    // 4. Encrypt and store new tokens
    let new_encrypted_access = client
        .encrypt_token(&result.access_token)
        .map_err(|e| format!("encrypt new access_token: {e}"))?;

    // Use rotated refresh_token if server provided one, otherwise keep old
    let new_encrypted_refresh = match &result.refresh_token {
        Some(new_rt) => Some(
            client
                .encrypt_token(new_rt)
                .map_err(|e| format!("encrypt new refresh_token: {e}"))?,
        ),
        None => Some(encrypted_refresh.clone()),
    };

    let new_expires_at = result.expires_in.map(|secs| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
            + secs
    });

    store
        .store_mcp_credentials(
            integration_id,
            "oauth_token",
            &new_encrypted_access,
            new_encrypted_refresh.as_deref(),
            new_expires_at,
            result.scope.as_deref(),
        )
        .map_err(|e| format!("store new credentials: {e}"))?;

    // 5. Update in-memory session
    let plain_refresh = result.refresh_token.unwrap_or(refresh_token);
    client
        .update_session_token(
            integration_id,
            mcp::OAuthTokens {
                access_token: result.access_token.clone(),
                refresh_token: Some(plain_refresh),
                expires_at: new_expires_at,
                scope: result.scope,
            },
        )
        .await;

    Ok(result.access_token)
}

/// Attempt proactive token refresh if an integration's OAuth token is expired.
/// Returns true if a refresh happened. Shared by the per-tool MCP proxies.
async fn maybe_refresh_token(store: &db::Store, bridge: &mcp::Bridge, integration_id: &str) -> bool {
    let integration = match store.get_mcp_integration(integration_id) {
        Ok(Some(i)) if i.auth_type == "oauth" => i,
        _ => return false,
    };

    let cred = match store.get_mcp_credential_full(&integration.id, "oauth_token") {
        Ok(Some(c)) => c,
        _ => return false,
    };

    if !is_token_expired(cred.expires_at) || cred.refresh_token.is_none() {
        return false;
    }

    info!(integration = integration_id, "MCP token expired, attempting refresh");
    match refresh_mcp_token(store, bridge.client(), integration_id).await {
        Ok(_) => {
            info!(integration = integration_id, "MCP token refreshed");
            true
        }
        Err(e) => {
            warn!(integration = integration_id, error = %e, "MCP token refresh failed");
            false
        }
    }
}

/// Canonical MCP tool execution: proactive token refresh, call via the bridge, and a
/// single 401-retry that refreshes the token before retrying. This is THE call pathway
/// for MCP tools — invoked by the per-tool proxy tools (`McpProxyTool`). The input is
/// forwarded verbatim to the underlying tool (no argument stripping), so a tool whose
/// own schema requires `resource`/`action` receives them unchanged.
pub async fn call_mcp_tool(
    store: &db::Store,
    bridge: &mcp::Bridge,
    integration_id: &str,
    tool_name: &str,
    input: serde_json::Value,
) -> ToolResult {
    // Proactive refresh: if the token is expired, refresh before calling.
    maybe_refresh_token(store, bridge, integration_id).await;

    match bridge.call_tool(integration_id, tool_name, input.clone()).await {
        Ok(result) => {
            if result.is_error {
                ToolResult::error(result.content)
            } else {
                ToolResult::ok(result.content)
            }
        }
        Err(e) => {
            let err_str = e.to_string();
            // Retry once on 401 (token may have been revoked server-side before expiry).
            if err_str.contains("401") || err_str.contains("Unauthorized") {
                info!(integration = %integration_id, "MCP 401, attempting token refresh");
                match refresh_mcp_token(store, bridge.client(), integration_id).await {
                    Ok(_) => match bridge.call_tool(integration_id, tool_name, input).await {
                        Ok(result) => {
                            if result.is_error {
                                ToolResult::error(result.content)
                            } else {
                                ToolResult::ok(result.content)
                            }
                        }
                        Err(retry_err) => ToolResult::error(format!(
                            "MCP call failed after token refresh: {}",
                            retry_err
                        )),
                    },
                    Err(refresh_err) => {
                        let _ =
                            store.set_mcp_connection_status(integration_id, "disconnected", 0);
                        ToolResult::error(format!(
                            "MCP authentication expired and refresh failed: {}. Reconnect the integration in settings.",
                            refresh_err
                        ))
                    }
                }
            } else {
                ToolResult::error(format!("MCP call failed: {}", e))
            }
        }
    }
}

impl McpTool {
    pub fn new(bridge: Arc<mcp::Bridge>) -> Self {
        Self { bridge }
    }

    /// Build a dynamic description listing connected servers. This tool only
    /// enumerates servers; each server's tools are called via their own proxy tools.
    fn build_description(&self) -> String {
        let mut desc = String::from(
            "List connected MCP servers. Usage: mcp(action: \"list\").\n\n\
             To CALL a tool on a server, use that tool's own proxy tool named \
             `mcp__<server>__<tool>` (e.g. `mcp__neboloop__customer`). Discover the exact \
             names and argument schemas with tool_search(query: \"<server or capability>\"), \
             then call the proxy directly — its arguments match the server's real schema.\n\n",
        );

        let connected = self.bridge.connected_tools();
        if connected.is_empty() {
            desc.push_str(
                "No MCP servers currently connected. Add servers in Connectors settings.\n",
            );
        } else {
            desc.push_str("Connected servers:\n");
            for (server, tools) in &connected {
                let display = server.replace('_', ".");
                desc.push_str(&format!("- {} → {}\n", display, tools.join(", ")));
            }
        }

        desc
    }
}

impl DynTool for McpTool {
    fn name(&self) -> &str {
        "mcp"
    }

    fn description(&self) -> String {
        self.build_description()
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list"],
                    "description": "Only \"list\" — enumerate connected MCP servers. To call a tool, use its `mcp__<server>__<tool>` proxy (find it with tool_search)."
                }
            },
            "additionalProperties": false
        })
    }

    fn requires_approval(&self) -> bool {
        // Enumeration only — read-only, no approval needed.
        false
    }

    fn execute_dyn<'a>(
        &'a self,
        _ctx: &'a ToolContext,
        _input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let connected = self.bridge.connected_tools();
            if connected.is_empty() {
                return ToolResult::ok(
                    "No MCP servers connected. Add servers in Connectors settings.",
                );
            }
            let lines: Vec<String> = connected
                .iter()
                .map(|(server, tools)| {
                    format!(
                        "- {} ({} tools): call via mcp__{}__<tool> (e.g. mcp__{}__{})",
                        server.replace('_', "."),
                        tools.len(),
                        server,
                        server,
                        tools.first().map(|t| t.as_str()).unwrap_or("<tool>")
                    )
                })
                .collect();
            ToolResult::ok(format!(
                "{} connected MCP server(s). Discover a tool's schema with tool_search, then call its mcp__<server>__<tool> proxy:\n{}",
                connected.len(),
                lines.join("\n")
            ))
        })
    }
}
