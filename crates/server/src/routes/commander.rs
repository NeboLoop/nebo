use axum::Router;

use crate::handlers;
use crate::state::AppState;

/// Commander (multi-agent coordination canvas) routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/commander/graph", axum::routing::get(handlers::commander::get_graph))
        .route("/commander/layout", axum::routing::put(handlers::commander::save_layout))
        .route("/commander/teams", axum::routing::post(handlers::commander::create_team))
        .route("/commander/teams/{id}", axum::routing::put(handlers::commander::update_team))
        .route("/commander/teams/{id}", axum::routing::delete(handlers::commander::delete_team))
        .route("/commander/edges", axum::routing::post(handlers::commander::create_edge))
        .route("/commander/edges/{id}", axum::routing::delete(handlers::commander::delete_edge))
}
