//! Hand-off trace — what the owner sees of work one employee passes to
//! another: who sent what to whom, whether it is still going, and what came
//! back. The record is `db::Handoff` (one row per delivery, nested under the
//! hand-off its sender was working on); the coworker rail writes it for a
//! message or a team post's ask (`coworker::send_coworker_message`,
//! `coworker::run_in_thread`), the case opener for an assignment
//! (`workflow::cases::open_assignment` / `settle_assignment`), and the
//! owner's Stop ends it (`chat_dispatch::stop_session`). Every change a
//! running server makes is broadcast as [`EVENT`] with the row as the app
//! reads it ([`view`]).

use std::collections::HashSet;

pub use crate::handlers::handoffs::HandoffView;
use crate::state::AppState;

/// Hub event carrying one hand-off ([`HandoffView`]) whenever it changes.
pub const EVENT: &str = "handoff_updated";

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn name_of(store: &db::Store, agent_id: &str) -> String {
    if agent_id.is_empty() {
        return String::new();
    }
    store.get_agent(agent_id).ok().flatten().map(|a| a.name).unwrap_or_else(|| agent_id.to_string())
}

/// Where conversation `session_key` opens: a team's thread, or the
/// conversation an employee's session is holding.
fn session_link(store: &db::Store, session_key: &str) -> String {
    if let Some(team_id) = session_key.strip_prefix(db::TEAM_THREAD_PREFIX) {
        return tools::owner_notify::link::team(team_id);
    }
    let agent_id = types::keyparser::extract_agent_id(session_key);
    if agent_id.is_empty() {
        return String::new();
    }
    tools::owner_notify::link::session_chat(store, &agent_id, session_key)
}

/// `h` as the app reads it.
pub fn view(store: &db::Store, h: db::Handoff) -> HandoffView {
    let receiver_link = match h.receiver_run_id.as_deref().filter(|_| h.kind == "assignment") {
        Some(case) => tools::owner_notify::link::case(&h.to_agent_id, case),
        None => session_link(store, &h.receiver_session),
    };
    HandoffView {
        from_name: name_of(store, &h.from_agent_id),
        to_name: name_of(store, &h.to_agent_id),
        sender_link: session_link(store, &h.sender_session),
        receiver_link,
        id: h.id,
        parent_id: h.parent_id,
        kind: h.kind,
        from_agent_id: h.from_agent_id,
        to_agent_id: h.to_agent_id,
        team_id: h.team_id,
        sender_session: h.sender_session,
        sender_run_id: h.sender_run_id,
        receiver_session: h.receiver_session,
        ask: h.ask,
        status: h.status,
        result: h.result,
        error: h.error,
        created_at: h.created_at,
        started_at: h.started_at,
        finished_at: h.finished_at,
    }
}

fn broadcast(state: &AppState, h: db::Handoff) {
    let v = view(&state.store, h);
    match serde_json::to_value(&v) {
        Ok(payload) => state.hub.broadcast(EVENT, payload),
        Err(e) => tracing::warn!(error = %e, "handoff: event not serialized"),
    }
}

/// A message on the coworker rail is about to be delivered: it is recorded
/// as queued, under the hand-off its sender was working on. Returns its id,
/// or `None` when it could not be recorded (the delivery goes on: the trace
/// never blocks work).
pub(crate) struct Delivery<'a> {
    pub from_agent_id: &'a str,
    pub to_agent_id: &'a str,
    pub team_id: &'a str,
    pub sender_session: &'a str,
    pub sender_run_id: Option<&'a str>,
    pub receiver_session: &'a str,
    pub ask: &'a str,
}

pub(crate) fn queued(state: &AppState, d: &Delivery<'_>) -> Option<String> {
    let id = uuid::Uuid::new_v4().to_string();
    let row = db::NewHandoff {
        id: &id,
        kind: "message",
        from_agent_id: d.from_agent_id,
        to_agent_id: d.to_agent_id,
        team_id: d.team_id,
        sender_session: d.sender_session,
        sender_run_id: d.sender_run_id,
        receiver_session: d.receiver_session,
        receiver_run_id: None,
        ask: d.ask,
        status: "queued",
    };
    match state.store.create_handoff(&row, now()) {
        Ok(h) => {
            broadcast(state, h);
            Some(id)
        }
        Err(e) => {
            tracing::warn!(error = %e, to = %d.to_agent_id, "handoff: not recorded");
            None
        }
    }
}

