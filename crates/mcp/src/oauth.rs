//! OAuth for remote MCP servers: discovery (RFC 9728 protected-resource
//! metadata, then RFC 8414 / OIDC authorization-server metadata) and the ONE
//! token request (code exchange and refresh), with its failures classified.
//!
//! Classification follows Claude Code's MCP auth provider: only a definitive
//! refusal of the grant ends a sign-in (`McpError::TokenRejected`); 5xx, 429,
//! timeouts and network errors are transient and never cost the owner a
//! sign-in.

use std::sync::Arc;
use std::time::Duration;

use crate::{McpClient, McpError, OAuthDiscovery, OAuthMetadata, TokenSet};

/// Each token or discovery request gets this long.
const OAUTH_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// `expires_in` is clamped to this range; a server that omits it is taken to
/// mean one hour.
const MIN_EXPIRES_IN: i64 = 60;
const MAX_EXPIRES_IN: i64 = 24 * 60 * 60;
const DEFAULT_EXPIRES_IN: i64 = 60 * 60;

/// OAuth error codes that mean the grant itself is dead: the refresh token
/// is revoked, expired or already spent, or the client is gone.
const TERMINAL_ERRORS: &[&str] = &["invalid_grant", "invalid_client", "unauthorized_client"];

/// Non-standard codes some servers use where RFC 6749 says `invalid_grant`
/// (Slack's token rotation, observed by Claude Code).
const INVALID_GRANT_ALIASES: &[&str] =
    &["invalid_refresh_token", "expired_refresh_token", "token_expired"];

/// What a token request asks for.
pub enum TokenGrant<'a> {
    AuthorizationCode {
        code: &'a str,
        code_verifier: &'a str,
        redirect_uri: &'a str,
    },
    RefreshToken(&'a str),
}

/// The client a token request is made as.
pub struct TokenClient<'a> {
    pub token_endpoint: &'a str,
    pub client_id: &'a str,
    pub client_secret: Option<&'a str>,
    /// RFC 8707 resource indicator, when discovery found one.
    pub resource: Option<&'a str>,
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// When a token issued now with this `expires_in` expires.
pub fn token_expiry(expires_in: Option<i64>) -> i64 {
    now_secs() + expires_in.unwrap_or(DEFAULT_EXPIRES_IN).clamp(MIN_EXPIRES_IN, MAX_EXPIRES_IN)
}

/// Classify a token endpoint's error answer. Terminal codes (and a 401, the
/// endpoint refusing the client) end the sign-in; everything else is
/// transient.
fn token_error(status: u16, body: &str) -> McpError {
    let code = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string));
    let snippet: String = body.chars().take(200).collect();
    let terminal = status == 401
        || code.as_deref().is_some_and(|c| {
            TERMINAL_ERRORS.contains(&c) || INVALID_GRANT_ALIASES.contains(&c)
        });
    if terminal {
        McpError::TokenRejected(format!("token endpoint returned {status}: {snippet}"))
    } else {
        McpError::Auth(format!("token endpoint returned {status}: {snippet}"))
    }
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

/// RFC 9728 protected-resource metadata (the fields we use).
#[derive(serde::Deserialize)]
struct ProtectedResource {
    #[serde(default)]
    resource: Option<String>,
    #[serde(default)]
    authorization_servers: Vec<String>,
    #[serde(default)]
    scopes_supported: Option<Vec<String>>,
}

/// Split a URL into origin and path (path without a trailing slash; empty
/// for the root).
fn split_url(url: &str) -> (String, String) {
    let trimmed = url.trim_end_matches('/');
    let Some(pos) = trimmed.find("://") else {
        return (trimmed.to_string(), String::new());
    };
    let after = &trimmed[pos + 3..];
    match after.find('/') {
        Some(slash) => (
            trimmed[..pos + 3 + slash].to_string(),
            trimmed[pos + 3 + slash..].to_string(),
        ),
        None => (trimmed.to_string(), String::new()),
    }
}

