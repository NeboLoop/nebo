use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Json;

use crate::state::AppState;
use super::{to_error_response, HandlerResult};

/// GET /api/v1/skills/:name/secrets
/// Returns declared secrets and their configuration status (never exposes values).
pub async fn list_skill_secrets(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let skill = state
        .skill_loader
        .get(&name)
        .await
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    let declarations = skill.secrets();
    let stored = state
        .store
        .list_skill_secrets(&name)
        .unwrap_or_default();
    let stored_keys: std::collections::HashSet<String> =
        stored.into_iter().map(|(k, _)| k).collect();

    let secrets: Vec<serde_json::Value> = declarations
        .iter()
        .map(|d| {
            serde_json::json!({
                "key": d.key,
                "label": d.label,
                "hint": d.hint,
                "required": d.required,
                "configured": stored_keys.contains(&d.key),
            })
        })
        .collect();

    Ok(Json(serde_json::json!({ "secrets": secrets })))
}

/// PUT /api/v1/skills/:name/secrets
/// Set a secret for a skill. Body: { "key": "BRAVE_API_KEY", "value": "..." }
pub async fn set_skill_secret(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    let key = body["key"]
        .as_str()
        .ok_or_else(|| to_error_response(types::NeboError::Validation("key required".into())))?;
    let value = body["value"]
        .as_str()
        .ok_or_else(|| to_error_response(types::NeboError::Validation("value required".into())))?;

    if value.is_empty() {
        return Err(to_error_response(types::NeboError::Validation(
            "value must not be empty".into(),
        )));
    }

    let encrypted = auth::credential::encrypt(value).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(types::api::ErrorResponse {
                error: format!("encryption failed: {}", e),
            }),
        )
    })?;

    state
        .store
        .set_skill_secret(&name, key, &encrypted)
        .map_err(to_error_response)?;

    Ok(Json(serde_json::json!({ "success": true, "key": key })))
}

/// DELETE /api/v1/skills/:name/secrets/:key
/// Remove a configured secret.
pub async fn delete_skill_secret(
    State(state): State<AppState>,
    Path((name, key)): Path<(String, String)>,
) -> HandlerResult<serde_json::Value> {
    state
        .store
        .delete_skill_secret(&name, &key)
        .map_err(to_error_response)?;

    Ok(Json(serde_json::json!({ "success": true })))
}

fn skills_dir() -> Result<std::path::PathBuf, (axum::http::StatusCode, Json<types::api::ErrorResponse>)> {
    config::user_dir()
        .map(|d| d.join("skills"))
        .map_err(to_error_response)
}

/// GET /api/v1/extensions
pub async fn list_extensions(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let skills: Vec<serde_json::Value> = state
        .skill_loader
        .list()
        .await
        .into_iter()
        .map(|s| {
            let mut info = serde_json::json!({
                "name": s.name,
                "enabled": s.enabled,
                "source": s.source,
            });
            if !s.description.is_empty() {
                info["description"] = serde_json::json!(s.description);
            }
            if !s.version.is_empty() {
                info["version"] = serde_json::json!(s.version);
            }
            if !s.triggers.is_empty() {
                info["triggers"] = serde_json::json!(s.triggers);
            }
            if let Some(ref path) = s.source_path {
                info["path"] = serde_json::json!(path.to_string_lossy());
            }
            // Include secret declarations so the UI knows what needs configuring
            let declarations = s.secrets();
            if !declarations.is_empty() {
                let stored = state.store.list_skill_secrets(&s.name).unwrap_or_default();
                let stored_keys: std::collections::HashSet<String> =
                    stored.into_iter().map(|(k, _)| k).collect();
                let secrets: Vec<serde_json::Value> = declarations
                    .iter()
                    .map(|d| {
                        serde_json::json!({
                            "key": d.key,
                            "label": d.label,
                            "hint": d.hint,
                            "required": d.required,
                            "configured": stored_keys.contains(&d.key),
                        })
                    })
                    .collect();
                info["secrets"] = serde_json::json!(secrets);
                info["needsConfiguration"] = serde_json::json!(
                    declarations.iter().any(|d| d.required && !stored_keys.contains(&d.key))
                );
            }
            info
        })
        .collect();
    Ok(Json(serde_json::json!({"extensions": skills})))
}

