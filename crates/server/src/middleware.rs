use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Instant;

use axum::extract::{ConnectInfo, Request};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Json, Response};
use tokio::sync::Mutex;

use types::api::ErrorResponse;

/// Claims extracted from a validated JWT, stored in request extensions.
#[derive(Clone, Debug)]
pub struct AuthClaims {
    pub user_id: String,
    pub email: String,
}

/// Axum middleware that validates JWT from the Authorization header.
/// On success, inserts `AuthClaims` into request extensions.
pub async fn jwt_auth(mut request: Request, next: Next) -> Response {
    // No fallback here, ever. An absent or empty secret is a server wiring
    // fault, and an empty HS256 key verifies any token an attacker signs with
    // the empty string — so refuse the request loudly instead of validating
    // against nothing.
    let secret = match request.extensions().get::<JwtSecret>() {
        Some(JwtSecret(s)) if !s.is_empty() => s.clone(),
        Some(_) => {
            tracing::error!(
                "jwt_auth: JWT secret is empty — refusing request; check auth.access_secret"
            );
            return misconfigured();
        }
        None => {
            tracing::error!(
                "jwt_auth: no JwtSecret extension in request — refusing request; the \
                 Extension layer must be listed AFTER from_fn(jwt_auth) so it is outermost"
            );
            return misconfigured();
        }
    };

    let auth_header = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok());

    let token = match auth_header {
        Some(header) => {
            let parts: Vec<&str> = header.splitn(2, ' ').collect();
            if parts.len() != 2 || !parts[0].eq_ignore_ascii_case("bearer") {
                return auth_error("invalid authorization header format");
            }
            parts[1]
        }
        None => {
            return auth_error("missing authorization header");
        }
    };

    match auth::validate_jwt_claims(token, &secret) {
        Ok(claims) => {
            request.extensions_mut().insert(AuthClaims {
                user_id: claims.sub,
                email: claims.email,
            });
            next.run(request).await
        }
        Err(_) => auth_error("invalid token"),
    }
}

/// Wrapper type for the JWT secret, stored in request extensions via a layer.
#[derive(Clone)]
pub struct JwtSecret(pub String);

/// A request that could not be authenticated because the server is wired
/// wrong. 500, not 401: the caller's credentials were never the problem, and
/// dressing a server fault as a credential failure is how this class of bug
/// hides in ordinary auth noise.
fn misconfigured() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorResponse {
            error: "authentication is not configured".to_string(),
        }),
    )
        .into_response()
}

fn auth_error(message: &str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(ErrorResponse {
            error: message.to_string(),
        }),
    )
        .into_response()
}

/// Security headers applied to all routes (no CSP — that's per-route).
/// HSTS, Permissions-Policy, X-Frame-Options, X-Content-Type-Options,
/// X-XSS-Protection, Referrer-Policy.
pub async fn security_headers(request: Request, next: Next) -> Response {
    // Frameable surfaces: the chat-embed page (app SDK iframes), run-produced
    // files (rendered in the Work panel's sandboxed iframe), and the
    // standalone /work document viewer (framed by the web Library so the
    // owner stays on the console while the bot's renderer does the formats).
    let is_embed = request.uri().path().starts_with("/chat-embed/")
        || request.uri().path().starts_with("/api/v1/files/")
        || request.uri().path().starts_with("/work/");
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        "permissions-policy",
        "accelerometer=(), camera=(self), geolocation=(), gyroscope=(), magnetometer=(), microphone=(self), payment=(), usb=()"
            .parse()
            .unwrap(),
    );
    headers.insert(
        "strict-transport-security",
        "max-age=31536000; includeSubDomains; preload"
            .parse()
            .unwrap(),
    );
    headers.insert("x-content-type-options", "nosniff".parse().unwrap());
    if !is_embed {
        headers.insert("x-frame-options", "DENY".parse().unwrap());
    }
    headers.insert("x-xss-protection", "1; mode=block".parse().unwrap());
    headers.insert(
        "referrer-policy",
        "strict-origin-when-cross-origin".parse().unwrap(),
    );
    response
}

