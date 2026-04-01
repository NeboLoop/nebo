use axum::Router;

use crate::handlers;
use crate::state::AppState;

/// Notification routes (JWT-protected).
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/notifications", axum::routing::get(handlers::notification::list_notifications))
        .route("/notifications/{id}/read", axum::routing::put(handlers::notification::mark_read))
        .route("/notifications/read-all", axum::routing::put(handlers::notification::mark_all_read))
        .route("/notifications/{id}", axum::routing::delete(handlers::notification::delete_notification))
        .route("/notifications/unread-count", axum::routing::get(handlers::notification::unread_count))
}
