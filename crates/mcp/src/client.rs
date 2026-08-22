use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::crypto::Encryptor;
use crate::stdio::StdioSession;
use crate::{McpError, McpToolDef, McpToolResult, OAuthMetadata, OAuthTokens, RefreshResult};

/// MCP client for connecting to external MCP servers.
/// Handles OAuth 2.0 flows, token management, and tool invocation.
///
/// Two transports, one canonical surface: `sessions` holds remote HTTP/SSE
/// servers (stateless POSTs), `stdio_sessions` holds local stdio servers
/// (a long-lived child process each). `call_tool`/`close_session` route by
/// which map an integration lives in.
pub struct McpClient {
    http: reqwest::Client,
    encryptor: Arc<Encryptor>,
    sessions: RwLock<HashMap<String, Session>>,
    stdio_sessions: RwLock<HashMap<String, Arc<StdioSession>>>,
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
            stdio_sessions: RwLock::new(HashMap::new()),
        }
    }

    /// Connect to a local stdio MCP server: spawn the process, handshake, and
    /// return its tools. The session is held for the lifetime of the connection
    /// so `call_tool` can reuse the same process.
    pub async fn connect_stdio(
        &self,
        integration_id: &str,
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
    ) -> Result<Vec<McpToolDef>, McpError> {
        let session = StdioSession::spawn(command, args, env).await?;
        let tools = session.list_tools().await?;
        self.stdio_sessions
            .write()
            .await
            .insert(integration_id.to_string(), session);
        Ok(tools)
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
        let well_known = format!("{}/.well-known/oauth-authorization-server", origin);
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

        let mut req = self
            .http
            .post(url)
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

        let tools: Vec<McpToolDef> = serde_json::from_value(tools_val).unwrap_or_default();

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
        self.call_tool_scoped(integration_id, tool_name, input, None).await
    }

    /// Call a tool inside a confidentiality scope (a client matter).
    ///
    /// The scope travels as a HEADER, never as a tool argument: an argument is
    /// something the model authors, and a model that can name its own matter
    /// can name somebody else's. The server treats the header as the authority
    /// and refuses anything outside it.
    pub async fn call_tool_scoped(
        &self,
        integration_id: &str,
        tool_name: &str,
        input: serde_json::Value,
        matter: Option<&str>,
    ) -> Result<McpToolResult, McpError> {
        // Stdio servers route over their live child process.
        if let Some(session) = self.stdio_sessions.read().await.get(integration_id).cloned() {
            return session.call_tool(tool_name, input).await;
        }

        let sessions = self.sessions.read().await;
        let session = sessions
            .get(integration_id)
            .ok_or_else(|| McpError::NotFound(format!("session {}", integration_id)))?;

        let url = session.server_url.trim_end_matches('/');

        let mut req = self
            .http
            .post(url)
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
        if let Some(m) = matter {
            req = req.header("X-Nebo-Memory-Domain", m);
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
            let message = err
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error");
            warn!(
                tool = tool_name,
                code = code,
                error = message,
                "MCP server returned JSON-RPC error"
            );
            return Ok(McpToolResult {
                content: format!("MCP error ({}): {}", code, message),
                is_error: true,
            });
        }

        // JSON-RPC response: { "result": { "content": [...], "isError": false } }
        let result_val = body.get("result").cloned().unwrap_or(body.clone());

        Ok(parse_call_result(tool_name, &result_val))
    }

    /// Close a session for an integration.
    pub async fn close_session(&self, integration_id: &str) {
        self.sessions.write().await.remove(integration_id);
        // Stdio servers own a child process — kill it on close.
        if let Some(session) = self.stdio_sessions.write().await.remove(integration_id) {
            session.shutdown().await;
        }
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

        let resp = self
            .http
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

        let t: TokenResponse = resp
            .json()
            .await
            .map_err(|e| McpError::Auth(format!("decode refresh response: {e}")))?;

        Ok(RefreshResult {
            access_token: t.access_token,
            refresh_token: t.refresh_token,
            expires_in: t.expires_in,
            scope: t.scope,
        })
    }

    /// Update the access token in an existing session (after a refresh).
    pub async fn update_session_token(&self, integration_id: &str, tokens: OAuthTokens) {
        let mut sessions = self.sessions.write().await;
        if let Some(session) = sessions.get_mut(integration_id) {
            session.tokens = Some(tokens);
        }
    }
}

