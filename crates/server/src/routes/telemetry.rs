use axum::Router;

use crate::handlers;
use crate::state::AppState;

/// Client-side telemetry routes.
pub fn routes() -> Router<AppState> {
    Router::new().route(
        "/client/events",
        axum::routing::post(handlers::telemetry::client_event),
    )
}
