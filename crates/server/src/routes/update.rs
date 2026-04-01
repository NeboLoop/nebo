use axum::Router;

use crate::handlers;
use crate::state::AppState;

/// Update check and apply routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/update/check", axum::routing::get(handlers::agent::update_check))
        .route("/update/apply", axum::routing::post(handlers::agent::update_apply))
}
