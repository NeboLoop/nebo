//! Permission asks: the one card, as the Inbox, the phone and the open chat
//! read and answer it (Technical Design §2.12.5).

use axum::extract::{Path, Query, State};
use axum::response::Json;
use serde::{Deserialize, Serialize};

use agent::harness::permissions::{Answer, AnsweredVia, Ask, AskError, AskStatus};
use types::NeboError;

use super::{HandlerResult, to_error_response};
use crate::state::AppState;

/// One ask as the owner sees it: who wants to do what, why it asked, and
/// where it stands.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionAskCard {
    pub id: String,
    pub agent_id: String,
    /// The employee's name.
    pub employee: String,
    pub session_key: String,
    /// What it wants to do, as its activity line ("sending a text to …").
    pub sentence: String,
    /// Why it asked, in plain words.
    pub reason: String,
    /// Whether "Allow always" is offered (a locked must-ask can't be loosened).
    pub allow_always: bool,
    /// Whether "This once" is offered (an employee's extra needs are granted
    /// for good or not at all).
    pub this_once: bool,
    /// open | allowed | declined | expired
    pub status: String,
    /// allow_always | this_once | no, once answered.
    pub answer: Option<String>,
    pub created_at: i64,
    pub expires_at: i64,
}

/// The card for `ask`.
pub(crate) fn card(state: &AppState, ask: &Ask) -> PermissionAskCard {
    let (status, answer) = match ask.status {
        AskStatus::Open => ("open", None),
        AskStatus::Answered { answer: Answer::No, .. } => ("declined", Some(Answer::No)),
        AskStatus::Answered { answer, .. } => ("allowed", Some(answer)),
        AskStatus::Expired => ("expired", None),
    };
    PermissionAskCard {
        id: ask.id.clone(),
        agent_id: ask.agent_id.clone(),
        employee: employee_name(state, &ask.agent_id),
        session_key: ask.session_key.clone(),
        sentence: ask.sentence.clone(),
        reason: ask.reason().to_string(),
        allow_always: ask.allow_always_offered(&state.store),
        this_once: ask.this_once_offered(),
        status: status.to_string(),
        answer: answer.map(|a| a.as_str().to_string()),
        created_at: ask.created_at,
        expires_at: ask.expires_at,
    }
}

/// The employee's name; the main assistant goes by the bot's own name.
fn employee_name(state: &AppState, agent_id: &str) -> String {
    let named = |n: String| (!n.trim().is_empty()).then_some(n);
    let agent = (!agent_id.is_empty())
        .then(|| state.store.get_agent(agent_id).ok().flatten())
        .flatten()
        .and_then(|a| named(a.name));
    agent
        .or_else(|| state.store.get_agent_profile().ok().flatten().and_then(|p| named(p.name)))
        .unwrap_or_else(|| "Nebo".to_string())
}

fn ask_error(e: AskError) -> (axum::http::StatusCode, Json<types::api::ErrorResponse>) {
    to_error_response(match e {
        AskError::NotFound => NeboError::NotFound,
        AskError::Settled(_) => NeboError::Validation("this was already answered".into()),
        AskError::Store(msg) => NeboError::Database(msg),
    })
}

#[derive(Debug, Deserialize)]
pub struct ListAsksQuery {
    /// Only the asks of this session (the open chat).
    pub session: Option<String>,
}

/// The asks waiting on the owner.
#[derive(Debug, Serialize)]
pub struct PermissionAsksResponse {
    pub asks: Vec<PermissionAskCard>,
}

/// GET /api/v1/permissions/asks — the asks waiting on the owner, oldest
/// first; `?session=` narrows them to one chat.
pub async fn list_permission_asks(
    State(state): State<AppState>,
    Query(q): Query<ListAsksQuery>,
) -> HandlerResult<PermissionAsksResponse> {
    let asks = state.permission_asks.open(q.session.as_deref()).map_err(ask_error)?;
    Ok(Json(PermissionAsksResponse { asks: asks.iter().map(|a| card(&state, a)).collect() }))
}

/// GET /api/v1/permissions/asks/{id} — one ask and where it stands.
pub async fn get_permission_ask(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<PermissionAskCard> {
    let ask = state.permission_asks.get(&id).map_err(ask_error)?.ok_or_else(|| ask_error(AskError::NotFound))?;
    Ok(Json(card(&state, &ask)))
}

#[derive(Debug, Deserialize)]
pub struct AnswerAskBody {
    /// allow_always | this_once | no
    pub answer: String,
    /// chat | inbox | mobile
    pub via: String,
}

/// POST /api/v1/permissions/asks/{id}/answer — the owner's answer. The
/// first answer anywhere wins; a later one gets the card as it was settled.
pub async fn answer_permission_ask(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<AnswerAskBody>,
) -> HandlerResult<PermissionAskCard> {
    let invalid = |msg: &str| to_error_response(NeboError::Validation(msg.to_string()));
    let answer = Answer::parse(&body.answer).ok_or_else(|| invalid("answer must be allow_always, this_once or no"))?;
    let via = AnsweredVia::parse(&body.via).ok_or_else(|| invalid("via must be chat, inbox or mobile"))?;
    match state.permission_asks.answer(&state.tools, &id, answer, via) {
        Ok(settled) => Ok(Json(card(&state, &settled.ask))),
        Err(AskError::Settled(ask)) => Ok(Json(card(&state, &ask))),
        Err(e) => Err(ask_error(e)),
    }
}
