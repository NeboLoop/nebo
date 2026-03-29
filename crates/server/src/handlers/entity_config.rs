use std::collections::HashMap;

use axum::extract::{Path, State};
use axum::response::Json;

use crate::entity_config;
use crate::state::AppState;
use super::{to_error_response, HandlerResult};
use types::NeboError;

/// GET /entity-config/{entity_type}/{entity_id}
pub async fn get_entity_config(
    State(state): State<AppState>,
    Path((entity_type, entity_id)): Path<(String, String)>,
) -> HandlerResult<serde_json::Value> {
    let (settings, global_permissions, heartbeat_md) = load_globals(&state)?;
    let entity = state
        .store
        .get_entity_config(&entity_type, &entity_id)
        .map_err(to_error_response)?;
    let resolved = entity_config::resolve(
        &entity_type,
        &entity_id,
        entity.as_ref(),
        &settings,
        &global_permissions,
        &heartbeat_md,
    );
    Ok(Json(serde_json::json!({ "config": resolved })))
}

/// PUT /entity-config/{entity_type}/{entity_id}
pub async fn update_entity_config(
    State(state): State<AppState>,
    Path((entity_type, entity_id)): Path<(String, String)>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    // Validate entity_type
    if !["main", "agent", "channel"].contains(&entity_type.as_str()) {
        return Err(to_error_response(NeboError::Validation(
            "entity_type must be main, agent, or channel".into(),
        )));
    }
    state
        .store
        .upsert_entity_config(&entity_type, &entity_id, &body)
        .map_err(to_error_response)?;

    // Return resolved config
    let (settings, global_permissions, heartbeat_md) = load_globals(&state)?;
    let entity = state
        .store
        .get_entity_config(&entity_type, &entity_id)
        .map_err(to_error_response)?;
    let resolved = entity_config::resolve(
        &entity_type,
        &entity_id,
        entity.as_ref(),
        &settings,
        &global_permissions,
        &heartbeat_md,
    );
    Ok(Json(serde_json::json!({ "config": resolved })))
}

/// DELETE /entity-config/{entity_type}/{entity_id}
pub async fn delete_entity_config(
    State(state): State<AppState>,
    Path((entity_type, entity_id)): Path<(String, String)>,
) -> HandlerResult<serde_json::Value> {
    state
        .store
        .delete_entity_config(&entity_type, &entity_id)
        .map_err(to_error_response)?;
    Ok(Json(serde_json::json!({ "message": "Config reset" })))
}

/// Load global settings, permissions, and heartbeat content for resolution.
fn load_globals(
    state: &AppState,
) -> Result<
    (db::models::Setting, HashMap<String, bool>, String),
    (axum::http::StatusCode, Json<types::api::ErrorResponse>),
> {
    let settings = state
        .store
        .get_settings()
        .map_err(to_error_response)?
        .unwrap_or_else(|| db::models::Setting {
            id: 1,
            autonomous_mode: 0,
            auto_approve_read: 0,
            auto_approve_write: 0,
            auto_approve_bash: 0,
            heartbeat_interval_minutes: 0,
            comm_enabled: 0,
            comm_plugin: String::new(),
            developer_mode: 0,
            auto_update: 1,
            updated_at: 0,
        });

    // Parse global permissions from user profile tool_permissions
    let global_permissions: HashMap<String, bool> = state
        .store
        .get_user_profile()
        .ok()
        .flatten()
        .and_then(|p| p.tool_permissions)
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default();

    // Read heartbeat content from filesystem
    let heartbeat_md = config::data_dir()
        .ok()
        .map(|d| std::fs::read_to_string(d.join("HEARTBEAT.md")).unwrap_or_default())
        .unwrap_or_default();

    Ok((settings, global_permissions, heartbeat_md))
}
