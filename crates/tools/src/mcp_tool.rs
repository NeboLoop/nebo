use std::sync::Arc;

use tracing::{info, warn};

use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

/// McpTool is a STRAP domain tool for calling connected MCP servers.
/// Usage: mcp(server: "monument.sh", resource: "project", action: "list")
pub struct McpTool {
    bridge: Arc<mcp::Bridge>,
    store: Arc<db::Store>,
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

    let token_endpoint = oauth_config.oauth_token_endpoint
        .ok_or("no token_endpoint on integration")?;
    let client_id = oauth_config.oauth_client_id
        .ok_or("no client_id on integration")?;

    // Decrypt client_secret if present
    let client_secret = oauth_config.oauth_client_secret
        .as_deref()
        .and_then(|enc| client.decrypt_token(enc).ok());

    // 2. Read credential with refresh_token
    let cred = store
        .get_mcp_credential_full(integration_id, "oauth_token")
        .map_err(|e| format!("read credential: {e}"))?
        .ok_or("no credential found")?;

    let encrypted_refresh = cred.refresh_token
        .ok_or("no refresh_token stored")?;
    let refresh_token = client
        .decrypt_token(&encrypted_refresh)
        .map_err(|e| format!("decrypt refresh_token: {e}"))?;

    // 3. Call refresh endpoint
    let result = client
        .refresh_token(&token_endpoint, &client_id, client_secret.as_deref(), &refresh_token)
        .await
        .map_err(|e| format!("refresh request failed: {e}"))?;

    // 4. Encrypt and store new tokens
    let new_encrypted_access = client
        .encrypt_token(&result.access_token)
        .map_err(|e| format!("encrypt new access_token: {e}"))?;

    // Use rotated refresh_token if server provided one, otherwise keep old
    let new_encrypted_refresh = match &result.refresh_token {
        Some(new_rt) => Some(
            client.encrypt_token(new_rt)
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

    store.store_mcp_credentials(
        integration_id,
        "oauth_token",
        &new_encrypted_access,
        new_encrypted_refresh.as_deref(),
        new_expires_at,
        result.scope.as_deref(),
    ).map_err(|e| format!("store new credentials: {e}"))?;

    // 5. Update in-memory session
    let plain_refresh = result.refresh_token.unwrap_or(refresh_token);
    client.update_session_token(
        integration_id,
        mcp::OAuthTokens {
            access_token: result.access_token.clone(),
            refresh_token: Some(plain_refresh),
            expires_at: new_expires_at,
            scope: result.scope,
        },
    ).await;

    Ok(result.access_token)
}

impl McpTool {
    pub fn new(bridge: Arc<mcp::Bridge>, store: Arc<db::Store>) -> Self {
        Self { bridge, store }
    }

    /// Build a dynamic description showing connected servers and their tools.
    fn build_description(&self) -> String {
        let mut desc = String::from(
            "Call tools on connected MCP servers.\n\n\
             Resources (server name) and Actions (tool name) depend on which servers are connected.\n\n"
        );

        // List connected servers and their tools from the bridge
        let connected = self.bridge.connected_tools();
        if connected.is_empty() {
            desc.push_str("No MCP servers currently connected. Add servers in Connectors settings.\n");
        } else {
            desc.push_str("Connected servers:\n");
            for (server, tools) in &connected {
                let display = server.replace('_', ".");
                desc.push_str(&format!("- {} → {}\n", display, tools.join(", ")));
            }
            desc.push_str("\nExamples:\n");
            if let Some((server, tools)) = connected.first() {
                let display = server.replace('_', ".");
                if let Some(tool) = tools.first() {
                    desc.push_str(&format!(
                        "  mcp(server: \"{}\", resource: \"{}\", action: \"list\")\n",
                        display, tool
                    ));
                }
            }
        }

        desc
    }

    /// Attempt proactive token refresh if expired, returning true if refresh happened.
    async fn maybe_refresh_token(&self, integration_id: &str) -> bool {
        // Check if this is an OAuth integration with expired token
        let integration = match self.store.get_mcp_integration(integration_id) {
            Ok(Some(i)) if i.auth_type == "oauth" => i,
            _ => return false,
        };

        let cred = match self.store.get_mcp_credential_full(&integration.id, "oauth_token") {
            Ok(Some(c)) => c,
            _ => return false,
        };

        if !is_token_expired(cred.expires_at) {
            return false;
        }

        if cred.refresh_token.is_none() {
            return false;
        }

        info!(integration = integration_id, "MCP token expired, attempting refresh");
        match refresh_mcp_token(&self.store, self.bridge.client(), integration_id).await {
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
                "server": {
                    "type": "string",
                    "description": "MCP server name (e.g., \"monument.sh\")"
                },
                "resource": {
                    "type": "string",
                    "description": "Tool/resource name on the server (e.g., \"project\", \"todo\")"
                },
                "action": {
                    "type": "string",
                    "description": "Action to perform (e.g., \"list\", \"create\", \"get\")"
                }
            },
            "required": ["server", "resource"],
            "additionalProperties": true
        })
    }

