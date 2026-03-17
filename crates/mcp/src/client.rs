use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio::sync::RwLock;
use tracing::info;

use crate::crypto::Encryptor;
use crate::{McpError, McpToolDef, McpToolResult, OAuthMetadata, OAuthTokens};

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
            .filter(|c| c.block_type == "text" && !c.text.is_empty())
            .map(|c| c.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");

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
}

/// Parse a JSON-RPC response from an SSE (Server-Sent Events) body.
/// SSE format: `event: message\ndata: {"jsonrpc":"2.0",...}\n\n`
/// Extracts and returns the JSON from the last `data:` line.
fn parse_sse_json(text: &str) -> Result<serde_json::Value, McpError> {
    let mut last_data = None;
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(data) = trimmed.strip_prefix("data:") {
            let json_str = data.trim();
            if !json_str.is_empty() {
                last_data = Some(json_str.to_string());
            }
        }
    }
    match last_data {
        Some(json_str) => serde_json::from_str(&json_str)
            .map_err(|e| McpError::Other(format!("SSE data is not valid JSON: {e}"))),
        None => Err(McpError::Other(
            "No data found in SSE response".to_string(),
        )),
    }
}