/// Strict CSP for API routes only. Blocks all content loading since API responses
/// should never render HTML/scripts. Matches Go's APISecurityHeaders().
///
/// The one exception is the OAuth callback (`/api/v1/integrations/oauth/callback`),
/// a self-contained HTML page the browser navigates to directly. HTML responses get
/// a page-appropriate CSP that permits their inline styles/script; everything else
/// keeps the lockdown.
pub async fn api_security_headers(request: Request, next: Next) -> Response {
    // Run-produced files (/api/v1/files/) render inside the app's own Work
    // panel via a sandboxed iframe, so they must be frameable by the app
    // (localhost covers both the embedded SPA and the Vite dev server) —
    // everything else keeps frame-ancestors 'none'. The iframe carries
    // sandbox="allow-scripts" with no allow-same-origin, so framed content
    // runs with an opaque origin and cannot reach the API or storage.
    let is_served_file = request.uri().path().starts_with("/files/")
        || request.uri().path().starts_with("/api/v1/files/");
    let mut response = next.run(request).await;

    let is_html = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/html"));

    let csp = if is_served_file {
        "frame-ancestors 'self' http://localhost:* http://127.0.0.1:* tauri:"
    } else if is_html {
        // 'self' so app UIs served over HTTP (browser popups, cloud bots via the
        // tunnel) can load their own bundled scripts/styles — a SvelteKit or Vite
        // build ships chunked files, not inline blocks. neboapp:// never hits this.
        "default-src 'none'; style-src 'self' 'unsafe-inline'; script-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src *; frame-ancestors 'none'"
    } else {
        "default-src 'none'; frame-ancestors 'none'"
    };

    let headers = response.headers_mut();
    headers.insert("content-security-policy", csp.parse().unwrap());
    headers.insert(
        "cache-control",
        "no-store, no-cache, must-revalidate, private"
            .parse()
            .unwrap(),
    );
    headers.insert("pragma", "no-cache".parse().unwrap());
    response
}

/// Who may reach this server, fixed when it binds (PRD Permissions §4.8).
#[derive(Clone, Debug)]
pub struct Boundary {
    /// The port the server listens on.
    pub port: u16,
    /// Bound off loopback (`NEBO_HOST`): the network can reach it.
    pub network: bool,
    /// The install's API key (`NEBO_MCP_API_KEY`), when one is set.
    pub install_key: Option<String>,
}

impl Boundary {
    pub fn for_bind(host: &str, port: u16) -> Self {
        Self {
            port,
            network: !is_loopback_bind(host),
            install_key: install_key(),
        }
    }
}

