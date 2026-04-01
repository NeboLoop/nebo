use axum::Router;

use crate::handlers;
use crate::state::AppState;

/// Rate-limited auth routes (login, register, refresh, etc.).
pub fn auth_routes() -> Router<AppState> {
    Router::new()
        .route("/auth/login", axum::routing::post(handlers::auth::login))
        .route("/auth/register", axum::routing::post(handlers::auth::register))
        .route("/auth/refresh", axum::routing::post(handlers::auth::refresh))
        .route("/auth/forgot", axum::routing::post(handlers::auth::forgot_password))
        .route("/auth/reset", axum::routing::post(handlers::auth::reset_password))
        .route("/auth/verify", axum::routing::post(handlers::auth::verify_email))
        .route("/auth/resend", axum::routing::post(handlers::auth::resend_verification))
}

/// Public auth config route (no rate limit).
pub fn public_routes() -> Router<AppState> {
    Router::new()
        .route("/auth/config", axum::routing::get(handlers::auth::config))
}
