use axum::extract::{Path, Query, State};
use axum::response::Json;
use serde::Deserialize;

use super::{HandlerResult, to_error_response};
use crate::middleware::AuthClaims;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
}

fn default_limit() -> i64 {
    50
}

/// Extract user_id from optional JWT claims, falling back to the local user for desktop mode.
fn user_id(claims: Option<axum::Extension<AuthClaims>>, state: &AppState) -> String {
    claims
        .map(|c| c.0.user_id)
        .or_else(|| state.store.ensure_local_user_id().ok())
        .unwrap_or_default()
}

/// GET /api/v1/notifications
pub async fn list_notifications(
    State(state): State<AppState>,
    claims: Option<axum::Extension<AuthClaims>>,
    Query(q): Query<ListQuery>,
) -> HandlerResult<serde_json::Value> {
    let notifs = state
        .store
        .list_user_notifications(&user_id(claims, &state), q.limit, q.offset)
        .map_err(to_error_response)?;
    Ok(Json(serde_json::json!({"notifications": notifs})))
}

/// PUT /api/v1/notifications/:id/read
pub async fn mark_read(
    State(state): State<AppState>,
    claims: Option<axum::Extension<AuthClaims>>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    state
        .store
        .mark_notification_read(&id, &user_id(claims, &state))
        .map_err(to_error_response)?;
    Ok(Json(serde_json::json!({"success": true})))
}

/// PUT /api/v1/notifications/read-all
pub async fn mark_all_read(
    State(state): State<AppState>,
    claims: Option<axum::Extension<AuthClaims>>,
) -> HandlerResult<serde_json::Value> {
    state
        .store
        .mark_all_notifications_read(&user_id(claims, &state))
        .map_err(to_error_response)?;
    Ok(Json(serde_json::json!({"success": true})))
}

/// DELETE /api/v1/notifications/:id
pub async fn delete_notification(
    State(state): State<AppState>,
    claims: Option<axum::Extension<AuthClaims>>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    state
        .store
        .delete_notification(&id, &user_id(claims, &state))
        .map_err(to_error_response)?;
    Ok(Json(serde_json::json!({"success": true})))
}

/// GET /api/v1/notifications/unread-count
pub async fn unread_count(
    State(state): State<AppState>,
    claims: Option<axum::Extension<AuthClaims>>,
) -> HandlerResult<serde_json::Value> {
    let count = state
        .store
        .count_unread_notifications(&user_id(claims, &state))
        .unwrap_or(0);
    Ok(Json(serde_json::json!({"count": count})))
}
