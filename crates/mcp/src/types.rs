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
    /// The token endpoint definitively refused the grant (`invalid_grant`,
    /// `invalid_client`, `unauthorized_client`, or a 401): the stored sign-in
    /// is dead and only the owner signing in again revives it. Every other
    /// token-endpoint failure (5xx, 429, timeouts, network) is transient.
    #[error("sign-in rejected: {0}")]
    TokenRejected(String),
    /// The MCP server answered 401: it refused the bearer token.
    #[error("unauthorized: {0}")]
    Unauthorized(String),
    #[error("{0}")]
    Other(String),
}

/// An MCP tool definition from an external server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolDef {
    pub name: String,
    /// The server's own name for the tool in plain words ("Agent
    /// Management"), when it gives one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
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
    /// The older place a server puts the tool's plain-words name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only_hint: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destructive_hint: Option<bool>,
}

impl McpToolDef {
    /// The tool's plain-words name, when the server gives one: `title`, else
    /// `annotations.title`.
    pub fn display_title(&self) -> Option<String> {
        self.title
            .as_deref()
            .or_else(|| self.annotations.as_ref()?.title.as_deref())
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
    }

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

/// Tokens a token endpoint issued (code exchange or refresh). `expires_at` is
/// always set: `expires_in` is clamped to 60 s..24 h, and a server that omits
/// it is taken to mean one hour, so no stored token is ever "valid forever".
#[derive(Debug, Clone)]
pub struct TokenSet {
    pub access_token: String,
    /// A rotated refresh token, when the server issued one. `None` means the
    /// server kept the old one valid; the caller keeps it.
    pub refresh_token: Option<String>,
    pub expires_at: i64,
    pub scope: Option<String>,
}

/// Where and how a server signs in, from discovery.
#[derive(Debug, Clone)]
pub struct OAuthDiscovery {
    pub metadata: OAuthMetadata,
    /// RFC 8707 resource indicator, sent on authorize, code exchange and
    /// refresh. Set only when the server publishes RFC 9728
    /// protected-resource metadata, as the MCP SDK does.
    pub resource: Option<String>,
    /// The scope to request: the server's advertised scopes (RFC 9728
    /// `scopes_supported`), plus `offline_access` when its authorization
    /// server offers it. `None` when the server advertises none.
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scopes_supported: Option<Vec<String>>,
}

#[cfg(test)]
mod title_tests {
    use super::McpToolDef;

    #[test]
    fn a_tools_title_is_its_own_then_its_annotations() {
        let own: McpToolDef = serde_json::from_value(serde_json::json!({
            "name": "agent", "title": "Agent Management", "annotations": {"title": "Agents"}
        }))
        .unwrap();
        assert_eq!(own.display_title().as_deref(), Some("Agent Management"));
        let annotated: McpToolDef =
            serde_json::from_value(serde_json::json!({"name": "agent", "annotations": {"title": "Agents"}})).unwrap();
        assert_eq!(annotated.display_title().as_deref(), Some("Agents"));
        let none: McpToolDef = serde_json::from_value(serde_json::json!({"name": "agent", "title": "  "})).unwrap();
        assert_eq!(none.display_title(), None);
    }
}
