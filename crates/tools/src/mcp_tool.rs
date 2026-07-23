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
    token_expires_within(expires_at, 60)
}

/// Whether a stored OAuth token expires within `secs` from now. The proactive
/// refresher uses a wide window so tokens are renewed well before expiry and
/// never reach a 401 at connect time.
pub fn token_expires_within(expires_at: Option<i64>, secs: i64) -> bool {
    match expires_at {
        Some(exp) => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            now >= (exp - secs)
        }
        None => false, // no expiry info = assume valid
    }
}

/// Outcome of resolving an OAuth MCP integration's access token for a connect attempt.
pub enum TokenResolution {
    /// Connect with this token. `None` = non-OAuth / no token needed.
    Ready(Option<String>),
    /// Token is expired and could not be refreshed (refresh failed, no refresh
    /// token, or no stored token). Surface "needs reauth" — do NOT connect with a
    /// stale token, which would 401 and silently drop the server.
    NeedsReauth,
}

/// Resolve the access token to connect an MCP integration with — the single
/// canonical path for startup reconnect, manual connect, sync, and the test
/// button. Refreshes an expired token when possible; on failure returns
/// `NeedsReauth` instead of falling through to the stale token.
pub async fn resolve_mcp_token(
    store: &db::Store,
    client: &mcp::McpClient,
    integration: &db::models::McpIntegration,
) -> TokenResolution {
    if integration.auth_type == "api_key" {
        // Static bearer token — no expiry, no refresh. Missing/undecryptable key
        // surfaces as needs-reauth so Settings → MCP prompts for it.
        return match store.get_mcp_credential_full(&integration.id, "api_key") {
            Ok(Some(cred)) => match client.decrypt_token(&cred.credential_value) {
                Ok(t) => TokenResolution::Ready(Some(t)),
                Err(_) => TokenResolution::NeedsReauth,
            },
            _ => TokenResolution::NeedsReauth,
        };
    }
    if integration.auth_type != "oauth" {
        return TokenResolution::Ready(None);
    }
    let cred = match store.get_mcp_credential_full(&integration.id, "oauth_token") {
        Ok(Some(c)) => c,
        _ => return TokenResolution::NeedsReauth, // OAuth but no stored token
    };
    if !is_token_expired(cred.expires_at) {
        return match client.decrypt_token(&cred.credential_value) {
            Ok(t) => TokenResolution::Ready(Some(t)),
            Err(_) => TokenResolution::NeedsReauth,
        };
    }
    if cred.refresh_token.is_none() {
        return TokenResolution::NeedsReauth;
    }
    match refresh_mcp_token(store, client, &integration.id).await {
        Ok(new_token) => TokenResolution::Ready(Some(new_token)),
        Err(e) => {
            warn!(integration = %integration.id, error = %e, "MCP token refresh failed — needs reauth");
            TokenResolution::NeedsReauth
        }
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

/// Repair model-stringified structured args against the tool's input schema.
///
/// Weaker models (and some OpenAI-compatible gateways) emit object/array-typed
/// parameters as JSON *strings* — `{"policy": "{\"default_route\":...}"}` instead
/// of `{"policy": {...}}` — which the server then rejects as "type string, want
/// object". For each top-level property the schema declares as `object`/`array`,
/// if the model supplied a string that parses to that type, replace it with the
/// parsed value. Values that already match the declared type are left untouched,
/// so a legitimately string-typed field is never mangled.
pub(crate) fn coerce_schema_types(input: &mut serde_json::Value, schema: &serde_json::Value) {
    let (Some(obj), Some(props)) = (
        input.as_object_mut(),
        schema.get("properties").and_then(|p| p.as_object()),
    ) else {
        return;
    };
    for (key, val) in obj.iter_mut() {
        let serde_json::Value::String(s) = val else {
            continue;
        };
        let Some(types) = props.get(key).and_then(|p| p.get("type")) else {
            continue;
        };
        let accepts = |t: &str| match types {
            serde_json::Value::String(one) => one == t,
            serde_json::Value::Array(many) => many.iter().any(|v| v.as_str() == Some(t)),
            _ => false,
        };
        let wants_object = accepts("object");
        let wants_array = accepts("array");
        if !wants_object && !wants_array {
            continue;
        }
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(s) {
            if (wants_object && parsed.is_object()) || (wants_array && parsed.is_array()) {
                *val = parsed;
            }
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

#[cfg(test)]
mod coerce_tests {
    use super::coerce_schema_types;
    use serde_json::json;

    #[test]
    fn parses_stringified_object_and_array_leaves_strings_alone() {
        let schema = json!({
            "properties": {
                "policy": {"type": ["null", "object"]},
                "tags":   {"type": "array"},
                "key":    {"type": "string"},
            }
        });
        let mut input = json!({
            "policy": "{\"default_route\":{\"provider\":\"dashscope\",\"model\":\"glm-5.2\"}}",
            "tags":   "[\"a\",\"b\"]",
            "key":    "{\"not\":\"parsed\"}",
        });
        coerce_schema_types(&mut input, &schema);

        assert_eq!(input["policy"]["default_route"]["model"], "glm-5.2");
        assert!(input["tags"].is_array());
        // string-typed field is never mangled, even though it looks like JSON
        assert_eq!(input["key"], "{\"not\":\"parsed\"}");
    }

    #[test]
    fn already_correct_object_is_untouched() {
        let schema = json!({"properties": {"policy": {"type": "object"}}});
        let mut input = json!({"policy": {"default_route": {"model": "x"}}});
        let before = input.clone();
        coerce_schema_types(&mut input, &schema);
        assert_eq!(input, before);
    }
}
