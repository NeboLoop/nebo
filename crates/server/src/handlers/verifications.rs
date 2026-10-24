//! Verified code work, read by the app: the badge on a reply, a run card or
//! the Workforce, and each round in run history (`crate::verification`).

use axum::extract::{Path, Query, State};
use axum::response::Json;
use serde::{Deserialize, Serialize};

use super::{HandlerResult, to_error_response};
use crate::state::AppState;

/// The badge: what the owner reads on the work.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Badge {
    /// `verified` | `not_verified` | `checking` | `waiting`
    pub state: String,
    /// Independent reviewers the work had, across its rounds.
    pub reviewers: usize,
    /// The lenses they reviewed through.
    pub lenses: Vec<String>,
    pub rounds: i64,
    /// The checks the last round ran, and whether all passed.
    pub checks: Vec<String>,
    pub checks_passed: bool,
    pub open_blockers: i64,
    pub open_majors: i64,
    pub open_minors: i64,
    /// Minors deferred with a reason.
    pub deferred: usize,
    /// Why it is not verified, or why it hasn't landed: a code the app
    /// words in the owner's language, never a sentence or a raw error.
    pub reason: Option<types::Why>,
    /// The commit it landed as; empty while it waits in its copy.
    pub landed: String,
    /// Where the work is: `landed`, `copy` (waiting in its own copy), `app`
    /// (an app's own pages: its history is the undo), `helper` (a helper's
    /// copy, landed by the conversation that started it), `folder` (made in
    /// place: a coding agent's own folder) or `nothing` (nothing to land).
    pub place: String,
    /// What the loop is doing now: `working` (the change is still being
    /// written), `checks` (the project's own checks run), `reviewers` (the
    /// reviewers read it), `fixing` (the builder fixes what they found),
    /// `live` (checking it live), `waiting` (on the owner) or `done`.
    pub phase: String,
    /// The review round it is on, or last had (0 before the first).
    pub round: usize,
    /// The project, by the name the owner knows it: an app's name, else
    /// its folder's.
    pub project: String,
}

/// How one file changed, as the app shows it.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FileView {
    pub path: String,
    /// `added` | `modified` | `deleted` | `renamed`
    pub status: String,
    pub added: u32,
    pub removed: u32,
}

/// One reviewer of a round, as the app shows it.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReviewerView {
    pub lens: String,
    /// `waiting` (the checks run first) | `running` | `clean` | `found` |
    /// `failed` (it could not finish)
    pub state: String,
    /// What it found so far.
    pub findings: usize,
}

/// One of the project's own checks in a round, as the app shows it.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CheckView {
    pub label: String,
    /// `waiting` | `running` | `passed` | `failed`
    pub state: String,
    pub duration_ms: u64,
    /// The last lines of its output, when it failed.
    pub tail: String,
}

/// One step of a verification's history, as the app shows it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoundView {
    #[serde(flatten)]
    pub round: db::VerificationRound,
    /// What the step came to: a code the app words.
    pub result: Option<types::Why>,
    /// What this step reviewed: the whole change in round 1, what changed
    /// since the last round after it.
    pub files: Vec<FileView>,
    /// Each reviewer, with its state and what it found so far.
    pub reviewers: Vec<ReviewerView>,
    /// Each of the project's own checks, with its state.
    pub check_runs: Vec<CheckView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerificationView {
    #[serde(flatten)]
    pub verification: db::Verification,
    /// The employee who did the work.
    pub agent_name: String,
    pub badge: Badge,
    /// Every step of its history, in order.
    pub history: Vec<RoundView>,
    /// The whole change being checked, file by file.
    pub changed: Vec<FileView>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerificationsQuery {
    /// The conversation's chat (its replies' badges).
    pub chat: Option<String>,
    /// The conversation (session key).
    pub session: Option<String>,
    /// A workflow run.
    pub run: Option<String>,
    /// An employee.
    pub agent: Option<String>,
    /// Only those still being checked or waiting on the owner.
    pub live: Option<bool>,
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerificationList {
    pub verifications: Vec<VerificationView>,
}

/// GET /api/v1/verifications?chat=&session=&run=&agent=&live=&limit= — newest first.
pub async fn list_verifications(
    State(state): State<AppState>,
    Query(q): Query<VerificationsQuery>,
) -> HandlerResult<VerificationList> {
    let rows = state
        .store
        .list_verifications(&db::VerificationQuery {
            chat_id: q.chat.as_deref(),
            session_key: q.session.as_deref(),
            workflow_run_id: q.run.as_deref(),
            agent_id: q.agent.as_deref(),
            live: q.live.unwrap_or(false),
            limit: q.limit.unwrap_or(100).clamp(1, 500),
            ..Default::default()
        })
        .map_err(to_error_response)?;
    Ok(Json(VerificationList {
        verifications: rows
            .into_iter()
            .map(|v| crate::verification::view(&state.store, v))
            .collect(),
    }))
}

/// GET /api/v1/verifications/{id} — one verification with every step of its history.
pub async fn get_verification(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<VerificationView> {
    let v = state
        .store
        .get_verification(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;
    Ok(Json(crate::verification::view(&state.store, v)))
}
