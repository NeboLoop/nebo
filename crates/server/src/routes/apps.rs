use axum::{Router, routing};
use tower_http::cors::{Any, CorsLayer};

use crate::handlers::apps;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    // App SDK fetches from neboapp:// pages (opaque origin) need permissive
    // CORS so WebKit allows the cross-origin response through to JS.
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        .route(
            "/apps/{agent_id}/ui/",
            routing::get(apps::serve_app_ui_root),
        )
        .route(
            "/apps/{agent_id}/ui/{*path}",
            routing::get(apps::serve_app_ui),
        )
        .route(
            "/apps/{agent_id}/api/{*path}",
            routing::any(apps::proxy_to_sidecar),
        )
        .route(
            "/apps/{agent_id}/agents/invoke",
            routing::post(apps::invoke_agent),
        )
        .route(
            "/apps/{agent_id}/agents/stream",
            routing::post(apps::stream_agent),
        )
        .route(
            "/apps/{agent_id}/janus/complete",
            routing::post(apps::janus_complete),
        )
        .route(
            "/apps/{agent_id}/janus/stream",
            routing::post(apps::janus_stream),
        )
        .route("/apps/{agent_id}/storage", routing::get(apps::list_storage))
        .route(
            "/apps/{agent_id}/storage/{key}",
            routing::get(apps::get_storage),
        )
        .route(
            "/apps/{agent_id}/storage/{key}",
            routing::put(apps::put_storage),
        )
        .route(
            "/apps/{agent_id}/storage/{key}",
            routing::delete(apps::delete_storage),
        )
        .route(
            "/apps/{agent_id}/http/proxy",
            routing::post(apps::http_proxy),
        )
        .route(
            "/apps/{agent_id}/identity",
            routing::get(apps::get_identity),
        )
        .layer(cors)
}
