use axum::Router;

use crate::handlers;
use crate::state::AppState;

/// Entity config (per-entity settings) routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/entity-config/{entity_type}/{entity_id}", axum::routing::get(handlers::entity_config::get_entity_config))
        .route("/entity-config/{entity_type}/{entity_id}", axum::routing::put(handlers::entity_config::update_entity_config))
        .route("/entity-config/{entity_type}/{entity_id}", axum::routing::delete(handlers::entity_config::delete_entity_config))
}
