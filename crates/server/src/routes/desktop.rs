use axum::Router;

use crate::handlers;
use crate::state::AppState;

/// Teach-a-task recording control for the bot's desktop session. Stop
/// carries the owner's narration, so it wears the upload ceiling.
pub fn routes(max_upload_bytes: usize) -> Router<AppState> {
    Router::new()
        .route(
            "/desktop/teach/start",
            axum::routing::post(handlers::desktop::teach_start),
        )
        .route(
            "/desktop/teach/{id}/video",
            axum::routing::get(handlers::desktop::teach_video),
        )
        .route(
            "/desktop/teach/stop",
            axum::routing::post(handlers::desktop::teach_stop)
                .layer(super::files::upload_limit(max_upload_bytes)),
        )
}
