use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("crypto error: {0}")]
    Crypto(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("auth error: {0}")]
    Auth(String),
    #[error("{0}")]
    Other(String),
}

/// An MCP tool definition from an external server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolDef {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "inputSchema"
    )]
    pub input_schema: Option<serde_json::Value>,
    /// The server's hints about the tool (`readOnlyHint`, `destructiveHint`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<McpToolAnnotations>,
    /// Loading and result hints the MCP ecosystem carries in `_meta`.
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Map<String, serde_json::Value>>,
}

/// An MCP tool's annotations, as the protocol names them.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolAnnotations {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only_hint: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destructive_hint: Option<bool>,
}

impl McpToolDef {
    /// The server says the tool changes nothing.
    pub fn read_only(&self) -> bool {
        self.annotations.as_ref().and_then(|a| a.read_only_hint).unwrap_or(false)
    }

    /// The server says the tool may delete or overwrite.
    pub fn destructive(&self) -> bool {
        self.annotations.as_ref().and_then(|a| a.destructive_hint).unwrap_or(false)
    }

    fn meta(&self, key: &str) -> Option<&serde_json::Value> {
        self.meta.as_ref()?.get(&format!("anthropic/{key}"))
    }

    /// `_meta`'s words for tool search, whitespace collapsed (a newline would
    /// break the one-name-per-line listing).
    pub fn search_hint(&self) -> Option<String> {
        let hint = self.meta("searchHint")?.as_str()?;
        let hint = hint.split_whitespace().collect::<Vec<_>>().join(" ");
        (!hint.is_empty()).then_some(hint)
    }

    /// `_meta`'s result size past which the result is saved to disk.
    pub fn max_result_chars(&self) -> Option<usize> {
        self.meta("maxResultSizeChars")?
            .as_f64()
            .filter(|n| n.is_finite() && *n > 0.0)
            .map(|n| n as usize)
    }
}

/// Result from calling an MCP tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolResult {
    pub content: String,
    #[serde(default)]
    pub is_error: bool,
}

/// Connection status for an MCP integration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionStatus {
    Connected,
    Disconnected,
    Error,
}

/// OAuth token pair for MCP authentication.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthTokens {
    pub access_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

/// Result of a successful OAuth token refresh.
#[derive(Debug, Clone)]
pub struct RefreshResult {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: Option<i64>,
    pub scope: Option<String>,
}

/// OAuth metadata from a well-known endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthMetadata {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registration_endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revocation_endpoint: Option<String>,
}