/// The receiving employee's run for hand-off `id` has started.
pub(crate) fn running(state: &AppState, id: &str) {
    match state.store.start_handoff(id, now()) {
        Ok(Some(h)) => broadcast(state, h),
        Ok(None) => {}
        Err(e) => tracing::warn!(error = %e, handoff = %id, "handoff: start not recorded"),
    }
}

/// Hand-off `id` never reached its employee: `error` says why.
pub(crate) fn failed(state: &AppState, id: &str, error: &str) {
    match state.store.finish_handoff(id, "failed", "", error, now()) {
        Ok(Some(h)) => broadcast(state, h),
        Ok(None) => {}
        Err(e) => tracing::warn!(error = %e, handoff = %id, "handoff: failure not recorded"),
    }
}

/// A turn in the receiving conversation `receiver_session` ended. Once the
/// conversation has answered everything that addressed it (its own
/// hand-offs answered, its helpers done: `addressing::turn_ended`), every
/// hand-off still going into it ends — done with what it said, or failed
/// with the run's error when it said nothing. Until then the hand-off is
/// still running. Hand-offs the owner's Stop ended during the turn
/// (`chat_dispatch::stop_session`, since `run_started`) are announced here,
/// where the stopped run ends.
pub(crate) fn turn_ended(state: &AppState, receiver_session: &str, reply: &str, error: Option<&str>, run_started: i64) {
    let answered = state.store.unanswered_in_seat(receiver_session).map(|open| open.is_empty()).unwrap_or(false);
    if answered {
        let ended = match error.filter(|e| !e.trim().is_empty() && reply.trim().is_empty()) {
            Some(e) => state.store.finish_handoffs_into(receiver_session, "failed", "", e, now()),
            None => state.store.finish_handoffs_into(receiver_session, "done", reply, "", now()),
        };
        match ended {
            Ok(rows) => rows.into_iter().for_each(|h| broadcast(state, h)),
            Err(e) => tracing::warn!(error = %e, seat = %receiver_session, "handoff: end not recorded"),
        }
    }
    let stopped = state.store.list_handoffs(&db::HandoffQuery { into_session: Some(receiver_session), limit: 20, ..Default::default() });
    for h in stopped.unwrap_or_default() {
        if h.status == "stopped" && h.finished_at.is_some_and(|t| t >= run_started) {
            broadcast(state, h);
        }
    }
}

/// The owner's Stop ended the work in `session_key`: every hand-off still
/// going into it is stopped. Store-only (the Stop has no hub); the stopped
/// run's end announces it ([`turn_ended`]).
pub(crate) fn stopped_into(store: &db::Store, session_key: &str) {
    if let Err(e) = store.finish_handoffs_into(session_key, "stopped", "", "", now()) {
        tracing::warn!(error = %e, session = %session_key, "handoff: stop not recorded");
    }
}

/// Stop message hand-off `id`: its receiving conversation stops (and
/// everyone it asked in turn, down the chain), which records it stopped
/// (`chat_dispatch::stop_session`).
pub async fn stop(state: &AppState, id: &str) -> Result<(), String> {
    let h = state
        .store
        .get_handoff(id)
        .map_err(|e| format!("load hand-off: {e}"))?
        .ok_or_else(|| format!("No hand-off with id {id}"))?;
    if !db::HANDOFF_LIVE.contains(&h.status.as_str()) {
        return Ok(());
    }
    // An assignment is the assignee's own case, closed by the assignee with
    // its outcome; it is not stopped from here.
    if h.kind != "message" {
        return Err("An assignment is the assignee's own work; it closes from its case.".to_string());
    }
    crate::chat_dispatch::stop_session(&state.store, &state.helpers, &state.run_registry, &h.receiver_session).await;
    if let Ok(Some(h)) = state.store.get_handoff(id) {
        broadcast(state, h);
    }
    Ok(())
}

/// Every hand-off still going (queued or running), newest first.
pub fn live(state: &AppState) -> Vec<HandoffView> {
    state
        .store
        .list_handoffs(&db::HandoffQuery { live: true, ..Default::default() })
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "handoff: live list not read");
            Vec::new()
        })
        .into_iter()
        .map(|h| view(&state.store, h))
        .collect()
}

/// The conversations a hand-off still going is worked in: a run in one of
/// them IS that hand-off, so a list of running work shows the hand-off and
/// not the run a second time.
pub fn receiver_sessions(state: &AppState) -> HashSet<String> {
    state
        .store
        .list_handoffs(&db::HandoffQuery { live: true, ..Default::default() })
        .unwrap_or_default()
        .into_iter()
        .map(|h| h.receiver_session)
        .collect()
}
