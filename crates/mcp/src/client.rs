use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio::sync::RwLock;
use tracing::{info, warn, debug};

use crate::crypto::Encryptor;
use crate::{McpError, McpToolDef, McpToolResult, OAuthMetadata, OAuthTokens, RefreshResult};

/// MCP client for connecting to external MCP servers.
/// Handles OAuth 2.0 flows, token management, and tool invocation.
pub struct McpClient {
    http: reqwest::Client,
    encryptor: Arc<Encryptor>,
    sessions: RwLock<HashMap<String, Session>>,
}

struct Session {
    server_url: String,
    tokens: Option<OAuthTokens>,
    _tools: Vec<McpToolDef>,
}

impl McpClient {
    pub fn new(encryptor: Arc<Encryptor>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .user_agent("nebo-mcp/1.0")
                .build()
                .unwrap_or_default(),
            encryptor,
            sessions: RwLock::new(HashMap::new()),
        }
    }

    /// Discover OAuth metadata from a server's well-known endpoint.
    /// Per RFC 8414, the well-known URL is relative to the server's origin, not the MCP path.
    pub async fn discover_oauth(&self, server_url: &str) -> Result<OAuthMetadata, McpError> {
        // Extract origin (scheme + host + port) from server URL.
        // e.g. "https://monument.sh/mcp" → "https://monument.sh"
        let origin = {
            let trimmed = server_url.trim_end_matches('/');
            if let Some(pos) = trimmed.find("://") {
                let after_scheme = &trimmed[pos + 3..];
                // Find the first '/' after the host (if any)
                match after_scheme.find('/') {
                    Some(slash) => trimmed[..pos + 3 + slash].to_string(),
                    None => trimmed.to_string(),
                }
            } else {
                trimmed.to_string()
            }
        };
        let well_known = format!(
            "{}/.well-known/oauth-authorization-server",
            origin
        );
        let resp = self.http.get(&well_known).send().await?;
        if !resp.status().is_success() {
            return Err(McpError::Auth(format!(
                "OAuth discovery returned {}",
                resp.status()
            )));
        }
        let metadata: OAuthMetadata = resp.json().await?;
        Ok(metadata)
    }

    /// List tools from an external MCP server (JSON-RPC 2.0 over Streamable HTTP).
    /// All requests go to the MCP endpoint URL directly, not sub-paths.
    pub async fn list_tools(
        &self,
        integration_id: &str,
        server_url: &str,
        access_token: Option<&str>,
    ) -> Result<Vec<McpToolDef>, McpError> {
        let url = server_url.trim_end_matches('/');

        info!(
            url = url,
            has_token = access_token.is_some(),
            "MCP tools/list request"
        );

        let mut req = self.http.post(url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .json(&json!({
                "jsonrpc": "2.0",
                "method": "tools/list",
                "params": {},
                "id": 1
            }));
        if let Some(token) = access_token {
            req = req.bearer_auth(token);
        }

        let resp = req.send().await?;
        let status = resp.status();
        let resp_text = resp.text().await.unwrap_or_default();

        if !status.is_success() {
            return Err(McpError::Other(format!(
                "tools/list returned {} — {}",
                status,
                resp_text.chars().take(200).collect::<String>()
            )));
        }

        // Parse response — could be raw JSON or SSE (event: message\ndata: {...})
        let content_type = resp_text.chars().next().unwrap_or(' ');
        debug!(
            url = url,
            status = %status,
            response_len = resp_text.len(),
            first_char = %content_type,
            response_preview = %resp_text.chars().take(500).collect::<String>(),
            "MCP tools/list raw response"
        );

        let body: serde_json::Value = if resp_text.starts_with('{') || resp_text.starts_with('[') {
            serde_json::from_str(&resp_text)
                .map_err(|e| McpError::Other(format!("invalid JSON: {e}")))?
        } else {
            // Parse SSE: extract JSON from "data: {...}" lines
            parse_sse_json(&resp_text)?
        };

        let tools_val = body
            .get("result")
            .and_then(|r| r.get("tools"))
            .or_else(|| body.get("tools"))
            .cloned()
            .unwrap_or(json!([]));

        let tools: Vec<McpToolDef> = serde_json::from_value(tools_val)
            .unwrap_or_default();

        // Cache in session
        let mut sessions = self.sessions.write().await;
        sessions.insert(
            integration_id.to_string(),
            Session {
                server_url: server_url.to_string(),
                tokens: access_token.map(|t| OAuthTokens {
                    access_token: t.to_string(),
                    refresh_token: None,
                    expires_at: None,
                    scope: None,
                }),
                _tools: tools.clone(),
            },
        );

        info!(
            integration = integration_id,
            tools = tools.len(),
            "listed MCP tools"
        );
        Ok(tools)
    }

    /// Call a tool on an external MCP server (JSON-RPC 2.0 over Streamable HTTP).
    pub async fn call_tool(
        &self,
        integration_id: &str,
        tool_name: &str,
        input: serde_json::Value,
    ) -> Result<McpToolResult, McpError> {
        let sessions = self.sessions.read().await;
        let session = sessions
            .get(integration_id)
            .ok_or_else(|| McpError::NotFound(format!("session {}", integration_id)))?;

        let url = session.server_url.trim_end_matches('/');

        let mut req = self.http.post(url)
            .header("Content-Type", "application/json")
            .json(&json!({
                "jsonrpc": "2.0",
                "method": "tools/call",
                "params": {
                    "name": tool_name,
                    "arguments": input,
                },
                "id": 2
            }));

        if let Some(ref tokens) = session.tokens {
            req = req.bearer_auth(&tokens.access_token);
        }

        let resp = req.send().await?;
        let status = resp.status();
        let resp_text = resp.text().await.unwrap_or_default();

        if !status.is_success() {
            return Err(McpError::Other(format!(
                "tools/call {} returned {} — {}",
                tool_name,
                status,
                resp_text.chars().take(200).collect::<String>()
            )));
        }

        // Parse response — raw JSON or SSE
        let body: serde_json::Value = if resp_text.starts_with('{') || resp_text.starts_with('[') {
            serde_json::from_str(&resp_text)
                .map_err(|e| McpError::Other(format!("invalid JSON: {e}")))?
        } else {
            parse_sse_json(&resp_text)?
        };

        // Check for JSON-RPC error response first
        if let Some(err) = body.get("error") {
            let code = err.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
            let message = err.get("message").and_then(|m| m.as_str()).unwrap_or("unknown error");
            warn!(tool = tool_name, code = code, error = message, "MCP server returned JSON-RPC error");
            return Ok(McpToolResult {
                content: format!("MCP error ({}): {}", code, message),
                is_error: true,
            });
        }

        // JSON-RPC response: { "result": { "content": [...], "isError": false } }
        let result_val = body.get("result").cloned().unwrap_or(body.clone());

        #[derive(serde::Deserialize)]
        struct CallResult {
            #[serde(default)]
            content: Vec<ContentBlock>,
            #[serde(default, alias = "isError")]
            is_error: bool,
        }

        #[derive(serde::Deserialize)]
        struct ContentBlock {
            #[serde(rename = "type", default)]
            block_type: String,
            #[serde(default)]
            text: String,
        }

        let result: CallResult = serde_json::from_value(result_val)
            .unwrap_or(CallResult { content: vec![], is_error: true });

        let content = result
            .content
            .iter()
            .filter(|c| !c.text.is_empty())
            .map(|c| c.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        if content.is_empty() && !result.content.is_empty() {
            warn!(
                tool = tool_name,
                block_count = result.content.len(),
                block_types = %result.content.iter().map(|c| c.block_type.as_str()).collect::<Vec<_>>().join(", "),
                "MCP tool returned content blocks but all had empty text"
            );
        } else if content.is_empty() && result.content.is_empty() {
            warn!(
                tool = tool_name,
                response_preview = %resp_text.chars().take(500).collect::<String>(),
                "MCP tool returned no content blocks"
            );
        }

        Ok(McpToolResult {
            content,
            is_error: result.is_error,
        })
    }

    /// Close a session for an integration.
    pub async fn close_session(&self, integration_id: &str) {
        let mut sessions = self.sessions.write().await;
        sessions.remove(integration_id);
    }

    /// Encrypt a token for storage.
    pub fn encrypt_token(&self, token: &str) -> Result<String, McpError> {
        self.encryptor.encrypt_b64(token.as_bytes())
    }

    /// Decrypt a stored token.
    pub fn decrypt_token(&self, encrypted: &str) -> Result<String, McpError> {
        let bytes = self.encryptor.decrypt_b64(encrypted)?;
        String::from_utf8(bytes).map_err(|e| McpError::Crypto(e.to_string()))
    }

    /// Refresh an OAuth access token using the refresh_token grant type (RFC 6749 §6).
    /// Takes plaintext (decrypted) values. Caller handles encrypt/decrypt and persistence.
    pub async fn refresh_token(
        &self,
        token_endpoint: &str,
        client_id: &str,
        client_secret: Option<&str>,
        refresh_token: &str,
    ) -> Result<RefreshResult, McpError> {
        let mut params = vec![
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", client_id),
        ];
        if let Some(secret) = client_secret {
            params.push(("client_secret", secret));
        }

        let resp = self.http
            .post(token_endpoint)
            .form(&params)
            .timeout(Duration::from_secs(15))
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(McpError::Auth(format!(
                "token refresh returned {status}: {}",
                text.chars().take(200).collect::<String>()
            )));
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

        let t: TokenResponse = resp.json().await
            .map_err(|e| McpError::Auth(format!("decode refresh response: {e}")))?;

        Ok(RefreshResult {
            access_token: t.access_token,
            refresh_token: t.refresh_token,
            expires_in: t.expires_in,
            scope: t.scope,
        })
    }

    /// Update the access token in an existing session (after a refresh).
    pub async fn update_session_token(
        &self,
        integration_id: &str,
        tokens: OAuthTokens,
    ) {
        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.get_mut(integration_id) {
            session.tokens = Some(tokens);
        }
    }
}

/// Parse a JSON-RPC response from an SSE (Server-Sent Events) body.
/// SSE format: `event: message\ndata: {"jsonrpc":"2.0",...}\n\n`
/// Extracts and returns the JSON from the last `data:` line.
fn parse_sse_json(text: &str) -> Result<serde_json::Value, McpError> {
    let mut last_data = None;
    let mut event_type = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(evt) = trimmed.strip_prefix("event:") {
            event_type = Some(evt.trim().to_string());
        } else if let Some(data) = trimmed.strip_prefix("data:") {
            let json_str = data.trim();
            if !json_str.is_empty() {
                last_data = Some(json_str.to_string());
            }
        }
    }

    debug!(
        event_type = ?event_type,
        data_preview = ?last_data.as_ref().map(|d| d.chars().take(300).collect::<String>()),
        raw_lines = text.lines().count(),
        raw_preview = %text.chars().take(500).collect::<String>(),
        "parsing SSE response"
    );

    match last_data {
        Some(json_str) => serde_json::from_str(&json_str).map_err(|e| {
            warn!(
                error = %e,
                data = %json_str.chars().take(200).collect::<String>(),
                "SSE data JSON parse failed"
            );
            McpError::Other(format!("SSE data is not valid JSON: {e}"))
        }),
        None => {
            warn!(
                raw = %text.chars().take(500).collect::<String>(),
                "no data: line found in SSE response"
            );
            Err(McpError::Other(
                "No data found in SSE response".to_string(),
            ))
        }
    }
}
