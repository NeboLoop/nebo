use axum::Router;

use crate::handlers;
use crate::state::AppState;

/// Teach-a-task recording control for the bot's desktop session.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/desktop/teach/start",
            axum::routing::post(handlers::desktop::teach_start),
        )
        .route(
            "/desktop/teach/stop",
            axum::routing::post(handlers::desktop::teach_stop),
        )
}
