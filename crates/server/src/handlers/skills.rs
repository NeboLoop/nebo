use axum::extract::{Path, State};
use axum::response::Json;

use crate::state::AppState;
use super::{to_error_response, HandlerResult};

fn skills_dir() -> Result<std::path::PathBuf, (axum::http::StatusCode, Json<types::api::ErrorResponse>)> {
    config::user_dir()
        .map(|d| d.join("skills"))
        .map_err(to_error_response)
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
    let dir = skills_dir()?;

    // Delete SKILL.md directory if exists
    let skill_dir = dir.join(&name);
    if skill_dir.is_dir() {
        std::fs::remove_dir_all(&skill_dir)
            .map_err(|e| to_error_response(types::NeboError::Io(e)))?;
    }

    // Delete .yaml / .yaml.disabled files if they exist
    let yaml_path = dir.join(format!("{}.yaml", name));
    if yaml_path.exists() {
        let _ = std::fs::remove_file(&yaml_path);
    }
    let disabled_path = dir.join(format!("{}.yaml.disabled", name));
    if disabled_path.exists() {
        let _ = std::fs::remove_file(&disabled_path);
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

    // Fall back to .yaml toggle
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

/// List all skill files from a directory, including SKILL.md subdirectories.
/// Returns metadata including source, description, version, and triggers where available.
fn list_skill_files(dir: &std::path::Path) -> Vec<serde_json::Value> {
    let mut skills = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let file_name = path.file_name().unwrap_or_default().to_string_lossy().to_string();

            // SKILL.md in subdirectory
            if path.is_dir() && !file_name.ends_with(".disabled") {
                if let Some(md_path) = find_skill_md_in_dir(&path) {
                    let mut info = serde_json::json!({
                        "name": file_name,
                        "enabled": true,
                        "path": md_path.to_string_lossy(),
                        "source": "user",
                    });
                    // Parse frontmatter for metadata
                    if let Ok(data) = std::fs::read(&md_path) {
                        if let Ok(skill) = tools::skills::parse_skill_md(&data) {
                            info["description"] = serde_json::json!(skill.description);
                            info["version"] = serde_json::json!(skill.version);
                            if !skill.triggers.is_empty() {
                                info["triggers"] = serde_json::json!(skill.triggers);
                            }
                        }
                    }
                    skills.push(info);
                }
                continue;
            }

            // Disabled SKILL.md directory
            if path.is_dir() && file_name.ends_with(".disabled") {
                let base_name = file_name.trim_end_matches(".disabled");
                skills.push(serde_json::json!({
                    "name": base_name,
                    "enabled": false,
                    "path": path.to_string_lossy(),
                    "source": "user",
                }));
                continue;
            }

            // Flat .yaml files
            if file_name.ends_with(".yaml") && !file_name.ends_with(".disabled") {
                let name = file_name.trim_end_matches(".yaml");
                skills.push(serde_json::json!({
                    "name": name,
                    "enabled": true,
                    "path": path.to_string_lossy(),
                    "source": "user",
                }));
            } else if file_name.ends_with(".yaml.disabled") {
                let name = file_name.trim_end_matches(".yaml.disabled");
                skills.push(serde_json::json!({
                    "name": name,
                    "enabled": false,
                    "path": path.to_string_lossy(),
                    "source": "user",
                }));
            }
        }
    }
    skills.sort_by(|a, b| {
        a["name"].as_str().unwrap_or("").cmp(b["name"].as_str().unwrap_or(""))
    });
    skills
}

/// Find a SKILL.md file in a directory (case-insensitive).
fn find_skill_md_in_dir(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.eq_ignore_ascii_case("skill.md") {
            return Some(entry.path());
        }
    }
    None
}
