//! The case inspector, read-only: what the engine knows about a case — who
//! it is for, who owns it, what it waits on and since when, what came in
//! and what went out, what needs attention — with the ledger's receipts
//! beside every turn's words. Nothing here changes a case.

use axum::extract::{Path, Query, State};
use axum::response::Json;
use serde::{Deserialize, Serialize};

use super::{to_error_response, HandlerResult};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct CasesQuery {
    /// Limit to one employee's cases.
    pub agent: Option<String>,
    pub limit: Option<i64>,
}

/// What a case waits on now.
#[derive(Debug, Serialize)]
pub struct CaseWait {
    pub id: i64,
    /// `signal` (a person or source), `approval`, `timer`, `any`.
    pub on: String,
    pub reason: String,
    pub since: i64,
    pub wake_at: Option<i64>,
    pub superseded_at: Option<i64>,
}

/// One row of the effect ledger: a send the run attempted.
#[derive(Debug, Serialize)]
pub struct CaseReceipt {
    pub id: i64,
    pub provider: String,
    pub state: String,
    pub attempts: i64,
    pub to: Option<String>,
    pub reference: Option<String>,
    pub result: Option<String>,
    pub at: i64,
}

/// One turn of the case: a workflow run, with its receipts.
#[derive(Debug, Serialize)]
pub struct CaseTurn {
    pub id: String,
    pub state: String,
    pub model: Option<String>,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
    pub output: Option<String>,
    pub receipts: Vec<CaseReceipt>,
}

/// One line of the case's durable history.
#[derive(Debug, Serialize)]
pub struct CaseHistoryLine {
    pub id: i64,
    pub kind: String,
    pub text: String,
    pub at: i64,
}