/// Whether `NEBO_HOST` keeps the server on this machine.
pub fn is_loopback_bind(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

/// The install's API key: `NEBO_MCP_API_KEY`, when set and non-empty.
pub fn install_key() -> Option<String> {
    std::env::var("NEBO_MCP_API_KEY").ok().filter(|k| !k.is_empty())
}

/// The bearer token on a request, if it carries one.
fn bearer(headers: &axum::http::HeaderMap) -> Option<&str> {
    let (scheme, token) = headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then_some(token.trim())
}

/// A request the bot's own tunnel forwarded: the hub checked the owner
/// before the request entered the tunnel, and the tunnel stamped it with a
/// secret that exists only in this process (`comm::tunnel`).
pub(crate) fn came_through_tunnel(headers: &axum::http::HeaderMap) -> bool {
    headers
        .get("x-nebo-tunnel-auth")
        .and_then(|v| v.to_str().ok())
        == Some(comm::tunnel::tunnel_auth_secret())
}

/// Whether `Host` names this machine: a loopback name (or the desktop
/// shell's `tauri.localhost`) with this server's port or none. A page that
/// DNS-rebinds its own domain onto 127.0.0.1 still sends its own domain here.
fn host_is_local(host: &str, port: u16) -> bool {
    let (name, host_port) = match host.strip_prefix('[') {
        Some(v6) => match v6.split_once(']') {
            Some((name, rest)) => (name, rest.strip_prefix(':')),
            None => return false,
        },
        None => match host.split_once(':') {
            Some((name, p)) => (name, Some(p)),
            None => (host, None),
        },
    };
    let name = name.to_ascii_lowercase();
    matches!(name.as_str(), "localhost" | "127.0.0.1" | "::1" | "tauri.localhost")
        && host_port.is_none_or(|p| p.parse::<u16>().ok() == Some(port))
}

/// The one gate every request passes before any route (REST, WebSocket,
/// `/agent/*`, static files).
///
/// - Through the tunnel: admitted — the hub authenticated the owner, and the
///   Host is the browser's (`neboai.com`), which is why the stamp decides.
/// - From the network on a non-loopback bind: the install key is required.
///   `/health` alone is exempt — it reports only status and version, and it
///   is the path an orchestrator's liveness probe calls without credentials.
/// - From this machine: `Host` must name this machine (DNS rebinding).
pub async fn local_boundary(
    axum::extract::State(boundary): axum::extract::State<Boundary>,
    request: Request,
    next: Next,
) -> Response {
    let headers = request.headers();
    if came_through_tunnel(headers) {
        return next.run(request).await;
    }
    // No peer address means the server was not served with connect info:
    // treat the caller as the network, never as this machine.
    let from_this_machine = request
        .extensions()
        .get::<ConnectInfo<std::net::SocketAddr>>()
        .is_some_and(|ci| ci.0.ip().is_loopback());
    if boundary.network && !from_this_machine {
        if request.uri().path() == "/health" {
            return next.run(request).await;
        }
        let presented = bearer(headers).filter(|t| !t.is_empty());
        return match (boundary.install_key.as_deref(), presented) {
            (Some(key), Some(token)) if token == key => next.run(request).await,
            _ => boundary_refusal(
                StatusCode::UNAUTHORIZED,
                "this server is reachable from the network: send the install's API key \
                 (NEBO_MCP_API_KEY) as Authorization: Bearer <key>",
            ),
        };
    }
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .or_else(|| request.uri().authority().map(|a| a.as_str()));
    match host {
        Some(host) if host_is_local(host, boundary.port) => next.run(request).await,
        _ => {
            tracing::warn!(host = ?host, path = %request.uri().path(), "refused a request for a foreign host");
            boundary_refusal(StatusCode::FORBIDDEN, "host not allowed")
        }
    }
}

fn boundary_refusal(status: StatusCode, message: &str) -> Response {
    (
        status,
        Json(ErrorResponse {
            error: message.to_string(),
        }),
    )
        .into_response()
}

/// What the `/agent/mcp` key check needs, fixed at startup.
#[derive(Clone)]
pub struct McpAuth {
    /// The install's API key (`NEBO_MCP_API_KEY`), when one is set.
    pub install_key: Option<String>,
    /// Live per-run credentials (see `agent::tool_credentials`).
    pub credentials: agent::ToolCredentials,
}

/// Opt-in API key auth for the MCP endpoint.
/// If `NEBO_MCP_API_KEY` is set, requires `Authorization: Bearer <key>` — or
/// a live run credential (`X-Nebo-Run-Credential`): a CLI provider's tool
/// calls carry their run's credential, never the install key.
/// If not set, the endpoint is open (localhost-only use case).
pub async fn mcp_api_key_auth(
    axum::extract::State(auth): axum::extract::State<McpAuth>,
    request: Request,
    next: Next,
) -> Response {
    // No key configured → skip auth (zero-config localhost mode)
    let Some(expected) = auth.install_key else {
        return next.run(request).await;
    };

    let run_credential = request
        .headers()
        .get(agent::tool_credentials::HEADER)
        .and_then(|v| v.to_str().ok());
    if run_credential.is_some_and(|t| auth.credentials.grant(t).is_some()) {
        return next.run(request).await;
    }

    let auth_header = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok());

    let token = match auth_header {
        Some(header) => {
            let parts: Vec<&str> = header.splitn(2, ' ').collect();
            if parts.len() != 2 || !parts[0].eq_ignore_ascii_case("bearer") {
                return mcp_auth_error("invalid authorization header format");
            }
            parts[1]
        }
        None => {
            return mcp_auth_error("MCP API key required (set NEBO_MCP_API_KEY)");
        }
    };

    if token != expected {
        return mcp_auth_error("invalid MCP API key");
    }

    next.run(request).await
}

