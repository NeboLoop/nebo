use axum::Router;

use crate::handlers;
use crate::state::AppState;

/// Workroom routes.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/workrooms",
            axum::routing::get(handlers::workrooms::list_workrooms),
        )
        .route(
            "/workrooms",
            axum::routing::post(handlers::workrooms::create_workroom),
        )
        .route(
            "/workrooms/{channelId}",
            axum::routing::delete(handlers::workrooms::delete_workroom),
        )
        .route(
            "/workrooms/{channelId}/messages",
            axum::routing::get(handlers::workrooms::get_workroom_messages),
        )
        .route(
            "/workrooms/{channelId}/send",
            axum::routing::post(handlers::workrooms::send_workroom_message),
        )
}