/// POST /api/v1/skills
pub async fn create_skill(
    State(_state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    let name = body["name"]
        .as_str()
        .ok_or_else(|| to_error_response(types::NeboError::Validation("name required".into())))?;
    let content = body["content"].as_str().unwrap_or("");

    let dir = skills_dir()?;
    let path = tools::skills::write_skill(&dir, name, content)
        .map_err(|e| to_error_response(types::NeboError::Internal(e)))?;

    Ok(Json(serde_json::json!({
        "name": name,
        "path": path.to_string_lossy(),
    })))
}

/// GET /api/v1/skills/:name
pub async fn get_skill(
    State(_state): State<AppState>,
    Path(name): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let dir = skills_dir()?;

    let path = tools::skills::resolve_skill_path(&dir, &name)
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    let content = std::fs::read_to_string(&path)
        .map_err(|e| to_error_response(types::NeboError::Io(e)))?;

    Ok(Json(serde_json::json!({
        "name": name,
        "content": content,
    })))
}

/// GET /api/v1/skills/:name/content
pub async fn get_skill_content(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> HandlerResult<serde_json::Value> {
    get_skill(State(state), Path(name)).await
}

/// PUT /api/v1/skills/:name
pub async fn update_skill(
    State(_state): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    let dir = skills_dir()?;

    let path = tools::skills::resolve_skill_path(&dir, &name)
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    if let Some(content) = body["content"].as_str() {
        std::fs::write(&path, content)
            .map_err(|e| to_error_response(types::NeboError::Io(e)))?;
    }

    Ok(Json(serde_json::json!({"name": name, "success": true})))
}

/// DELETE /api/v1/skills/:name
pub async fn delete_skill(
    State(_state): State<AppState>,
    Path(name): Path<String>,
) -> HandlerResult<serde_json::Value> {
    // Delete from user/skills/
    if let Ok(user_dir) = config::user_dir() {
        let dir = user_dir.join("skills").join(&name);
        if dir.is_dir() {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    // Delete from nebo/skills/ (marketplace installs)
    if let Ok(nebo_dir) = config::nebo_dir() {
        let dir = nebo_dir.join("skills").join(&name);
        if dir.is_dir() {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    Ok(Json(serde_json::json!({"success": true})))
}

/// POST /api/v1/skills/:name/toggle
pub async fn toggle_skill(
    State(_state): State<AppState>,
    Path(name): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let dir = skills_dir()?;

    // Check SKILL.md directory first
    let skill_dir = dir.join(&name);
    let disabled_dir = dir.join(format!("{}.disabled", name));
    if skill_dir.is_dir() {
        std::fs::rename(&skill_dir, &disabled_dir)
            .map_err(|e| to_error_response(types::NeboError::Io(e)))?;
        return Ok(Json(serde_json::json!({"name": name, "enabled": false})));
    }
    if disabled_dir.is_dir() {
        std::fs::rename(&disabled_dir, &skill_dir)
            .map_err(|e| to_error_response(types::NeboError::Io(e)))?;
        return Ok(Json(serde_json::json!({"name": name, "enabled": true})));
    }

    // Check SKILL.md inside directory for enable/disable via rename
    let skill_md = skill_dir.join("SKILL.md");
    let skill_md_disabled = skill_dir.join("SKILL.md.disabled");
    if skill_md.exists() {
        std::fs::rename(&skill_md, &skill_md_disabled)
            .map_err(|e| to_error_response(types::NeboError::Io(e)))?;
        return Ok(Json(serde_json::json!({"name": name, "enabled": false})));
    }
    if skill_md_disabled.exists() {
        std::fs::rename(&skill_md_disabled, &skill_md)
            .map_err(|e| to_error_response(types::NeboError::Io(e)))?;
        return Ok(Json(serde_json::json!({"name": name, "enabled": true})));
    }

    Err(to_error_response(types::NeboError::NotFound))
}

