//! Hand-offs between employees, read and stopped by the app: the trace
//! `crate::handoff` keeps of work one employee passes to another.

use axum::extract::{Path, Query, State};
use axum::response::Json;
use serde::{Deserialize, Serialize};

use super::{HandlerResult, to_error_response};
use crate::state::AppState;

/// One hand-off as the app reads it: the record, with both employees'
/// names and where each side of it opens.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoffView {
    pub id: String,
    pub parent_id: Option<String>,
    /// `message` | `assignment`
    pub kind: String,
    pub from_agent_id: String,
    pub from_name: String,
    pub to_agent_id: String,
    pub to_name: String,
    pub team_id: String,
    /// The conversation that handed the work on.
    pub sender_session: String,
    /// The turn in it that handed the work on, when one did.
    pub sender_run_id: Option<String>,
    /// Where the sender's side opens in the app.
    pub sender_link: String,
    /// The conversation the receiving employee works it in.
    pub receiver_session: String,
    /// Where the receiving employee's run opens in the app.
    pub receiver_link: String,
    pub ask: String,
    /// queued | running | done | failed | stopped
    pub status: String,
    pub result: String,
    pub error: String,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoffsQuery {
    /// Handed on from this conversation (session key).
    pub from: Option<String>,
    /// Worked in this conversation (session key).
    pub into: Option<String>,
    /// Sent or received by this employee.
    pub agent: Option<String>,
    /// Only those still going.
    pub live: Option<bool>,
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoffList {
    pub handoffs: Vec<HandoffView>,
}

/// One hand-off with every hand-off made under it, down the chain (each
/// names its parent).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoffDetail {
    pub handoff: HandoffView,
    pub descendants: Vec<HandoffView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HandoffStopResponse {
    pub stopped: bool,
}

/// Deepest chain a hand-off's detail reads (the rail caps hops lower).
const MAX_CHAIN: usize = 16;

/// GET /api/v1/handoffs?from=&into=&agent=&live=&limit= — hand-offs, newest first.
pub async fn list_handoffs(State(state): State<AppState>, Query(q): Query<HandoffsQuery>) -> HandlerResult<HandoffList> {
    let rows = state
        .store
        .list_handoffs(&db::HandoffQuery {
            from_session: q.from.as_deref(),
            into_session: q.into.as_deref(),
            agent_id: q.agent.as_deref(),
            parent_id: None,
            live: q.live.unwrap_or(false),
            limit: q.limit.unwrap_or(100).clamp(1, 500),
        })
        .map_err(to_error_response)?;
    Ok(Json(HandoffList { handoffs: rows.into_iter().map(|h| crate::handoff::view(&state.store, h)).collect() }))
}

/// GET /api/v1/handoffs/{id} — one hand-off and the chain under it.
pub async fn get_handoff(State(state): State<AppState>, Path(id): Path<String>) -> HandlerResult<HandoffDetail> {
    let store = &state.store;
    let h = store.get_handoff(&id).map_err(to_error_response)?.ok_or_else(|| to_error_response(types::NeboError::NotFound))?;
    let mut descendants = Vec::new();
    let mut level = vec![h.id.clone()];
    for _ in 0..MAX_CHAIN {
        let mut next = Vec::new();
        for parent in &level {
            let children = store
                .list_handoffs(&db::HandoffQuery { parent_id: Some(parent), ..Default::default() })
                .map_err(to_error_response)?;
            for c in children {
                next.push(c.id.clone());
                descendants.push(crate::handoff::view(store, c));
            }
        }
        if next.is_empty() {
            break;
        }
        level = next;
    }
    Ok(Json(HandoffDetail { handoff: crate::handoff::view(store, h), descendants }))
}

/// POST /api/v1/handoffs/{id}/stop — stop a message hand-off's work.
pub async fn stop_handoff(State(state): State<AppState>, Path(id): Path<String>) -> HandlerResult<HandoffStopResponse> {
    crate::handoff::stop(&state, &id)
        .await
        .map_err(|e| to_error_response(types::NeboError::Validation(e)))?;
    Ok(Json(HandoffStopResponse { stopped: true }))
}
