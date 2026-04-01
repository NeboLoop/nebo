use axum::Router;

use crate::handlers;
use crate::state::AppState;

/// Memory routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/memories", axum::routing::get(handlers::memory::list_memories))
        .route("/memories/search", axum::routing::get(handlers::memory::search_memories))
        .route("/memories/stats", axum::routing::get(handlers::memory::get_stats))
        .route("/memories/{id}", axum::routing::get(handlers::memory::get_memory))
        .route("/memories/{id}", axum::routing::put(handlers::memory::update_memory))
        .route("/memories/{id}", axum::routing::delete(handlers::memory::delete_memory))
}
