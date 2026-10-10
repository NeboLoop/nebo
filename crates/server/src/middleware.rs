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

/// The Permissions-Policy every response carries unless its handler set one.
pub const PERMISSIONS_POLICY: &str =
    "accelerometer=(), camera=(self), geolocation=(), gyroscope=(), magnetometer=(), microphone=(self), payment=(), usb=()";

/// The same policy for an app page whose manifest declares `device:motion`:
/// the page may read the accelerometer and gyroscope (a tilt-to-steer game).
pub const PERMISSIONS_POLICY_WITH_MOTION: &str =
    "accelerometer=(self), camera=(self), geolocation=(), gyroscope=(self), magnetometer=(), microphone=(self), payment=(), usb=()";

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
    // A handler that set its own (an app page that declares
    // `device:motion`) keeps it; everything else gets the default.
    if !headers.contains_key("permissions-policy") {
        headers.insert("permissions-policy", axum::http::HeaderValue::from_static(PERMISSIONS_POLICY));
    }
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

/// Who may reach this server and how each caller proves itself, fixed when
/// it binds (PRD Permissions §4.8; `local_boundary`).
#[derive(Clone)]
pub struct Boundary {
    /// The port the server listens on.
    pub port: u16,
    /// Bound off loopback (`NEBO_HOST`): the network can reach it.
    pub network: bool,
    /// The install key (`config::ensure_install_key`): the owner's own
    /// clients send it. `None` only when Nebo's folder can't be written, and
    /// then no key opens anything.
    pub install_key: Option<String>,
    /// The browser session the install key signs in (`local_access`).
    pub session: Option<String>,
    /// Live per-run credentials (`agent::tool_credentials`): a CLI
    /// provider's tool calls to `/agent/mcp` carry their run's.
    pub credentials: agent::ToolCredentials,
    /// The running apps: a sidecar's `NEBO_APP_TOKEN` reaches its own app's
    /// routes.
    pub apps: Apps,
}

/// The running apps, by id (`AppState::app_lifecycles`).
pub type Apps = Arc<tokio::sync::RwLock<HashMap<String, Arc<crate::app_lifecycle::AppLifecycle>>>>;

impl Boundary {
    pub fn for_bind(host: &str, port: u16, credentials: agent::ToolCredentials, apps: Apps) -> Self {
        let install_key = config::ensure_install_key()
            .map_err(|e| tracing::error!(error = %e, "the install key could not be read or made: no client can prove itself"))
            .ok();
        Self {
            port,
            network: !is_loopback_bind(host),
            session: install_key.as_deref().map(crate::local_access::session_for),
            install_key,
            credentials,
            apps,
        }
    }
}

