use axum::Router;

use crate::handlers;
use crate::state::AppState;

/// Background work routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/background", axum::routing::get(handlers::background::list_background))
        .route("/background/stop-all", axum::routing::post(handlers::background::stop_all_background))
        .route("/background/{id}/output", axum::routing::get(handlers::background::background_output))
        .route("/background/{id}/{action}", axum::routing::post(handlers::background::background_action))
}
