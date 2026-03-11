//! Store proxy handlers — forward marketplace queries to NeboLoop API.

use axum::extract::{Path, Query, State};
use axum::response::Json;
use serde::Deserialize;

use super::{to_error_response, HandlerResult};
use crate::codes::build_api_client;
use crate::state::AppState;
use types::NeboError;

#[derive(Deserialize)]
pub struct StoreQuery {
    pub q: Option<String>,
    pub category: Option<String>,
    pub page: Option<i64>,
    #[serde(rename = "pageSize")]
    pub page_size: Option<i64>,
}

/// GET /store/apps — list marketplace apps.
pub async fn list_store_apps(
    State(state): State<AppState>,
    Query(params): Query<StoreQuery>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp = api
        .list_apps(
            params.q.as_deref(),
            params.category.as_deref(),
            params.page,
            params.page_size,
        )
        .await
        .map_err(|e| to_error_response(NeboError::Internal(format!("list_apps: {e}"))))?;
    Ok(Json(serde_json::to_value(resp).unwrap_or_default()))
}

/// GET /store/skills — list marketplace skills.
pub async fn list_store_skills(
    State(state): State<AppState>,
    Query(params): Query<StoreQuery>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp = api
        .list_skills(
            params.q.as_deref(),
            params.category.as_deref(),
            params.page,
            params.page_size,
        )
        .await
        .map_err(|e| to_error_response(NeboError::Internal(format!("list_skills: {e}"))))?;
    Ok(Json(serde_json::to_value(resp).unwrap_or_default()))
}

/// GET /store/workflows — list marketplace workflows.
pub async fn list_store_workflows(
    State(state): State<AppState>,
    Query(params): Query<StoreQuery>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp = api
        .list_workflows(
            params.q.as_deref(),
            params.category.as_deref(),
            params.page,
            params.page_size,
        )
        .await
        .map_err(|e| to_error_response(NeboError::Internal(format!("list_workflows: {e}"))))?;
    Ok(Json(serde_json::to_value(resp).unwrap_or_default()))
}

/// POST /store/skills/{id}/install — install a skill by code/id.
pub async fn install_store_skill(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    let resp = api
        .install_skill(&id)
        .await
        .map_err(|e| to_error_response(NeboError::Internal(format!("install_skill: {e}"))))?;
    Ok(Json(serde_json::to_value(resp).unwrap_or_default()))
}

/// DELETE /store/skills/{id}/install — uninstall a skill.
pub async fn uninstall_store_skill(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let api = build_api_client(&state).map_err(to_error_response)?;
    api.uninstall_skill(&id)
        .await
        .map_err(|e| to_error_response(NeboError::Internal(format!("uninstall_skill: {e}"))))?;
    Ok(Json(serde_json::json!({ "success": true })))
}
