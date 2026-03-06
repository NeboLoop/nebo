use axum::extract::{Path, State};
use axum::response::Json;

use crate::state::AppState;
use super::{to_error_response, HandlerResult};

fn skills_dir() -> Result<std::path::PathBuf, (axum::http::StatusCode, Json<types::api::ErrorResponse>)> {
    let dir = config::data_dir().map_err(to_error_response)?;
    Ok(dir.join("skills"))
}

/// GET /api/v1/extensions
pub async fn list_extensions(
    State(_state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let dir = skills_dir()?;
    let skills = list_skill_files(&dir);
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
    // Best-effort: skills dir may already exist
    let _ = std::fs::create_dir_all(&dir);

    let file_path = dir.join(format!("{}.yaml", name));
    std::fs::write(&file_path, content)
        .map_err(|e| to_error_response(types::NeboError::Io(e)))?;

    Ok(Json(serde_json::json!({
        "name": name,
        "path": file_path.to_string_lossy(),
    })))
}

/// GET /api/v1/skills/:name
pub async fn get_skill(
    State(_state): State<AppState>,
    Path(name): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let dir = skills_dir()?;
    let file_path = dir.join(format!("{}.yaml", name));

    if !file_path.exists() {
        return Err(to_error_response(types::NeboError::NotFound));
    }

    let content = std::fs::read_to_string(&file_path)
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
    let file_path = dir.join(format!("{}.yaml", name));

    if !file_path.exists() {
        return Err(to_error_response(types::NeboError::NotFound));
    }

    if let Some(content) = body["content"].as_str() {
        std::fs::write(&file_path, content)
            .map_err(|e| to_error_response(types::NeboError::Io(e)))?;
    }

    Ok(Json(serde_json::json!({"name": name, "success": true})))
}

/// DELETE /api/v1/skills/:name
pub async fn delete_skill(
    State(_state): State<AppState>,
    Path(name): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let dir = skills_dir()?;
    let file_path = dir.join(format!("{}.yaml", name));

    if file_path.exists() {
        std::fs::remove_file(&file_path)
            .map_err(|e| to_error_response(types::NeboError::Io(e)))?;
    }

    Ok(Json(serde_json::json!({"success": true})))
}

/// POST /api/v1/skills/:name/toggle
pub async fn toggle_skill(
    State(_state): State<AppState>,
    Path(name): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let dir = skills_dir()?;
    let enabled_path = dir.join(format!("{}.yaml", name));
    let disabled_path = dir.join(format!("{}.yaml.disabled", name));

    if enabled_path.exists() {
        std::fs::rename(&enabled_path, &disabled_path)
            .map_err(|e| to_error_response(types::NeboError::Io(e)))?;
        Ok(Json(serde_json::json!({"name": name, "enabled": false})))
    } else if disabled_path.exists() {
        std::fs::rename(&disabled_path, &enabled_path)
            .map_err(|e| to_error_response(types::NeboError::Io(e)))?;
        Ok(Json(serde_json::json!({"name": name, "enabled": true})))
    } else {
        Err(to_error_response(types::NeboError::NotFound))
    }
}

fn list_skill_files(dir: &std::path::Path) -> Vec<serde_json::Value> {
    let mut skills = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let file_name = path.file_name().unwrap_or_default().to_string_lossy().to_string();

            if file_name.ends_with(".yaml") && !file_name.ends_with(".disabled") {
                let name = file_name.trim_end_matches(".yaml");
                skills.push(serde_json::json!({
                    "name": name,
                    "enabled": true,
                    "path": path.to_string_lossy(),
                }));
            } else if file_name.ends_with(".yaml.disabled") {
                let name = file_name.trim_end_matches(".yaml.disabled");
                skills.push(serde_json::json!({
                    "name": name,
                    "enabled": false,
                    "path": path.to_string_lossy(),
                }));
            }
        }
    }
    skills.sort_by(|a, b| {
        a["name"].as_str().unwrap_or("").cmp(b["name"].as_str().unwrap_or(""))
    });
    skills
}
