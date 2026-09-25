//! Route definitions grouped by domain.
//!
//! Each sub-module exports a `routes() -> Router<AppState>` function that
//! defines the route tree for its domain.  The main [`api_routes`] function
//! composes them all under `/api/v1`.

mod agent;
mod apps;
mod artifact_updates;
mod auth;
mod browser;
mod desktop;
mod chat;
mod commander;
mod entity_config;
pub mod files;
mod import;
mod integrations;
mod memory;
mod neboai;
mod notifications;
mod permissions;
mod plugins;
mod provider;
mod roles;
mod setup;
mod telemetry;
mod dashboard;
mod skills;
mod store;
mod tasks;
mod update;
mod user;
mod workflows;
mod backups;
mod teams;
mod org;
mod layers;

use axum::Router;

use crate::middleware::{self, JwtSecret};
use crate::state::AppState;

/// Compose all API sub-routers into the `/api/v1` router.
pub fn api_routes(jwt_secret: JwtSecret, max_upload_bytes: usize) -> Router<AppState> {
    // Auth routes with rate limiting (10 req/min per IP).
    //
    // LAYER ORDER IS LOAD-BEARING. In axum each `.layer` wraps what came
    // before it, so the LAST layer listed is the outermost one and is the
    // first to see the request. A middleware that reads an `Extension` must
    // therefore be listed BEFORE that Extension, or it runs while the value
    // is not in the request yet. Listing them the other way round is what
    // left `jwt_auth` reading a secret that was never there.
    let auth_limiter = middleware::RateLimiter::new(10, std::time::Duration::from_secs(60));
    let auth_routes = auth::auth_routes()
        .layer(axum::middleware::from_fn(middleware::rate_limit))
        .layer(axum::Extension(auth_limiter));

    // Public routes (no auth required)
    let public = Router::new()
        .merge(auth::public_routes())
        .merge(setup::routes())
        .merge(telemetry::routes())
        .merge(dashboard::routes())
        .merge(chat::routes())
        .merge(agent::routes())
        .merge(memory::routes())
        .merge(provider::routes())
        .merge(skills::routes())
        .merge(tasks::routes())
        .merge(import::routes())
        .merge(integrations::routes())
        .merge(browser::routes())
        .merge(desktop::routes())
        .merge(update::routes())
        .merge(files::routes(max_upload_bytes))
        .merge(neboai::routes())
        .merge(workflows::routes())
        .merge(teams::routes())
        .merge(backups::routes())
        .merge(org::routes())
        .merge(layers::routes(max_upload_bytes))
        .merge(roles::routes())
        .merge(commander::routes())
        .merge(plugins::routes())
        .merge(store::routes())
        .merge(entity_config::routes())
        .merge(notifications::routes())
        .merge(permissions::routes())
        .merge(apps::routes())
        .merge(artifact_updates::routes())
        .merge(user::public_routes())
        .merge(self::codes_and_deps());

    // Protected routes (JWT required). Extension last = outermost, so the
    // secret is in the request by the time `jwt_auth` reads it. See the note
    // on `auth_routes` above.
    let protected = user::protected_routes()
        .layer(axum::middleware::from_fn(middleware::jwt_auth))
        .layer(axum::Extension(jwt_secret));

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
        .route("/runs/active", axum::routing::get(active_runs_handler))
}

/// GET /api/v1/runs/active — list all top-level active agent runs.
async fn active_runs_handler(
    axum::extract::State(state): axum::extract::State<AppState>,
) -> axum::Json<serde_json::Value> {
    let runs = state.run_registry.list_top_level().await;
    axum::Json(serde_json::json!({ "runs": runs }))
}