/// Whether a protected-resource `resource` may be used for `server_url`:
/// same origin, and the server's path sits under the resource's path.
fn resource_covers(resource: &str, server_url: &str) -> bool {
    let (r_origin, r_path) = split_url(resource);
    let (s_origin, s_path) = split_url(server_url);
    r_origin.eq_ignore_ascii_case(&s_origin)
        && (r_path.is_empty() || s_path == r_path || s_path.starts_with(&format!("{r_path}/")))
}

impl McpClient {
    /// The per-integration refresh lock: every refresh of one integration's
    /// token in this process runs under it, one at a time.
    pub fn refresh_lock(&self, integration_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self
            .refresh_locks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        locks.entry(integration_id.to_string()).or_default().clone()
    }

    async fn get_json<T: serde::de::DeserializeOwned>(&self, url: &str) -> Option<T> {
        let resp = self
            .http
            .get(url)
            .timeout(OAUTH_REQUEST_TIMEOUT)
            .send()
            .await
            .ok()?;
        if !resp.status().is_success() {
            return None;
        }
        resp.json().await.ok()
    }

    /// RFC 9728: the server's protected-resource metadata, path-aware first.
    async fn protected_resource(&self, server_url: &str) -> Option<ProtectedResource> {
        let (origin, path) = split_url(server_url);
        if !path.is_empty()
            && let Some(prm) = self
                .get_json(&format!("{origin}/.well-known/oauth-protected-resource{path}"))
                .await
        {
            return Some(prm);
        }
        self.get_json(&format!("{origin}/.well-known/oauth-protected-resource"))
            .await
    }

    /// RFC 8414 / OIDC metadata for an authorization server, trying the
    /// well-known locations in the MCP SDK's order.
    async fn authorization_server(&self, issuer: &str) -> Result<OAuthMetadata, McpError> {
        let (origin, path) = split_url(issuer);
        let mut urls = Vec::new();
        if path.is_empty() {
            urls.push(format!("{origin}/.well-known/oauth-authorization-server"));
            urls.push(format!("{origin}/.well-known/openid-configuration"));
        } else {
            urls.push(format!("{origin}/.well-known/oauth-authorization-server{path}"));
            urls.push(format!("{origin}/.well-known/openid-configuration{path}"));
            urls.push(format!("{origin}{path}/.well-known/openid-configuration"));
        }
        let mut last = None;
        for url in &urls {
            let resp = match self.http.get(url).timeout(OAUTH_REQUEST_TIMEOUT).send().await {
                Ok(r) => r,
                Err(e) => {
                    last = Some(McpError::Http(e));
                    continue;
                }
            };
            if !resp.status().is_success() {
                last = Some(McpError::Auth(format!("OAuth discovery returned {}", resp.status())));
                continue;
            }
            return Ok(resp.json().await?);
        }
        Err(last.unwrap_or_else(|| McpError::Auth("OAuth discovery returned nothing".into())))
    }

    /// Discover how an MCP server signs in. Protected-resource metadata
    /// (RFC 9728) names the authorization server, the resource and the
    /// scopes; without it, the server's own origin is the authorization
    /// server (the pre-2025-06 MCP layout).
    pub async fn discover_oauth(&self, server_url: &str) -> Result<OAuthDiscovery, McpError> {
        let prm = self.protected_resource(server_url).await;
        let issuer = prm
            .as_ref()
            .and_then(|p| p.authorization_servers.first().cloned())
            .unwrap_or_else(|| split_url(server_url).0);
        let metadata = self.authorization_server(&issuer).await?;

        let resource = prm.as_ref().map(|p| match p.resource.as_deref() {
            Some(r) if resource_covers(r, server_url) => r.to_string(),
            _ => server_url.to_string(),
        });
        let scope = prm
            .as_ref()
            .and_then(|p| p.scopes_supported.as_ref())
            .filter(|s| !s.is_empty())
            .map(|scopes| {
                let mut scopes = scopes.clone();
                let offline = metadata
                    .scopes_supported
                    .as_ref()
                    .is_some_and(|s| s.iter().any(|x| x == "offline_access"));
                if offline && !scopes.iter().any(|x| x == "offline_access") {
                    scopes.push("offline_access".to_string());
                }
                scopes.join(" ")
            });
        Ok(OAuthDiscovery {
            metadata,
            resource,
            scope,
        })
    }