/// Whether `NEBO_HOST` keeps the server on this machine.
pub fn is_loopback_bind(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

/// The bearer token on a request, if it carries one.
pub(crate) fn bearer(headers: &axum::http::HeaderMap) -> Option<&str> {
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

/// A credential a caller carried as its path's first segment,
/// `/k/<credential>/…`, taken off the path by `path_credential`.
#[derive(Clone, Debug)]
pub struct PathCredential(pub String);

/// Takes a credential carried as the path's first segment (`/k/<credential>`)
/// off the path, before routing, into the request's `PathCredential`. A
/// process that knows Nebo only as a base URL joins its paths onto it:
/// the plugins (`NEBO_LOCAL_URL`, `napp::plugin::plugin_base_env`), a
/// harness fixture (`NEBO_TEST_SERVER`). The route, and the request log,
/// see the path without it. It wraps the whole router: a layer on the
/// router runs after routing, too late to change the path.
pub fn path_credential(mut request: Request) -> Request {
    let Some(rest) = request.uri().path().strip_prefix("/k/") else {
        return request;
    };
    let (credential, tail) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    if credential.is_empty() {
        return request;
    }
    let credential = credential.to_string();
    let path_and_query = match request.uri().query() {
        Some(q) => format!("{tail}?{q}"),
        None => tail.to_string(),
    };
    let mut parts = request.uri().clone().into_parts();
    parts.path_and_query = path_and_query.parse().ok();
    if let Ok(uri) = axum::http::Uri::from_parts(parts) {
        *request.uri_mut() = uri;
        // An app's own routes check their caller themselves (`handlers::apps`),
        // from the headers: a page Nebo's headless browser opened with an
        // app-view pass (`napp::app_view`) shows it there too.
        if napp::app_view::app_of_path(request.uri().path()).is_some()
            && let Ok(value) = axum::http::HeaderValue::from_str(&credential)
        {
            request.headers_mut().insert(APP_VIEW_PASS, value);
        }
        request.extensions_mut().insert(PathCredential(credential));
    }
    request
}

/// The header an app's route reads a path credential from
/// ([`app_view_admits`]).
const APP_VIEW_PASS: &str = "x-nebo-app-view";

/// Whether the request carries a live app-view pass for `app_id`
/// (`napp::app_view`): the page and routes of the one app Nebo's headless
/// browser opened. Only a live pass for that app counts, so the header is
/// worth no more than the pass it carries.
pub(crate) fn app_view_admits(headers: &axum::http::HeaderMap, app_id: &str) -> bool {
    headers
        .get(APP_VIEW_PASS)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|pass| napp::app_view::admits(pass, app_id))
}

/// Nebo's own UI origins: the app as this server serves it, and the Vite
/// dev (5173) and preview (4173) servers that proxy to it. CORS admits them
/// (`cors_layer`).
pub(crate) const UI_ORIGINS: &[&str] = &[
    "http://localhost:27895",
    "http://127.0.0.1:27895",
    "http://localhost:5173",
    "http://127.0.0.1:5173",
    "http://localhost:4173",
    "http://127.0.0.1:4173",
];

/// Routes a caller reaches with no proof: each proves its caller itself, or
/// has nothing to protect.
fn proves_itself(method: &axum::http::Method, path: &str) -> bool {
    // A file in the workspace, read: the folder an employee works in, and
    // its commands read it anyway. The Work panel renders a document in a
    // sandboxed frame whose origin is opaque, so the images and styles it
    // loads from beside it carry no session.
    if matches!(*method, axum::http::Method::GET | axum::http::Method::HEAD) && path.starts_with("/api/v1/files/") {
        return true;
    }
    matches!(
        path,
        // Status and version only: a liveness probe calls it bare.
        "/health"
        // The sign-in ticket is the proof (`local_access`).
        | "/api/v1/local-session"
        // A browser coming back from a sign-in elsewhere: the pending
        // flow's state is the proof, and it is the owner's system browser,
        // which holds no session.
        | "/auth/neboai/callback"
        | "/api/v1/integrations/oauth/callback"
        // The app SDK: a public script, loaded by app pages from other
        // origins.
        | "/sdk/nebo.global.js"
        // The browser-extension relay: its handler checks the relay's own
        // secret and refuses any browser.
        | "/ws/extension"
    )
    // Employees as models: the key minted on the employee's Connect tab,
    // checked by `openai::api_key_auth`.
    || path.starts_with("/v1/")
}

/// The routes the plugins' credential reaches: the relays that carry a
/// plugin's calls to the hub, and the phone line (`napp::plugin::plugin_base_env`).
fn plugin_route(path: &str) -> bool {
    matches!(
        path,
        "/api/v1/plugins/oauth/token"
            | "/api/v1/phone/bind"
            | "/api/v1/phone/unbind"
            | "/api/v1/phone/call"
            | "/api/v1/phone/optout"
            | "/api/v1/phone/presence"
            | "/ws/voice/conversation"
    ) || path
        .strip_prefix("/api/v1/plugins/")
        .and_then(|rest| rest.split_once('/'))
        .is_some_and(|(slug, rest)| !slug.is_empty() && (rest == "proxy" || rest.starts_with("proxy/")))
}

/// The app whose routes `path` is, for an app sidecar's token.
fn app_of(path: &str) -> Option<&str> {
    path.strip_prefix("/api/v1/apps/")?.split('/').next().filter(|id| !id.is_empty())
}

impl Boundary {
    /// The caller holds the install key, as its bearer token or its path's
    /// credential.
    fn holds_key(&self, bearer: Option<&str>, path_credential: Option<&str>) -> bool {
        let Some(key) = self.install_key.as_deref() else { return false };
        [bearer, path_credential]
            .into_iter()
            .flatten()
            .any(|t| !t.is_empty() && crate::handlers::ws::constant_time_eq(t, key))
    }

    /// A browser that signed in (`local_access`), on a request from Nebo's
    /// own page: a page another server on this computer serves is the same
    /// site as this one, and a SameSite cookie rides its requests too, so
    /// the request itself must be same-origin. Browsers say so in
    /// `Sec-Fetch-Site`; one that doesn't send it is judged by its Origin.
    fn signed_in_browser(&self, headers: &axum::http::HeaderMap) -> bool {
        let Some(session) = self.session.as_deref() else { return false };
        let holds = headers
            .get_all(axum::http::header::COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(';'))
            .filter_map(|c| c.trim().split_once('='))
            .any(|(name, value)| name == crate::local_access::COOKIE && crate::handlers::ws::constant_time_eq(value, session));
        if !holds {
            return false;
        }
        match headers.get("sec-fetch-site").and_then(|v| v.to_str().ok()) {
            Some(site) => matches!(site, "same-origin" | "none"),
            None => headers
                .get(axum::http::header::ORIGIN)
                .and_then(|v| v.to_str().ok())
                .is_none_or(|origin| self.is_ui_origin(origin)),
        }
    }

    fn is_ui_origin(&self, origin: &str) -> bool {
        UI_ORIGINS.contains(&origin)
            || ["http://localhost", "http://127.0.0.1", "http://[::1]"]
                .iter()
                .any(|base| origin.strip_prefix(base).and_then(|p| p.strip_prefix(':')) == Some(self.port.to_string().as_str()))
    }

    /// A credential scoped to what one kind of process calls, on a route it
    /// reaches: a CLI provider's run credential (`/agent/mcp`), the plugins'
    /// credential (`plugin_route`), an app sidecar's token (its own app's
    /// routes).
    async fn scoped(&self, path: &str, headers: &axum::http::HeaderMap, bearer: Option<&str>, path_credential: Option<&str>) -> bool {
        if path == "/agent/mcp"
            && headers
                .get(agent::tool_credentials::HEADER)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|t| self.credentials.grant(t).is_some())
        {
            return true;
        }
        let plugins = napp::plugin::plugin_local_token();
        if plugin_route(path) && [bearer, path_credential].into_iter().flatten().any(|t| crate::handlers::ws::constant_time_eq(t, plugins)) {
            return true;
        }
        // Nebo's own headless browser opening one app (`napp::app_view`):
        // its pass reaches that app's page and routes, nothing else.
        if let (Some(pass), Some(app)) = (path_credential, napp::app_view::app_of_path(path))
            && napp::app_view::admits(pass, app)
        {
            return true;
        }
        if let (Some(token), Some(app)) = (bearer, app_of(path)) {
            let lifecycle = self.apps.read().await.get(app).cloned();
            if let Some(lifecycle) = lifecycle {
                let expected = lifecycle.app_token().await;
                return !expected.is_empty() && crate::handlers::ws::constant_time_eq(token, &expected);
            }
        }
        false
    }
}

