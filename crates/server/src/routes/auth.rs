use axum::Router;

use crate::handlers;
use crate::state::AppState;

/// Rate-limited auth routes (login, register, refresh, etc.).
pub fn auth_routes() -> Router<AppState> {
    Router::new()
        .route("/auth/login", axum::routing::post(handlers::auth::login))
        .route(
            "/auth/register",
            axum::routing::post(handlers::auth::register),
        )
        .route(
            "/auth/refresh",
            axum::routing::post(handlers::auth::refresh),
        )
        .route(
            "/auth/forgot",
            axum::routing::post(handlers::auth::forgot_password),
        )
        .route(
            "/auth/reset",
            axum::routing::post(handlers::auth::reset_password),
        )
        .route(
            "/auth/verify",
            axum::routing::post(handlers::auth::verify_email),
        )
        .route(
            "/auth/resend",
            axum::routing::post(handlers::auth::resend_verification),
        )
}

/// Public auth config route (no rate limit), and a browser's sign-in to the
/// local API (`local_access`): the ticket exchange, and a ticket for the
/// owner's own client.
pub fn public_routes() -> Router<AppState> {
    Router::new()
        .route("/auth/config", axum::routing::get(handlers::auth::config))
        .route("/local-session", axum::routing::get(crate::local_access::sign_in))
        .route("/local-session/ticket", axum::routing::post(crate::local_access::new_ticket))
}
