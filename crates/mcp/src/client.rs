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
    pub async fn discover_oauth(&self, server_url: &str) -> Result<OAuthMetadata, McpError> {
        let well_known = format!(
            "{}/.well-known/oauth-authorization-server",
            server_url.trim_end_matches('/')
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

    /// List tools from an external MCP server.
    pub async fn list_tools(
        &self,
        integration_id: &str,
        server_url: &str,
        access_token: Option<&str>,
    ) -> Result<Vec<McpToolDef>, McpError> {
        let url = format!("{}/tools/list", server_url.trim_end_matches('/'));

        let mut req = self.http.post(&url).json(&json!({"method": "tools/list"}));
        if let Some(token) = access_token {
            req = req.bearer_auth(token);
        }

        let resp = req.send().await?;
        if !resp.status().is_success() {
            return Err(McpError::Other(format!(
                "list tools returned {}",
                resp.status()
            )));
        }

        #[derive(serde::Deserialize)]
        struct ListResult {
            #[serde(default)]
            tools: Vec<McpToolDef>,
        }

        let result: ListResult = resp.json().await?;

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
                _tools: result.tools.clone(),
            },
        );

        info!(
            integration = integration_id,
            tools = result.tools.len(),
            "listed MCP tools"
        );
        Ok(result.tools)
    }

    /// Call a tool on an external MCP server.
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

        let url = format!("{}/tools/call", session.server_url.trim_end_matches('/'));

        let mut req = self.http.post(&url).json(&json!({
            "method": "tools/call",
            "params": {
                "name": tool_name,
                "arguments": input,
            }
        }));

        if let Some(ref tokens) = session.tokens {
            req = req.bearer_auth(&tokens.access_token);
        }

        let resp = req.send().await?;
        if !resp.status().is_success() {
            return Err(McpError::Other(format!(
                "call tool {} returned {}",
                tool_name,
                resp.status()
            )));
        }

        #[derive(serde::Deserialize)]
        struct CallResult {
            #[serde(default)]
            content: Vec<ContentBlock>,
            #[serde(default)]
            is_error: bool,
        }

        #[derive(serde::Deserialize)]
        struct ContentBlock {
            #[serde(rename = "type", default)]
            block_type: String,
            #[serde(default)]
            text: String,
        }

        let result: CallResult = resp.json().await?;

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