/// The one gate every request passes before any route (REST, WebSocket,
/// `/agent/*`, static files). Every caller proves who it is, on loopback as
/// from the network: nothing on this computer is trusted for where it
/// connects from. On Windows nothing confines an employee's commands, and
/// on macOS and Linux the sandbox was the only thing between a command and
/// every route here, the one that changes its own permissions included.
///
/// - Through the tunnel: admitted — the hub authenticated the owner, and the
///   Host is the browser's (`neboai.com`), which is why the stamp decides.
/// - From the network on a non-loopback bind: the install key is required.
///   `/health` alone is exempt — it reports only status and version, and it
///   is the path an orchestrator's liveness probe calls without credentials.
/// - From this machine: `Host` must name this machine (DNS rebinding), and
///   the caller proves itself: the install key (the owner's clients), a
///   signed-in browser on Nebo's own page (`local_access`), or a credential
///   scoped to the route (`Boundary::scoped`). A route that proves its
///   caller itself is let through (`proves_itself`).
///
/// No employee command holds any of these: Nebo's own settings are kept out
/// of its environment, the install key's file is in Nebo's folder, which its
/// commands can't read, and each process Nebo starts gets only its own
/// scoped credential.
pub async fn local_boundary(
    axum::extract::State(boundary): axum::extract::State<Boundary>,
    request: Request,
    next: Next,
) -> Response {
    let headers = request.headers();
    if came_through_tunnel(headers) {
        return next.run(request).await;
    }
    let presented = bearer(headers).filter(|t| !t.is_empty());
    let path_credential = request.extensions().get::<PathCredential>().map(|c| c.0.as_str());
    let holds_key = boundary.holds_key(presented, path_credential);
    // No peer address means the server was not served with connect info:
    // treat the caller as the network, never as this machine.
    let from_this_machine = request
        .extensions()
        .get::<ConnectInfo<std::net::SocketAddr>>()
        .is_some_and(|ci| ci.0.ip().is_loopback());
    if boundary.network && !from_this_machine {
        if request.uri().path() == "/health" || holds_key {
            return next.run(request).await;
        }
        return boundary_refusal(
            StatusCode::UNAUTHORIZED,
            "this server is reachable from the network: send the install's API key \
             (NEBO_MCP_API_KEY, or the key in Nebo's folder, .install-key) as Authorization: Bearer <key>",
        );
    }
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .or_else(|| request.uri().authority().map(|a| a.as_str()));
    if !host.is_some_and(|host| host_is_local(host, boundary.port)) {
        tracing::warn!(host = ?host, path = %request.uri().path(), "refused a request for a foreign host");
        return boundary_refusal(StatusCode::FORBIDDEN, "host not allowed");
    }
    let path = request.uri().path();
    if holds_key
        || proves_itself(request.method(), path)
        || boundary.signed_in_browser(headers)
        || boundary.scoped(path, headers, presented, path_credential).await
    {
        return next.run(request).await;
    }
    tracing::debug!(path = %path, "refused a caller that proved nothing");
    unproven(&request)
}

