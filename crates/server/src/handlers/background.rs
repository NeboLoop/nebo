//! The owner's view of background work (`crate::background`): the list,
//! what ended lately, an item's output, the actions on an item, and the
//! stop that stops everything.

use axum::extract::{Path, Query, State};
use axum::response::Json;
use serde::{Deserialize, Serialize};

use super::{HandlerResult, to_error_response};
use crate::background::{self, ActError, BackgroundTask, FinishedTask};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundQuery {
    /// One employee's work ("main" or empty: the main bot's); everything
    /// when absent.
    pub agent_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundListResponse {
    pub tasks: Vec<BackgroundTask>,
    /// What ended lately, newest first.
    pub finished: Vec<FinishedTask>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundActionResponse {
    pub id: String,
    pub action: String,
}

/// The end of a piece of work's output.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundOutputResponse {
    pub output: String,
    /// It had more before what is shown.
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StopEverythingResponse {
    pub stopped: usize,
}

fn act_error(e: ActError) -> (axum::http::StatusCode, Json<types::api::ErrorResponse>) {
    to_error_response(match e {
        ActError::NotFound => types::NeboError::NotFound,
        ActError::NotAllowed => types::NeboError::Validation("That can't be done to this background work.".into()),
        ActError::Failed(e) => types::NeboError::Internal(e),
    })
}

/// GET /api/v1/background
pub async fn list_background(
    State(state): State<AppState>,
    Query(q): Query<BackgroundQuery>,
) -> HandlerResult<BackgroundListResponse> {
    let agent_id = q.agent_id.as_deref();
    Ok(Json(BackgroundListResponse {
        tasks: background::collect(&state, agent_id).await,
        finished: background::finished(agent_id),
    }))
}

/// GET /api/v1/background/{id}/output
pub async fn background_output(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<BackgroundOutputResponse> {
    let tail = background::output(&state, &id).await.map_err(act_error)?;
    Ok(Json(BackgroundOutputResponse { output: tail.output, truncated: tail.truncated }))
}

/// POST /api/v1/background/{id}/{action} — stop, cancel, approve, pause,
/// delete or off.
pub async fn background_action(
    State(state): State<AppState>,
    Path((id, action)): Path<(String, String)>,
) -> HandlerResult<BackgroundActionResponse> {
    background::act(&state, &id, &action).await.map_err(act_error)?;
    Ok(Json(BackgroundActionResponse { id, action }))
}

/// POST /api/v1/background/stop-all — stop every helper, turn, workflow run
/// and background command.
pub async fn stop_all_background(State(state): State<AppState>) -> HandlerResult<StopEverythingResponse> {
    let stopped = background::stop_everything(&state).await;
    Ok(Json(StopEverythingResponse { stopped }))
}
