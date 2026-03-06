use axum::extract::State;
use axum::response::Json;

use crate::middleware::AuthClaims;
use crate::state::AppState;
use super::{to_error_response, HandlerResult};

/// GET /api/v1/user/me
pub async fn get_current_user(
    State(state): State<AppState>,
    axum::Extension(claims): axum::Extension<AuthClaims>,
) -> HandlerResult<serde_json::Value> {
    let user = state
        .auth
        .get_user_by_id(&claims.user_id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::UserNotFound))?;

    Ok(Json(serde_json::json!({
        "id": user.id,
        "email": user.email,
        "name": user.name,
        "avatarUrl": user.avatar_url,
        "role": user.role,
        "createdAt": user.created_at,
    })))
}

/// PUT /api/v1/user/me
pub async fn update_current_user(
    State(state): State<AppState>,
    axum::Extension(claims): axum::Extension<AuthClaims>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    state
        .store
        .update_user(
            &claims.user_id,
            body["name"].as_str(),
            body["email"].as_str(),
            body["avatarUrl"].as_str(),
        )
        .map_err(to_error_response)?;

    let user = state
        .auth
        .get_user_by_id(&claims.user_id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::UserNotFound))?;

    Ok(Json(serde_json::json!({
        "id": user.id,
        "email": user.email,
        "name": user.name,
        "avatarUrl": user.avatar_url,
        "role": user.role,
        "createdAt": user.created_at,
    })))
}

/// POST /api/v1/user/me/change-password
pub async fn change_password(
    State(state): State<AppState>,
    axum::Extension(claims): axum::Extension<AuthClaims>,
    Json(req): Json<types::api::ChangePasswordRequest>,
) -> HandlerResult<serde_json::Value> {
    state
        .auth
        .change_password(&claims.user_id, &req.current_password, &req.new_password)
        .map_err(to_error_response)?;
    Ok(Json(serde_json::json!({"success": true})))
}

/// DELETE /api/v1/user/me
pub async fn delete_account(
    State(state): State<AppState>,
    axum::Extension(claims): axum::Extension<AuthClaims>,
) -> HandlerResult<serde_json::Value> {
    state.store.delete_user(&claims.user_id).map_err(to_error_response)?;
    Ok(Json(serde_json::json!({"success": true})))
}

/// GET /api/v1/user/me/profile
pub async fn get_profile(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let profile = state.store.get_user_profile().map_err(to_error_response)?;
    Ok(Json(serde_json::json!({ "profile": profile_to_json(profile) })))
}

/// PUT /api/v1/user/me/profile
pub async fn update_profile(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    // Handle onboardingCompleted separately
    if let Some(completed) = body["onboardingCompleted"].as_bool() {
        state.store.set_onboarding_completed(completed).map_err(to_error_response)?;
    }

    state
        .store
        .update_user_profile(
            body["displayName"].as_str(),
            body["bio"].as_str(),
            body["location"].as_str(),
            body["timezone"].as_str(),
            body["occupation"].as_str(),
            body["interests"].as_str(),
            body["communicationStyle"].as_str(),
            body["goals"].as_str(),
            body["context"].as_str(),
        )
        .map_err(to_error_response)?;
    let profile = state.store.get_user_profile().map_err(to_error_response)?;
    Ok(Json(serde_json::json!({ "profile": profile_to_json(profile) })))
}

/// Convert UserProfile to camelCase JSON matching the frontend's expected format.
fn profile_to_json(profile: Option<db::models::UserProfile>) -> serde_json::Value {
    match profile {
        Some(p) => serde_json::json!({
            "userId": p.user_id,
            "displayName": p.display_name,
            "bio": p.bio,
            "location": p.location,
            "timezone": p.timezone,
            "occupation": p.occupation,
            "interests": p.interests,
            "communicationStyle": p.communication_style,
            "goals": p.goals,
            "context": p.context,
            "onboardingCompleted": p.onboarding_completed.map_or(false, |v| v != 0),
            "onboardingStep": p.onboarding_step,
            "toolPermissions": p.tool_permissions,
            "termsAcceptedAt": p.terms_accepted_at,
            "createdAt": p.created_at,
            "updatedAt": p.updated_at,
        }),
        None => serde_json::json!(null),
    }
}

/// GET /api/v1/user/me/preferences
pub async fn get_preferences(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let prefs = state.store.get_user_preferences().map_err(to_error_response)?;
    Ok(Json(serde_json::json!(prefs)))
}

/// PUT /api/v1/user/me/preferences
pub async fn update_preferences(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    state
        .store
        .update_user_preferences(
            body["theme"].as_str(),
            body["language"].as_str(),
            body["timezone"].as_str(),
            body["emailNotifications"].as_i64().map(|v| v != 0),
            body["inappNotifications"].as_i64().map(|v| v != 0),
        )
        .map_err(to_error_response)?;
    let prefs = state.store.get_user_preferences().map_err(to_error_response)?;
    Ok(Json(serde_json::json!(prefs)))
}

/// GET /api/v1/user/me/permissions
pub async fn get_permissions(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let profile = state.store.get_user_profile().map_err(to_error_response)?;
    let permissions = profile
        .and_then(|p| p.tool_permissions)
        .unwrap_or_else(|| "{}".to_string());
    Ok(Json(serde_json::json!({"permissions": permissions})))
}

/// PUT /api/v1/user/me/permissions
pub async fn update_permissions(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    let permissions = body.to_string();
    state
        .store
        .update_tool_permissions(&permissions)
        .map_err(to_error_response)?;
    Ok(Json(serde_json::json!({"success": true})))
}

/// POST /api/v1/user/me/accept-terms
pub async fn accept_terms(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    state.store.accept_terms().map_err(to_error_response)?;
    Ok(Json(serde_json::json!({"success": true})))
}
