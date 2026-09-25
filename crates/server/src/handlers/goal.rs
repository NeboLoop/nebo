//! The owner's side of the agreed goal: `/goal` in the chat and the same
//! actions over REST, one implementation for both. Every change is
//! broadcast as `goal_status` so the thread's goal line follows it.

use agent::harness::goal::{AgreedGoal, CLEAR_WORDS, GoalObserver, GoalSource, GoalStore};
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
    let sessions = state.harness.sessions();
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

/// Where the harness tells the app about a session's agreed goal: its
/// status goes to every open thread, a kickoff starts (or joins) a turn on
/// the session, and the helpers the session started are its running work.
pub(crate) struct GoalOutlet {
    state: AppState,
}

impl GoalOutlet {
    pub(crate) fn new(state: AppState) -> Self {
        Self { state }
    }

    /// The session key of the session row `session_id`.
    fn key_of(&self, session_id: &str) -> Option<String> {
        self.state.store.get_session(session_id).ok().flatten().and_then(|s| s.name)
    }
}

impl GoalObserver for GoalOutlet {
    fn status(&self, goal: &AgreedGoal) {
        if let Some(key) = self.key_of(&goal.session_id) {
            broadcast(&self.state, &key, goal);
        }
    }

    fn kickoff(&self, goal: &AgreedGoal, prompt: String) {
        let Some(key) = self.key_of(&goal.session_id) else {
            return;
        };
        let state = self.state.clone();
        tokio::spawn(async move {
            super::ws::dispatch_hidden_prompt(&state, &key, prompt).await;
        });
    }

    fn background(&self, session_id: &str) -> Vec<agent::harness::compact::restore::RunningWork> {
        let Some(key) = self.key_of(session_id) else {
            return Vec::new();
        };
        self.state
            .helpers
            .list(&key)
            .into_iter()
            .filter(|h| h.running)
            .map(|h| agent::harness::compact::restore::RunningWork::helper(h.task_id, h.description))
            .collect()
    }
}

/// The session's goal, if it ever had one.
fn current(state: &AppState, key_or_id: &str) -> Result<Option<SessionGoalStatus>, NeboError> {
    let Some((id, key)) = session_for(state, key_or_id, false)? else {
        return Ok(None);
    };
    Ok(GoalStore::new(state.harness.sessions(), &id)
        .get()?
        .map(|g| status_of(&key, &g)))
}

/// The owner sets the goal. `Ok` carries the kickoff that starts work on
/// it; `Err` is the message the owner reads.
fn set(
    state: &AppState,
    key_or_id: &str,
    condition: &str,
) -> Result<(SessionGoalStatus, String), String> {
    let (id, key) = session_for(state, key_or_id, true)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "That conversation was not found.".to_string())?;
    let goal = GoalStore::new(state.harness.sessions(), &id)
        .set(condition, GoalSource::OwnerCommand)
        .map_err(|e| e.to_string())?;
    broadcast(state, &key, &goal);
    Ok((status_of(&key, &goal), goal.kickoff()))
}

/// The owner clears the goal. `None` when there was none being pursued.
fn clear(state: &AppState, key_or_id: &str) -> Result<Option<SessionGoalStatus>, NeboError> {
    let Some((id, key)) = session_for(state, key_or_id, false)? else {
        return Ok(None);
    };
    let cleared = GoalStore::new(state.harness.sessions(), &id).clear()?;
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

/// PUT /api/v1/agent/sessions/:id/goal — sets the goal and starts work on it.
pub async fn set_session_goal(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<SetGoalRequest>,
) -> HandlerResult<SessionGoalResponse> {
    let (goal, kickoff) = set(&state, &id, &body.condition)
        .map_err(|e| to_error_response(NeboError::Validation(e)))?;
    // Work starts on it now, as with `/goal` in the chat.
    let session_key = goal.session_id.clone();
    tokio::spawn(async move {
        super::ws::dispatch_hidden_prompt(&state, &session_key, kickoff).await;
    });
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

/// What `/goal` answers: the owner's reply, and the kickoff when a goal was
/// set.
pub(crate) struct GoalReply {
    pub text: String,
    pub kickoff: Option<String>,
}

/// The arguments of a `/goal` command, `None` when the prompt is not one.
pub(crate) fn command_args(prompt: &str) -> Option<&str> {
    let rest = prompt.trim().strip_prefix("/goal")?;
    (rest.is_empty() || rest.starts_with(char::is_whitespace)).then(|| rest.trim())
}

/// `/goal` in the chat: no argument shows the goal, a clear word clears it,
/// anything else sets it and starts work on it.
pub(crate) fn slash(state: &AppState, session_key: &str, args: &str) -> GoalReply {
    let reply = |text: String| GoalReply {
        text,
        kickoff: None,
    };
    let args = args.trim();
    if args.is_empty() {
        return reply(match current(state, session_key) {
            Ok(Some(g)) if g.status != "cleared" => describe(&g),
            Ok(_) => "No goal is set. `/goal <end state>` sets one; work starts on it and continues until a check confirms it's met.".to_string(),
            Err(e) => format!("Couldn't read the goal: {e}"),
        });
    }
    if CLEAR_WORDS.iter().any(|w| args.eq_ignore_ascii_case(w)) {
        return reply(match clear(state, session_key) {
            Ok(Some(_)) => "Goal cleared.".to_string(),
            Ok(None) => "There's no goal to clear.".to_string(),
            Err(e) => format!("Couldn't clear the goal: {e}"),
        });
    }
    match set(state, session_key, args) {
        Ok((g, kickoff)) => GoalReply {
            text: format!(
                "Goal set: {}\n\nWork continues until a separate check confirms it's met. `/goal clear` stops it.",
                g.condition
            ),
            kickoff: Some(kickoff),
        },
        Err(e) => reply(e),
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

    #[test]
    fn only_a_goal_command_is_one() {
        assert_eq!(command_args("/goal"), Some(""));
        assert_eq!(
            command_args("  /goal  all tests pass "),
            Some("all tests pass")
        );
        assert_eq!(command_args("/goal\tclear"), Some("clear"));
        assert_eq!(command_args("/goals"), None);
        assert_eq!(command_args("set a /goal"), None);
    }
}
