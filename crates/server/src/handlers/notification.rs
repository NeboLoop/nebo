use axum::extract::{Path, State};
use axum::response::Json;

use crate::middleware::AuthClaims;
use crate::state::AppState;
use super::{to_error_response, HandlerResult};

/// GET /api/v1/notifications
pub async fn list_notifications(
    State(state): State<AppState>,
    axum::Extension(claims): axum::Extension<AuthClaims>,
) -> HandlerResult<serde_json::Value> {
    let notifs = state
        .store
        .list_user_notifications(&claims.user_id, 50, 0)
        .map_err(to_error_response)?;
    Ok(Json(serde_json::json!({"notifications": notifs})))
}

/// PUT /api/v1/notifications/:id/read
pub async fn mark_read(
    State(state): State<AppState>,
    axum::Extension(claims): axum::Extension<AuthClaims>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    state
        .store
        .mark_notification_read(&id, &claims.user_id)
        .map_err(to_error_response)?;
    Ok(Json(serde_json::json!({"success": true})))
}

/// PUT /api/v1/notifications/read-all
pub async fn mark_all_read(
    State(state): State<AppState>,
    axum::Extension(claims): axum::Extension<AuthClaims>,
) -> HandlerResult<serde_json::Value> {
    state
        .store
        .mark_all_notifications_read(&claims.user_id)
        .map_err(to_error_response)?;
    Ok(Json(serde_json::json!({"success": true})))
}

/// DELETE /api/v1/notifications/:id
pub async fn delete_notification(
    State(state): State<AppState>,
    axum::Extension(claims): axum::Extension<AuthClaims>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    state
        .store
        .delete_notification(&id, &claims.user_id)
        .map_err(to_error_response)?;
    Ok(Json(serde_json::json!({"success": true})))
}

/// GET /api/v1/notifications/unread-count
pub async fn unread_count(
    State(state): State<AppState>,
    axum::Extension(claims): axum::Extension<AuthClaims>,
) -> HandlerResult<serde_json::Value> {
    let count = state
        .store
        .count_unread_notifications(&claims.user_id)
        .unwrap_or(0);
    Ok(Json(serde_json::json!({"count": count})))
}
