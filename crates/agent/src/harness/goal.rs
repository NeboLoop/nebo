//! The agreed goal: an end state the owner set (`/goal`) or approved from a
//! `suggest_goal` call. It is a stop condition, not a prompt anchor: when the
//! model answers without tool calls, a separate done check reads the
//! transcript and says whether the end state is reached, quoting the
//! transcript. Not met, the turn continues with the reason; met, impossible
//! or paused, it ends. Set and clear each write one attachment row; the goal
//! is never re-rendered on later calls.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ai::{ChatRequest, Message, Provider, RequestTrace, StreamEvent, StreamEventType};
use tokio::sync::mpsc;
use tracing::{info, warn};
use types::NeboError;

use super::events::{TurnEvent, reminder_for};
use super::reminders::{Reminders, SessionTranscript};
use super::turn::TurnExit;
use super::turn_end::{EndCheck, EndVerdict, TurnEnd};
use crate::session::SessionManager;

/// The longest condition, in characters: the owner reads all of it on the
/// approval card.
pub const MAX_CONDITION_CHARS: usize = 500;

/// Unmet checks in one turn before the goal pauses.
pub const UNMET_CHECKS_BEFORE_PAUSE: u8 = 3;

/// How long the done check may take before the goal pauses.
pub const DONE_CHECK_DEADLINE: Duration = Duration::from_secs(10);

/// The name the goal check's continue reminder carries.
pub const GOAL_CHECK: &str = "goal_check";

/// Most transcript characters one done check reads; the oldest are left out.
const TRANSCRIPT_BUDGET_CHARS: usize = 120_000;
/// Most characters of one tool call's input or result in the transcript.
const TOOL_TEXT_CHARS: usize = 2_000;
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
    /// The done check did not answer within [`DONE_CHECK_DEADLINE`].
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
    #[error("The goal is {0} characters; keep it to {MAX_CONDITION_CHARS}.")]
    TooLong(usize),
    #[error(transparent)]
    Store(#[from] NeboError),
}

/// Told whenever a goal's status changes, so the owner sees it.
pub type GoalStatusSink = Arc<dyn Fn(&AgreedGoal) + Send + Sync>;

/// One session's goal over the `session_goals` table. Set and clear write
/// their attachment row into the session's conversation.
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
    /// goal at a time), and tell the model once.
    pub fn set(&self, condition: &str, source: GoalSource) -> Result<AgreedGoal, GoalError> {
        let condition = valid_condition(condition)?;
        let row = self.store().put_session_goal(
            self.session_id,
            &condition,
            source.as_str(),
            GoalStatus::Active.as_str(),
        )?;
        self.announce(TurnEvent::GoalSet { condition })?;
        Ok(AgreedGoal::from_row(row))
    }

    /// Clear the goal. `None` when there was none being pursued, and then
    /// nothing is written.
    pub fn clear(&self) -> Result<Option<AgreedGoal>, NeboError> {
        let pursued = self
            .get()?
            .filter(|g| matches!(g.status, GoalStatus::Active | GoalStatus::Paused(_)));
        if pursued.is_none() {
            return Ok(None);
        }
        let row = self.store().update_session_goal_status(
            self.session_id,
            GoalStatus::Cleared.as_str(),
            None,
            false,
        )?;
        self.announce(TurnEvent::GoalCleared)?;
        Ok(row.map(AgreedGoal::from_row))
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

    /// Write the event's attachment row now: set and clear happen between
    /// calls (a slash command, an approval card, a tool call), and the next
    /// call loads the row with the rest of the conversation.
    fn announce(&self, event: TurnEvent) -> Result<(), NeboError> {
        let Some((name, _, text)) = reminder_for(&event) else {
            return Ok(());
        };
        let mut reminders = Reminders::default();
        reminders.fact(name, text);
        reminders.attach(&SessionTranscript {
            sessions: self.sessions,
            session_id: self.session_id,
        })?;
        reminders.landed();
        Ok(())
    }
}

