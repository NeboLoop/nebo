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
    /// The next JSON-RPC id a remote request carries.
    next_id: std::sync::atomic::AtomicI64,
}

struct Session {
    server_url: String,
    tokens: Option<OAuthTokens>,
    /// The session the server issued at `initialize` (`Mcp-Session-Id`),
    /// sent on every request after it. `None` for a server that issues none.
    mcp_session: Option<String>,
    /// The protocol version `initialize` settled on (`MCP-Protocol-Version`).
    protocol: Option<String>,
    _tools: Vec<McpToolDef>,
}

/// The header a Streamable HTTP server names its session in.
const SESSION_HEADER: &str = "mcp-session-id";

/// The protocol version Nebo asks for at `initialize`.
pub(crate) const PROTOCOL_VERSION: &str = "2025-06-18";

/// `initialize`'s params, the same on both transports: the protocol
/// version, the client's capabilities (the MCP Apps extension,
/// `io.modelcontextprotocol/ui`, among them) and who the client is.
pub(crate) fn initialize_params() -> serde_json::Value {
    let mut extensions = serde_json::Map::new();
    extensions.insert(crate::ui::EXTENSION_ID.to_string(), crate::ui::client_extension());
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": { "extensions": extensions },
        "clientInfo": { "name": "nebo", "version": env!("CARGO_PKG_VERSION") }
    })
}

/// Where one request to a remote server goes, and what it carries.
struct HttpTarget {
    url: String,
    token: Option<String>,
    mcp_session: Option<String>,
    protocol: Option<String>,
}

impl HttpTarget {
    fn request(&self, http: &reqwest::Client) -> reqwest::RequestBuilder {
        let mut req = http
            .post(&self.url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream");
        if let Some(token) = &self.token {
            req = req.bearer_auth(token);
        }
        if let Some(session) = &self.mcp_session {
            req = req.header(SESSION_HEADER, session);
        }
        if let Some(protocol) = &self.protocol {
            req = req.header("MCP-Protocol-Version", protocol);
        }
        req
    }
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
            next_id: std::sync::atomic::AtomicI64::new(1),
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
    /// All requests go to the MCP endpoint URL directly, not sub-paths. A
    /// server first listed here gets the lifecycle's `initialize` handshake
    /// (its session, if it issues one, is kept for every request after);
    /// listing it again reuses the session it has.
    pub async fn list_tools(
        &self,
        integration_id: &str,
        server_url: &str,
        access_token: Option<&str>,
    ) -> Result<Vec<McpToolDef>, McpError> {
        let url = server_url.trim_end_matches('/').to_string();

        info!(
            url = %url,
            has_token = access_token.is_some(),
            "MCP tools/list request"
        );

        let known = {
            let sessions = self.sessions.read().await;
            sessions
                .get(integration_id)
                .filter(|s| s.server_url.trim_end_matches('/') == url)
                .map(|s| (s.mcp_session.clone(), s.protocol.clone()))
        };
        let (mcp_session, protocol) = match known {
            Some(handshake) => handshake,
            None => self.handshake(integration_id, &url, access_token).await?,
        };
        let mut target = HttpTarget { url: url.clone(), token: access_token.map(str::to_string), mcp_session, protocol };
        let body = match self.post(integration_id, &target, "tools/list", json!({}), &[]).await {
            Err(McpError::SessionExpired(_)) => {
                (target.mcp_session, target.protocol) = self.handshake(integration_id, &url, access_token).await?;
                self.post(integration_id, &target, "tools/list", json!({}), &[]).await?
            }
            other => other?,
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
                mcp_session: target.mcp_session,
                protocol: target.protocol,
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

    /// The lifecycle's opening for a remote server: `initialize` with this
    /// client's capabilities ([`initialize_params`]), then
    /// `notifications/initialized`. Returns the session the server issued
    /// (`Mcp-Session-Id`) and the protocol version it settled on. A server
    /// that answers `initialize` with a JSON-RPC error doesn't keep the
    /// lifecycle; it is spoken to without a session, as before.
    async fn handshake(
        &self,
        integration_id: &str,
        url: &str,
        token: Option<&str>,
    ) -> Result<(Option<String>, Option<String>), McpError> {
        let opening = HttpTarget { url: url.to_string(), token: token.map(str::to_string), mcp_session: None, protocol: None };
        let (body, mcp_session) = self.post_with_session(integration_id, &opening, "initialize", initialize_params(), &[]).await?;
        if let Some(err) = body.get("error") {
            warn!(url, error = %err, "MCP server refused initialize; speaking to it without a session");
            return Ok((None, None));
        }
        let protocol = body
            .get("result")
            .and_then(|r| r.get("protocolVersion"))
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let target = HttpTarget { mcp_session: mcp_session.clone(), protocol: protocol.clone(), ..opening };
        if let Err(e) = self.notify_http(&target, "notifications/initialized").await {
            warn!(url, error = %e, "MCP server did not take notifications/initialized");
        }
        Ok((mcp_session, protocol))
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

        let headers: Vec<(&str, &str)> = matter.map(|m| ("X-Nebo-Memory-Domain", m)).into_iter().collect();
        let body = self
            .http_request(integration_id, "tools/call", json!({ "name": tool_name, "arguments": input }), &headers)
            .await?;

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
                result: None,
            });
        }

        // JSON-RPC response: { "result": { "content": [...], "isError": false } }
        let result_val = body.get("result").cloned().unwrap_or(body.clone());

        Ok(parse_call_result(tool_name, &result_val))
    }