#[derive(Debug, Serialize)]
pub struct CaseSummary {
    pub id: String,
    pub owner: String,
    pub case_type: String,
    pub subject_id: String,
    pub aliases: Vec<String>,
    pub state: String,
    pub result: Option<String>,
    pub summary: String,
    pub opened_at: i64,
    pub ended_at: Option<i64>,
    pub waiting_for: Option<CaseWait>,
    pub last_inbound: Option<CaseHistoryLine>,
    pub last_outbound: Option<CaseReceipt>,
    pub last_transition: Option<i64>,
    pub attention: Option<String>,
    pub previous_case: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CasesList {
    pub cases: Vec<CaseSummary>,
}

#[derive(Debug, Serialize)]
pub struct CaseDetail {
    pub case: CaseSummary,
    pub turns: Vec<CaseTurn>,
    pub history: Vec<CaseHistoryLine>,
    pub waits: Vec<CaseWait>,
}

fn wait_view(w: &db::EngineWait) -> CaseWait {
    CaseWait { id: w.id, on: w.on_kind.clone(), reason: w.reason.clone(), since: w.created_at, wake_at: w.deadline, superseded_at: w.superseded_at }
}

fn receipt_view(e: &db::EngineEffect) -> CaseReceipt {
    CaseReceipt {
        id: e.id,
        provider: e.provider.clone(),
        state: e.state.clone(),
        attempts: e.attempts,
        to: e.counterparty.clone(),
        reference: e.provider_ref.clone(),
        result: e.result.clone(),
        at: e.completed_at.unwrap_or(e.created_at),
    }
}

fn line(e: &db::EngineEvent) -> CaseHistoryLine {
    CaseHistoryLine { id: e.id, kind: e.kind.clone(), text: e.payload.chars().take(600).collect(), at: e.created_at }
}

fn history(store: &db::Store, target: &str) -> Result<Vec<db::EngineEvent>, types::NeboError> {
    let mut events = store.engine_events_for("run", target, 200)?;
    events.sort_by_key(|e| e.id);
    Ok(events)
}

fn summarize(store: &db::Store, run: &db::EngineRun) -> Result<CaseSummary, types::NeboError> {
    let inputs: serde_json::Value = run.inputs.as_deref().and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default();
    let case = &inputs["_case"];
    let subject_id = case["subject_id"].as_str().unwrap_or("").to_string();
    let key = case["key"].as_str().unwrap_or("");
    let aliases = if subject_id.is_empty() {
        vec![]
    } else {
        store.engine_subject_aliases(&subject_id)?.into_iter().map(|(k, v)| format!("{k}:{v}")).collect()
    };
    let waiting_for = if run.state == "waiting" {
        run.current_wait_id.and_then(|id| store.engine_get_wait(id).ok().flatten()).map(|w| wait_view(&w))
    } else {
        None
    };
    let own = history(store, &run.id)?;
    let attention = own.iter().rev().find(|e| e.kind == "needs_attention").map(|e| e.payload.clone());
    let last_transition = own.last().map(|e| e.created_at).or(run.ended_at).or(run.started_at).or(Some(run.created_at));
    let last_inbound = if key.is_empty() { None } else { history(store, key)?.iter().rev().find(|e| e.kind == "signal").map(line) };
    let mut receipts = Vec::new();
    for turn in store.engine_children(&run.id)? {
        receipts.extend(store.engine_effects_for_run(&turn.id)?.iter().filter(|e| e.state == "completed").map(receipt_view));
    }
    let last_outbound = receipts.into_iter().max_by_key(|r| (r.at, r.id));
    Ok(CaseSummary {
        id: run.id.clone(),
        owner: run.agent_id.clone(),
        case_type: case["case_type"].as_str().unwrap_or("").to_string(),
        subject_id,
        aliases,
        state: run.state.clone(),
        result: run.result.clone(),
        summary: run.summary.clone(),
        opened_at: run.created_at,
        ended_at: run.ended_at,
        waiting_for,
        last_inbound,
        last_outbound,
        last_transition,
        attention,
        previous_case: case["previous_case"]["id"].as_str().map(str::to_string),
    })
}

/// GET /api/v1/cases?agent=<id>&limit=<n> — cases, newest first.
pub async fn list_cases(State(state): State<AppState>, Query(q): Query<CasesQuery>) -> HandlerResult<CasesList> {
    let store = &state.store;
    let runs = store.engine_cases(q.agent.as_deref(), q.limit.unwrap_or(50).clamp(1, 500)).map_err(to_error_response)?;
    let mut cases = Vec::with_capacity(runs.len());
    for run in &runs {
        cases.push(summarize(store, run).map_err(to_error_response)?);
    }
    Ok(Json(CasesList { cases }))
}

/// GET /api/v1/cases/{id} — one case with its turns, receipts, history, and waits.
pub async fn get_case(State(state): State<AppState>, Path(id): Path<String>) -> HandlerResult<CaseDetail> {
    let store = &state.store;
    let run = store
        .engine_get_run(&id)
        .map_err(to_error_response)?
        .filter(|r| r.kind == "case")
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;
    let case = summarize(store, &run).map_err(to_error_response)?;
    let mut turns = Vec::new();
    for t in store.engine_children(&run.id).map_err(to_error_response)? {
        let receipts = store.engine_effects_for_run(&t.id).map_err(to_error_response)?.iter().map(receipt_view).collect();
        let model = store.workflow_run_model(&t.id).map_err(to_error_response)?;
        turns.push(CaseTurn {
            id: t.id.clone(),
            state: t.state.clone(),
            model,
            started_at: t.started_at,
            ended_at: t.ended_at,
            output: t.result.as_deref().or(t.error.as_deref()).map(|s| s.chars().take(1200).collect()),
            receipts,
        });
    }
    let history = history(store, &run.id).map_err(to_error_response)?.iter().map(line).collect();
    let waits = store.engine_waits_for_run(&run.id).map_err(to_error_response)?.iter().map(wait_view).collect();
    Ok(Json(CaseDetail { case, turns, history, waits }))
}
