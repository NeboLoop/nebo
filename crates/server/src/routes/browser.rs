use axum::Router;

use crate::handlers;
use crate::state::AppState;

/// Browser status routes (extension + built-in tier-2 availability).
pub fn routes() -> Router<AppState> {
    Router::new().route(
        "/browser/status",
        axum::routing::get(handlers::browser::browser_status),
    )
}