    /// Read a view (`resources/read` of a `ui://` resource) from a connected
    /// server, over whichever transport it has. The content must be
    /// `text/html;profile=mcp-app` (`ui::view_from_read`).
    pub async fn read_resource(&self, integration_id: &str, uri: &str) -> Result<crate::ui::UiResource, McpError> {
        let params = json!({ "uri": uri });
        let result = if let Some(session) = self.stdio_sessions.read().await.get(integration_id).cloned() {
            session.request("resources/read", params).await?
        } else {
            let body = self.http_request(integration_id, "resources/read", params, &[]).await?;
            if let Some(err) = body.get("error") {
                let code = err.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
                let message = err.get("message").and_then(|m| m.as_str()).unwrap_or("unknown error");
                return Err(McpError::Other(format!("Connector 'resources/read' error ({code}): {message}")));
            }
            body.get("result").cloned().unwrap_or(serde_json::Value::Null)
        };
        crate::ui::view_from_read(uri, &result)
    }

    /// One request on a connected remote server's session; the reply's
    /// whole JSON-RPC body. A server that forgot the session (`404`, as the
    /// transport says it answers) gets the handshake again and the request
    /// once more.
    async fn http_request(
        &self,
        integration_id: &str,
        method: &str,
        params: serde_json::Value,
        headers: &[(&str, &str)],
    ) -> Result<serde_json::Value, McpError> {
        let mut target = {
            let sessions = self.sessions.read().await;
            let session = sessions
                .get(integration_id)
                .ok_or_else(|| McpError::NotFound(format!("session {}", integration_id)))?;
            HttpTarget {
                url: session.server_url.trim_end_matches('/').to_string(),
                token: session.tokens.as_ref().map(|t| t.access_token.clone()),
                mcp_session: session.mcp_session.clone(),
                protocol: session.protocol.clone(),
            }
        };
        match self.post(integration_id, &target, method, params.clone(), headers).await {
            Err(McpError::SessionExpired(_)) => {
                info!(integration = integration_id, "MCP server forgot its session; opening a new one");
                (target.mcp_session, target.protocol) =
                    self.handshake(integration_id, &target.url, target.token.as_deref()).await?;
                if let Some(session) = self.sessions.write().await.get_mut(integration_id) {
                    session.mcp_session = target.mcp_session.clone();
                    session.protocol = target.protocol.clone();
                }
                self.post(integration_id, &target, method, params, headers).await
            }
            other => other,
        }
    }

    /// POST one JSON-RPC request; the reply's body.
    async fn post(
        &self,
        integration_id: &str,
        target: &HttpTarget,
        method: &str,
        params: serde_json::Value,
        headers: &[(&str, &str)],
    ) -> Result<serde_json::Value, McpError> {
        self.post_with_session(integration_id, target, method, params, headers).await.map(|(body, _)| body)
    }

    /// POST one JSON-RPC request; the reply's body, and the session the
    /// server issued with it (`Mcp-Session-Id`), if any. The body is raw
    /// JSON or an SSE stream (its last message is the reply); a server that
    /// streams that its tools changed gets them listed again.
    async fn post_with_session(
        &self,
        integration_id: &str,
        target: &HttpTarget,
        method: &str,
        params: serde_json::Value,
        headers: &[(&str, &str)],
    ) -> Result<(serde_json::Value, Option<String>), McpError> {
        let id = self.next_id.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut req = target
            .request(&self.http)
            .json(&json!({ "jsonrpc": "2.0", "method": method, "params": params, "id": id }));
        for (name, value) in headers {
            req = req.header(*name, *value);
        }

        let resp = req.send().await?;
        let status = resp.status();
        let issued = resp
            .headers()
            .get(SESSION_HEADER)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let resp_text = resp.text().await.unwrap_or_default();

        if !status.is_success() {
            let detail = format!(
                "{} returned {} — {}",
                method,
                status,
                resp_text.chars().take(200).collect::<String>()
            );
            return Err(if status == reqwest::StatusCode::UNAUTHORIZED {
                McpError::Unauthorized(detail)
            } else if status == reqwest::StatusCode::NOT_FOUND && target.mcp_session.is_some() {
                McpError::SessionExpired(detail)
            } else {
                McpError::Other(detail)
            });
        }

        debug!(
            url = %target.url,
            method,
            status = %status,
            response_len = resp_text.len(),
            response_preview = %::types::redact::redact(&resp_text).chars().take(500).collect::<String>(),
            "MCP raw response"
        );

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
        Ok((body, issued))
    }

    /// POST one JSON-RPC notification (no reply is read).
    async fn notify_http(&self, target: &HttpTarget, method: &str) -> Result<(), McpError> {
        let resp = target
            .request(&self.http)
            .json(&json!({ "jsonrpc": "2.0", "method": method }))
            .send()
            .await?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(McpError::Other(format!("{method} returned {}", resp.status())))
        }
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

    McpToolResult { content, is_error, result: Some(result.clone()) }
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
