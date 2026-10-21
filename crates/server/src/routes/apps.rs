use axum::{Router, routing};
use tower_http::cors::{Any, CorsLayer};

use crate::handlers::{app_plugins, apps};
use crate::state::AppState;

pub fn routes(max_upload_bytes: usize) -> Router<AppState> {
    // App SDK fetches from neboapp:// pages (opaque origin) need permissive
    // CORS so WebKit allows the cross-origin response through to JS.
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    // An app's page has one address, `/apps/<id>/ui/…` at the root
    // (`lib.rs`), never a second one under `/api/v1`.
    Router::new()
        .route(
            "/apps/{agent_id}/api/{*path}",
            routing::any(apps::proxy_to_sidecar),
        )
        .route("/apps/{agent_id}/sidecar", routing::get(apps::sidecar_state))
        .route("/apps/{agent_id}/desktop", routing::get(apps::desktop_app))
        .route(
            "/apps/{agent_id}/sidecar/restart",
            routing::post(apps::restart_sidecar),
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
        .route(
            "/apps/{agent_id}/janus/decide",
            routing::post(apps::janus_decide),
        )
        // The app runs an installed plugin its manifest grants, through the
        // employee tool's runner; long work as a job it reads.
        .route(
            "/apps/{agent_id}/plugins/{slug}/run",
            routing::post(app_plugins::run),
        )
        .route(
            "/apps/{agent_id}/plugins/jobs/{job}",
            routing::get(app_plugins::job),
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
        // nebo.share: a file into Work, and the Share dialog on it. As big
        // as any upload (a presentation carries its images).
        .route(
            "/apps/{agent_id}/share",
            routing::post(apps::share_file).layer(super::files::upload_limit(max_upload_bytes)),
        )
        // nebo.files: a file in a workspace folder the app's manifest
        // grants (`filesystem:read:<folder>`), read only, with byte ranges.
        .route(
            "/apps/{agent_id}/files/{*path}",
            routing::get(apps::serve_app_file),
        )
        .route(
            "/apps/{agent_id}/identity",
            routing::get(apps::get_identity),
        )
        // App Developer mode: the developer script's console batches, and
        // its "Send to <employee>".
        .route("/apps/{agent_id}/devlog", routing::post(apps::post_devlog))
        .route("/apps/{agent_id}/devlog/send", routing::post(apps::send_devlog))
        .layer(cors)
}
