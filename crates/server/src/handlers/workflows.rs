use axum::extract::{Path, Query, State};
use axum::response::Json;
use serde::Deserialize;
use tools::workflows::WorkflowManager;

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

/// GET /workflows
pub async fn list_workflows(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> HandlerResult<serde_json::Value> {
    let limit = q.limit.min(100);
    let workflows = state.store.list_workflows(limit, q.offset).map_err(to_error_response)?;
    let total = state.store.count_workflows().unwrap_or(0);
    Ok(Json(serde_json::json!({
        "workflows": workflows,
        "total": total,
    })))
}

/// POST /workflows
pub async fn create_workflow(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    let name = body["name"]
        .as_str()
        .ok_or_else(|| to_error_response(types::NeboError::Validation("name required".into())))?;
    let definition = body["definition"]
        .as_str()
        .ok_or_else(|| to_error_response(types::NeboError::Validation("definition required".into())))?;

    // Validate the workflow definition
    let def = workflow::parser::parse_workflow(definition)
        .map_err(|e| to_error_response(types::NeboError::Validation(e.to_string())))?;

    let id = uuid::Uuid::new_v4().to_string();
    let code = body["code"].as_str();
    let version = body["version"].as_str().unwrap_or("1.0");
    let skill_md = body["skillMd"].as_str();
    let manifest = body["manifest"].as_str();

    let wf = state
        .store
        .create_workflow(&id, code, name, version, definition, skill_md, manifest)
        .map_err(to_error_response)?;

    // Register triggers (cron schedules)
    workflow::triggers::register_triggers(&def, &state.store);

    state.hub.broadcast(
        "workflow_installed",
        serde_json::json!({ "workflowId": wf.id, "name": wf.name }),
    );

    Ok(Json(serde_json::json!({ "workflow": wf })))
}

/// GET /workflows/{id}
pub async fn get_workflow(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let wf = state
        .store
        .get_workflow(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;
    Ok(Json(serde_json::json!({ "workflow": wf })))
}

/// PUT /workflows/{id}
pub async fn update_workflow(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    let existing = state
        .store
        .get_workflow(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    let name = body["name"].as_str().unwrap_or(&existing.name);
    let version = body["version"].as_str().unwrap_or(&existing.version);
    let definition = body["definition"].as_str().unwrap_or(&existing.definition);
    let skill_md = body["skillMd"].as_str().or(existing.skill_md.as_deref());
    let manifest = body["manifest"].as_str().or(existing.manifest.as_deref());

    // Validate definition if changed
    let def = workflow::parser::parse_workflow(definition)
        .map_err(|e| to_error_response(types::NeboError::Validation(e.to_string())))?;

    state
        .store
        .update_workflow(&id, name, version, definition, skill_md, manifest)
        .map_err(to_error_response)?;

    // Re-register triggers
    workflow::triggers::unregister_triggers(&def.id, &state.store);
    workflow::triggers::register_triggers(&def, &state.store);

    let updated = state
        .store
        .get_workflow(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    Ok(Json(serde_json::json!({ "workflow": updated })))
}

/// DELETE /workflows/{id}
pub async fn delete_workflow(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let wf = state
        .store
        .get_workflow(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    // Unregister triggers
    workflow::triggers::unregister_triggers(&id, &state.store);

    // Delete bindings then workflow
    if let Err(e) = state.store.delete_workflow_bindings(&id) {
        tracing::warn!(workflow_id = %id, error = %e, "failed to delete workflow bindings");
    }
    state.store.delete_workflow(&id).map_err(to_error_response)?;

    state.hub.broadcast(
        "workflow_uninstalled",
        serde_json::json!({ "workflowId": id, "name": wf.name }),
    );

    Ok(Json(serde_json::json!({ "message": "Workflow deleted" })))
}

/// POST /workflows/{id}/toggle
pub async fn toggle_workflow(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    state.store.toggle_workflow(&id).map_err(to_error_response)?;
    let wf = state
        .store
        .get_workflow(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;
    Ok(Json(serde_json::json!({ "workflow": wf })))
}

/// POST /workflows/{id}/run — manual trigger
///
/// Delegates to WorkflowManager.run() which creates a run record and spawns
/// background execution via the workflow engine with full provider access.
pub async fn run_workflow(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    let inputs = body.get("inputs").cloned().unwrap_or(serde_json::json!({}));

    let run_id = state
        .workflow_manager
        .run(&id, inputs, "manual")
        .await
        .map_err(|e| to_error_response(types::NeboError::Internal(e)))?;

    let run = state
        .store
        .get_workflow_run(&run_id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    Ok(Json(serde_json::json!({ "run": run })))
}

/// GET /workflows/{id}/runs
pub async fn list_runs(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<ListQuery>,
) -> HandlerResult<serde_json::Value> {
    let limit = q.limit.min(100);
    let runs = state
        .store
        .list_workflow_runs(&id, limit, q.offset)
        .map_err(to_error_response)?;
    let total = state.store.count_workflow_runs(&id).unwrap_or(0);
    Ok(Json(serde_json::json!({
        "runs": runs,
        "total": total,
    })))
}

/// GET /workflows/{id}/runs/{runId}
pub async fn get_run(
    State(state): State<AppState>,
    Path((id, run_id)): Path<(String, String)>,
) -> HandlerResult<serde_json::Value> {
    let run = state
        .store
        .get_workflow_run(&run_id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;
    // Validate run belongs to requested workflow
    if run.workflow_id != id {
        return Err(to_error_response(types::NeboError::NotFound));
    }
    let activities = state
        .store
        .list_activity_results(&run_id)
        .map_err(to_error_response)?;
    Ok(Json(serde_json::json!({
        "run": run,
        "activities": activities,
    })))
}

/// GET /workflows/{id}/bindings
pub async fn list_bindings(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let bindings = state
        .store
        .list_workflow_bindings(&id)
        .map_err(to_error_response)?;
    let total = bindings.len();
    Ok(Json(serde_json::json!({ "bindings": bindings, "total": total })))
}

/// PUT /workflows/{id}/bindings
pub async fn update_bindings(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    // Verify workflow exists
    state
        .store
        .get_workflow(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    let bindings = body["bindings"]
        .as_array()
        .ok_or_else(|| to_error_response(types::NeboError::Validation("bindings array required".into())))?;

    // Clear existing and upsert new
    state.store.delete_workflow_bindings(&id).map_err(to_error_response)?;

    for b in bindings {
        let interface_name = b["interfaceName"]
            .as_str()
            .ok_or_else(|| to_error_response(types::NeboError::Validation("interfaceName required".into())))?;
        let tool_code = b["toolCode"]
            .as_str()
            .ok_or_else(|| to_error_response(types::NeboError::Validation("toolCode required".into())))?;
        state
            .store
            .upsert_workflow_binding(&id, interface_name, tool_code)
            .map_err(to_error_response)?;
    }

    let updated = state
        .store
        .list_workflow_bindings(&id)
        .map_err(to_error_response)?;
    Ok(Json(serde_json::json!({ "bindings": updated })))
}
