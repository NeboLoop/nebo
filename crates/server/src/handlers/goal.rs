//! The owner's side of the agreed goal: `/goal` in the chat and the same
//! actions over REST, one implementation for both. Every change is
//! broadcast as `goal_status` so the thread's goal line follows it.

use agent::harness::goal::{AgreedGoal, GoalSource, GoalStore};
use axum::extract::{Path, State};
use axum::response::Json;
use serde::{Deserialize, Serialize};
use types::NeboError;

use super::{HandlerResult, to_error_response};
use crate::state::AppState;

/// A session's goal as the owner sees it; also the `goal_status` event.
#[derive(Debug, Clone, Serialize)]
pub struct SessionGoalStatus {
    /// The session key the thread knows.
    pub session_id: String,
    pub condition: String,
    /// Done checks that found it unmet.
    pub turns: u32,
    pub last_reason: Option<String>,
    /// active, met, impossible, paused:<why> or cleared.
    pub status: String,
}

#[derive(Debug, Serialize)]
pub struct SessionGoalResponse {
    /// Absent when the session has never had a goal.
    pub goal: Option<SessionGoalStatus>,
}

#[derive(Debug, Deserialize)]
pub struct SetGoalRequest {
    pub condition: String,
}

fn status_of(session_key: &str, goal: &AgreedGoal) -> SessionGoalStatus {
    SessionGoalStatus {
        session_id: session_key.to_string(),
        condition: goal.condition.clone(),
        turns: goal.turns,
        last_reason: goal.last_reason.clone(),
        status: goal.status.as_str().to_string(),
    }
}

/// Tell every open thread a session's goal changed.
fn broadcast(state: &AppState, session_key: &str, goal: &AgreedGoal) {
    state.hub.broadcast(
        "goal_status",
        serde_json::to_value(status_of(session_key, goal)).unwrap_or_default(),
    );
}

/// `(session id, session key)` for a session key or id. `create` makes the
/// session when a key names none yet (a goal set before the first message).
fn session_for(
    state: &AppState,
    key_or_id: &str,
    create: bool,
) -> Result<Option<(String, String)>, NeboError> {
    if let Some(s) = state.store.get_session(key_or_id)? {
        let key = s.name.unwrap_or_else(|| key_or_id.to_string());
        return Ok(Some((s.id, key)));
    }
    let sessions = state.runner.sessions();
    match sessions.resolve_session_id_by_key(key_or_id) {
        Ok(id) => Ok(Some((id, key_or_id.to_string()))),
        Err(NeboError::NotFound) if create => {
            let s = sessions.get_or_create(key_or_id, "")?;
            Ok(Some((s.id, key_or_id.to_string())))
        }
        Err(NeboError::NotFound) => Ok(None),
        Err(e) => Err(e),
    }
}

/// The session's goal, if it ever had one.
fn current(
    state: &AppState,
    key_or_id: &str,
) -> Result<Option<SessionGoalStatus>, NeboError> {
    let Some((id, key)) = session_for(state, key_or_id, false)? else {
        return Ok(None);
    };
    Ok(GoalStore::new(state.runner.sessions(), &id)
        .get()?
        .map(|g| status_of(&key, &g)))
}

/// The owner sets the goal. `Err` is the message the owner reads.
fn set(
    state: &AppState,
    key_or_id: &str,
    condition: &str,
) -> Result<SessionGoalStatus, String> {
    let (id, key) = session_for(state, key_or_id, true)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "That conversation was not found.".to_string())?;
    let goal = GoalStore::new(state.runner.sessions(), &id)
        .set(condition, GoalSource::OwnerCommand)
        .map_err(|e| e.to_string())?;
    broadcast(state, &key, &goal);
    Ok(status_of(&key, &goal))
}

/// The owner clears the goal. `None` when there was none being pursued.
fn clear(
    state: &AppState,
    key_or_id: &str,
) -> Result<Option<SessionGoalStatus>, NeboError> {
    let Some((id, key)) = session_for(state, key_or_id, false)? else {
        return Ok(None);
    };
    let cleared = GoalStore::new(state.runner.sessions(), &id).clear()?;
    if let Some(goal) = &cleared {
        broadcast(state, &key, goal);
    }
    Ok(cleared.map(|g| status_of(&key, &g)))
}

/// GET /api/v1/agent/sessions/:id/goal
pub async fn get_session_goal(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<SessionGoalResponse> {
    let goal = current(&state, &id).map_err(to_error_response)?;
    Ok(Json(SessionGoalResponse { goal }))
}

/// PUT /api/v1/agent/sessions/:id/goal
pub async fn set_session_goal(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<SetGoalRequest>,
) -> HandlerResult<SessionGoalResponse> {
    let goal = set(&state, &id, &body.condition)
        .map_err(|e| to_error_response(NeboError::Validation(e)))?;
    Ok(Json(SessionGoalResponse { goal: Some(goal) }))
}

/// DELETE /api/v1/agent/sessions/:id/goal
pub async fn clear_session_goal(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<SessionGoalResponse> {
    clear(&state, &id).map_err(to_error_response)?;
    let goal = current(&state, &id).map_err(to_error_response)?;
    Ok(Json(SessionGoalResponse { goal }))
}

/// `/goal` in the chat: no argument shows the goal, `clear` clears it,
/// anything else sets it.
pub(crate) fn slash(state: &AppState, session_key: &str, args: &str) -> String {
    let args = args.trim();
    if args.is_empty() {
        return match current(state, session_key) {
            Ok(Some(g)) if g.status != "cleared" => describe(&g),
            Ok(_) => "No goal is set. `/goal <end state>` sets one; work then continues until a check confirms it's met.".to_string(),
            Err(e) => format!("Couldn't read the goal: {e}"),
        };
    }
    if args.eq_ignore_ascii_case("clear") {
        return match clear(state, session_key) {
            Ok(Some(_)) => "Goal cleared.".to_string(),
            Ok(None) => "There's no goal to clear.".to_string(),
            Err(e) => format!("Couldn't clear the goal: {e}"),
        };
    }
    match set(state, session_key, args) {
        Ok(g) => format!(
            "Goal set: {}\n\nWork continues until a separate check confirms it's met. `/goal clear` stops it.",
            g.condition
        ),
        Err(e) => e,
    }
}

fn describe(g: &SessionGoalStatus) -> String {
    let mut line = format!("Goal: {} ({} turns)", g.condition, g.turns);
    if let Some(reason) = &g.last_reason {
        line.push_str(&format!(" · last check: {reason}"));
    }
    match g.status.as_str() {
        "active" => {}
        "met" => line.push_str("\n\nMet."),
        "impossible" => line.push_str("\n\nThe check found it can't be reached."),
        _ => line.push_str("\n\nPaused. Your next message resumes it."),
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_goal_line_reads_goal_turns_and_last_check() {
        let mut g = SessionGoalStatus {
            session_id: "agent:a:web".into(),
            condition: "all invoices are sent".into(),
            turns: 2,
            last_reason: Some("\"1 of 3 sent\"".into()),
            status: "active".into(),
        };
        assert_eq!(
            describe(&g),
            "Goal: all invoices are sent (2 turns) · last check: \"1 of 3 sent\""
        );
        g.status = "paused:unmet_too_often".into();
        assert!(describe(&g).ends_with("Your next message resumes it."));
    }
}
