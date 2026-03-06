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
pub async fn jwt_auth(
    mut request: Request,
    next: Next,
) -> Response {
    let secret = request
        .extensions()
        .get::<JwtSecret>()
        .map(|s| s.0.clone())
        .unwrap_or_default();

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
    headers.insert("x-frame-options", "DENY".parse().unwrap());
    headers.insert("x-xss-protection", "1; mode=block".parse().unwrap());
    headers.insert(
        "referrer-policy",
        "strict-origin-when-cross-origin".parse().unwrap(),
    );
    response
}

/// Strict CSP for API routes only. Blocks all content loading since API responses
/// should never render HTML/scripts. Matches Go's APISecurityHeaders().
pub async fn api_security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        "content-security-policy",
        "default-src 'none'; frame-ancestors 'none'"
            .parse()
            .unwrap(),
    );
    headers.insert(
        "cache-control",
        "no-store, no-cache, must-revalidate, private"
            .parse()
            .unwrap(),
    );
    headers.insert("pragma", "no-cache".parse().unwrap());
    response
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
    let limiter = request
        .extensions()
        .get::<RateLimiter>()
        .cloned();

    let limiter = match limiter {
        Some(l) => l,
        None => return next.run(request).await,
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
