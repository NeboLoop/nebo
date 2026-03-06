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

/// GET /api/v1/tasks
pub async fn list_tasks(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> HandlerResult<serde_json::Value> {
    let tasks = state.store.list_cron_jobs(q.limit, q.offset).map_err(to_error_response)?;
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
    let schedule = body["schedule"]
        .as_str()
        .ok_or_else(|| to_error_response(types::NeboError::Validation("schedule required".into())))?;
    let command = body["command"].as_str().unwrap_or("");
    let task_type = body["taskType"].as_str().unwrap_or("agent");
    let message = body["message"].as_str();
    let deliver = body["deliver"].as_str();
    let instructions = body["instructions"].as_str();
    let enabled = body["enabled"].as_bool().unwrap_or(true);

    let task = state
        .store
        .create_cron_job(name, schedule, command, task_type, message, deliver, instructions, enabled)
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
    let instructions = body["instructions"].as_str().or(existing.instructions.as_deref());
    let enabled = body["enabled"]
        .as_bool()
        .unwrap_or(existing.enabled.map(|e| e != 0).unwrap_or(true));

    state
        .store
        .upsert_cron_job(&name, schedule, command, task_type, message, deliver, instructions, enabled)
        .map_err(to_error_response)?;

    let updated = state.store.get_cron_job_by_name(&name).map_err(to_error_response)?;
    Ok(Json(serde_json::json!(updated)))
}

/// DELETE /api/v1/tasks/:name
pub async fn delete_task(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> HandlerResult<serde_json::Value> {
    state.store.delete_cron_job_by_name(&name).map_err(to_error_response)?;
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
    state.store.toggle_cron_job(task.id).map_err(to_error_response)?;
    let updated = state.store.get_cron_job_by_name(&name).map_err(to_error_response)?;
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

    // Create a history entry for this run
    let history = state
        .store
        .create_cron_history(task.id)
        .map_err(to_error_response)?;

    // Mark the run start
    state
        .store
        .update_cron_job_last_run(task.id, None)
        .map_err(to_error_response)?;

    let history_id = history.id;
    let store = state.store.clone();
    let runner = state.runner.clone();
    let hub = state.hub.clone();
    let task_type = task.task_type.clone();
    let command = task.command.clone();
    let message = task.message.clone();
    let task_name = name.clone();
    let task_id = task.id;

    // Execute the task in the background
    tokio::spawn(async move {
        let (success, output) = match task_type.as_str() {
            "bash" => {
                // Execute shell command
                match tokio::process::Command::new("bash")
                    .arg("-c")
                    .arg(&command)
                    .output()
                    .await
                {
                    Ok(result) => {
                        let stdout = String::from_utf8_lossy(&result.stdout).to_string();
                        let stderr = String::from_utf8_lossy(&result.stderr).to_string();
                        let output = if stderr.is_empty() {
                            stdout
                        } else {
                            format!("{}\n[stderr] {}", stdout, stderr)
                        };
                        (result.status.success(), output)
                    }
                    Err(e) => (false, format!("Failed to execute: {}", e)),
                }
            }
            "agent" => {
                // Execute via agent runner
                let prompt = message.as_deref().unwrap_or("");
                if prompt.is_empty() {
                    (false, "No prompt/message configured for agent task".to_string())
                } else {
                    match runner.chat(prompt).await {
                        Ok(response) => (true, response),
                        Err(e) => (false, format!("Agent error: {}", e)),
                    }
                }
            }
            other => (false, format!("Unknown task type: {}", other)),
        };

        // Update history with result
        let (out, err) = if success {
            (Some(output.as_str()), None)
        } else {
            (None, Some(output.as_str()))
        };
        let _ = store.update_cron_history(history_id, success, out, err);
        let _ = store.update_cron_job_last_run(task_id, Some(&output));

        hub.broadcast(
            "task_complete",
            serde_json::json!({
                "task": task_name,
                "success": success,
                "output": if output.len() > 500 { &output[..500] } else { &output },
            }),
        );
    });

    Ok(Json(serde_json::json!({
        "success": true,
        "historyId": history_id,
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
