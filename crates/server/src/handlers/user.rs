use axum::extract::State;
use axum::response::Json;

use super::{HandlerResult, to_error_response};
use crate::middleware::AuthClaims;
use crate::state::AppState;

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
    state
        .store
        .delete_user(&claims.user_id)
        .map_err(to_error_response)?;
    Ok(Json(serde_json::json!({"success": true})))
}

/// GET /api/v1/user/me/profile
pub async fn get_profile(State(state): State<AppState>) -> HandlerResult<serde_json::Value> {
    let profile = state.store.get_user_profile().map_err(to_error_response)?;
    Ok(Json(
        serde_json::json!({ "profile": profile_to_json(profile) }),
    ))
}

/// PUT /api/v1/user/me/profile
pub async fn update_profile(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    // Handle onboardingCompleted separately
    if let Some(completed) = body["onboardingCompleted"].as_bool() {
        state
            .store
            .set_onboarding_completed(completed)
            .map_err(to_error_response)?;
    }

    // interests arrives as a JSON array from the settings page; the column is
    // TEXT, so serialize it. A plain string is accepted too (legacy callers).
    let interests_json = if body["interests"].is_array() {
        Some(body["interests"].to_string())
    } else {
        body["interests"].as_str().map(String::from)
    };
    state
        .store
        .update_user_profile(
            body["displayName"].as_str(),
            body["bio"].as_str(),
            body["location"].as_str(),
            body["timezone"].as_str(),
            body["occupation"].as_str(),
            interests_json.as_deref(),
            body["communicationStyle"].as_str(),
            body["goals"].as_str(),
            body["context"].as_str(),
            body["accountType"].as_str(),
        )
        .map_err(to_error_response)?;
    let profile = state.store.get_user_profile().map_err(to_error_response)?;
    Ok(Json(
        serde_json::json!({ "profile": profile_to_json(profile) }),
    ))
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
            // Stored as JSON text; hand the frontend a real array.
            "interests": p.interests.as_deref()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                .filter(|v| v.is_array())
                .unwrap_or_else(|| serde_json::json!([])),
            "communicationStyle": p.communication_style,
            "goals": p.goals,
            "context": p.context,
            "onboardingCompleted": p.onboarding_completed.map_or(false, |v| v != 0),
            "onboardingStep": p.onboarding_step,
            "accountType": p.account_type,
            "termsAcceptedAt": p.terms_accepted_at,
            "createdAt": p.created_at,
            "updatedAt": p.updated_at,
        }),
        None => serde_json::json!(null),
    }
}

/// GET /api/v1/user/me/preferences
pub async fn get_preferences(State(state): State<AppState>) -> HandlerResult<serde_json::Value> {
    let prefs = state
        .store
        .get_user_preferences()
        .map_err(to_error_response)?;
    Ok(Json(serde_json::json!({"preferences": prefs})))
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
            body["startPage"].as_str().filter(|v| *v == "chat" || *v == "dashboard"),
        )
        .map_err(to_error_response)?;
    let prefs = state
        .store
        .get_user_preferences()
        .map_err(to_error_response)?;
    Ok(Json(serde_json::json!(prefs)))
}

/// GET /api/v1/user/me/permissions — the company's capability toggles and
/// saved commands, read from the company rules.
pub async fn get_permissions(State(state): State<AppState>) -> HandlerResult<serde_json::Value> {
    // The API contract (`UserGetPermissionsResponse`) declares
    // `permissions: ToolPermission[]`.
    let mut permissions: Vec<serde_json::Value> = crate::entity_config::company_toggles(&state.store)
        .into_iter()
        .map(|(tool, allowed)| serde_json::json!({ "tool": tool, "allowed": allowed }))
        .collect();
    permissions.sort_by(|a, b| a["tool"].as_str().cmp(&b["tool"].as_str()));
    // `capabilities` is the canonical toggle list (key/label/desc) from the
    // single source of truth in `tools::capabilities`. `approvedCommands` are
    // the always-allowed shell-command prefixes — surfaced so Settings can
    // show + revoke them (no invisible durable grants).
    Ok(Json(serde_json::json!({
        "permissions": permissions,
        "capabilities": tools::capabilities::CAPABILITIES,
        "approvedCommands": approved_commands(&state.store).into_iter().map(|(_, p)| p).collect::<Vec<_>>(),
    })))
}

/// The company's always-allowed command prefixes: allow rules on
/// `run_command` with a command-prefix field.
fn approved_commands(store: &db::Store) -> Vec<(String, String)> {
    store
        .permission_rules_in(&types::permissions::Scope::Company)
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.effect == types::permissions::Effect::Allow && r.key == types::permissions::RuleKey::Tool("run_command".into()))
        .filter_map(|r| match r.field {
            Some(types::permissions::RuleField::CommandPrefix(p)) => Some((r.id, p)),
            _ => None,
        })
        .collect()
}

/// PUT /api/v1/user/me/approved-commands — replace the always-allowed prefix
/// list (used by Settings to remove an entry).
pub async fn update_approved_commands(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    use types::permissions::{Effect, Rule, RuleField, RuleKey, RuleSource, Scope, Writer};
    let commands: Vec<String> = body
        .get("commands")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let rule_err = |e: types::permissions::RuleError| to_error_response(types::NeboError::Validation(e.to_string()));
    for (id, prefix) in approved_commands(&state.store) {
        if !commands.contains(&prefix) {
            state.store.remove_permission_rule(&id, &Writer::Owner).map_err(rule_err)?;
        }
    }
    for prefix in commands {
        let rule = Rule {
            id: uuid::Uuid::new_v4().to_string(),
            scope: Scope::Company,
            key: RuleKey::Tool("run_command".into()),
            field: Some(RuleField::CommandPrefix(prefix)),
            effect: Effect::Allow,
            money: None,
            source: RuleSource::Owner,
            locked: false,
            created_at: chrono::Utc::now().timestamp(),
        };
        state.store.write_permission_rule(&rule, &Writer::Owner).map_err(rule_err)?;
    }
    Ok(Json(serde_json::json!({ "success": true })))
}

/// PUT /api/v1/user/me/permissions — the company's capability toggles, as
/// the company's rules.
pub async fn update_permissions(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    // Clients send `{ permissions: { tool: allowed, … } }`: the main
    // assistant's toggles are the company's.
    let map = body.get("permissions").cloned().unwrap_or(body);
    let mut patch = serde_json::json!({ "permissions": map });
    crate::entity_config::apply_permission_patch(&state.store, "main", "main", &mut patch)
        .map_err(|e| to_error_response(types::NeboError::Validation(e.to_string())))?;
    Ok(Json(serde_json::json!({"success": true})))
}

/// POST /api/v1/user/me/accept-terms
pub async fn accept_terms(State(state): State<AppState>) -> HandlerResult<serde_json::Value> {
    state.store.accept_terms().map_err(to_error_response)?;
    Ok(Json(serde_json::json!({"success": true})))
}