fn mcp_auth_error(message: &str) -> Response {
    // Return JSON-RPC error for MCP clients
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": null,
        "error": {
            "code": -32000,
            "message": message,
        }
    });
    (StatusCode::UNAUTHORIZED, Json(body)).into_response()
}

/// In-memory rate limiter state.
#[derive(Clone)]
pub struct RateLimiter {
    buckets: Arc<Mutex<HashMap<IpAddr, (u32, Instant)>>>,
    max_requests: u32,
    window: std::time::Duration,
}

impl RateLimiter {
    pub fn new(max_requests: u32, window: std::time::Duration) -> Self {
        Self {
            buckets: Arc::new(Mutex::new(HashMap::new())),
            max_requests,
            window,
        }
    }
}

/// Rate limiting middleware for auth routes.
/// Uses ConnectInfo (RemoteAddr) only — intentionally ignores X-Forwarded-For
/// because it is trivially spoofable by any client. Matches Go's DefaultKeyFunc.
pub async fn rate_limit(request: Request, next: Next) -> Response {
    // Same rule as `jwt_auth`: this middleware is only ever installed together
    // with its `RateLimiter` extension, so a missing one means the layers are
    // wired wrong. Letting the request through unlimited would turn that
    // mistake into a silently unprotected login door.
    let limiter = match request.extensions().get::<RateLimiter>().cloned() {
        Some(l) => l,
        None => {
            tracing::error!(
                "rate_limit: no RateLimiter extension in request — refusing request; the \
                 Extension layer must be listed AFTER from_fn(rate_limit) so it is outermost"
            );
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "rate limiting is not configured".to_string(),
                }),
            )
                .into_response();
        }
    };

    // Extract client IP from peer address only (RemoteAddr).
    // X-Forwarded-For is intentionally ignored — it is trivially spoofable.
    // For deployments behind a trusted reverse proxy, add a TrustedProxy variant.
    let ip = request
        .extensions()
        .get::<ConnectInfo<std::net::SocketAddr>>()
        .map(|ci| ci.0.ip())
        .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));

    let now = Instant::now();
    let mut buckets = limiter.buckets.lock().await;
    let entry = buckets.entry(ip).or_insert((0, now));

    // Reset window if expired
    if now.duration_since(entry.1) >= limiter.window {
        *entry = (0, now);
    }

    entry.0 += 1;
    if entry.0 > limiter.max_requests {
        drop(buckets);
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(ErrorResponse {
                error: "rate limit exceeded, try again later".to_string(),
            }),
        )
            .into_response();
    }
    drop(buckets);

    next.run(request).await
}

