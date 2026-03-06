use axum::extract::{Path, Query, State};
use axum::response::Json;
use serde::Deserialize;

use crate::state::AppState;
use super::{to_error_response, HandlerResult};

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

/// Parse ROLE.md content: extract YAML frontmatter between `---` delimiters.
fn parse_role_md(content: &str) -> Result<(RoleFrontmatter, String), types::NeboError> {
    let trimmed = content.trim();
    if !trimmed.starts_with("---") {
        return Err(types::NeboError::Validation(
            "ROLE.md must start with YAML frontmatter (---)".into(),
        ));
    }

    let after_first = &trimmed[3..];
    let end_pos = after_first
        .find("---")
        .ok_or_else(|| types::NeboError::Validation("missing closing --- in frontmatter".into()))?;

    let yaml_str = &after_first[..end_pos];
    let body = after_first[end_pos + 3..].trim().to_string();

    let fm: RoleFrontmatter = serde_yaml::from_str(yaml_str)
        .map_err(|e| types::NeboError::Validation(format!("invalid YAML frontmatter: {}", e)))?;

    Ok((fm, body))
}

#[derive(Debug, Deserialize)]
struct RoleFrontmatter {
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    workflows: Vec<String>,
    #[serde(default)]
    tools: Vec<String>,
    #[serde(default)]
    skills: Vec<String>,
    #[serde(default)]
    pricing: Option<RolePricing>,
}

#[derive(Debug, Deserialize)]
struct RolePricing {
    #[serde(default)]
    model: String,
    #[serde(default)]
    cost: f64,
}

/// GET /roles
pub async fn list_roles(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> HandlerResult<serde_json::Value> {
    let limit = q.limit.min(100);
    let roles = state.store.list_roles(limit, q.offset).map_err(to_error_response)?;
    let total = state.store.count_roles().unwrap_or(0);
    Ok(Json(serde_json::json!({
        "roles": roles,
        "total": total,
    })))
}

/// POST /roles
pub async fn create_role(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    let role_md = body["roleMd"]
        .as_str()
        .ok_or_else(|| to_error_response(types::NeboError::Validation("roleMd required".into())))?;

    let (fm, _body) = parse_role_md(role_md).map_err(to_error_response)?;

    let id = uuid::Uuid::new_v4().to_string();
    let code = body["code"].as_str();
    let name = if fm.name.is_empty() {
        body["name"]
            .as_str()
            .ok_or_else(|| to_error_response(types::NeboError::Validation("name required in body or frontmatter".into())))?
    } else {
        &fm.name
    };
    let description = if fm.description.is_empty() {
        body["description"].as_str().unwrap_or("")
    } else {
        &fm.description
    };

    let frontmatter_json = serde_json::json!({
        "workflows": fm.workflows,
        "tools": fm.tools,
        "skills": fm.skills,
        "pricing": fm.pricing.as_ref().map(|p| serde_json::json!({
            "model": p.model,
            "cost": p.cost,
        })),
    });

    let pricing_model = fm.pricing.as_ref().map(|p| p.model.as_str());
    let pricing_cost = fm.pricing.as_ref().map(|p| p.cost);

    let role = state
        .store
        .create_role(
            &id,
            code,
            name,
            description,
            role_md,
            &frontmatter_json.to_string(),
            pricing_model,
            pricing_cost,
        )
        .map_err(to_error_response)?;

    state.hub.broadcast(
        "role_installed",
        serde_json::json!({ "roleId": role.id, "name": role.name }),
    );

    // Report missing dependencies
    let missing_deps = check_dependencies(&fm, &state);

    Ok(Json(serde_json::json!({
        "role": role,
        "missingDeps": missing_deps,
    })))
}

/// GET /roles/{id}
pub async fn get_role(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let role = state
        .store
        .get_role(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;
    Ok(Json(serde_json::json!({ "role": role })))
}

/// PUT /roles/{id}
pub async fn update_role(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    let existing = state
        .store
        .get_role(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    let role_md = body["roleMd"].as_str().unwrap_or(&existing.role_md);
    let (fm, _body) = parse_role_md(role_md).map_err(to_error_response)?;

    let name = if fm.name.is_empty() {
        body["name"].as_str().unwrap_or(&existing.name)
    } else {
        &fm.name
    };
    let description = if fm.description.is_empty() {
        body["description"].as_str().unwrap_or(&existing.description)
    } else {
        &fm.description
    };

    let frontmatter_json = serde_json::json!({
        "workflows": fm.workflows,
        "tools": fm.tools,
        "skills": fm.skills,
        "pricing": fm.pricing.as_ref().map(|p| serde_json::json!({
            "model": p.model,
            "cost": p.cost,
        })),
    });

    let pricing_model = fm.pricing.as_ref().map(|p| p.model.as_str());
    let pricing_cost = fm.pricing.as_ref().map(|p| p.cost);

    state
        .store
        .update_role(
            &id,
            name,
            description,
            role_md,
            &frontmatter_json.to_string(),
            pricing_model,
            pricing_cost,
        )
        .map_err(to_error_response)?;

    let updated = state
        .store
        .get_role(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    Ok(Json(serde_json::json!({ "role": updated })))
}

/// DELETE /roles/{id}
pub async fn delete_role(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let role = state
        .store
        .get_role(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    state.store.delete_role(&id).map_err(to_error_response)?;

    state.hub.broadcast(
        "role_uninstalled",
        serde_json::json!({ "roleId": id, "name": role.name }),
    );

    Ok(Json(serde_json::json!({ "message": "Role deleted" })))
}

/// POST /roles/{id}/toggle
pub async fn toggle_role(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    state.store.toggle_role(&id).map_err(to_error_response)?;
    let role = state
        .store
        .get_role(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;
    Ok(Json(serde_json::json!({ "role": role })))
}

/// POST /roles/{id}/install-deps — attempt to resolve and install all dependencies
pub async fn install_deps(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let role = state
        .store
        .get_role(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    let (fm, _body) = parse_role_md(&role.role_md).map_err(to_error_response)?;
    let missing = check_dependencies(&fm, &state);

    // Report what's missing — actual NeboLoop downloads handled by install code redemption flow

    let total = missing.len();
    Ok(Json(serde_json::json!({
        "roleId": id,
        "missingDeps": missing,
        "total": total,
    })))
}

/// Check which dependencies are missing for a role.
fn check_dependencies(fm: &RoleFrontmatter, state: &AppState) -> Vec<serde_json::Value> {
    let mut missing = Vec::new();

    // Check skills (file-based)
    if let Ok(data_dir) = config::data_dir() {
        let skills_dir = data_dir.join("skills");
        for skill_code in &fm.skills {
            let skill_path = skills_dir.join(format!("{}.yaml", skill_code));
            let skill_dir_path = skills_dir.join(skill_code).join("SKILL.md");
            if !skill_path.exists() && !skill_dir_path.exists() {
                missing.push(serde_json::json!({
                    "type": "skill",
                    "code": skill_code,
                }));
            }
        }
    }

    // Check workflows (DB-based)
    for wf_code in &fm.workflows {
        if state.store.get_workflow_by_code(wf_code).ok().flatten().is_none() {
            missing.push(serde_json::json!({
                "type": "workflow",
                "code": wf_code,
            }));
        }
    }

    // Check tools (napp registry — in-memory)
    // Tool check requires async napp_registry access — report as unchecked for now
    for tool_code in &fm.tools {
        missing.push(serde_json::json!({
            "type": "tool",
            "code": tool_code,
            "status": "unchecked",
        }));
    }

    missing
}
