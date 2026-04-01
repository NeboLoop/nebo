//! Route definitions grouped by domain.
//!
//! Each sub-module exports a `routes() -> Router<AppState>` function that
//! defines the route tree for its domain.  The main [`api_routes`] function
//! composes them all under `/api/v1`.

mod agent;
mod auth;
mod chat;
mod commander;
mod entity_config;
mod files;
mod integrations;
mod memory;
mod neboloop;
mod notifications;
mod plugins;
mod provider;
mod roles;
mod setup;
mod skills;
mod store;
mod tasks;
mod update;
mod user;
mod workflows;

use axum::Router;

use crate::middleware::{self, JwtSecret};
use crate::state::AppState;

/// Compose all API sub-routers into the `/api/v1` router.
pub fn api_routes(jwt_secret: JwtSecret) -> Router<AppState> {
    // Auth routes with rate limiting (10 req/min per IP)
    let auth_limiter = middleware::RateLimiter::new(10, std::time::Duration::from_secs(60));
    let auth_routes = auth::auth_routes()
        .layer(axum::Extension(auth_limiter))
        .layer(axum::middleware::from_fn(middleware::rate_limit));

    // Public routes (no auth required)
    let public = Router::new()
        .merge(auth::public_routes())
        .merge(setup::routes())
        .merge(chat::routes())
        .merge(agent::routes())
        .merge(memory::routes())
        .merge(provider::routes())
        .merge(skills::routes())
        .merge(tasks::routes())
        .merge(integrations::routes())
        .merge(update::routes())
        .merge(files::routes())
        .merge(neboloop::routes())
        .merge(workflows::routes())
        .merge(roles::routes())
        .merge(commander::routes())
        .merge(plugins::routes())
        .merge(store::routes())
        .merge(entity_config::routes())
        .merge(user::public_routes())
        .merge(self::codes_and_deps());

    // Protected routes (JWT required)
    let protected = user::protected_routes()
        .merge(notifications::routes())
        .layer(axum::Extension(jwt_secret))
        .layer(axum::middleware::from_fn(middleware::jwt_auth));

    Router::new()
        .merge(auth_routes)
        .merge(public)
        .merge(protected)
}

/// Codes and dependency cascade routes (small enough to inline here).
fn codes_and_deps() -> Router<AppState> {
    use crate::codes;
    use crate::deps;

    Router::new()
        .route("/codes", axum::routing::post(codes::submit_code))
        .route("/deps/approve", axum::routing::post(deps::approve_deps))
}
