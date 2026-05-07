use axum::{Router, routing};

use crate::handlers::apps;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/apps/{agent_id}/ui/{*path}", routing::get(apps::serve_app_ui))
        .route("/apps/{agent_id}/api/{*path}", routing::any(apps::proxy_to_sidecar))
        .route("/apps/{agent_id}/storage", routing::get(apps::list_storage))
        .route("/apps/{agent_id}/storage/{key}", routing::get(apps::get_storage))
        .route("/apps/{agent_id}/storage/{key}", routing::put(apps::put_storage))
        .route("/apps/{agent_id}/storage/{key}", routing::delete(apps::delete_storage))
        .route("/apps/{agent_id}/http/proxy", routing::post(apps::http_proxy))
}