    fn requires_approval(&self) -> bool {
        true
    }

    fn execute_dyn<'a>(
        &'a self,
        _ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let server = match input.get("server").and_then(|v| v.as_str()) {
                Some(s) => s,
                None => return ToolResult::error("server is required. Specify the MCP server name."),
            };
            let resource = match input.get("resource").and_then(|v| v.as_str()) {
                Some(r) => r,
                None => return ToolResult::error("resource is required. Specify the tool name on the server."),
            };

            // Slugify server name to match the bridge key
            let server_slug = server
                .to_lowercase()
                .chars()
                .map(|c| if c.is_alphanumeric() { c } else { '_' })
                .collect::<String>();
            let server_slug = server_slug.trim_matches('_');

            // Find the integration by matching slug against connected tools
            let connected = self.bridge.connected_tools();
            let (integration_id, _tools) = match connected.iter().find(|(s, _)| {
                s == server_slug || s.contains(server_slug)
            }) {
                Some((_, tools)) => {
                    // Find integration ID from bridge connections
                    match self.bridge.find_integration_for_tool(server_slug, resource) {
                        Some(id) => (id, tools),
                        None => return ToolResult::error(format!(
                            "Server '{}' is connected but tool '{}' not found. Available tools: {}",
                            server, resource, tools.join(", ")
                        )),
                    }
                }
                None => {
                    let available: Vec<String> = connected.iter()
                        .map(|(s, _)| s.replace('_', "."))
                        .collect();
                    return ToolResult::error(format!(
                        "MCP server '{}' is not connected. Connected servers: {}",
                        server,
                        if available.is_empty() { "none".to_string() } else { available.join(", ") }
                    ));
                }
            };

            // Build the input for the MCP call — pass everything except server/resource
            let mut mcp_input = input.clone();
            if let Some(obj) = mcp_input.as_object_mut() {
                obj.remove("server");
                obj.remove("resource");
            }

            // Proactive refresh: if token is expired, refresh before calling
            self.maybe_refresh_token(&integration_id).await;

            // Call the tool via bridge
            match self.bridge.call_tool(&integration_id, resource, mcp_input.clone()).await {
                Ok(result) => {
                    if result.is_error {
                        ToolResult::error(result.content)
                    } else {
                        ToolResult::ok(result.content)
                    }
                }
                Err(e) => {
                    let err_str = e.to_string();
                    // Retry once on 401 (token may have been revoked server-side before expiry)
                    if err_str.contains("401") || err_str.contains("Unauthorized") {
                        info!(integration = %integration_id, "MCP 401, attempting token refresh");
                        match refresh_mcp_token(&self.store, self.bridge.client(), &integration_id).await {
                            Ok(_) => {
                                match self.bridge.call_tool(&integration_id, resource, mcp_input).await {
                                    Ok(result) => {
                                        if result.is_error {
                                            ToolResult::error(result.content)
                                        } else {
                                            ToolResult::ok(result.content)
                                        }
                                    }
                                    Err(retry_err) => ToolResult::error(format!(
                                        "MCP call failed after token refresh: {}", retry_err
                                    )),
                                }
                            }
                            Err(refresh_err) => {
                                let _ = self.store.set_mcp_connection_status(&integration_id, "disconnected", 0);
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
        })
    }
}
