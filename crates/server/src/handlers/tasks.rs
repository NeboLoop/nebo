use axum::extract::{Path, Query, State};
use axum::response::Json;
use serde::Deserialize;

use super::{HandlerResult, to_error_response};
use crate::state::AppState;

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

/// GET /api/v1/tasks
pub async fn list_tasks(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> HandlerResult<serde_json::Value> {
    let tasks = state
        .store
        .list_cron_jobs(q.limit, q.offset)
        .map_err(to_error_response)?;
    let total = state.store.count_cron_jobs().unwrap_or(0);
    Ok(Json(serde_json::json!({
        "tasks": tasks,
        "total": total,
    })))
}

/// POST /api/v1/tasks
pub async fn create_task(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    let name = body["name"]
        .as_str()
        .ok_or_else(|| to_error_response(types::NeboError::Validation("name required".into())))?;
    let schedule = body["schedule"].as_str().ok_or_else(|| {
        to_error_response(types::NeboError::Validation("schedule required".into()))
    })?;
    let command = body["command"].as_str().unwrap_or("");
    let task_type = body["taskType"].as_str().unwrap_or("agent");
    let message = body["message"].as_str();
    let deliver = body["deliver"].as_str();
    let instructions = body["instructions"].as_str();
    let enabled = body["enabled"].as_bool().unwrap_or(true);

    let agent_id = body["agentId"].as_str();
    let channel_ctx_json = body["channelCtxJson"].as_str();
    let task = state
        .store
        .create_cron_job(
            name,
            schedule,
            command,
            task_type,
            message,
            deliver,
            instructions,
            enabled,
            agent_id,
            channel_ctx_json,
        )
        .map_err(to_error_response)?;
    Ok(Json(serde_json::json!(task)))
}

/// GET /api/v1/tasks/:name
pub async fn get_task(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let task = state
        .store
        .get_cron_job_by_name(&name)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;
    Ok(Json(serde_json::json!(task)))
}

/// PUT /api/v1/tasks/:name
pub async fn update_task(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    let existing = state
        .store
        .get_cron_job_by_name(&name)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    let schedule = body["schedule"].as_str().unwrap_or(&existing.schedule);
    let command = body["command"].as_str().unwrap_or(&existing.command);
    let task_type = body["taskType"].as_str().unwrap_or(&existing.task_type);
    let message = body["message"].as_str().or(existing.message.as_deref());
    let deliver = body["deliver"].as_str().or(existing.deliver.as_deref());
    let instructions = body["instructions"]
        .as_str()
        .or(existing.instructions.as_deref());
    let enabled = body["enabled"]
        .as_bool()
        .unwrap_or(existing.enabled.map(|e| e != 0).unwrap_or(true));

    let agent_id = body["agentId"]
        .as_str()
        .or(existing.agent_id.as_deref());
    let channel_ctx_json = body["channelCtxJson"]
        .as_str()
        .or(existing.channel_ctx_json.as_deref());
    state
        .store
        .upsert_cron_job(
            &name,
            schedule,
            command,
            task_type,
            message,
            deliver,
            instructions,
            enabled,
            agent_id,
            channel_ctx_json,
        )
        .map_err(to_error_response)?;

    let updated = state
        .store
        .get_cron_job_by_name(&name)
        .map_err(to_error_response)?;
    Ok(Json(serde_json::json!(updated)))
}

/// DELETE /api/v1/tasks/:name
pub async fn delete_task(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> HandlerResult<serde_json::Value> {
    state
        .store
        .delete_cron_job_by_name(&name)
        .map_err(to_error_response)?;
    // A deleted scheduled task leaves memory too — same rule as employees
    // and workflows: nothing removed may keep being reported as existing.
    let note = format!(
        "NOTE: the scheduled task '{}' was deleted on {}; it no longer exists — do not report it as active",
        name,
        chrono::Local::now().format("%Y-%m-%d")
    );
    let _ = state.store.tombstone_memories_mentioning(&name, &note);
    Ok(Json(serde_json::json!({"success": true})))
}

/// POST /api/v1/tasks/:name/toggle
pub async fn toggle_task(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let task = state
        .store
        .get_cron_job_by_name(&name)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;
    state
        .store
        .toggle_cron_job(task.id)
        .map_err(to_error_response)?;
    let updated = state
        .store
        .get_cron_job_by_name(&name)
        .map_err(to_error_response)?;
    Ok(Json(serde_json::json!(updated)))
}

/// POST /api/v1/tasks/:name/run
pub async fn run_task(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let task = state
        .store
        .get_cron_job_by_name(&name)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;

    // One fire, queued to the engine; it executes the job the same way a
    // scheduled fire runs and announces `task_complete` when it settles.
    let run_id = state
        .store
        .queue_cron_run(&task, true)
        .map_err(to_error_response)?;

    Ok(Json(serde_json::json!({
        "success": true,
        "historyId": run_id,
        "message": "Task execution started",
    })))
}

/// GET /api/v1/tasks/:name/history
pub async fn list_task_history(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(q): Query<ListQuery>,
) -> HandlerResult<serde_json::Value> {
    let task = state
        .store
        .get_cron_job_by_name(&name)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;
    let history = state
        .store
        .list_cron_history(task.id, q.limit, q.offset)
        .map_err(to_error_response)?;
    Ok(Json(serde_json::json!({"history": history})))
}