fn valid_condition(condition: &str) -> Result<String, GoalError> {
    let condition = condition.trim();
    if condition.is_empty() {
        return Err(GoalError::Empty);
    }
    let chars = condition.chars().count();
    if chars > MAX_CONDITION_CHARS {
        return Err(GoalError::TooLong(chars));
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
}

impl DoneJudge {
    pub fn for_providers(providers: &[Arc<dyn Provider>]) -> Option<Self> {
        let (provider, model) =
            match super::model_call::resolve_aux(&config::ModelsConfig::load(), providers) {
                Some(routed) => routed,
                None => (crate::summarizer::pick_cheapest(providers)?, String::new()),
            };
        Some(Self { provider, model })
    }
}

const DONE_CHECK_SYSTEM: &str = "You check whether an agreed goal has been reached. Read the conversation transcript, then judge from the transcript alone whether the goal's end state is met.

Answer with one JSON object and nothing else, in one of these shapes:
- {\"met\": true, \"reason\": \"<quote the transcript text that shows the end state was reached>\"}
- {\"met\": false, \"reason\": \"<quote what is missing or what stands in the way>\"}
- {\"met\": false, \"impossible\": true, \"reason\": \"<why the end state can never be reached in this conversation>\"}

Always give a reason, quoting the transcript wherever you can. When the transcript holds no clear evidence that the end state was reached, answer {\"met\": false, \"reason\": \"the transcript does not show it yet\"}.

Use impossible only when the end state truly cannot be reached here: it contradicts itself, it needs something that is not available, or the work has tried every reasonable way and said it cannot be done. The assistant saying it is impossible is evidence, not proof; judge it yourself. Slow progress or unfinished work is not impossible. When unsure, leave impossible out.";

/// Run the done check over `transcript`. `None` = the check was
/// unavailable (no answer within [`DONE_CHECK_DEADLINE`], an error, or an
/// answer that was not the JSON asked for); the goal pauses.
pub async fn check_goal(
    judge: &DoneJudge,
    trace: RequestTrace,
    transcript: &[Message],
    goal: &AgreedGoal,
) -> Option<GoalVerdict> {
    let req = ChatRequest {
        messages: vec![Message {
            role: "user".to_string(),
            content: format!(
                "<transcript>\n{}\n</transcript>\n\nFrom the transcript above alone: has this agreed goal been reached?\n\nGoal: {}",
                render_transcript(transcript),
                goal.condition
            ),
            ..Default::default()
        }],
        max_tokens: MAX_VERDICT_TOKENS,
        system: DONE_CHECK_SYSTEM.to_string(),
        model: judge.model.clone(),
        ..ChatRequest::new(trace)
    };
    let answer = tokio::time::timeout(DONE_CHECK_DEADLINE, async {
        let mut rx = judge.provider.stream(&req).await.ok()?;
        let mut text = String::new();
        while let Some(ev) = rx.recv().await {
            match ev.event_type {
                StreamEventType::Text => text.push_str(&ev.text),
                StreamEventType::Error => return None,
                StreamEventType::Done => break,
                _ => {}
            }
        }
        Some(text)
    })
    .await;
    match answer {
        Ok(Some(text)) => parse_verdict(&text),
        Ok(None) => None,
        Err(_) => {
            warn!(condition = %goal.condition, "done check: no answer within the deadline");
            None
        }
    }
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

/// The transcript as the done check reads it: every row labelled, tool
/// traffic clipped, the oldest rows left out past the budget.
fn render_transcript(messages: &[Message]) -> String {
    let rows: Vec<String> = messages.iter().flat_map(render_row).collect();
    let mut kept = Vec::new();
    let mut used = 0usize;
    for row in rows.iter().rev() {
        if used + row.len() > TRANSCRIPT_BUDGET_CHARS && !kept.is_empty() {
            break;
        }
        used += row.len();
        kept.push(row.as_str());
    }
    let left_out = rows.len() - kept.len();
    kept.reverse();
    let body = kept.join("\n\n");
    if left_out == 0 {
        body
    } else {
        format!(
            "[{left_out} earlier entries were left out to fit. If the evidence may be in them, answer not met.]\n\n{body}"
        )
    }
}

fn render_row(m: &Message) -> Vec<String> {
    let mut out = Vec::new();
    let text = m.content.trim();
    if !text.is_empty() {
        let label = match m.role.as_str() {
            "assistant" => "Assistant",
            "user" if text.starts_with("<system-reminder>") => "System note",
            "user" => "Owner",
            _ => "System",
        };
        out.push(format!("{label}: {text}"));
    }
    for call in m
        .tool_calls
        .iter()
        .flat_map(|v| v.as_array().into_iter().flatten())
    {
        let name = call.get("name").and_then(|v| v.as_str()).unwrap_or("tool");
        let input = call.get("input").map(|v| v.to_string()).unwrap_or_default();
        out.push(format!("Assistant called {name}: {}", clip(&input)));
    }
    for result in m
        .tool_results
        .iter()
        .flat_map(|v| v.as_array().into_iter().flatten())
    {
        let content = result.get("content").and_then(|v| v.as_str()).unwrap_or("");
        let label = if result
            .get("is_error")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            "Tool error"
        } else {
            "Tool result"
        };
        out.push(format!("{label}: {}", clip(content)));
    }
    out
}

fn clip(s: &str) -> String {
    match s.char_indices().nth(TOOL_TEXT_CHARS) {
        Some((at, _)) => format!("{}…", &s[..at]),
        None => s.to_string(),
    }
}

/// The agreed-goal end check: registered for chat turns; runs the done check
/// only while the session has an active goal.
pub struct GoalCheck {
    pub sessions: SessionManager,
    pub session_id: String,
    /// `None` = no model to check with; an active goal pauses.
    pub judge: Option<DoneJudge>,
    pub trace: RequestTrace,
    pub on_status: GoalStatusSink,
}

impl GoalCheck {
    fn goals(&self) -> GoalStore<'_> {
        GoalStore::new(&self.sessions, &self.session_id)
    }

    fn record(&self, status: GoalStatus, reason: Option<&str>, unmet: bool) {
        match self.goals().record_check(status, reason, unmet) {
            Ok(Some(goal)) => (self.on_status)(&goal),
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
            Ok(None) => return EndVerdict::Stop,
            Err(e) => {
                warn!(session_id = %self.session_id, error = %e, "goal: loading the goal failed");
                return EndVerdict::Stop;
            }
        };
        // Helpers or background work still running: their completion wakes
        // the session, and that turn's end checks the goal. Nothing is
        // judged on a transcript that is still waiting on them.
        if end.background_running {
            info!(session_id = %self.session_id, "goal: check deferred, background work is running");
            return EndVerdict::Stop;
        }
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
        EndVerdict::Continue(TurnEvent::EndCheckContinue {
            check: GOAL_CHECK,
            text: format!(
                "The agreed goal isn't met yet: {reason}. Keep working toward: {}",
                goal.condition
            ),
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
    pub on_status: GoalStatusSink,
}

/// The `suggest_goal` behaviour. Holds the sessions with a card waiting on
/// the owner, so a session has one suggestion out at a time.
#[derive(Clone, Default)]
pub struct Suggestions {
    waiting: Arc<Mutex<HashSet<String>>>,
}

impl Suggestions {
    /// Handle one `suggest_goal` call. `Ok` is the tool result the model
    /// reads; `Err` is a refusal it can correct. With `ask_owner` the card
    /// goes out and the call returns at once: approved, the goal is set and
    /// the model is told by its attachment; declined, the condition is
    /// remembered and never suggested again, and the model is not told.
    pub async fn suggest(
        &self,
        cx: SuggestContext<'_>,
        input: SuggestInput,
    ) -> Result<String, String> {
        let condition = valid_condition(&input.condition).map_err(|e| e.to_string())?;
        let goals = GoalStore::new(cx.sessions, cx.session_id);
        let current = goals.get().map_err(|e| e.to_string())?;
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
            let goal = goals
                .set(&condition, GoalSource::OwnersOwnWords)
                .map_err(|e| e.to_string())?;
            (cx.on_status)(&goal);
            return Ok("The goal is set, without asking, because the owner's own words stated it. The owner sees it and can clear it with /goal clear. Keep working.".to_string());
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
        let on_status = cx.on_status.clone();
        tokio::spawn(async move {
            let answer = answer_rx.await;
            waiting.lock().unwrap().remove(&session_id);
            let goals = GoalStore::new(&sessions, &session_id);
            match answer.as_deref() {
                Ok("once") | Ok("always") => {
                    match goals.set(&condition, GoalSource::SuggestedApproved) {
                        Ok(goal) => on_status(&goal),
                        Err(e) => {
                            warn!(session_id, error = %e, "goal: setting the approved goal failed")
                        }
                    }
                }
                Ok(_) => {
                    if let Err(e) = goals.record_decline(&condition) {
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
    use ai::{EventReceiver, ProviderError};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Answers each done check with the next scripted reply; `None` never
    /// answers (the deadline passes). Counts the calls it saw.
    struct Judge {
        replies: Mutex<Vec<Option<&'static str>>>,
        calls: AtomicUsize,
        seen: Mutex<Vec<ChatRequest>>,
    }

    impl Judge {
        fn new(replies: Vec<Option<&'static str>>) -> Arc<Self> {
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
                Some(text) => {
                    let _ = tx.send(StreamEvent::text(text)).await;
                    let _ = tx.send(StreamEvent::done()).await;
                }
                // Hold the stream open past the deadline.
                None => {
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_secs(3600)).await;
                        drop(tx);
                    });
                }
            }
            Ok(rx)
        }
    }

    fn session() -> (SessionManager, String) {
        let path = std::env::temp_dir().join(format!("nebo-goal-{}.db", uuid::Uuid::new_v4()));
        let store = Arc::new(db::Store::new(path.to_str().unwrap()).expect("store"));
        let sessions = SessionManager::new(store);
        let id = sessions
            .get_or_create("agent:a1:web", "")
            .expect("session")
            .id;
        (sessions, id)
    }

    fn statuses() -> (GoalStatusSink, Arc<Mutex<Vec<AgreedGoal>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink_seen = seen.clone();
        (
            Arc::new(move |g: &AgreedGoal| sink_seen.lock().unwrap().push(g.clone())),
            seen,
        )
    }

    fn goal_check(
        sessions: &SessionManager,
        id: &str,
        judge: &Arc<Judge>,
    ) -> (GoalCheck, Arc<Mutex<Vec<AgreedGoal>>>) {
        let (on_status, seen) = statuses();
        (
            GoalCheck {
                sessions: sessions.clone(),
                session_id: id.to_string(),
                judge: Some(DoneJudge {
                    provider: judge.clone(),
                    model: String::new(),
                }),
                trace: RequestTrace::new("goal_check"),
                on_status,
            },
            seen,
        )
    }

    fn end(transcript: &[Message], checks_this_turn: u8) -> TurnEnd<'_> {
        TurnEnd {
            transcript,
            step: 1,
            checks_this_turn,
            background_running: false,
        }
    }

    fn said(role: &str, text: &str) -> Message {
        Message {
            role: role.into(),
            content: text.into(),
            ..Default::default()
        }
    }

    /// The attachment rows in the session's conversation: (kind, content).
    fn attachments(sessions: &SessionManager, id: &str) -> Vec<(String, String)> {
        sessions
            .store()
            .get_chat_messages(&sessions.active_chat_id(id))
            .unwrap()
            .into_iter()
            .filter_map(|m| {
                let meta: serde_json::Value = serde_json::from_str(m.metadata.as_deref()?).ok()?;
                let kind = meta.pointer("/attachment/kind")?.as_str()?.to_string();
                Some((kind, m.content))
            })
            .collect()
    }

    #[tokio::test]
    async fn no_goal_means_no_end_check_call() {
        let (sessions, id) = session();
        let judge = Judge::new(vec![]);
        let (check, seen) = goal_check(&sessions, &id, &judge);
        assert!(matches!(check.check(&end(&[], 0)).await, EndVerdict::Stop));

        // A cleared goal is no goal either.
        let goals = GoalStore::new(&sessions, &id);
        goals
            .set("the report is sent", GoalSource::OwnerCommand)
            .unwrap();
        goals.clear().unwrap();
        assert!(matches!(check.check(&end(&[], 0)).await, EndVerdict::Stop));
        assert_eq!(judge.calls.load(Ordering::SeqCst), 0);
        assert!(seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn unmet_goal_continues_with_quoted_reason() {
        let (sessions, id) = session();
        GoalStore::new(&sessions, &id)
            .set("all three invoices are sent", GoalSource::OwnerCommand)
            .unwrap();
        let judge = Judge::new(vec![Some(
            "{\"met\": false, \"reason\": \"\\\"Sent invoice 1 of 3\\\" - two are still unsent\"}",
        )]);
        let (check, seen) = goal_check(&sessions, &id, &judge);
        let transcript = [
            said("user", "Send the three invoices."),
            said("assistant", "Sent invoice 1 of 3."),
        ];
        let EndVerdict::Continue(TurnEvent::EndCheckContinue { check: name, text }) =
            check.check(&end(&transcript, 0)).await
        else {
            panic!("an unmet goal continues the turn");
        };
        assert_eq!(name, GOAL_CHECK);
        assert_eq!(
            text,
            "The agreed goal isn't met yet: \"Sent invoice 1 of 3\" - two are still unsent. Keep working toward: all three invoices are sent"
        );
        let (reminder, _, _) =
            reminder_for(&TurnEvent::EndCheckContinue { check: name, text }).unwrap();
        assert_eq!(reminder, "goal_check");

        // The check read the transcript and nothing else: no tools.
        let req = judge.seen.lock().unwrap()[0].clone();
        assert!(req.tools.is_empty());
        assert!(
            req.messages[0]
                .content
                .contains("Owner: Send the three invoices.")
        );
        assert!(
            req.messages[0]
                .content
                .contains("Assistant: Sent invoice 1 of 3.")
        );
        assert!(
            req.messages[0]
                .content
                .ends_with("Goal: all three invoices are sent")
        );

        let goal = GoalStore::new(&sessions, &id).get().unwrap().unwrap();
        assert_eq!(goal.status, GoalStatus::Active);
        assert_eq!(goal.turns, 1);
        assert_eq!(
            goal.last_reason.as_deref(),
            Some("\"Sent invoice 1 of 3\" - two are still unsent")
        );
        assert_eq!(
            seen.lock().unwrap().len(),
            1,
            "the owner sees the bumped count"
        );
    }

    #[tokio::test]
    async fn met_and_impossible_end_the_turn() {
        let (sessions, id) = session();
        let goals = GoalStore::new(&sessions, &id);
        goals
            .set("the site is live", GoalSource::OwnerCommand)
            .unwrap();
        let judge = Judge::new(vec![
            Some("Here: {\"met\": true, \"reason\": \"\\\"Deployed to example.com\\\"\"}"),
            Some("{\"met\": false, \"impossible\": true, \"reason\": \"no host account exists\"}"),
        ]);
        let (check, _) = goal_check(&sessions, &id, &judge);
        assert_eq!(
            match check.check(&end(&[], 0)).await {
                EndVerdict::Exit(exit) => exit,
                _ => panic!("met ends the turn"),
            },
            TurnExit::GoalMet {
                reason: "\"Deployed to example.com\"".into()
            }
        );
        assert_eq!(goals.get().unwrap().unwrap().status, GoalStatus::Met);

        goals
            .set("the site is live", GoalSource::OwnerCommand)
            .unwrap();
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
        goals
            .set("the backlog is empty", GoalSource::OwnerCommand)
            .unwrap();
        // No answer within the deadline, then an answer that isn't the JSON.
        let judge = Judge::new(vec![None, Some("I think so")]);
        let (check, seen) = goal_check(&sessions, &id, &judge);
        assert!(matches!(
            check.check(&end(&[], 0)).await,
            EndVerdict::Exit(TurnExit::GoalPaused(Pause::CheckUnavailable))
        ));
        let paused = goals.get().unwrap().unwrap();
        assert_eq!(paused.status, GoalStatus::Paused(Pause::CheckUnavailable));
        assert_eq!(
            seen.lock().unwrap().last().unwrap().status,
            paused.status,
            "the owner sees the pause"
        );

        // A paused goal is not checked; the owner's next message resumes it.
        assert!(matches!(check.check(&end(&[], 0)).await, EndVerdict::Stop));
        assert_eq!(judge.calls.load(Ordering::SeqCst), 1);
        assert_eq!(goals.resume().unwrap().unwrap().status, GoalStatus::Active);
        assert!(matches!(
            check.check(&end(&[], 0)).await,
            EndVerdict::Exit(TurnExit::GoalPaused(Pause::CheckUnavailable))
        ));

        // No model to check with pauses the same way, without a call.
        goals.resume().unwrap();
        let (mut no_judge, _) = goal_check(&sessions, &id, &judge);
        no_judge.judge = None;
        assert!(matches!(
            no_judge.check(&end(&[], 0)).await,
            EndVerdict::Exit(TurnExit::GoalPaused(Pause::CheckUnavailable))
        ));
        assert_eq!(judge.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn three_unmet_checks_pause() {
        let (sessions, id) = session();
        let goals = GoalStore::new(&sessions, &id);
        goals
            .set("every call site is migrated", GoalSource::OwnerCommand)
            .unwrap();
        let unmet = Some("{\"met\": false, \"reason\": \"\\\"4 left\\\"\"}");
        let judge = Judge::new(vec![unmet, unmet, unmet]);
        let (check, _) = goal_check(&sessions, &id, &judge);
        assert!(matches!(
            check.check(&end(&[], 0)).await,
            EndVerdict::Continue(_)
        ));
        assert!(matches!(
            check.check(&end(&[], 1)).await,
            EndVerdict::Continue(_)
        ));
        assert!(matches!(
            check.check(&end(&[], 2)).await,
            EndVerdict::Exit(TurnExit::GoalPaused(Pause::UnmetTooOften))
        ));
        let goal = goals.get().unwrap().unwrap();
        assert_eq!(goal.status, GoalStatus::Paused(Pause::UnmetTooOften));
        assert_eq!(goal.turns, 3);
    }

    #[tokio::test]
    async fn background_work_defers_the_check() {
        let (sessions, id) = session();
        GoalStore::new(&sessions, &id)
            .set("the research is written up", GoalSource::OwnerCommand)
            .unwrap();
        let judge = Judge::new(vec![]);
        let (check, _) = goal_check(&sessions, &id, &judge);
        let waiting = TurnEnd {
            transcript: &[],
            step: 3,
            checks_this_turn: 0,
            background_running: true,
        };
        assert!(matches!(check.check(&waiting).await, EndVerdict::Stop));
        assert_eq!(judge.calls.load(Ordering::SeqCst), 0);
        assert!(
            GoalStore::new(&sessions, &id).active().unwrap().is_some(),
            "still pursued"
        );
    }

    #[test]
    fn goal_set_and_clear_write_one_attachment_each() {
        let (sessions, id) = session();
        let goals = GoalStore::new(&sessions, &id);
        goals
            .set("  the report is sent  ", GoalSource::OwnerCommand)
            .unwrap();
        let rows = attachments(&sessions, &id);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "goal_set");
        assert!(rows[0].1.starts_with("<system-reminder>"));
        assert!(
            rows[0].1.contains("Agreed goal: the report is sent."),
            "{}",
            rows[0].1
        );

        goals.clear().unwrap();
        // A second clear has nothing to clear and writes nothing.
        assert!(goals.clear().unwrap().is_none());
        let kinds: Vec<String> = attachments(&sessions, &id)
            .into_iter()
            .map(|r| r.0)
            .collect();
        assert_eq!(kinds, ["goal_set", "goal_cleared"]);

        // The owner's thread never shows them.
        let meta = sessions
            .store()
            .get_chat_messages(&sessions.active_chat_id(&id))
            .unwrap()
            .into_iter()
            .all(|m| {
                m.metadata
                    .as_deref()
                    .is_some_and(|s| s.contains("\"isMeta\":true"))
            });
        assert!(meta);
    }

    #[test]
    fn a_new_conversation_starts_without_the_goal() {
        let (sessions, id) = session();
        GoalStore::new(&sessions, &id)
            .set("the report is sent", GoalSource::OwnerCommand)
            .unwrap();
        sessions.reset(&id).unwrap();
        assert!(GoalStore::new(&sessions, &id).get().unwrap().is_none());
    }

    #[test]
    fn a_goal_is_one_condition_of_at_most_500_chars() {
        let (sessions, id) = session();
        let goals = GoalStore::new(&sessions, &id);
        assert!(matches!(
            goals.set("   ", GoalSource::OwnerCommand),
            Err(GoalError::Empty)
        ));
        let long = "é".repeat(MAX_CONDITION_CHARS + 1);
        assert!(matches!(
            goals.set(&long, GoalSource::OwnerCommand),
            Err(GoalError::TooLong(501))
        ));
        goals
            .set(&"é".repeat(MAX_CONDITION_CHARS), GoalSource::OwnerCommand)
            .unwrap();
        goals
            .set("the second goal", GoalSource::OwnerCommand)
            .unwrap();
        let goal = goals.get().unwrap().unwrap();
        assert_eq!(
            goal.condition, "the second goal",
            "one goal at a time: the new one replaces it"
        );
        assert_eq!(
            attachments(&sessions, &id).len(),
            2,
            "each set is told once"
        );
    }

    struct Card {
        sessions: SessionManager,
        id: String,
        approvals: tools::ApprovalChannels,
        events: mpsc::Sender<StreamEvent>,
        rx: mpsc::Receiver<StreamEvent>,
        suggestions: Suggestions,
        seen: Arc<Mutex<Vec<AgreedGoal>>>,
        on_status: GoalStatusSink,
    }

    impl Card {
        fn new() -> Self {
            let (sessions, id) = session();
            let (events, rx) = mpsc::channel(8);
            let (on_status, seen) = statuses();
            Self {
                sessions,
                id,
                approvals: Default::default(),
                events,
                rx,
                suggestions: Suggestions::default(),
                seen,
                on_status,
            }
        }

        async fn suggest(
            &self,
            call_id: &str,
            condition: &str,
            ask_owner: bool,
        ) -> Result<String, String> {
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
                        on_status: self.on_status.clone(),
                    },
                    input,
                )
                .await
        }

        async fn answer(&self, call_id: &str, decision: &str) {
            let tx = self
                .approvals
                .lock()
                .await
                .remove(call_id)
                .expect("a card waits");
            tx.send(decision.to_string()).unwrap();
            // Let the waiting task record the answer.
            for _ in 0..50 {
                tokio::task::yield_now().await;
                if self.suggestions.waiting.lock().unwrap().is_empty() {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        fn goals(&self) -> GoalStore<'_> {
            GoalStore::new(&self.sessions, &self.id)
        }
    }

    #[tokio::test]
    async fn an_approved_suggestion_sets_the_goal_through_the_card() {
        let mut card = Card::new();
        let told = card
            .suggest("call-1", "all tests in the auth suite pass", true)
            .await
            .unwrap();
        assert!(told.contains("card for the owner to approve"));
        let ev = card.rx.recv().await.unwrap();
        assert_eq!(ev.event_type, StreamEventType::ApprovalRequest);
        let shown = ev.tool_call.unwrap();
        assert_eq!(
            (shown.id.as_str(), shown.name.as_str()),
            ("call-1", "suggest_goal")
        );
        assert_eq!(shown.input["condition"], "all tests in the auth suite pass");
        assert!(
            card.goals().active().unwrap().is_none(),
            "nothing is set before the owner answers"
        );

        // One suggestion out at a time.
        assert!(
            card.suggest("call-2", "something else", true)
                .await
                .is_err()
        );

        card.answer("call-1", "once").await;
        let goal = card.goals().active().unwrap().expect("approved");
        assert_eq!(goal.source, GoalSource::SuggestedApproved);
        assert_eq!(card.seen.lock().unwrap().len(), 1);
        let kinds: Vec<String> = attachments(&card.sessions, &card.id)
            .into_iter()
            .map(|r| r.0)
            .collect();
        assert_eq!(kinds, ["goal_set"], "the model hears of the approval once");
    }

    #[tokio::test]
    async fn declined_goal_is_never_suggested_again() {
        let card = Card::new();
        card.suggest("call-1", "the whole site is rewritten", true)
            .await
            .unwrap();
        card.answer("call-1", "deny").await;
        assert!(card.goals().active().unwrap().is_none());
        assert!(
            attachments(&card.sessions, &card.id).is_empty(),
            "a decline is not told to the model"
        );

        let again = card
            .suggest("call-2", "The whole  site is REWRITTEN", true)
            .await;
        assert!(again.unwrap_err().contains("declined"));
        // Not even set directly.
        assert!(
            card.suggest("call-3", "the whole site is rewritten", false)
                .await
                .is_err()
        );
        assert!(card.approvals.lock().await.is_empty(), "no second card");

        // The decline outlives a goal set and cleared later.
        card.goals()
            .set("the homepage loads", GoalSource::OwnerCommand)
            .unwrap();
        card.goals().clear().unwrap();
        assert!(
            card.suggest("call-4", "the whole site is rewritten", true)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn direct_set_only_with_owners_words() {
        let mut card = Card::new();
        // ask_owner false: the model says the owner's own words stated it;
        // the goal is set at once, visibly, without a card.
        let told = card
            .suggest("call-1", "the migration runs clean", false)
            .await
            .unwrap();
        assert!(told.contains("owner's own words"));
        let goal = card.goals().active().unwrap().unwrap();
        assert_eq!(goal.source, GoalSource::OwnersOwnWords);
        assert!(card.approvals.lock().await.is_empty());
        assert!(card.rx.try_recv().is_err(), "no card");
        assert_eq!(card.seen.lock().unwrap().len(), 1, "the owner sees it set");

        // Left out, ask_owner defaults to asking.
        let input: SuggestInput =
            serde_json::from_value(serde_json::json!({"condition": "x"})).unwrap();
        assert!(input.ask_owner);
    }

    #[tokio::test]
    async fn an_unanswered_card_neither_sets_nor_declines() {
        let card = Card::new();
        card.suggest("call-1", "the inbox is at zero", true)
            .await
            .unwrap();
        drop(card.approvals.lock().await.remove("call-1"));
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(card.goals().get().unwrap().is_none());
        assert!(card.suggestions.waiting.lock().unwrap().is_empty());
        card.suggest("call-2", "the inbox is at zero", true)
            .await
            .unwrap();
    }

    #[test]
    fn a_long_transcript_keeps_its_end() {
        let rows: Vec<Message> = (0..400)
            .map(|i| said("assistant", &format!("step {i} {}", "x".repeat(1_000))))
            .collect();
        let text = render_transcript(&rows);
        assert!(text.starts_with('['), "the left-out note leads");
        assert!(text.contains("step 399"));
        assert!(!text.contains("step 0 "));
        assert!(text.len() <= TRANSCRIPT_BUDGET_CHARS + 200);

        let tool = Message {
            role: "assistant".into(),
            content: String::new(),
            tool_calls: Some(
                serde_json::json!([{"id": "c", "name": "run_command", "input": {"command": "cargo test"}}]),
            ),
            tool_results: None,
            images: None,
        };
        let result = Message {
            role: "tool".into(),
            content: String::new(),
            tool_calls: None,
            tool_results: Some(
                serde_json::json!([{"tool_call_id": "c", "content": "2 failed", "is_error": true}]),
            ),
            images: None,
        };
        let text = render_transcript(&[tool, result]);
        assert!(text.contains("Assistant called run_command: {\"command\":\"cargo test\"}"));
        assert!(text.contains("Tool error: 2 failed"));
    }

    #[test]
    fn verdicts_parse_and_default_to_not_met_evidence() {
        assert_eq!(
            parse_verdict("{\"met\": false}").unwrap().reason,
            "the transcript does not show it yet"
        );
        let v =
            parse_verdict("{\"met\": true, \"impossible\": true, \"reason\": \"done\"}").unwrap();
        assert!(v.met && !v.impossible, "met wins over impossible");
        assert!(parse_verdict("yes").is_none());
        assert!(parse_verdict("{\"reason\": \"no verdict\"}").is_none());
    }
}
