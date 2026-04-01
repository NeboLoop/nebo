use axum::Router;

use crate::handlers;
use crate::state::AppState;

/// Skills and extensions routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/extensions", axum::routing::get(handlers::skills::list_extensions))
        .route("/skills", axum::routing::post(handlers::skills::create_skill))
        .route("/skills/{name}", axum::routing::get(handlers::skills::get_skill))
        .route("/skills/{name}", axum::routing::put(handlers::skills::update_skill))
        .route("/skills/{name}", axum::routing::delete(handlers::skills::delete_skill))
        .route("/skills/{name}/content", axum::routing::get(handlers::skills::get_skill_content))
        .route("/skills/{name}/toggle", axum::routing::post(handlers::skills::toggle_skill))
        .route("/skills/{name}/secrets", axum::routing::get(handlers::skills::list_skill_secrets))
        .route("/skills/{name}/secrets", axum::routing::put(handlers::skills::set_skill_secret))
        .route("/skills/{name}/secrets/{key}", axum::routing::delete(handlers::skills::delete_skill_secret))
}
