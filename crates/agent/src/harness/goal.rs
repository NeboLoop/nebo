//! The agreed goal: an end state the owner set (`/goal`) or approved from a
//! `suggest_goal` call. It is a stop condition, not a prompt anchor: when the
//! model answers without tool calls, a separate done check reads the
//! transcript and says whether the end state is reached, quoting the
//! transcript. Not met, the turn continues with the reason; met, impossible
//! or paused, it ends.
//!
//! Setting a goal starts work on it at once: a kickoff turn (or, while a turn
//! runs, a kickoff queued into it). The goal line the owner sees is UI only;
//! the model hears of a goal through its kickoff and its checks, never on
//! every call, and a clear is never told to it.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ai::{ChatRequest, Message, Provider, ProviderError, RequestTrace, StreamEvent, StreamEventType};
use tokio::sync::mpsc;
use tracing::{info, warn};
use types::NeboError;

use super::events::{TurnEvent, attachment_for};
use super::turn::TurnExit;
use super::turn_end::{EndCheck, EndVerdict, TurnEnd};
use crate::session::SessionManager;

/// The longest goal the owner may set with `/goal`.
pub const MAX_OWNER_CONDITION_CHARS: usize = 4_000;

/// The longest goal a suggestion may carry: the owner reads all of it on the
/// approval card.
pub const MAX_SUGGESTED_CONDITION_CHARS: usize = 500;

/// What `/goal <word>` clears the goal with.
pub const CLEAR_WORDS: &[&str] = &["clear", "stop", "off", "reset", "none", "cancel"];

/// Unmet checks in one turn before the goal pauses.
pub const UNMET_CHECKS_BEFORE_PAUSE: u8 = 8;

/// How long one done check may take before the goal pauses.
pub const DONE_CHECK_DEADLINE: Duration = Duration::from_secs(30);

/// The first check-in while background work holds the check back; each
/// later one waits twice as long, up to four times this.
pub const CHECK_IN_AFTER: Duration = Duration::from_secs(30 * 60);

/// Check-ins while background work runs before they stop.
pub const MAX_CHECK_INS: u32 = 3;

/// The name the goal check's continue reminder carries.
pub const GOAL_CHECK: &str = "goal_check";

/// Share of the check model's window the transcript may fill, and the share
/// a retry after an overflow uses.
const TRANSCRIPT_SHARE: f64 = 0.5;
const OVERFLOW_RETRY_SHARE: f64 = 0.25;
/// The window assumed when the check model's is not known.
const DEFAULT_WINDOW_TOKENS: usize = 80_000;
const MAX_VERDICT_TOKENS: i32 = 400;

/// A session's agreed goal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgreedGoal {
    pub session_id: String,
    pub condition: String,
    pub source: GoalSource,
    pub status: GoalStatus,
    /// Done checks that found the goal unmet.
    pub turns: u32,
    pub last_reason: Option<String>,
    /// Conditions the owner declined; never suggested again.
    pub declined: Vec<String>,
}

impl AgreedGoal {
    fn from_row(row: db::SessionGoal) -> Self {
        Self {
            session_id: row.session_id,
            condition: row.condition,
            source: GoalSource::parse(&row.source),
            status: GoalStatus::parse(&row.status),
            turns: row.turns.max(0) as u32,
            last_reason: row.last_reason,
            declined: row.declined,
        }
    }

    pub fn is_active(&self) -> bool {
        self.status == GoalStatus::Active
    }

    /// Whether the owner declined `condition` (case and spacing aside).
    pub fn was_declined(&self, condition: &str) -> bool {
        let wanted = normalized(condition);
        self.declined.iter().any(|d| normalized(d) == wanted)
    }

    /// The hidden prompt that starts work on the goal: its `goal_set` text.
    pub fn kickoff(&self) -> String {
        attachment_for(&TurnEvent::GoalSet(self.condition.clone()))
            .map(|a| a.text)
            .unwrap_or_default()
    }
}

/// How the goal came to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalSource {
    OwnerCommand,
    SuggestedApproved,
    OwnersOwnWords,
}

impl GoalSource {
    pub fn as_str(self) -> &'static str {
        match self {
            GoalSource::OwnerCommand => "owner_command",
            GoalSource::SuggestedApproved => "suggested_approved",
            GoalSource::OwnersOwnWords => "owners_own_words",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "suggested_approved" => GoalSource::SuggestedApproved,
            "owners_own_words" => GoalSource::OwnersOwnWords,
            _ => GoalSource::OwnerCommand,
        }
    }

    /// The longest condition this source may set.
    fn max_chars(self) -> usize {
        match self {
            GoalSource::OwnerCommand => MAX_OWNER_CONDITION_CHARS,
            GoalSource::SuggestedApproved | GoalSource::OwnersOwnWords => {
                MAX_SUGGESTED_CONDITION_CHARS
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalStatus {
    Active,
    Met,
    Impossible,
    Paused(Pause),
    Cleared,
}

impl GoalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            GoalStatus::Active => "active",
            GoalStatus::Met => "met",
            GoalStatus::Impossible => "impossible",
            GoalStatus::Paused(Pause::CheckUnavailable) => "paused:check_unavailable",
            GoalStatus::Paused(Pause::UnmetTooOften) => "paused:unmet_too_often",
            GoalStatus::Paused(Pause::LimitReached) => "paused:limit_reached",
            GoalStatus::Paused(Pause::Stopped) => "paused:stopped",
            GoalStatus::Cleared => "cleared",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "active" => GoalStatus::Active,
            "met" => GoalStatus::Met,
            "impossible" => GoalStatus::Impossible,
            "paused:check_unavailable" => GoalStatus::Paused(Pause::CheckUnavailable),
            "paused:unmet_too_often" => GoalStatus::Paused(Pause::UnmetTooOften),
            "paused:limit_reached" => GoalStatus::Paused(Pause::LimitReached),
            "paused:stopped" => GoalStatus::Paused(Pause::Stopped),
            _ => GoalStatus::Cleared,
        }
    }
}

/// Why the goal stopped being pursued. Every pause resumes on the owner's
/// next message ([`GoalStore::resume`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pause {
    /// The done check did not answer within [`DONE_CHECK_DEADLINE`], or
    /// failed twice.
    CheckUnavailable,
    /// [`UNMET_CHECKS_BEFORE_PAUSE`] checks in one turn found it unmet.
    UnmetTooOften,
    /// The turn hit its step or spend limit.
    LimitReached,
    /// The turn was stopped.
    Stopped,
}

/// The done check's answer; `reason` quotes the transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalVerdict {
    pub met: bool,
    pub impossible: bool,
    pub reason: String,
}

