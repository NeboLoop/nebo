use axum::Router;

use crate::handlers;
use crate::state::AppState;

/// Permission routes: the asks waiting on the owner and their answers.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/permissions/asks", axum::routing::get(handlers::permissions::list_permission_asks))
        .route("/permissions/asks/{id}", axum::routing::get(handlers::permissions::get_permission_ask))
        .route(
            "/permissions/asks/{id}/answer",
            axum::routing::post(handlers::permissions::answer_permission_ask),
        )
}
