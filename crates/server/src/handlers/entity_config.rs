use axum::extract::{Path, State};
use axum::response::Json;

use super::{HandlerResult, to_error_response};
use crate::entity_config;
use crate::state::AppState;
use types::NeboError;

/// GET /entity-config/{entity_type}/{entity_id}
pub async fn get_entity_config(
    State(state): State<AppState>,
    Path((entity_type, entity_id)): Path<(String, String)>,
) -> HandlerResult<serde_json::Value> {
    let (settings, heartbeat_md) = load_globals(&state)?;
    let entity = state
        .store
        .get_entity_config(&entity_type, &entity_id)
        .map_err(to_error_response)?;
    let resolved = entity_config::resolve(
        &entity_type,
        &entity_id,
        entity.as_ref(),
        &settings,
        entity_config::permission_view(&state.store, &entity_type, &entity_id),
        &heartbeat_md,
    );
    Ok(Json(serde_json::json!({ "config": resolved })))
}

/// PUT /entity-config/{entity_type}/{entity_id}
pub async fn update_entity_config(
    State(state): State<AppState>,
    Path((entity_type, entity_id)): Path<(String, String)>,
    Json(mut body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    // Validate entity_type
    if !["main", "agent", "channel"].contains(&entity_type.as_str()) {
        return Err(to_error_response(NeboError::Validation(
            "entity_type must be main, agent, or channel".into(),
        )));
    }
    // An employee with an outside door is multi-chat (owner rule 09-25):
    // the switch-off is refused, and the refusal says why. A phone line is
    // checked live at NeboAI too, as the memory-isolation lock is.
    if entity_type == "agent" && body.get("multiChat").is_some() {
        let mut doors = state.store.outside_doors(&entity_id).map_err(to_error_response)?;
        if !doors.iter().any(|d| d == "phonecall")
            && super::neboai::agent_has_phone_line(&state, &entity_id).await
        {
            doors.push("phonecall".to_string());
        }
        let name_of = |slug: &str| state.plugin_store.get_manifest(slug).map(|m| m.name);
        if let Some(refusal) = crate::outside::multi_chat_lock(&body, &doors, name_of) {
            return Err(to_error_response(NeboError::Validation(refusal)));
        }
    }
    // Permission edits are the owner's rules; the rest is the config row.
    entity_config::apply_permission_patch(&state.store, &entity_type, &entity_id, &mut body)
        .map_err(|e| to_error_response(NeboError::Validation(e.to_string())))?;
    state
        .store
        .upsert_entity_config(&entity_type, &entity_id, &body)
        .map_err(to_error_response)?;

    // Return resolved config
    let (settings, heartbeat_md) = load_globals(&state)?;
    let entity = state
        .store
        .get_entity_config(&entity_type, &entity_id)
        .map_err(to_error_response)?;
    let resolved = entity_config::resolve(
        &entity_type,
        &entity_id,
        entity.as_ref(),
        &settings,
        entity_config::permission_view(&state.store, &entity_type, &entity_id),
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

/// Load global settings and heartbeat content for resolution.
fn load_globals(
    state: &AppState,
) -> Result<
    (db::models::Setting, String),
    (axum::http::StatusCode, Json<types::api::ErrorResponse>),
> {
    let settings = state
        .store
        .get_settings()
        .map_err(to_error_response)?
        .unwrap_or_else(|| db::models::Setting {
            id: 1,
            auto_install_deps: 0,
            auto_approve_read: 0,
            auto_approve_write: 0,
            auto_approve_bash: 0,
            heartbeat_interval_minutes: 0,
            comm_enabled: 0,
            comm_plugin: String::new(),
            developer_mode: 0,
            auto_update: 1,
            full_access: 0,
            updated_at: 0,
        });

    // Read heartbeat content from filesystem
    let heartbeat_md = config::data_dir()
        .ok()
        .map(|d| std::fs::read_to_string(d.join("HEARTBEAT.md")).unwrap_or_default())
        .unwrap_or_default();

    Ok((settings, heartbeat_md))
}