/// Why a goal could not be set.
#[derive(Debug, thiserror::Error)]
pub enum GoalError {
    #[error("The goal is empty. Say the end state the work should reach.")]
    Empty,
    #[error("The goal is {0} characters; keep it to {1}.")]
    TooLong(usize, usize),
    #[error(transparent)]
    Store(#[from] NeboError),
}

/// What the goal tells the rest of the app about one session.
pub trait GoalObserver: Send + Sync {
    /// The goal's status changed: show it to the owner.
    fn status(&self, goal: &AgreedGoal);
    /// Start a turn on the session with this hidden prompt, or queue it into
    /// the turn that is running.
    fn kickoff(&self, goal: &AgreedGoal, prompt: String);
    /// The helpers and background work the session has running.
    fn background(&self) -> Vec<String>;
}

/// One session's goal over the `session_goals` table. Storage only: the
/// caller starts the kickoff and tells the owner.
pub struct GoalStore<'a> {
    sessions: &'a SessionManager,
    session_id: &'a str,
}

impl<'a> GoalStore<'a> {
    pub fn new(sessions: &'a SessionManager, session_id: &'a str) -> Self {
        Self {
            sessions,
            session_id,
        }
    }

    fn store(&self) -> &db::Store {
        self.sessions.store()
    }

    pub fn get(&self) -> Result<Option<AgreedGoal>, NeboError> {
        Ok(self
            .store()
            .get_session_goal(self.session_id)?
            .map(AgreedGoal::from_row))
    }

    /// The goal while it is being worked toward.
    pub fn active(&self) -> Result<Option<AgreedGoal>, NeboError> {
        Ok(self.get()?.filter(AgreedGoal::is_active))
    }

    /// Make `condition` the session's goal, replacing any earlier one (one
    /// goal at a time).
    pub fn set(&self, condition: &str, source: GoalSource) -> Result<AgreedGoal, GoalError> {
        let condition = valid_condition(condition, source.max_chars())?;
        let row = self.store().put_session_goal(
            self.session_id,
            &condition,
            source.as_str(),
            GoalStatus::Active.as_str(),
        )?;
        Ok(AgreedGoal::from_row(row))
    }

    /// Clear the goal. `None` when there was none being pursued.
    pub fn clear(&self) -> Result<Option<AgreedGoal>, NeboError> {
        let pursued = self
            .get()?
            .filter(|g| matches!(g.status, GoalStatus::Active | GoalStatus::Paused(_)));
        if pursued.is_none() {
            return Ok(None);
        }
        self.record_check(GoalStatus::Cleared, None, false)
    }

    /// Record a done check or a pause: the new status, the check's reason,
    /// and one more unmet turn when `unmet`.
    pub fn record_check(
        &self,
        status: GoalStatus,
        reason: Option<&str>,
        unmet: bool,
    ) -> Result<Option<AgreedGoal>, NeboError> {
        Ok(self
            .store()
            .update_session_goal_status(self.session_id, status.as_str(), reason, unmet)?
            .map(AgreedGoal::from_row))
    }

    /// The owner declined `condition`; it is never suggested again.
    pub fn record_decline(&self, condition: &str) -> Result<(), NeboError> {
        self.store()
            .add_session_goal_decline(self.session_id, condition.trim())
    }

    /// The owner's next message resumes a paused goal.
    pub fn resume(&self) -> Result<Option<AgreedGoal>, NeboError> {
        match self.get()? {
            Some(g) if matches!(g.status, GoalStatus::Paused(_)) => {
                self.record_check(GoalStatus::Active, None, false)
            }
            _ => Ok(None),
        }
    }
}

fn valid_condition(condition: &str, max_chars: usize) -> Result<String, GoalError> {
    let condition = condition.trim();
    if condition.is_empty() {
        return Err(GoalError::Empty);
    }
    let chars = condition.chars().count();
    if chars > max_chars {
        return Err(GoalError::TooLong(chars, max_chars));
    }
    Ok(condition.to_string())
}

