use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Json, Response};

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
