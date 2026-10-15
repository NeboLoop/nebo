use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio::sync::{RwLock, mpsc};
use tracing::{debug, info, warn};

use crate::crypto::Encryptor;
use crate::stdio::StdioSession;
use crate::{McpError, McpToolDef, McpToolResult, OAuthTokens};

/// MCP client for connecting to external MCP servers.
/// Handles OAuth 2.0 flows, token management, and tool invocation.
///
/// Two transports, one canonical surface: `sessions` holds remote HTTP/SSE
/// servers (stateless POSTs), `stdio_sessions` holds local stdio servers
/// (a long-lived child process each). `call_tool`/`close_session` route by
/// which map an integration lives in.
pub struct McpClient {
    pub(crate) http: reqwest::Client,
    encryptor: Arc<Encryptor>,
    sessions: RwLock<HashMap<String, Session>>,
    stdio_sessions: RwLock<HashMap<String, Arc<StdioSession>>>,
    /// Where a server's `notifications/tools/list_changed` goes: the id of
    /// the integration whose tools changed. The bridge takes the receiver
    /// (`Bridge::watch_tool_changes`) and lists the server's tools again.
    tools_changed: mpsc::UnboundedSender<String>,
    tools_changed_rx: std::sync::Mutex<Option<mpsc::UnboundedReceiver<String>>>,
    /// One refresh lock per integration (`McpClient::refresh_lock`).
    pub(crate) refresh_locks: std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

struct Session {
    server_url: String,
    tokens: Option<OAuthTokens>,
    _tools: Vec<McpToolDef>,
}

impl McpClient {
    pub fn new(encryptor: Arc<Encryptor>) -> Self {
        let (tools_changed, tools_changed_rx) = mpsc::unbounded_channel();
        Self {
            http: tls::http_client()
                .timeout(Duration::from_secs(30))
                .user_agent("nebo-mcp/1.0")
                .build()
                .expect("reqwest MCP client builder is infallible with these options"),
            encryptor,
            sessions: RwLock::new(HashMap::new()),
            stdio_sessions: RwLock::new(HashMap::new()),
            tools_changed,
            tools_changed_rx: std::sync::Mutex::new(Some(tools_changed_rx)),
            refresh_locks: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// The integrations whose servers said their tools changed
    /// (`notifications/tools/list_changed`), one id per notice. Taken once.
    pub fn take_tools_changed(&self) -> Option<mpsc::UnboundedReceiver<String>> {
        self.tools_changed_rx.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    /// List a connected server's tools again over the session it already
    /// has: the live stdio process, or the remote server's URL and token.
    pub async fn relist_tools(&self, integration_id: &str) -> Result<Vec<McpToolDef>, McpError> {
        if let Some(session) = self.stdio_sessions.read().await.get(integration_id).cloned() {
            return session.list_tools().await;
        }
        let (url, token) = {
            let sessions = self.sessions.read().await;
            let session = sessions
                .get(integration_id)
                .ok_or_else(|| McpError::NotFound(format!("session {}", integration_id)))?;
            (session.server_url.clone(), session.tokens.as_ref().map(|t| t.access_token.clone()))
        };
        self.list_tools(integration_id, &url, token.as_deref()).await
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
        let session = StdioSession::spawn(
            command,
            args,
            env,
            integration_id,
            self.tools_changed.clone(),
        )
        .await?;
        let tools = session.list_tools().await?;
        self.stdio_sessions
            .write()
            .await
            .insert(integration_id.to_string(), session);
        Ok(tools)
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
            let detail = format!(
                "tools/list returned {} — {}",
                status,
                resp_text.chars().take(200).collect::<String>()
            );
            return Err(if status == reqwest::StatusCode::UNAUTHORIZED {
                McpError::Unauthorized(detail)
            } else {
                McpError::Other(detail)
            });
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
            let detail = format!(
                "tools/call {} returned {} — {}",
                tool_name,
                status,
                resp_text.chars().take(200).collect::<String>()
            );
            return Err(if status == reqwest::StatusCode::UNAUTHORIZED {
                McpError::Unauthorized(detail)
            } else {
                McpError::Other(detail)
            });
        }

        // Parse response — raw JSON or SSE
        let body: serde_json::Value = if resp_text.starts_with('{') || resp_text.starts_with('[') {
            serde_json::from_str(&resp_text)
                .map_err(|e| McpError::Other(format!("invalid JSON: {e}")))?
        } else {
            // A server streams its notifications ahead of the answer; one
            // that says its tools changed gets them listed again.
            if sse_says_tools_changed(&resp_text) {
                let _ = self.tools_changed.send(integration_id.to_string());
            }
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
                content: format!("Connector error ({}): {}", code, message),
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
            result_preview = %::types::redact::redact(&result.to_string()).chars().take(500).collect::<String>(),
            "MCP tool returned no renderable content"
        );
        "(tool returned no content)".to_string()
    } else {
        parts.join("\n")
    };

    McpToolResult { content, is_error }
}

/// The JSON-RPC method a server sends when its tool list changed.
pub(crate) const TOOLS_LIST_CHANGED: &str = "notifications/tools/list_changed";

/// Whether an SSE response body carries `notifications/tools/list_changed`
/// among its messages.
fn sse_says_tools_changed(text: &str) -> bool {
    text.lines()
        .filter_map(|l| l.trim().strip_prefix("data:"))
        .filter_map(|d| serde_json::from_str::<serde_json::Value>(d.trim()).ok())
        .any(|m| m.get("method").and_then(|v| v.as_str()) == Some(TOOLS_LIST_CHANGED))
}

/// Whether a connector's error says the call's arguments don't fit the
/// tool: JSON-RPC's invalid params (-32602), or a server's own words for an
/// unknown or wrong field. The tool's definition may be stale, so it is
/// listed again before the call is tried once more.
pub fn is_schema_error(text: &str) -> bool {
    let t = text.to_lowercase();
    t.contains("(-32602)")
        || [
            "invalid params",
            "invalid arguments",
            "invalid argument",
            "unknown field",
            "unknown parameter",
            "unknown argument",
            "unexpected field",
            "unexpected property",
            "unexpected argument",
            "unrecognized key",
            "unrecognized field",
            "additional propert",
            "input validation error",
            "does not match schema",
            "schema validation",
        ]
        .iter()
        .any(|p| t.contains(p))
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
        data_preview = ?last_data.as_ref().map(|d| ::types::redact::redact(d).chars().take(300).collect::<String>()),
        raw_lines = text.lines().count(),
        raw_preview = %::types::redact::redact(text).chars().take(500).collect::<String>(),
        "parsing SSE response"
    );

    match last_data {
        Some(json_str) => serde_json::from_str(&json_str).map_err(|e| {
            warn!(
                error = %e,
                data = %::types::redact::redact(&json_str).chars().take(200).collect::<String>(),
                "SSE data JSON parse failed"
            );
            McpError::Other(format!("SSE data is not valid JSON: {e}"))
        }),
        None => {
            warn!(
                raw = %::types::redact::redact(text).chars().take(500).collect::<String>(),
                "no data: line found in SSE response"
            );
            Err(McpError::Other("No data found in SSE response".to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{is_schema_error, parse_call_result, sse_says_tools_changed};
    use serde_json::json;

    #[test]
    fn a_streamed_list_changed_notice_is_seen() {
        let body = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/tools/list_changed\"}\n\nevent: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{}}\n\n";
        assert!(sse_says_tools_changed(body));
        assert!(!sse_says_tools_changed("event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{}}\n"));
    }

    #[test]
    fn schema_errors_are_told_from_other_errors() {
        assert!(is_schema_error("Connector error (-32602): bad"));
        assert!(is_schema_error("Connector 'tools/call' error (-32602): bad"));
        assert!(is_schema_error("prices: unknown field `display_names`"));
        assert!(is_schema_error("Input validation error: additional properties not allowed"));
        assert!(!is_schema_error("Connector error (-32603): internal"));
        assert!(!is_schema_error("rate limited, try later"));
    }

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