fn normalized(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// The model the done check runs on: the aux route, or the cheapest
/// provider when none is routed.
#[derive(Clone)]
pub struct DoneJudge {
    pub provider: Arc<dyn Provider>,
    /// Empty = the provider's default model.
    pub model: String,
    /// The model's context window, in tokens.
    pub window_tokens: usize,
}

impl DoneJudge {
    pub fn for_providers(providers: &[Arc<dyn Provider>]) -> Option<Self> {
        let cfg = config::ModelsConfig::load();
        let (provider, model) = match super::model_call::resolve_aux(&cfg, providers) {
            Some(routed) => routed,
            None => (crate::summarizer::pick_cheapest(providers)?, String::new()),
        };
        let window_tokens = cfg
            .providers
            .get(provider.id())
            .and_then(|models| models.iter().find(|m| m.id == model))
            .map(|m| m.context_window.max(0) as usize)
            .filter(|&w| w > 0)
            .unwrap_or(DEFAULT_WINDOW_TOKENS);
        Some(Self {
            provider,
            model,
            window_tokens,
        })
    }
}

const DONE_CHECK_SYSTEM: &str = "You check whether an agreed goal has been reached. Read the conversation transcript, then judge from the transcript alone whether the goal's end state is met.

Answer with one JSON object and nothing else, in one of these shapes:
- {\"met\": true, \"reason\": \"<quote the transcript text that shows the end state was reached>\"}
- {\"met\": false, \"reason\": \"<quote what is missing or what stands in the way>\"}
- {\"met\": false, \"impossible\": true, \"reason\": \"<why the end state can never be reached in this conversation>\"}

Always give a reason, quoting the transcript wherever you can. When the transcript holds no clear evidence that the end state was reached, answer {\"met\": false, \"reason\": \"the transcript does not show it yet\"}.

Use impossible only when the end state truly cannot be reached here: it contradicts itself, it needs something that is not available, or the work has tried every reasonable way and said it cannot be done. The assistant saying it is impossible is evidence, not proof; judge it yourself. Slow progress or unfinished work is not impossible. When unsure, leave impossible out.";

/// What one done-check call came back with.
enum Answer {
    Verdict(GoalVerdict),
    /// The transcript did not fit the check model's window.
    Overflow,
    /// An error, or an answer that was not the JSON asked for.
    Failed,
    /// No answer within the deadline.
    TimedOut,
}

/// Run the done check over the transcript's real messages, as much of the
/// newest as fits half the check model's window. An error or an answer that
/// is not the JSON asked for is tried once more; an overflow is tried once
/// more with a quarter of the window. `None` = the check was unavailable
/// (no answer within [`DONE_CHECK_DEADLINE`], or both tries failed); the
/// goal pauses.
pub async fn check_goal(
    judge: &DoneJudge,
    trace: RequestTrace,
    transcript: &[Message],
    goal: &AgreedGoal,
) -> Option<GoalVerdict> {
    let mut share = TRANSCRIPT_SHARE;
    for _ in 0..2 {
        let budget = (judge.window_tokens as f64 * share) as usize;
        match ask(judge, trace.clone(), transcript, goal, budget).await {
            Answer::Verdict(v) => return Some(v),
            Answer::TimedOut => {
                warn!(condition = %goal.condition, "done check: no answer within the deadline");
                return None;
            }
            Answer::Overflow => share = OVERFLOW_RETRY_SHARE,
            Answer::Failed => {}
        }
    }
    None
}

async fn ask(
    judge: &DoneJudge,
    trace: RequestTrace,
    transcript: &[Message],
    goal: &AgreedGoal,
    budget_tokens: usize,
) -> Answer {
    let mut messages = fit_transcript(transcript, budget_tokens);
    messages.push(Message {
        role: "user".to_string(),
        content: format!(
            "From the conversation above alone: has this agreed goal been reached? Answer from the transcript's evidence only.\n\nGoal: {}",
            goal.condition
        ),
        ..Default::default()
    });
    let req = ChatRequest {
        messages,
        max_tokens: MAX_VERDICT_TOKENS,
        system: DONE_CHECK_SYSTEM.to_string(),
        model: judge.model.clone(),
        ..ChatRequest::new(trace)
    };
    let answer = tokio::time::timeout(DONE_CHECK_DEADLINE, async {
        let mut rx = match judge.provider.stream(&req).await {
            Ok(rx) => rx,
            Err(ProviderError::ContextOverflow) => return Answer::Overflow,
            Err(_) => return Answer::Failed,
        };
        let mut text = String::new();
        while let Some(ev) = rx.recv().await {
            match ev.event_type {
                StreamEventType::Text => text.push_str(&ev.text),
                StreamEventType::Error => return Answer::Failed,
                StreamEventType::Done => break,
                _ => {}
            }
        }
        parse_verdict(&text).map_or(Answer::Failed, Answer::Verdict)
    })
    .await;
    answer.unwrap_or(Answer::TimedOut)
}

fn parse_verdict(text: &str) -> Option<GoalVerdict> {
    #[derive(serde::Deserialize)]
    struct Raw {
        met: bool,
        #[serde(default)]
        impossible: bool,
        #[serde(default)]
        reason: String,
    }
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    let raw: Raw = serde_json::from_str(text.get(start..=end)?).ok()?;
    let reason = raw.reason.trim();
    Some(GoalVerdict {
        met: raw.met,
        impossible: !raw.met && raw.impossible,
        reason: if reason.is_empty() {
            "the transcript does not show it yet".to_string()
        } else {
            reason.to_string()
        },
    })
}

/// A message's size in tokens, roughly (4 characters each).
fn message_tokens(m: &Message) -> usize {
    let json = |v: &Option<serde_json::Value>| v.as_ref().map_or(0, |v| v.to_string().len());
    (m.content.len() + json(&m.tool_calls) + json(&m.tool_results)).div_ceil(4)
}

/// The transcript's newest messages that fit `budget_tokens`, whole, oldest
/// left out first. A kept tool result never goes without its call; a note
/// leads when anything was left out.
fn fit_transcript(transcript: &[Message], budget_tokens: usize) -> Vec<Message> {
    let mut used = 0usize;
    let mut start = transcript.len();
    while start > 0 {
        let size = message_tokens(&transcript[start - 1]);
        if used + size > budget_tokens {
            break;
        }
        used += size;
        start -= 1;
    }
    while start < transcript.len() && transcript[start].tool_results.is_some() {
        start += 1;
    }
    let mut kept = Vec::with_capacity(transcript.len() - start + 1);
    if start > 0 {
        kept.push(Message {
            role: "user".to_string(),
            content: format!(
                "[The {start} earliest messages of this conversation were left out to fit. If the evidence may be in them, answer not met.]"
            ),
            ..Default::default()
        });
    }
    kept.extend_from_slice(&transcript[start..]);
    kept
}

/// The check-ins while background work holds a session's goal check back:
/// one timer per session at a time, backed off, at most [`MAX_CHECK_INS`].
#[derive(Clone, Default)]
pub struct CheckIns {
    sessions: Arc<Mutex<HashMap<String, CheckInState>>>,
}

#[derive(Default)]
struct CheckInState {
    sent: u32,
    timer: Option<tokio::task::AbortHandle>,
}

/// How long check-in `n` (0-based) waits: doubling, up to four times the
/// first.
pub fn check_in_delay(n: u32) -> Duration {
    CHECK_IN_AFTER * 2u32.pow(n.min(2))
}

impl CheckIns {
    /// Schedule the next check-in for the session unless one is waiting or
    /// all have been sent. When it fires and the goal is still active, the
    /// observer kicks off a turn that lists what is still running.
    fn defer(&self, sessions: &SessionManager, session_id: &str, observer: Arc<dyn GoalObserver>) {
        let mut all = self.sessions.lock().unwrap();
        let state = all.entry(session_id.to_string()).or_default();
        if state.timer.is_some() || state.sent >= MAX_CHECK_INS {
            return;
        }
        let delay = check_in_delay(state.sent);
        state.sent += 1;
        let last = state.sent >= MAX_CHECK_INS;
        let this = self.clone();
        let sessions = sessions.clone();
        let session_id = session_id.to_string();
        let handle = tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            if let Some(state) = this.sessions.lock().unwrap().get_mut(&session_id) {
                state.timer = None;
            }
            let Ok(Some(goal)) = GoalStore::new(&sessions, &session_id).active() else {
                return;
            };
            let prompt = check_in_prompt(&goal.condition, delay, &observer.background(), last);
            observer.kickoff(&goal, prompt);
        });
        state.timer = Some(handle.abort_handle());
    }

    /// The goal was checked (or is gone): check-ins start over.
    fn reset(&self, session_id: &str) {
        if let Some(state) = self.sessions.lock().unwrap().remove(session_id)
            && let Some(timer) = state.timer
        {
            timer.abort();
        }
    }
}

fn check_in_prompt(condition: &str, waited: Duration, running: &[String], last: bool) -> String {
    let minutes = (waited.as_secs() / 60).max(1);
    let mut text = if running.is_empty() {
        format!(
            "The agreed goal is still active: {condition}. Its check waited {minutes} min for background work, which is no longer running (it finished or stopped without reporting back). Continue toward the goal."
        )
    } else {
        let list: Vec<String> = running.iter().map(|r| format!("- {r}")).collect();
        format!(
            "The agreed goal is still active: {condition}. Its check has waited {minutes} min because background work is still running:\n{}\nCheck on its progress. If it is progressing, say so briefly and keep waiting; if it is stuck or no longer needed, fix or stop it and continue toward the goal.",
            list.join("\n")
        )
    };
    if last && !running.is_empty() {
        text.push_str(" This is the last check-in while it runs: decide now whether to keep waiting or to stop it and finish another way.");
    }
    text
}

/// The agreed-goal end check: registered for chat turns; runs the done check
/// only while the session has an active goal.
pub struct GoalCheck {
    pub sessions: SessionManager,
    pub session_id: String,
    /// `None` = no model to check with; an active goal pauses.
    pub judge: Option<DoneJudge>,
    pub trace: RequestTrace,
    pub observer: Arc<dyn GoalObserver>,
    pub check_ins: CheckIns,
}

impl GoalCheck {
    fn goals(&self) -> GoalStore<'_> {
        GoalStore::new(&self.sessions, &self.session_id)
    }

    fn record(&self, status: GoalStatus, reason: Option<&str>, unmet: bool) {
        match self.goals().record_check(status, reason, unmet) {
            Ok(Some(goal)) => self.observer.status(&goal),
            Ok(None) => {}
            Err(e) => {
                warn!(session_id = %self.session_id, error = %e, "goal: recording the check failed")
            }
        }
    }

    fn pause(&self, why: Pause, reason: Option<&str>, unmet: bool) -> EndVerdict {
        self.record(GoalStatus::Paused(why), reason, unmet);
        EndVerdict::Exit(TurnExit::GoalPaused(why))
    }
}

#[async_trait::async_trait]
impl EndCheck for GoalCheck {
    fn name(&self) -> &'static str {
        GOAL_CHECK
    }

    async fn check(&self, end: &TurnEnd<'_>) -> EndVerdict {
        let goal = match self.goals().active() {
            Ok(Some(goal)) => goal,
            Ok(None) => {
                self.check_ins.reset(&self.session_id);
                return EndVerdict::Stop;
            }
            Err(e) => {
                warn!(session_id = %self.session_id, error = %e, "goal: loading the goal failed");
                return EndVerdict::Stop;
            }
        };
        // Helpers or background work still running: nothing is judged on a
        // transcript still waiting on them. Their completion wakes the
        // session; until then the goal checks in at backed-off intervals.
        if !self.observer.background().is_empty() {
            info!(session_id = %self.session_id, "goal: check deferred, background work is running");
            self.check_ins
                .defer(&self.sessions, &self.session_id, self.observer.clone());
            return EndVerdict::Stop;
        }
        self.check_ins.reset(&self.session_id);
        let Some(judge) = &self.judge else {
            return self.pause(Pause::CheckUnavailable, None, false);
        };
        let Some(verdict) = check_goal(judge, self.trace.clone(), end.transcript, &goal).await
        else {
            return self.pause(Pause::CheckUnavailable, None, false);
        };
        let reason = verdict.reason;
        if verdict.met {
            self.record(GoalStatus::Met, Some(&reason), false);
            return EndVerdict::Exit(TurnExit::GoalMet { reason });
        }
        if verdict.impossible {
            self.record(GoalStatus::Impossible, Some(&reason), false);
            return EndVerdict::Exit(TurnExit::GoalImpossible { reason });
        }
        if end.checks_this_turn + 1 >= UNMET_CHECKS_BEFORE_PAUSE {
            return self.pause(Pause::UnmetTooOften, Some(&reason), true);
        }
        self.record(GoalStatus::Active, Some(&reason), true);
        EndVerdict::Continue(TurnEvent::GoalCheck {
            reason,
            condition: goal.condition,
        })
    }
}

/// What the model sends with a `suggest_goal` call (Tools-Rewrite §3.4 B).
#[derive(Debug, Clone, serde::Deserialize)]
pub struct SuggestInput {
    /// The end state, stated so a separate check can confirm it from the
    /// conversation.
    pub condition: String,
    /// Ask the owner to approve. False only when the owner's own words in
    /// this conversation stated this outcome.
    #[serde(default = "ask_by_default")]
    pub ask_owner: bool,
}

fn ask_by_default() -> bool {
    true
}

/// Where a suggestion's approval card goes and whom a new goal is told to.
pub struct SuggestContext<'a> {
    pub sessions: &'a SessionManager,
    pub session_id: &'a str,
    /// The `suggest_goal` call: its id is the card's request id.
    pub call: &'a ai::ToolCall,
    pub approvals: &'a tools::ApprovalChannels,
    pub events: &'a mpsc::Sender<StreamEvent>,
    pub observer: Arc<dyn GoalObserver>,
}