#[cfg(test)]
mod boundary_tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use std::net::SocketAddr;
    use tower::ServiceExt;

    fn app(boundary: Boundary) -> Router {
        Router::new()
            .route("/api/v1/agents", axum::routing::get(|| async { "ok" }))
            .route("/health", axum::routing::get(|| async { "ok" }))
            .fallback(|| async { "spa" })
            .layer(axum::middleware::from_fn_with_state(boundary, local_boundary))
    }

    fn loopback_bind() -> Boundary {
        Boundary { port: 27895, network: false, install_key: None }
    }

    fn network_bind(key: Option<&str>) -> Boundary {
        Boundary { port: 27895, network: true, install_key: key.map(str::to_string) }
    }

    async fn status(
        boundary: Boundary,
        path: &str,
        peer: &str,
        headers: &[(&str, &str)],
    ) -> StatusCode {
        let mut req = HttpRequest::builder().uri(path);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let mut req = req.body(Body::empty()).unwrap();
        let peer: SocketAddr = peer.parse().unwrap();
        req.extensions_mut().insert(ConnectInfo(peer));
        app(boundary).oneshot(req).await.unwrap().status()
    }

    const LOCAL: &str = "127.0.0.1:50000";
    const LAN: &str = "192.168.1.20:50000";

    // DNS rebinding: a page on attacker.example re-points its own name at
    // 127.0.0.1 and calls the API as same-origin. The browser sends the
    // attacker's name as Host; nothing else about the request is unusual.
    #[tokio::test]
    async fn a_foreign_host_is_refused() {
        for path in ["/api/v1/agents", "/health", "/", "/ws"] {
            assert_eq!(
                status(loopback_bind(), path, LOCAL, &[("host", "attacker.example:27895")]).await,
                StatusCode::FORBIDDEN,
                "{path}"
            );
        }
        assert_eq!(
            status(loopback_bind(), "/api/v1/agents", LOCAL, &[("host", "127.0.0.1.attacker.example")]).await,
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn loopback_and_app_hosts_pass() {
        for host in [
            "localhost:27895",
            "127.0.0.1:27895",
            "[::1]:27895",
            "LOCALHOST:27895",
            // The desktop shell's raw reconnect ping sends no port.
            "localhost",
            "tauri.localhost",
        ] {
            assert_eq!(
                status(loopback_bind(), "/api/v1/agents", LOCAL, &[("host", host)]).await,
                StatusCode::OK,
                "{host}"
            );
        }
    }

    #[tokio::test]
    async fn a_loopback_name_on_another_port_is_refused() {
        assert_eq!(
            status(loopback_bind(), "/api/v1/agents", LOCAL, &[("host", "localhost:8080")]).await,
            StatusCode::FORBIDDEN
        );
    }

    // The hub tunnel preserves the browser's Host (`neboai.com`, both on the
    // pod holding the tunnel and on a peer hop), and the bot's tunnel stamps
    // every request it forwards with a per-boot secret. The stamp, not the
    // name, is what says "this came through the tunnel".
    #[tokio::test]
    async fn tunnel_requests_pass_with_the_browsers_host() {
        let stamp = comm::tunnel::tunnel_auth_secret();
        for host in ["neboai.com", "www.neboai.com", "localhost:5174"] {
            assert_eq!(
                status(
                    loopback_bind(),
                    "/api/v1/agents",
                    LOCAL,
                    &[("host", host), ("x-nebo-tunnel-auth", stamp)]
                )
                .await,
                StatusCode::OK,
                "{host}"
            );
        }
        assert_eq!(
            status(
                loopback_bind(),
                "/api/v1/agents",
                LOCAL,
                &[("host", "neboai.com"), ("x-nebo-tunnel-auth", "forged")]
            )
            .await,
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn a_network_bind_requires_the_install_key() {
        let b = || network_bind(Some("k-123"));
        for path in ["/api/v1/agents", "/", "/ws", "/agent/mcp"] {
            assert_eq!(
                status(b(), path, LAN, &[("host", "192.168.1.5:27895")]).await,
                StatusCode::UNAUTHORIZED,
                "{path}"
            );
            assert_eq!(
                status(b(), path, LAN, &[("host", "192.168.1.5:27895"), ("authorization", "Bearer wrong")]).await,
                StatusCode::UNAUTHORIZED,
                "{path}"
            );
        }
        assert_eq!(
            status(b(), "/api/v1/agents", LAN, &[("host", "192.168.1.5:27895"), ("authorization", "Bearer k-123")]).await,
            StatusCode::OK
        );
        assert_eq!(
            status(b(), "/api/v1/agents", LAN, &[("host", "nebo.example.com"), ("authorization", "bearer k-123")]).await,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn a_network_bind_without_a_key_is_never_open() {
        assert_eq!(
            status(network_bind(None), "/api/v1/agents", LAN, &[("host", "192.168.1.5:27895")]).await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            status(network_bind(None), "/api/v1/agents", LAN, &[("host", "192.168.1.5:27895"), ("authorization", "Bearer ")]).await,
            StatusCode::UNAUTHORIZED
        );
    }

    // A cloud bot binds 0.0.0.0 and is reached only through the tunnel; its
    // own process and sidecars call it on loopback, and the orchestrator's
    // liveness probe calls /health from the node. None of those carry a key.
    #[tokio::test]
    async fn a_network_bind_keeps_the_tunnel_local_callers_and_health_probe() {
        let stamp = comm::tunnel::tunnel_auth_secret();
        assert_eq!(
            status(network_bind(None), "/api/v1/agents", LOCAL, &[("host", "neboai.com"), ("x-nebo-tunnel-auth", stamp)]).await,
            StatusCode::OK
        );
        assert_eq!(
            status(network_bind(None), "/api/v1/agents", LOCAL, &[("host", "127.0.0.1:27895")]).await,
            StatusCode::OK
        );
        assert_eq!(
            status(network_bind(None), "/health", "10.244.1.1:40000", &[("host", "10.244.1.7:27895")]).await,
            StatusCode::OK
        );
        // Same-machine callers are still held to the Host check.
        assert_eq!(
            status(network_bind(None), "/api/v1/agents", LOCAL, &[("host", "attacker.example:27895")]).await,
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn a_loopback_bind_asks_for_no_key() {
        assert_eq!(
            status(Boundary { install_key: Some("k".into()), ..loopback_bind() }, "/api/v1/agents", LOCAL, &[("host", "localhost:27895")]).await,
            StatusCode::OK
        );
    }
}

#[cfg(test)]
mod mcp_auth_tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use tower::ServiceExt;

    fn app(auth: McpAuth) -> Router {
        Router::new().route(
            "/agent/mcp",
            axum::routing::post(|| async { "ok" })
                .layer(axum::middleware::from_fn_with_state(auth, mcp_api_key_auth)),
        )
    }

    async fn status(auth: McpAuth, headers: &[(&str, &str)]) -> StatusCode {
        let mut req = HttpRequest::builder().method("POST").uri("/agent/mcp");
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        app(auth).oneshot(req.body(Body::empty()).unwrap()).await.unwrap().status()
    }

    fn keyed(credentials: &agent::ToolCredentials) -> McpAuth {
        McpAuth { install_key: Some("k-123".into()), credentials: credentials.clone() }
    }

    fn grant() -> agent::RunGrant {
        agent::RunGrant {
            ctx: Default::default(),
            agent_id: "emp-1".into(),
        }
    }

    // A CLI provider's tool calls carry their run's credential, never the
    // install key: with a key set, the credential is what admits them.
    #[tokio::test]
    async fn a_live_run_credential_satisfies_the_key() {
        let credentials = agent::ToolCredentials::default();
        let guard = credentials.issue(grant());
        assert_eq!(
            status(keyed(&credentials), &[("x-nebo-run-credential", guard.token())]).await,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn an_ended_or_unknown_credential_does_not() {
        let credentials = agent::ToolCredentials::default();
        let token = credentials.issue(grant()).token().to_string(); // guard dropped: revoked
        assert_eq!(
            status(keyed(&credentials), &[("x-nebo-run-credential", &token)]).await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            status(keyed(&credentials), &[("x-nebo-run-credential", "made-up")]).await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(status(keyed(&credentials), &[]).await, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn the_install_key_still_works_and_no_key_means_open() {
        let credentials = agent::ToolCredentials::default();
        assert_eq!(
            status(keyed(&credentials), &[("authorization", "Bearer k-123")]).await,
            StatusCode::OK
        );
        assert_eq!(
            status(keyed(&credentials), &[("authorization", "Bearer nope")]).await,
            StatusCode::UNAUTHORIZED
        );
        let open = McpAuth { install_key: None, credentials };
        assert_eq!(status(open, &[]).await, StatusCode::OK);
    }
}
