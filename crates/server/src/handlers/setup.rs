use axum::extract::State;
use axum::response::Json;

use crate::state::AppState;
use super::{to_error_response, HandlerResult};

/// GET /api/v1/setup/status
pub async fn status() -> HandlerResult<serde_json::Value> {
    let complete = config::is_setup_complete().unwrap_or(false);
    Ok(Json(serde_json::json!({
        "setupComplete": complete,
    })))
}

/// POST /api/v1/setup/admin
pub async fn create_admin(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    let email = body["email"]
        .as_str()
        .ok_or_else(|| to_error_response(types::NeboError::Validation("email required".into())))?;
    let password = body["password"]
        .as_str()
        .ok_or_else(|| to_error_response(types::NeboError::Validation("password required".into())))?;
    let name = body["name"].as_str().unwrap_or("Admin");

    // Check if admin already exists
    if state.store.has_admin_user().unwrap_or(false) {
        return Err(to_error_response(types::NeboError::Validation(
            "Admin user already exists".into(),
        )));
    }

    let result = state
        .auth
        .register(email, password, name)
        .map_err(to_error_response)?;

    Ok(Json(serde_json::json!({
        "success": true,
        "token": result.token,
    })))
}

/// POST /api/v1/setup/complete
pub async fn complete(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    // Ensure there is at least one user
    let count = state.store.count_users().unwrap_or(0);
    if count == 0 {
        return Err(to_error_response(types::NeboError::Validation(
            "Create an admin user first".into(),
        )));
    }

    config::mark_setup_complete().map_err(|e| to_error_response(types::NeboError::Config(e.to_string())))?;

    Ok(Json(serde_json::json!({"success": true})))
}

/// GET /api/v1/setup/personality
pub async fn get_personality(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let _ = state.store.ensure_agent_profile();
    let profile = state.store.get_agent_profile().map_err(to_error_response)?;
    Ok(Json(serde_json::json!(profile)))
}

/// PUT /api/v1/setup/personality
pub async fn update_personality(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    let _ = state.store.ensure_agent_profile();
    state
        .store
        .update_agent_profile(
            body["name"].as_str(),
            body["personalityPreset"].as_str(),
            body["customPersonality"].as_str(),
            None, None, None, None, None, // voice, response, emoji, formality, proactivity
            body["emoji"].as_str(),
            body["creature"].as_str(),
            body["vibe"].as_str(),
            body["role"].as_str(),
            body["avatar"].as_str(),
            None, None, None, None, // rules, notes, quiet hours
        )
        .map_err(to_error_response)?;

    let profile = state.store.get_agent_profile().map_err(to_error_response)?;
    Ok(Json(serde_json::json!(profile)))
}