/// Flatten an MCP `tools/call` result into the text the model sees.
/// Shared by both transports (HTTP and stdio) — the ONE place a call result
/// becomes model-visible text. Text blocks pass through verbatim; every other
/// block shape (image, resource, resource_link, …) is preserved as compact
/// JSON instead of being dropped; `structuredContent` is appended
/// pretty-printed. Only a truly empty result yields the "(tool returned no
/// content)" placeholder, so the model never sees silence.
pub(crate) fn parse_call_result(tool_name: &str, result: &serde_json::Value) -> McpToolResult {
    let is_error = result
        .get("isError")
        .or_else(|| result.get("is_error"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let mut parts: Vec<String> = Vec::new();
    if let Some(blocks) = result.get("content").and_then(|c| c.as_array()) {
        for block in blocks {
            if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                    if !text.is_empty() {
                        parts.push(text.to_string());
                    }
                }
            } else {
                // Non-text block: keep the whole block as compact JSON
                // rather than dropping it.
                parts.push(block.to_string());
            }
        }
    }

    if let Some(structured) = result.get("structuredContent") {
        if !structured.is_null() {
            parts.push(
                serde_json::to_string_pretty(structured).unwrap_or_else(|_| structured.to_string()),
            );
        }
    }

    let content = if parts.is_empty() {
        warn!(
            tool = tool_name,
            result_preview = %result.to_string().chars().take(500).collect::<String>(),
            "MCP tool returned no renderable content"
        );
        "(tool returned no content)".to_string()
    } else {
        parts.join("\n")
    };

    McpToolResult { content, is_error }
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
            Err(McpError::Other("No data found in SSE response".to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_call_result;
    use serde_json::json;

    #[test]
    fn text_only_joins_blocks() {
        let result = json!({
            "content": [
                { "type": "text", "text": "line one" },
                { "type": "text", "text": "line two" }
            ],
            "isError": false
        });
        let r = parse_call_result("t", &result);
        assert_eq!(r.content, "line one\nline two");
        assert!(!r.is_error);
    }

    #[test]
    fn structured_content_only() {
        let structured = json!({ "record": { "name": "Acme", "balance": 42 } });
        let result = json!({
            "content": [],
            "structuredContent": structured
        });
        let r = parse_call_result("t", &result);
        assert_eq!(
            r.content,
            serde_json::to_string_pretty(&structured).unwrap()
        );
        assert!(!r.is_error);
    }

    #[test]
    fn mixed_text_and_structured() {
        let structured = json!({ "id": 7 });
        let result = json!({
            "content": [{ "type": "text", "text": "summary" }],
            "structuredContent": structured
        });
        let r = parse_call_result("t", &result);
        let expected = format!(
            "summary\n{}",
            serde_json::to_string_pretty(&structured).unwrap()
        );
        assert_eq!(r.content, expected);
    }

    #[test]
    fn image_block_preserved_as_json() {
        let result = json!({
            "content": [
                { "type": "image", "data": "aGVsbG8=", "mimeType": "image/png" }
            ]
        });
        let r = parse_call_result("t", &result);
        assert!(r.content.contains("\"type\":\"image\""));
        assert!(r.content.contains("image/png"));
        assert!(r.content.contains("aGVsbG8="));
    }

    #[test]
    fn truly_empty_returns_placeholder() {
        let r = parse_call_result("t", &json!({ "content": [] }));
        assert_eq!(r.content, "(tool returned no content)");
        assert!(!r.is_error);
    }

    #[test]
    fn is_error_passes_through() {
        let result = json!({
            "content": [{ "type": "text", "text": "boom" }],
            "isError": true
        });
        let r = parse_call_result("t", &result);
        assert_eq!(r.content, "boom");
        assert!(r.is_error);
    }
}