    /// The ONE token request (RFC 6749 §4.1.3 code exchange, §6 refresh).
    /// Takes plaintext values; the caller persists what comes back.
    pub async fn request_token(
        &self,
        client: &TokenClient<'_>,
        grant: TokenGrant<'_>,
    ) -> Result<TokenSet, McpError> {
        let mut params: Vec<(&str, &str)> = match &grant {
            TokenGrant::AuthorizationCode {
                code,
                code_verifier,
                redirect_uri,
            } => vec![
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", redirect_uri),
                ("code_verifier", code_verifier),
            ],
            TokenGrant::RefreshToken(rt) => {
                vec![("grant_type", "refresh_token"), ("refresh_token", rt)]
            }
        };
        params.push(("client_id", client.client_id));
        if let Some(secret) = client.client_secret {
            params.push(("client_secret", secret));
        }
        if let Some(resource) = client.resource {
            params.push(("resource", resource));
        }

        let resp = self
            .http
            .post(client.token_endpoint)
            .header("Accept", "application/json")
            .form(&params)
            .timeout(OAUTH_REQUEST_TIMEOUT)
            .send()
            .await?;
        let status = resp.status().as_u16();
        let body = resp.text().await?;
        if !(200..300).contains(&status) {
            return Err(token_error(status, &body));
        }
        // Some servers answer 200 with an error body: classify it the same.
        let t: TokenResponse = match serde_json::from_str(&body) {
            Ok(t) => t,
            Err(e) => {
                let has_error = serde_json::from_str::<serde_json::Value>(&body)
                    .ok()
                    .is_some_and(|v| v.get("error").is_some());
                return Err(if has_error {
                    token_error(400, &body)
                } else {
                    McpError::Auth(format!("decode token response: {e}"))
                });
            }
        };
        Ok(TokenSet {
            access_token: t.access_token,
            refresh_token: t.refresh_token.filter(|rt| !rt.is_empty()),
            expires_at: token_expiry(t.expires_in),
            scope: t.scope,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_codes_end_the_sign_in() {
        for body in [
            r#"{"error":"invalid_grant","error_description":"Invalid refresh token"}"#,
            r#"{"error":"invalid_client"}"#,
            r#"{"error":"unauthorized_client"}"#,
            r#"{"error":"invalid_refresh_token"}"#,
        ] {
            assert!(matches!(token_error(400, body), McpError::TokenRejected(_)), "{body}");
        }
        assert!(matches!(token_error(401, ""), McpError::TokenRejected(_)));
    }

    #[test]
    fn server_trouble_is_transient() {
        assert!(matches!(token_error(502, ""), McpError::Auth(_)));
        assert!(matches!(token_error(503, "<html>"), McpError::Auth(_)));
        assert!(matches!(token_error(429, r#"{"error":"slow_down"}"#), McpError::Auth(_)));
        assert!(matches!(
            token_error(400, r#"{"error":"temporarily_unavailable"}"#),
            McpError::Auth(_)
        ));
    }

    #[test]
    fn expiry_is_clamped_and_never_missing() {
        let now = now_secs();
        assert!((token_expiry(None) - now - DEFAULT_EXPIRES_IN).abs() <= 1);
        assert!((token_expiry(Some(5)) - now - MIN_EXPIRES_IN).abs() <= 1);
        assert!((token_expiry(Some(30 * 86400)) - now - MAX_EXPIRES_IN).abs() <= 1);
        assert!((token_expiry(Some(3600)) - now - 3600).abs() <= 1);
    }

    #[test]
    fn resource_must_cover_the_server() {
        assert!(resource_covers("https://a.example/mcp", "https://a.example/mcp"));
        assert!(resource_covers("https://a.example", "https://a.example/mcp"));
        assert!(!resource_covers("https://b.example/mcp", "https://a.example/mcp"));
        assert!(!resource_covers("https://a.example/mc", "https://a.example/mcp"));
    }
}