/// The refusal for a caller that proved nothing: a page for a browser
/// opening Nebo, words for everything else.
fn unproven(request: &Request) -> Response {
    let wants_page = request.method() == axum::http::Method::GET
        && request
            .headers()
            .get(axum::http::header::ACCEPT)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|a| a.contains("text/html"));
    if wants_page {
        return (
            StatusCode::UNAUTHORIZED,
            [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
            crate::local_access::NOT_SIGNED_IN,
        )
            .into_response();
    }
    boundary_refusal(
        StatusCode::UNAUTHORIZED,
        "Nebo's local API answers only its own app and the owner's clients: sign in from the Nebo \
         app (or `nebo open`), or send the install key as Authorization: Bearer <key>",
    )
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

    const KEY: &str = "k-123";

    /// The routes a caller reaches, behind the boundary, with the path
    /// credential taken off before routing, as `run` serves them. Each route
    /// answers with the path it was reached by.
    async fn send(boundary: Boundary, req: HttpRequest<Body>) -> (StatusCode, String) {
        let echo = |req: axum::extract::Request| async move { req.uri().path().to_string() };
        let app = Router::new()
            .route("/api/v1/agents", axum::routing::get(echo))
            .route("/api/v1/local-session", axum::routing::get(echo))
            .route("/api/v1/plugins/oauth/token", axum::routing::post(echo))
            .route("/api/v1/plugins/{slug}/proxy/{*rest}", axum::routing::get(echo))
            .route("/api/v1/plugins/{slug}/toggle", axum::routing::post(echo))
            .route("/api/v1/apps/{id}/storage", axum::routing::get(echo))
            .route("/apps/{id}/ui/{*path}", axum::routing::get(echo))
            .route("/agent/mcp", axum::routing::post(echo))
            .route("/v1/models", axum::routing::get(echo))
            .route("/api/v1/files/{*path}", axum::routing::get(echo))
            .route("/api/v1/files/upload", axum::routing::post(echo))
            .route("/health", axum::routing::get(echo))
            .route("/auth/neboai/callback", axum::routing::get(echo))
            .fallback(|| async { "spa" })
            .layer(axum::middleware::from_fn_with_state(boundary, local_boundary));
        let resp = app.map_request(path_credential).oneshot(req).await.unwrap();
        let status = resp.status();
        let body = axum::body::to_bytes(resp.into_body(), 1 << 16).await.unwrap();
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    fn boundary(network: bool, key: Option<&str>) -> Boundary {
        Boundary {
            port: 27895,
            network,
            install_key: key.map(str::to_string),
            session: key.map(crate::local_access::session_for),
            credentials: agent::ToolCredentials::default(),
            apps: Apps::default(),
        }
    }

    fn loopback_bind() -> Boundary {
        boundary(false, Some(KEY))
    }

    fn network_bind(key: Option<&str>) -> Boundary {
        boundary(true, key)
    }

    fn request(method: &str, path: &str, peer: &str, headers: &[(&str, &str)]) -> HttpRequest<Body> {
        let mut req = HttpRequest::builder().method(method).uri(path);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let mut req = req.body(Body::empty()).unwrap();
        let peer: SocketAddr = peer.parse().unwrap();
        req.extensions_mut().insert(ConnectInfo(peer));
        req
    }

    async fn status(boundary: Boundary, path: &str, peer: &str, headers: &[(&str, &str)]) -> StatusCode {
        send(boundary, request("GET", path, peer, headers)).await.0
    }

    const LOCAL: &str = "127.0.0.1:50000";
    const LAN: &str = "192.168.1.20:50000";
    const HOST: (&str, &str) = ("host", "localhost:27895");
    const KEYED: (&str, &str) = ("authorization", "Bearer k-123");

    // DNS rebinding: a page on attacker.example re-points its own name at
    // 127.0.0.1 and calls the API as same-origin. The browser sends the
    // attacker's name as Host; nothing else about the request is unusual.
    #[tokio::test]
    async fn a_foreign_host_is_refused() {
        for path in ["/api/v1/agents", "/health", "/", "/ws"] {
            assert_eq!(
                status(loopback_bind(), path, LOCAL, &[("host", "attacker.example:27895"), KEYED]).await,
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
    async fn loopback_and_app_hosts_pass_with_proof() {
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
                status(loopback_bind(), "/api/v1/agents", LOCAL, &[("host", host), KEYED]).await,
                StatusCode::OK,
                "{host}"
            );
        }
    }

    #[tokio::test]
    async fn a_loopback_name_on_another_port_is_refused() {
        assert_eq!(
            status(loopback_bind(), "/api/v1/agents", LOCAL, &[("host", "localhost:8080"), KEYED]).await,
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
        let b = || network_bind(Some(KEY));
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
            status(b(), "/api/v1/agents", LAN, &[("host", "192.168.1.5:27895"), KEYED]).await,
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

    // A cloud bot binds 0.0.0.0 and is reached through the tunnel; the
    // orchestrator's liveness probe calls /health from the node. Its own
    // processes on loopback prove themselves like any local caller.
    #[tokio::test]
    async fn a_network_bind_keeps_the_tunnel_local_callers_and_health_probe() {
        let stamp = comm::tunnel::tunnel_auth_secret();
        assert_eq!(
            status(network_bind(Some(KEY)), "/api/v1/agents", LOCAL, &[("host", "neboai.com"), ("x-nebo-tunnel-auth", stamp)]).await,
            StatusCode::OK
        );
        assert_eq!(
            status(network_bind(Some(KEY)), "/api/v1/agents", LOCAL, &[("host", "127.0.0.1:27895"), KEYED]).await,
            StatusCode::OK
        );
        assert_eq!(
            status(network_bind(Some(KEY)), "/health", "10.244.1.1:40000", &[("host", "10.244.1.7:27895")]).await,
            StatusCode::OK
        );
        // Same-machine callers are still held to the Host check.
        assert_eq!(
            status(network_bind(Some(KEY)), "/api/v1/agents", LOCAL, &[("host", "attacker.example:27895"), KEYED]).await,
            StatusCode::FORBIDDEN
        );
    }

    /// An employee's command on this computer (Windows: nothing confines it)
    /// calls the API with nothing to show: every route that does anything is
    /// refused, and a browser opening Nebo with no session is told how to
    /// sign in.
    #[tokio::test]
    async fn a_loopback_caller_with_no_proof_is_refused() {
        for path in ["/api/v1/agents", "/", "/ws", "/api/v1/plugins/gws/toggle", "/api/v1/apps/a1/storage"] {
            assert_eq!(status(loopback_bind(), path, LOCAL, &[HOST]).await, StatusCode::UNAUTHORIZED, "{path}");
        }
        for (method, path) in [("POST", "/agent/mcp"), ("POST", "/api/v1/files/upload")] {
            let (code, body) = send(loopback_bind(), request(method, path, LOCAL, &[HOST])).await;
            assert_eq!(code, StatusCode::UNAUTHORIZED, "{path}: {body}");
        }
        let (code, page) = send(loopback_bind(), request("GET", "/", LOCAL, &[HOST, ("accept", "text/html")])).await;
        assert_eq!(code, StatusCode::UNAUTHORIZED);
        assert!(page.contains("nebo open"), "{page}");
        // A wrong key, a key with no key configured, an empty bearer.
        assert_eq!(status(loopback_bind(), "/api/v1/agents", LOCAL, &[HOST, ("authorization", "Bearer k-12")]).await, StatusCode::UNAUTHORIZED);
        assert_eq!(status(boundary(false, None), "/api/v1/agents", LOCAL, &[HOST, KEYED]).await, StatusCode::UNAUTHORIZED);
        assert_eq!(status(loopback_bind(), "/api/v1/agents", LOCAL, &[HOST, ("authorization", "Bearer ")]).await, StatusCode::UNAUTHORIZED);
    }

    /// The owner's own clients (the CLI, the MCP bridge, the desktop shell,
    /// a harness fixture) send the install key, as a bearer token or as the
    /// path's credential; the route sees the path without it.
    #[tokio::test]
    async fn the_install_key_opens_every_route() {
        let (code, path) = send(loopback_bind(), request("GET", "/api/v1/agents", LOCAL, &[HOST, KEYED])).await;
        assert_eq!((code, path.as_str()), (StatusCode::OK, "/api/v1/agents"));
        let (code, path) = send(loopback_bind(), request("GET", "/k/k-123/api/v1/agents", LOCAL, &[HOST])).await;
        assert_eq!((code, path.as_str()), (StatusCode::OK, "/api/v1/agents"), "the credential comes off the path");
        let (code, _) = send(loopback_bind(), request("POST", "/agent/mcp", LOCAL, &[HOST, KEYED])).await;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(status(loopback_bind(), "/k/wrong/api/v1/agents", LOCAL, &[HOST]).await, StatusCode::UNAUTHORIZED);
    }

    /// A browser that signed in holds the session cookie, and it counts only
    /// on a request from Nebo's own page: another server on this computer
    /// is the same site, so its page's requests carry the cookie too.
    #[tokio::test]
    async fn a_signed_in_browser_counts_only_on_its_own_page() {
        let cookie = format!("{}={}", crate::local_access::COOKIE, crate::local_access::session_for(KEY));
        let c = || ("cookie", cookie.as_str());
        let ok = [
            vec![HOST, c(), ("sec-fetch-site", "same-origin")],
            vec![HOST, c(), ("sec-fetch-site", "none")],
            // A browser that sends no Sec-Fetch-Site: its Origin decides.
            vec![HOST, c(), ("origin", "http://localhost:5173")],
            vec![HOST, c(), ("origin", "http://localhost:27895")],
            vec![HOST, c()],
            vec![HOST, ("cookie", "theme=dark"), ("cookie", cookie.as_str()), ("sec-fetch-site", "same-origin")],
        ];
        for headers in ok {
            assert_eq!(status(loopback_bind(), "/api/v1/agents", LOCAL, &headers).await, StatusCode::OK, "{headers:?}");
        }
        let refused = [
            vec![HOST, c(), ("sec-fetch-site", "same-site")],
            vec![HOST, c(), ("sec-fetch-site", "cross-site")],
            vec![HOST, c(), ("origin", "http://localhost:3000")],
            vec![HOST, ("cookie", "nebo_session=forged"), ("sec-fetch-site", "same-origin")],
            vec![HOST, ("sec-fetch-site", "same-origin")],
        ];
        for headers in refused {
            assert_eq!(status(loopback_bind(), "/api/v1/agents", LOCAL, &headers).await, StatusCode::UNAUTHORIZED, "{headers:?}");
        }
    }

    /// Routes that prove their caller themselves answer with no proof, and
    /// still only on this machine's Host.
    #[tokio::test]
    async fn routes_that_prove_their_caller_themselves_pass() {
        for path in ["/health", "/api/v1/local-session", "/v1/models", "/auth/neboai/callback", "/api/v1/files/report/chart.png"] {
            assert_eq!(status(loopback_bind(), path, LOCAL, &[HOST]).await, StatusCode::OK, "{path}");
            assert_eq!(status(loopback_bind(), path, LOCAL, &[("host", "attacker.example")]).await, StatusCode::FORBIDDEN, "{path}");
        }
    }

    /// A CLI provider's tool calls carry their run's credential: it reaches
    /// `/agent/mcp` while the run lives, and nothing else.
    #[tokio::test]
    async fn a_run_credential_reaches_agent_mcp_alone() {
        let b = loopback_bind();
        let guard = b.credentials.issue(agent::RunGrant { ctx: Default::default(), agent_id: "emp-1".into() });
        let token = guard.token().to_string();
        let run = || ("x-nebo-run-credential", token.as_str());
        assert_eq!(send(b.clone(), request("POST", "/agent/mcp", LOCAL, &[HOST, run()])).await.0, StatusCode::OK);
        assert_eq!(status(b.clone(), "/api/v1/agents", LOCAL, &[HOST, run()]).await, StatusCode::UNAUTHORIZED);
        drop(guard);
        assert_eq!(send(b.clone(), request("POST", "/agent/mcp", LOCAL, &[HOST, run()])).await.0, StatusCode::UNAUTHORIZED, "an ended run's credential");
        assert_eq!(send(b, request("POST", "/agent/mcp", LOCAL, &[HOST, ("x-nebo-run-credential", "made-up")])).await.0, StatusCode::UNAUTHORIZED);
    }

    /// The plugins' credential (in `NEBO_LOCAL_URL`) reaches the relays and
    /// the phone line, and nothing else.
    #[tokio::test]
    async fn the_plugins_credential_reaches_the_plugin_routes_alone() {
        let token = napp::plugin::plugin_local_token();
        let (code, path) = send(loopback_bind(), request("POST", &format!("/k/{token}/api/v1/plugins/oauth/token"), LOCAL, &[HOST])).await;
        assert_eq!((code, path.as_str()), (StatusCode::OK, "/api/v1/plugins/oauth/token"));
        let (code, _) = send(loopback_bind(), request("GET", &format!("/k/{token}/api/v1/plugins/plaid/proxy/accounts/get"), LOCAL, &[HOST])).await;
        assert_eq!(code, StatusCode::OK);
        let bearer = format!("Bearer {token}");
        assert_eq!(status(loopback_bind(), "/api/v1/plugins/plaid/proxy/x", LOCAL, &[HOST, ("authorization", bearer.as_str())]).await, StatusCode::OK);
        for path in ["/api/v1/agents", "/api/v1/plugins/plaid/toggle"] {
            let (code, _) = send(loopback_bind(), request(if path.ends_with("toggle") { "POST" } else { "GET" }, &format!("/k/{token}{path}"), LOCAL, &[HOST])).await;
            assert_eq!(code, StatusCode::UNAUTHORIZED, "{path}");
        }
        assert!(plugin_route("/ws/voice/conversation") && plugin_route("/api/v1/phone/call"));
        assert!(!plugin_route("/api/v1/phone/lines") && !plugin_route("/api/v1/plugins//proxy/x") && !plugin_route("/api/v1/plugins/x/proxyish"));
    }

    /// Nebo's headless browser opening one app (`napp::app_view`): its pass
    /// reaches that app's page and routes, never another app's or anything
    /// else, and nothing once revoked.
    #[tokio::test]
    async fn an_app_view_pass_opens_its_own_app_alone() {
        let pass = napp::app_view::grant("a1", std::time::Duration::from_secs(60));
        let (code, path) = send(loopback_bind(), request("GET", &format!("/k/{pass}/apps/a1/ui/index.html"), LOCAL, &[HOST])).await;
        assert_eq!((code, path.as_str()), (StatusCode::OK, "/apps/a1/ui/index.html"));
        assert_eq!(send(loopback_bind(), request("GET", &format!("/k/{pass}/api/v1/apps/a1/storage"), LOCAL, &[HOST])).await.0, StatusCode::OK);
        for other in ["/apps/a2/ui/index.html", "/api/v1/apps/a2/storage", "/api/v1/agents"] {
            let (code, _) = send(loopback_bind(), request("GET", &format!("/k/{pass}{other}"), LOCAL, &[HOST])).await;
            assert_eq!(code, StatusCode::UNAUTHORIZED, "{other}");
        }
        napp::app_view::revoke(&pass);
        assert_eq!(send(loopback_bind(), request("GET", &format!("/k/{pass}/apps/a1/ui/index.html"), LOCAL, &[HOST])).await.0, StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn an_app_token_is_judged_on_its_own_app_routes() {
        assert_eq!(app_of("/api/v1/apps/a1/storage/k"), Some("a1"));
        assert_eq!(app_of("/api/v1/apps/"), None);
        assert_eq!(app_of("/api/v1/agents"), None);
    }
}