/// The `suggest_goal` behaviour. Holds the sessions with a card waiting on
/// the owner, so a session has one suggestion out at a time.
#[derive(Clone, Default)]
pub struct Suggestions {
    waiting: Arc<Mutex<HashSet<String>>>,
}

/// Set an approved or owner-stated goal, show it, and kick work off on it.
fn adopt(
    sessions: &SessionManager,
    session_id: &str,
    condition: &str,
    source: GoalSource,
    observer: &dyn GoalObserver,
) -> Result<AgreedGoal, GoalError> {
    let goal = GoalStore::new(sessions, session_id).set(condition, source)?;
    observer.status(&goal);
    observer.kickoff(&goal, goal.kickoff());
    Ok(goal)
}

impl Suggestions {
    /// Handle one `suggest_goal` call. `Ok` is the tool result the model
    /// reads; `Err` is a refusal it can correct. With `ask_owner` the card
    /// goes out and the call returns at once: approved, the goal is set and
    /// its kickoff reaches the model; declined, the condition is remembered
    /// and never suggested again, and the model is not told.
    pub async fn suggest(
        &self,
        cx: SuggestContext<'_>,
        input: SuggestInput,
    ) -> Result<String, String> {
        let condition = valid_condition(&input.condition, MAX_SUGGESTED_CONDITION_CHARS)
            .map_err(|e| e.to_string())?;
        let current = GoalStore::new(cx.sessions, cx.session_id)
            .get()
            .map_err(|e| e.to_string())?;
        if current.as_ref().is_some_and(|g| g.was_declined(&condition)) {
            return Err("The owner declined this goal. Don't suggest it again.".to_string());
        }
        if current
            .as_ref()
            .is_some_and(|g| g.is_active() && normalized(&g.condition) == normalized(&condition))
        {
            return Ok("That is already the agreed goal. Keep working toward it.".to_string());
        }
        if !input.ask_owner {
            adopt(
                cx.sessions,
                cx.session_id,
                &condition,
                GoalSource::OwnersOwnWords,
                cx.observer.as_ref(),
            )
            .map_err(|e| e.to_string())?;
            return Ok("The goal is set, without asking, because the owner's own words stated it. The owner sees it and can clear it with /goal clear. Keep working; its kickoff follows.".to_string());
        }
        if !self
            .waiting
            .lock()
            .unwrap()
            .insert(cx.session_id.to_string())
        {
            return Err("A goal suggestion is already waiting on the owner. Keep working; if they approve it, you will be told.".to_string());
        }
        let card = ai::ToolCall {
            id: cx.call.id.clone(),
            name: cx.call.name.clone(),
            input: serde_json::json!({ "condition": condition }),
        };
        let (answer_tx, answer_rx) = tokio::sync::oneshot::channel();
        cx.approvals.lock().await.insert(card.id.clone(), answer_tx);
        let _ = cx.events.send(StreamEvent::approval_request(card)).await;

        let waiting = self.waiting.clone();
        let sessions = cx.sessions.clone();
        let session_id = cx.session_id.to_string();
        let observer = cx.observer.clone();
        tokio::spawn(async move {
            let answer = answer_rx.await;
            waiting.lock().unwrap().remove(&session_id);
            match answer.as_deref() {
                Ok("once") | Ok("always") => {
                    if let Err(e) = adopt(
                        &sessions,
                        &session_id,
                        &condition,
                        GoalSource::SuggestedApproved,
                        observer.as_ref(),
                    ) {
                        warn!(session_id, error = %e, "goal: setting the approved goal failed");
                    }
                }
                Ok(_) => {
                    if let Err(e) = GoalStore::new(&sessions, &session_id).record_decline(&condition) {
                        warn!(session_id, error = %e, "goal: recording the decline failed");
                    }
                }
                // The card went away unanswered: neither set nor declined.
                Err(_) => {}
            }
        });
        Ok("The goal is on a card for the owner to approve. Keep working; don't wait for their answer. If they approve it, you will be told the goal is set; until then no new goal applies. If they decline, you won't be told; don't ask about it or suggest it again.".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ai::EventReceiver;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// How the scripted check model answers one call.
    #[derive(Clone, Copy)]
    enum Reply {
        Text(&'static str),
        /// Hold the stream open past the deadline.
        Silent,
        Overflow,
        Down,
    }

    /// Answers each done check with the next scripted reply. Keeps the
    /// requests it saw.
    struct Judge {
        replies: Mutex<Vec<Reply>>,
        calls: AtomicUsize,
        seen: Mutex<Vec<ChatRequest>>,
    }

    impl Judge {
        fn new(replies: Vec<Reply>) -> Arc<Self> {
            Arc::new(Self {
                replies: Mutex::new(replies),
                calls: AtomicUsize::new(0),
                seen: Mutex::new(Vec::new()),
            })
        }
    }

    #[async_trait::async_trait]
    impl Provider for Judge {
        fn id(&self) -> &str {
            "judge"
        }
        async fn stream(&self, req: &ChatRequest) -> Result<EventReceiver, ProviderError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.seen.lock().unwrap().push(req.clone());
            let reply = self.replies.lock().unwrap().remove(0);
            let (tx, rx) = mpsc::channel(4);
            match reply {
                Reply::Text(text) => {
                    let _ = tx.send(StreamEvent::text(text)).await;
                    let _ = tx.send(StreamEvent::done()).await;
                }
                Reply::Silent => {
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_secs(3600)).await;
                        drop(tx);
                    });
                }
                Reply::Overflow => return Err(ProviderError::ContextOverflow),
                Reply::Down => return Err(ProviderError::Request("down".into())),
            }
            Ok(rx)
        }
    }

    /// Records what the goal told the app; `running` is the background work.
    #[derive(Default)]
    struct Seen {
        statuses: Mutex<Vec<AgreedGoal>>,
        kickoffs: Mutex<Vec<String>>,
        running: Mutex<Vec<String>>,
    }

    impl GoalObserver for Seen {
        fn status(&self, goal: &AgreedGoal) {
            self.statuses.lock().unwrap().push(goal.clone());
        }
        fn kickoff(&self, _goal: &AgreedGoal, prompt: String) {
            self.kickoffs.lock().unwrap().push(prompt);
        }
        fn background(&self) -> Vec<String> {
            self.running.lock().unwrap().clone()
        }
    }

    fn session() -> (SessionManager, String) {
        let path = std::env::temp_dir().join(format!("nebo-goal-{}.db", uuid::Uuid::new_v4()));
        let store = Arc::new(db::Store::new(path.to_str().unwrap()).expect("store"));
        let sessions = SessionManager::new(store);
        let id = sessions.get_or_create("agent:a1:web", "").expect("session").id;
        (sessions, id)
    }

    fn judged_by(judge: &Arc<Judge>, window_tokens: usize) -> DoneJudge {
        DoneJudge {
            provider: judge.clone(),
            model: String::new(),
            window_tokens,
        }
    }

    fn goal_check(sessions: &SessionManager, id: &str, judge: &Arc<Judge>) -> (GoalCheck, Arc<Seen>) {
        let seen = Arc::new(Seen::default());
        (
            GoalCheck {
                sessions: sessions.clone(),
                session_id: id.to_string(),
                judge: Some(judged_by(judge, DEFAULT_WINDOW_TOKENS)),
                trace: RequestTrace::new("goal_check"),
                observer: seen.clone(),
                check_ins: CheckIns::default(),
            },
            seen,
        )
    }

    fn end(transcript: &[Message], checks_this_turn: u8) -> TurnEnd<'_> {
        TurnEnd {
            transcript,
            step: 1,
            checks_this_turn,
        }
    }

    fn said(role: &str, text: &str) -> Message {
        Message {
            role: role.into(),
            content: text.into(),
            ..Default::default()
        }
    }

    fn set(sessions: &SessionManager, id: &str, condition: &str) -> AgreedGoal {
        GoalStore::new(sessions, id)
            .set(condition, GoalSource::OwnerCommand)
            .unwrap()
    }

    const UNMET: Reply = Reply::Text("{\"met\": false, \"reason\": \"\\\"4 left\\\"\"}");

    #[tokio::test]
    async fn no_goal_means_no_end_check_call() {
        let (sessions, id) = session();
        let judge = Judge::new(vec![]);
        let (check, seen) = goal_check(&sessions, &id, &judge);
        assert!(matches!(check.check(&end(&[], 0)).await, EndVerdict::Stop));

        // A cleared goal is no goal either.
        set(&sessions, &id, "the report is sent");
        GoalStore::new(&sessions, &id).clear().unwrap();
        assert!(matches!(check.check(&end(&[], 0)).await, EndVerdict::Stop));
        assert_eq!(judge.calls.load(Ordering::SeqCst), 0);
        assert!(seen.statuses.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn unmet_goal_continues_with_quoted_reason() {
        let (sessions, id) = session();
        set(&sessions, &id, "all three invoices are sent");
        let judge = Judge::new(vec![Reply::Text(
            "{\"met\": false, \"reason\": \"\\\"Sent invoice 1 of 3\\\" - two are still unsent\"}",
        )]);
        let (check, seen) = goal_check(&sessions, &id, &judge);
        let transcript = [
            said("user", "Send the three invoices."),
            said("assistant", "Sent invoice 1 of 3."),
        ];
        let EndVerdict::Continue(event) = check.check(&end(&transcript, 0)).await else {
            panic!("an unmet goal continues the turn");
        };
        let row = attachment_for(&event).unwrap();
        assert_eq!(row.kind, GOAL_CHECK);
        assert_eq!(
            row.text,
            "The agreed goal isn't met yet: \"Sent invoice 1 of 3\" - two are still unsent. Keep working toward: all three invoices are sent."
        );

        // The check read the real messages, then the question; no tools.
        let req = judge.seen.lock().unwrap()[0].clone();
        assert!(req.tools.is_empty());
        assert_eq!(req.messages.len(), 3);
        assert_eq!(req.messages[0].content, "Send the three invoices.");
        assert_eq!(req.messages[1].content, "Sent invoice 1 of 3.");
        assert!(req.messages[2].content.ends_with("Goal: all three invoices are sent"));

        let goal = GoalStore::new(&sessions, &id).get().unwrap().unwrap();
        assert_eq!(goal.status, GoalStatus::Active);
        assert_eq!(goal.turns, 1);
        assert_eq!(
            goal.last_reason.as_deref(),
            Some("\"Sent invoice 1 of 3\" - two are still unsent")
        );
        assert_eq!(seen.statuses.lock().unwrap().len(), 1, "the owner sees the bumped count");
    }

    #[tokio::test]
    async fn met_and_impossible_end_the_turn() {
        let (sessions, id) = session();
        set(&sessions, &id, "the site is live");
        let judge = Judge::new(vec![
            Reply::Text("Here: {\"met\": true, \"reason\": \"\\\"Deployed to example.com\\\"\"}"),
            Reply::Text("{\"met\": false, \"impossible\": true, \"reason\": \"no host account exists\"}"),
        ]);
        let (check, _) = goal_check(&sessions, &id, &judge);
        let EndVerdict::Exit(exit) = check.check(&end(&[], 0)).await else {
            panic!("met ends the turn");
        };
        assert_eq!(
            exit,
            TurnExit::GoalMet {
                reason: "\"Deployed to example.com\"".into()
            }
        );
        let goals = GoalStore::new(&sessions, &id);
        assert_eq!(goals.get().unwrap().unwrap().status, GoalStatus::Met);

        set(&sessions, &id, "the site is live");
        assert!(matches!(
            check.check(&end(&[], 0)).await,
            EndVerdict::Exit(TurnExit::GoalImpossible { .. })
        ));
        assert_eq!(goals.get().unwrap().unwrap().status, GoalStatus::Impossible);
    }

    #[tokio::test(start_paused = true)]
    async fn unavailable_check_pauses_and_ends_the_turn() {
        let (sessions, id) = session();
        let goals = GoalStore::new(&sessions, &id);
        set(&sessions, &id, "the backlog is empty");
        // No answer within the 30 s deadline: paused at once, no retry.
        let judge = Judge::new(vec![Reply::Silent, Reply::Down, Reply::Text("I think so")]);
        let (check, seen) = goal_check(&sessions, &id, &judge);
        let started = tokio::time::Instant::now();
        assert!(matches!(
            check.check(&end(&[], 0)).await,
            EndVerdict::Exit(TurnExit::GoalPaused(Pause::CheckUnavailable))
        ));
        assert_eq!(started.elapsed(), DONE_CHECK_DEADLINE);
        assert_eq!(judge.calls.load(Ordering::SeqCst), 1);
        let paused = goals.get().unwrap().unwrap();
        assert_eq!(paused.status, GoalStatus::Paused(Pause::CheckUnavailable));
        assert_eq!(seen.statuses.lock().unwrap().last().unwrap().status, paused.status);

        // A paused goal is not checked; the owner's next message resumes it.
        assert!(matches!(check.check(&end(&[], 0)).await, EndVerdict::Stop));
        assert_eq!(goals.resume().unwrap().unwrap().status, GoalStatus::Active);

        // An error, then an answer that isn't the JSON: tried twice, paused.
        assert!(matches!(
            check.check(&end(&[], 0)).await,
            EndVerdict::Exit(TurnExit::GoalPaused(Pause::CheckUnavailable))
        ));
        assert_eq!(judge.calls.load(Ordering::SeqCst), 3);

        // No model to check with pauses the same way, without a call.
        goals.resume().unwrap();
        let (mut no_judge, _) = goal_check(&sessions, &id, &judge);
        no_judge.judge = None;
        assert!(matches!(
            no_judge.check(&end(&[], 0)).await,
            EndVerdict::Exit(TurnExit::GoalPaused(Pause::CheckUnavailable))
        ));
        assert_eq!(judge.calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn a_failed_check_is_tried_once_more() {
        let (sessions, id) = session();
        set(&sessions, &id, "the backlog is empty");
        let judge = Judge::new(vec![Reply::Down, UNMET]);
        let (check, _) = goal_check(&sessions, &id, &judge);
        assert!(matches!(check.check(&end(&[], 0)).await, EndVerdict::Continue(_)));
        assert_eq!(judge.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn eight_unmet_checks_pause() {
        let (sessions, id) = session();
        set(&sessions, &id, "every call site is migrated");
        let judge = Judge::new(vec![UNMET; 8]);
        let (check, _) = goal_check(&sessions, &id, &judge);
        for n in 0..7 {
            assert!(
                matches!(check.check(&end(&[], n)).await, EndVerdict::Continue(_)),
                "check {} continues",
                n + 1
            );
        }
        assert!(matches!(
            check.check(&end(&[], 7)).await,
            EndVerdict::Exit(TurnExit::GoalPaused(Pause::UnmetTooOften))
        ));
        let goal = GoalStore::new(&sessions, &id).get().unwrap().unwrap();
        assert_eq!(goal.status, GoalStatus::Paused(Pause::UnmetTooOften));
        assert_eq!(goal.turns, 8);
    }

    fn tool_step(i: usize, output: &str) -> [Message; 2] {
        let id = format!("c{i}");
        [
            Message {
                role: "assistant".into(),
                tool_calls: Some(serde_json::json!([{"id": id, "name": "run_command", "input": {"command": "cargo test"}}])),
                ..Default::default()
            },
            Message {
                role: "tool".into(),
                tool_results: Some(serde_json::json!([{"tool_call_id": id, "content": output, "is_error": false}])),
                ..Default::default()
            },
        ]
    }

    #[tokio::test]
    async fn the_check_reads_real_messages_to_half_the_window() {
        let (sessions, id) = session();
        set(&sessions, &id, "all tests pass");
        // A long tool result is sent whole: its last line is the evidence.
        let long = format!("{}\ntest result: ok. 412 passed; 0 failed", "running…\n".repeat(2_000));
        let mut transcript = vec![said("user", "Make the tests pass.")];
        for i in 0..30 {
            transcript.extend(tool_step(i, if i == 29 { &long } else { "2 failed" }));
        }
        let judge = Judge::new(vec![
            Reply::Overflow,
            Reply::Text("{\"met\": true, \"reason\": \"\\\"0 failed\\\"\"}"),
        ]);
        let dj = judged_by(&judge, 20_000);
        let goal = GoalStore::new(&sessions, &id).active().unwrap().unwrap();
        let verdict = check_goal(&dj, RequestTrace::new("goal_check"), &transcript, &goal).await;
        assert!(verdict.unwrap().met);

        let seen = judge.seen.lock().unwrap();
        let tokens = |req: &ChatRequest| req.messages.iter().map(message_tokens).sum::<usize>();
        let first = &seen[0];
        let last_result = first.messages[first.messages.len() - 2].tool_results.as_ref().unwrap();
        assert_eq!(last_result[0]["content"], long.as_str(), "nothing clipped");
        assert!(tokens(first) <= 10_000 + 200, "half the window: {}", tokens(first));
        assert!(first.messages[0].content.contains("left out to fit"));
        assert!(
            first.messages[1].tool_results.is_none(),
            "a kept tool result never goes without its call"
        );
        // The overflow retry sends a quarter of the window.
        assert!(tokens(&seen[1]) <= 5_000 + 200, "a quarter: {}", tokens(&seen[1]));
        assert!(tokens(&seen[1]) < tokens(first));
    }

    #[test]
    fn a_transcript_that_fits_is_sent_whole() {
        let transcript = vec![said("user", "hi"), said("assistant", "hello")];
        assert_eq!(fit_transcript(&transcript, 1_000), transcript);
    }

    #[tokio::test(start_paused = true)]
    async fn background_work_defers_the_check_and_checks_in() {
        let (sessions, id) = session();
        set(&sessions, &id, "the research is written up");
        let judge = Judge::new(vec![]);
        let (check, seen) = goal_check(&sessions, &id, &judge);
        seen.running.lock().unwrap().push("h1 · read the filings".into());

        assert!(matches!(check.check(&end(&[], 0)).await, EndVerdict::Stop));
        assert!(matches!(check.check(&end(&[], 0)).await, EndVerdict::Stop));
        assert_eq!(judge.calls.load(Ordering::SeqCst), 0, "nothing judged while it runs");
        assert!(GoalStore::new(&sessions, &id).active().unwrap().is_some());

        // One timer at a time; the first check-in after 30 min lists the work.
        tokio::time::sleep(CHECK_IN_AFTER + Duration::from_secs(1)).await;
        let kickoffs = seen.kickoffs.lock().unwrap().clone();
        assert_eq!(kickoffs.len(), 1);
        assert!(kickoffs[0].contains("still running:\n- h1 · read the filings"), "{}", kickoffs[0]);
        assert!(kickoffs[0].contains("30 min"));

        // Backed off: the next waits 60 min, the third 120 and says it is the
        // last; then no more.
        assert_eq!(check_in_delay(1), CHECK_IN_AFTER * 2);
        assert_eq!(check_in_delay(5), CHECK_IN_AFTER * 4);
        for waited in [60u64, 120] {
            check.check(&end(&[], 0)).await;
            tokio::time::sleep(Duration::from_secs(waited * 60 + 1)).await;
        }
        let kickoffs = seen.kickoffs.lock().unwrap().clone();
        assert_eq!(kickoffs.len(), 3);
        assert!(kickoffs[2].contains("last check-in"));
        check.check(&end(&[], 0)).await;
        tokio::time::sleep(CHECK_IN_AFTER * 8).await;
        assert_eq!(seen.kickoffs.lock().unwrap().len(), 3, "at most three");

        // Once the work is done, the goal is checked and check-ins start over.
        seen.running.lock().unwrap().clear();
        let judge = Judge::new(vec![UNMET]);
        let mut check = check;
        check.judge = Some(judged_by(&judge, DEFAULT_WINDOW_TOKENS));
        assert!(matches!(check.check(&end(&[], 0)).await, EndVerdict::Continue(_)));
        assert!(check.check_ins.sessions.lock().unwrap().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn a_check_in_for_a_cleared_goal_says_nothing() {
        let (sessions, id) = session();
        set(&sessions, &id, "the research is written up");
        let (check, seen) = goal_check(&sessions, &id, &Judge::new(vec![]));
        seen.running.lock().unwrap().push("h1".into());
        check.check(&end(&[], 0)).await;
        GoalStore::new(&sessions, &id).clear().unwrap();
        tokio::time::sleep(CHECK_IN_AFTER * 2).await;
        assert!(seen.kickoffs.lock().unwrap().is_empty());
    }

    #[test]
    fn set_and_clear_tell_the_transcript_nothing() {
        let (sessions, id) = session();
        let goals = GoalStore::new(&sessions, &id);
        let goal = goals
            .set("  the report is sent  ", GoalSource::OwnerCommand)
            .unwrap();
        assert_eq!(goal.condition, "the report is sent");
        goals.clear().unwrap();
        assert!(goals.clear().unwrap().is_none(), "nothing left to clear");
        assert!(
            sessions
                .store()
                .get_chat_messages(&sessions.active_chat_id(&id))
                .unwrap()
                .is_empty(),
            "the goal line is UI only; the kickoff is the model's word of it"
        );
        let kickoff = goal.kickoff();
        assert!(kickoff.starts_with("Agreed goal: the report is sent."), "{kickoff}");
        assert!(kickoff.contains("start"), "{kickoff}");
    }

    #[test]
    fn the_owner_sets_up_to_4000_chars_a_suggestion_500() {
        let (sessions, id) = session();
        let goals = GoalStore::new(&sessions, &id);
        assert!(matches!(
            goals.set("   ", GoalSource::OwnerCommand),
            Err(GoalError::Empty)
        ));
        goals
            .set(&"é".repeat(MAX_OWNER_CONDITION_CHARS), GoalSource::OwnerCommand)
            .unwrap();
        assert!(matches!(
            goals.set(&"é".repeat(MAX_OWNER_CONDITION_CHARS + 1), GoalSource::OwnerCommand),
            Err(GoalError::TooLong(4001, 4000))
        ));
        assert!(matches!(
            goals.set(&"é".repeat(501), GoalSource::OwnersOwnWords),
            Err(GoalError::TooLong(501, 500))
        ));
        goals.set("the second goal", GoalSource::OwnerCommand).unwrap();
        assert_eq!(
            goals.get().unwrap().unwrap().condition,
            "the second goal",
            "one goal at a time: the new one replaces it"
        );
    }

    #[test]
    fn a_new_conversation_starts_without_the_goal() {
        let (sessions, id) = session();
        set(&sessions, &id, "the report is sent");
        sessions.reset(&id).unwrap();
        assert!(GoalStore::new(&sessions, &id).get().unwrap().is_none());
    }

    struct Card {
        sessions: SessionManager,
        id: String,
        approvals: tools::ApprovalChannels,
        events: mpsc::Sender<StreamEvent>,
        rx: mpsc::Receiver<StreamEvent>,
        suggestions: Suggestions,
        seen: Arc<Seen>,
    }

    impl Card {
        fn new() -> Self {
            let (sessions, id) = session();
            let (events, rx) = mpsc::channel(8);
            Self {
                sessions,
                id,
                approvals: Default::default(),
                events,
                rx,
                suggestions: Suggestions::default(),
                seen: Arc::new(Seen::default()),
            }
        }

        async fn suggest(&self, call_id: &str, condition: &str, ask_owner: bool) -> Result<String, String> {
            let call = ai::ToolCall {
                id: call_id.into(),
                name: "suggest_goal".into(),
                input: serde_json::json!({ "condition": condition, "ask_owner": ask_owner }),
            };
            let input: SuggestInput = serde_json::from_value(call.input.clone()).unwrap();
            self.suggestions
                .suggest(
                    SuggestContext {
                        sessions: &self.sessions,
                        session_id: &self.id,
                        call: &call,
                        approvals: &self.approvals,
                        events: &self.events,
                        observer: self.seen.clone(),
                    },
                    input,
                )
                .await
        }

        async fn settle(&self) {
            for _ in 0..50 {
                tokio::task::yield_now().await;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        async fn answer(&self, call_id: &str, decision: &str) {
            let tx = self.approvals.lock().await.remove(call_id).expect("a card waits");
            tx.send(decision.to_string()).unwrap();
            self.settle().await;
        }

        fn goals(&self) -> GoalStore<'_> {
            GoalStore::new(&self.sessions, &self.id)
        }
    }

    #[tokio::test]
    async fn an_approved_suggestion_sets_the_goal_and_kicks_it_off() {
        let mut card = Card::new();
        let told = card
            .suggest("call-1", "all tests in the auth suite pass", true)
            .await
            .unwrap();
        assert!(told.contains("card for the owner to approve"));
        let ev = card.rx.recv().await.unwrap();
        assert_eq!(ev.event_type, StreamEventType::ApprovalRequest);
        let shown = ev.tool_call.unwrap();
        assert_eq!((shown.id.as_str(), shown.name.as_str()), ("call-1", "suggest_goal"));
        assert_eq!(shown.input["condition"], "all tests in the auth suite pass");
        assert!(card.goals().active().unwrap().is_none(), "nothing is set before the owner answers");

        // One suggestion out at a time.
        assert!(card.suggest("call-2", "something else", true).await.is_err());

        card.answer("call-1", "once").await;
        let goal = card.goals().active().unwrap().expect("approved");
        assert_eq!(goal.source, GoalSource::SuggestedApproved);
        assert_eq!(card.seen.statuses.lock().unwrap().len(), 1);
        assert_eq!(*card.seen.kickoffs.lock().unwrap(), [goal.kickoff()], "work starts on it once");
    }

    #[tokio::test]
    async fn declined_goal_is_never_suggested_again() {
        let card = Card::new();
        card.suggest("call-1", "the whole site is rewritten", true).await.unwrap();
        card.answer("call-1", "deny").await;
        assert!(card.goals().active().unwrap().is_none());
        assert!(card.seen.kickoffs.lock().unwrap().is_empty(), "a decline is not told to the model");

        let again = card.suggest("call-2", "The whole  site is REWRITTEN", true).await;
        assert!(again.unwrap_err().contains("declined"));
        // Not even set directly.
        assert!(card.suggest("call-3", "the whole site is rewritten", false).await.is_err());
        assert!(card.approvals.lock().await.is_empty(), "no second card");

        // The decline outlives a goal set and cleared later.
        card.goals().set("the homepage loads", GoalSource::OwnerCommand).unwrap();
        card.goals().clear().unwrap();
        assert!(card.suggest("call-4", "the whole site is rewritten", true).await.is_err());
    }

    #[tokio::test]
    async fn direct_set_only_with_owners_words() {
        let mut card = Card::new();
        // ask_owner false: the model says the owner's own words stated it;
        // the goal is set at once, visibly, without a card, and kicked off.
        let told = card.suggest("call-1", "the migration runs clean", false).await.unwrap();
        assert!(told.contains("owner's own words"));
        let goal = card.goals().active().unwrap().unwrap();
        assert_eq!(goal.source, GoalSource::OwnersOwnWords);
        assert!(card.approvals.lock().await.is_empty());
        assert!(card.rx.try_recv().is_err(), "no card");
        assert_eq!(card.seen.statuses.lock().unwrap().len(), 1, "the owner sees it set");
        assert_eq!(card.seen.kickoffs.lock().unwrap().len(), 1);

        // Left out, ask_owner defaults to asking.
        let input: SuggestInput = serde_json::from_value(serde_json::json!({"condition": "x"})).unwrap();
        assert!(input.ask_owner);
    }

    #[tokio::test]
    async fn an_unanswered_card_neither_sets_nor_declines() {
        let card = Card::new();
        card.suggest("call-1", "the inbox is at zero", true).await.unwrap();
        drop(card.approvals.lock().await.remove("call-1"));
        card.settle().await;
        assert!(card.goals().get().unwrap().is_none());
        assert!(card.suggestions.waiting.lock().unwrap().is_empty());
        card.suggest("call-2", "the inbox is at zero", true).await.unwrap();
    }

    #[test]
    fn verdicts_parse_and_default_to_not_met_evidence() {
        assert_eq!(
            parse_verdict("{\"met\": false}").unwrap().reason,
            "the transcript does not show it yet"
        );
        let v = parse_verdict("{\"met\": true, \"impossible\": true, \"reason\": \"done\"}").unwrap();
        assert!(v.met && !v.impossible, "met wins over impossible");
        assert!(parse_verdict("yes").is_none());
        assert!(parse_verdict("{\"reason\": \"no verdict\"}").is_none());
    }
}
