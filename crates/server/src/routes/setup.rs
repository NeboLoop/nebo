use axum::Router;

use crate::handlers;
use crate::state::AppState;

/// Setup routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/setup/status", axum::routing::get(handlers::setup::status))
        .route("/setup/admin", axum::routing::post(handlers::setup::create_admin))
        .route("/setup/complete", axum::routing::post(handlers::setup::complete))
        .route("/setup/personality", axum::routing::get(handlers::setup::get_personality))
        .route("/setup/personality", axum::routing::put(handlers::setup::update_personality))
}
