//! Helpers: one delegation model on the one loop. A helper runs in the
//! background by default, returns only its final message, and reports once
//! through a notification. Status, list and stop are scoped to the caller's
//! own session.
//!
//! [`Helpers`] is the registry every helper path goes through: `delegate`
//! ([`Helpers::delegate`]), `send_message` ([`Helpers::send`]),
//! `read_output` ([`Helpers::read_output`]), `stop_task` ([`Helpers::stop`])
//! and the owner's Stop ([`Helpers::stop_session`]). Every child request is
//! built by [`child::child_request`]; every result is collected by
//! [`collect`]; every completion is delivered by [`notify`].

pub mod child;
pub mod collect;
pub mod door;
pub mod notify;

pub use notify::render_notification;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use super::{Harness, HarnessError, SeatRequest, TurnHandle, TurnInput, TurnMode, TurnRequest};
use crate::session::SessionManager;
use child::Parent;
use types::permissions::Grant;
use types::provenance::ProvenanceClass;

/// Nesting depth cap. A helper at this depth has no helper tool; a launch
/// from it is refused as a backstop, so a helper that keeps delegating
/// can't nest without end.
pub const MAX_DEPTH: u8 = 3;

/// Most helpers running at once in one conversation's tree: the owner's
/// session, its helpers and theirs. A launch past it is refused and told not
/// to retry, so the model waits for results instead of relaunching. It is
/// a runaway brake on one tree, not a width limit on model calls: those take permits from the bot's one
/// adaptive pool (`crate::concurrency`), which stays the only brake on
/// fan-out width (CODE_AUDITOR §15).
pub const MAX_RUNNING: usize = 1000;

/// How long a foreground helper holds its parent's step. Past it the helper
/// moves to the background: it keeps running and reports by notification.
pub const FOREGROUND_BUDGET: Duration = Duration::from_secs(120);

/// A helper that emits nothing for this long is ended, and reports what it
/// had as partial.
pub const INACTIVITY_LIMIT: Duration = Duration::from_secs(10 * 60);

/// A helper turn's waits, as its calls enter them (`ToolContext::waiting`,
/// the request's progress) and its collector reads them.
fn waits_of(req: &TurnRequest) -> Arc<tools::Waiting> {
    req.progress.as_ref().map(|p| p.waiting.clone()).unwrap_or_default()
}

/// The rule key of the helper tool: what Explore and Plan helpers, and a
/// helper at the depth cap, can never call.
pub const HELPER_TOOL: &str = "delegate";

/// The pending-task row kind of a helper.
const ROW_KIND: &str = "helper";

/// What the model asks a helper to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelperSpec {
    pub description: String,
    pub prompt: String,
    pub kind: HelperKind,
    pub background: bool,
    pub isolation: Option<Isolation>,
    /// Skills the parent loaded, `(name, content)`: their instructions go
    /// with the work, written into the helper's thread before its first step.
    pub skills: Vec<(String, String)>,
    /// The speed the helper works at (a model id); `None` = its parent's.
    pub speed: Option<String>,
    /// How long its report may be, in characters, before it is saved to a
    /// file and previewed; `None` = in full up to [`collect::REPORT_LIMIT`].
    pub report_limit: Option<usize>,
}

impl HelperSpec {
    /// A `delegate` call's input: `description` and `prompt` required,
    /// `helper_type` one of the listed types (default general), `background`
    /// default true, `speed` the parent's unless named, `report_limit` the
    /// default unless set.
    pub fn from_input(input: &serde_json::Value) -> Result<Self, String> {
        let text = |key: &str| {
            input
                .get(key)
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        let description = text("description").ok_or("description is required: 3-5 words on what the helper does")?;
        let prompt = text("prompt").ok_or("prompt is required: the whole job, since the helper sees nothing else")?;
        let kind = match text("helper_type") {
            None => HelperKind::General,
            Some(t) => HelperKind::parse(&t).ok_or_else(|| {
                format!("helper_type '{t}' is not a helper type. Use general, explore or plan.")
            })?,
        };
        let background = input.get("background").and_then(|v| v.as_bool()).unwrap_or(true);
        let report_limit = input.get("report_limit").and_then(|v| v.as_u64()).filter(|n| *n > 0).map(|n| n as usize);
        Ok(Self { description, prompt, kind, background, isolation: None, skills: Vec::new(), speed: text("speed"), report_limit })
    }
}

/// A helper's type. Explore and Plan are enforced tool sets: no writes, no
/// mutating shell, no helper tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelperKind {
    General,
    Explore,
    Plan,
}

impl HelperKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "general" => Some(Self::General),
            "explore" => Some(Self::Explore),
            "plan" => Some(Self::Plan),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Explore => "explore",
            Self::Plan => "plan",
        }
    }

    /// Explore and Plan only look.
    fn read_only(self) -> bool {
        matches!(self, Self::Explore | Self::Plan)
    }

    pub const ALL: [HelperKind; 3] = [Self::General, Self::Explore, Self::Plan];

    /// When the type fits, and the tools it has: its line in the helper
    /// types listing, rendered `- type: when to use (Tools: …)` so the model
    /// picks a type by the job in front of it and knows what it can do.
    pub fn when_to_use(self) -> &'static str {
        match self {
            Self::General => "Work of several steps that produces something: researching a batch, a draft, a \
media piece, an outreach batch, a code change. It changes things only as far as its brief allows. (Tools: all \
of yours.)",
            Self::Explore => "Wide searches: when answering means going through many files, pages, listings or \
records and you need only the conclusion, not their contents. It finds and sums up; it doesn't review, audit \
or change anything. Say how wide to look: \"medium\", or \"very thorough\" for several places and naming \
conventions. (Tools: yours that only look; it changes nothing and starts no helpers.)",
            Self::Plan => "Designing how to carry out a piece of work of any kind: it returns the steps, the \
sources or files that matter and the trade-offs. (Tools: yours that only look; it changes nothing and starts \
no helpers.)",
        }
    }
}

/// The helper types a turn in `mode` can start, each with its line (the
/// `helper_types` listing): every type while the run can delegate, none
/// for an explore or plan helper or at the depth cap.
pub fn helper_types(mode: &TurnMode) -> std::collections::BTreeMap<String, String> {
    if !on_surface(mode, HELPER_TOOL) {
        return Default::default();
    }
    HelperKind::ALL.into_iter().map(|k| (k.as_str().to_string(), k.when_to_use().to_string())).collect()
}

/// Where an isolated helper works: its own copy of the project
/// (`crate::worktree::Isolation` prepares it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Isolation {
    Worktree,
}

/// What a launch returned.
#[derive(Debug, Clone)]
pub enum Launch {
    Background { task_id: String },
    Finished(Completion),
}

/// A finished helper, coworker or workflow run.
#[derive(Debug, Clone)]
pub struct Completion {
    pub task_id: String,
    pub description: String,
    pub status: CompletionStatus,
    /// Its words: the report after the question, if any; past the run's
    /// limit, the one spill path's note.
    pub result: String,
    /// The question for the owner this turn asked, if any.
    pub question: Option<collect::Asked>,
    /// What leads a run's report, never spilled: its pending questions
    /// ([`collect::assemble`]).
    pub lead: String,
    pub usage: ai::UsageInfo,
    /// The untrusted content the helper read: its result carries it to
    /// whoever reads it, as a tool result, a notification row or a wake.
    pub taint: Vec<ProvenanceClass>,
}

impl Completion {
    /// The whole report as its reader gets it: the question first, then
    /// the words.
    pub fn report(&self) -> String {
        match (self.lead.trim().is_empty(), self.result.trim().is_empty()) {
            (true, _) => self.result.clone(),
            (false, true) => self.lead.clone(),
            (false, false) => format!("{}\n\n{}", self.lead, self.result),
        }
    }
}

/// The summary words a helper's row keeps beside its report, so a restart
/// knows what the report is: `held` while its own helpers still ran, and
/// `asks_owner` when it waits on the owner's decision.
fn row_tag(held: bool, asks_owner: bool) -> String {
    [(held, ROW_HELD), (asks_owner, ROW_ASKS_OWNER)].iter().filter(|(on, _)| *on).map(|(_, w)| *w).collect::<Vec<_>>().join(" ")
}
const ROW_HELD: &str = "held";
/// The label of the reminder a helper with an open question gets.
const STILL_OPEN: &str = "Your question for the owner is still open";
const ROW_ASKS_OWNER: &str = "asks_owner";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionStatus {
    Done,
    Partial { why: String },
    Failed { error: String },
    Stopped,
}

/// What `delegate` returns for a helper that went to the background. It
/// ends the parent's work on that result: the notification is the only way
/// the result comes back.
pub fn launch_result(task_id: &str) -> String {
    format!(
        "Helper {task_id} is working in the background. You'll get a notification when it \
         finishes. Until then you know nothing about its result: don't report, guess or redo \
         its work. If the owner asks, say it's still running."
    )
}

/// How deep in the helper tree the session `key` sits: 0 for an owner,
/// scheduled or workflow session, 1 for its helpers, and so on. Every child
/// key is `subagent:<parent key>:<task id>` (single, parallel, DAG and
/// continuation alike), so this one count covers every path.
pub fn depth_of(key: &str) -> u8 {
    key.matches("subagent:").count().min(u8::MAX as usize) as u8
}

pub use types::keyparser::split_helper_key;

/// Whether the session `key` is `ancestor` or a helper's under it, at any
/// depth.
fn under(key: &str, ancestor: &str) -> bool {
    let mut key = key;
    loop {
        if key == ancestor {
            return true;
        }
        match split_helper_key(key) {
            Some((parent, _)) => key = parent,
            None => return false,
        }
    }
}

/// The session at the top of the helper tree `key` belongs to: `key`
/// itself for any session that is not a helper's.
fn root_of(key: &str) -> &str {
    let mut key = key;
    while let Some((parent, _)) = split_helper_key(key) {
        key = parent;
    }
    key
}

/// The session key of helper `task_id` of `parent_key`.
pub fn helper_key(parent_key: &str, task_id: &str) -> String {
    format!("subagent:{parent_key}:{task_id}")
}

/// Whether a turn in `mode` is offered the tool `tool_name`: listed, and
/// for the helper tool, able to start one. The helper tool never for Explore
/// or Plan helpers or at the depth cap; the workflow `exit` primitive only
/// to a workflow activity. Every run declares the same tools: what a run is
/// not offered stays declared and [`permits`] refuses the call.
pub fn on_surface(mode: &TurnMode, tool_name: &str) -> bool {
    match mode {
        TurnMode::Helper { kind, depth, .. } if tool_name == HELPER_TOOL => {
            !kind.read_only() && *depth < MAX_DEPTH
        }
        TurnMode::Helper { .. } if tool_name == GOAL_TOOL => false,
        TurnMode::Workflow(_) => true,
        _ => tool_name != EXIT_TOOL,
    }
}

/// The workflow primitive that ends an activity early.
const EXIT_TOOL: &str = "exit";

/// The tool that proposes the owner's agreed goal. A goal is the owner's,
/// agreed in their own conversation: a helper never proposes one. The tool
/// is left off every helper's tools, and a call from a helper is refused as
/// a backstop.
const GOAL_TOOL: &str = "suggest_goal";

/// Whether a turn in `mode` may make the call `target`. A helper's kind is
/// an enforced tool set, a helper at the depth cap cannot delegate, and only
/// a workflow activity can exit one, whatever tool shape the call arrives
/// in. The tools every run declares are the same; this is where a run's own
/// narrower set holds.
pub fn permits(mode: &TurnMode, target: &types::permissions::Target) -> Result<(), String> {
    if target.tool == EXIT_TOOL && !matches!(mode, TurnMode::Workflow(_)) {
        return Err(
            "exit ends a workflow activity, and this run isn't one. Finish the work, or say plainly why you \
             are stopping."
                .to_string(),
        );
    }
    let TurnMode::Helper { kind, depth, .. } = mode else {
        return Ok(());
    };
    if target.tool == GOAL_TOOL {
        return Err(
            "A goal is agreed with the owner in their own conversation; a helper doesn't propose one. Do \
             the task you were given and report."
                .to_string(),
        );
    }
    let delegating = target.key == HELPER_TOOL;
    if kind.read_only() && (delegating || !target.read_only) {
        return Err(format!(
            "A {} helper only looks: {} changes something or starts a helper. Report what you \
             found and what should change instead.",
            kind.as_str(),
            target.key
        ));
    }
    if delegating && *depth >= MAX_DEPTH {
        return Err(format!(
            "Helpers can't nest deeper than {MAX_DEPTH}. Do this part yourself with your own tools."
        ));
    }
    Ok(())
}

/// Starts one turn. The harness facade implements it; the helper registry
/// runs every helper turn through it.
#[async_trait::async_trait]
pub trait TurnStarter: Send + Sync {
    async fn start_turn(&self, req: TurnRequest) -> Result<TurnHandle, HarnessError>;
}

#[async_trait::async_trait]
impl TurnStarter for Harness {
    async fn start_turn(&self, req: TurnRequest) -> Result<TurnHandle, HarnessError> {
        Harness::start_turn(self, req).await
    }
}

/// A helper's progress for the owner's screen, keyed by the session that
/// started it. Never text in anyone's conversation.
#[derive(Debug, Clone)]
pub struct HelperEvent {
    pub parent_session_key: String,
    pub event: ai::StreamEvent,
}

/// One line of a caller's helper list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelperStatus {
    pub task_id: String,
    pub description: String,
    pub running: bool,
    /// The helper's own session, where its steps are kept.
    pub session_key: String,
    /// The call it is making now, by its structured name
    /// (`tools::humanize::call`); none before its first step.
    pub call: Option<tools::humanize::Call>,
    /// Background work's own progress line ("Searching 4 angles"); empty
    /// for a helper that works by calls.
    pub activity: String,
    /// The session that started it, where it reports.
    pub parent_key: String,
    /// The employee it works for (its parent's seat); empty for the main
    /// bot.
    pub agent_id: String,
    /// When its current run started (unix seconds).
    pub started_at: i64,
    /// It finished its turn and holds its report for work it started.
    pub waiting: bool,
}

/// A helper this process started.
struct Helper {
    parent_key: String,
    session_key: String,
    description: String,
    kind: HelperKind,
    /// The speed it was started at; `None` = its parent's.
    speed: Option<String>,
    /// How long its report may be before it is saved and previewed.
    report_limit: usize,
    /// The seat the helper was built from (its parent's), kept so a
    /// notification turn is built by the same constructor.
    parent_seat: SeatRequest,
    /// The parent's grant, the helper's ceiling.
    parent_grant: Option<types::permissions::Grant>,
    /// The phone app's platform the parent turn came from
    /// (`Delivery::platform`), kept for its notification turns.
    parent_platform: Option<String>,
    running: bool,
    cancel: CancellationToken,
    /// A foreground launch waiting on this helper's first completion.
    waiter: Option<oneshot::Sender<Completion>>,
    /// The run's report, assembled, held back while the helper's own
    /// helpers still run. Its turns stay in `earlier_reports`: a run that
    /// goes on reassembles from them, never from this.
    held: Option<Completion>,
    /// The turns of this run so far, as each turn reported them (never
    /// assembled): the run's report is assembled from them
    /// ([`collect::assemble`]).
    earlier_reports: Vec<Completion>,
    /// Its questions for the owner: each pending until the conversation
    /// that started it sends an answer (`send_message` with `answer`),
    /// whatever its own later turns or other messages say.
    questions: collect::Questions,
    /// An answer that reached it while a turn ran: it settles a question
    /// that same turn asks (the turn may have asked before it read it).
    answer_in_turn: Option<String>,
    /// It was stopped (`stop_task`, or its parent stopping): it never runs
    /// again on its own, and its report says stopped, never done.
    stopped: bool,
    /// The call it is making now, as the owner's screen was sent it.
    call: Option<tools::humanize::Call>,
    /// Background work's latest progress line, as the owner's screen was
    /// sent it.
    activity: String,
    /// When its current run started (unix seconds).
    started_at: i64,
}

impl Helper {
    /// Stop it: its turn's token, and a report it holds says stopped.
    fn stop(&mut self) {
        self.cancel.cancel();
        self.stopped = true;
        if let Some(held) = &mut self.held {
            held.status = CompletionStatus::Stopped;
        }
    }

    /// Keep a turn of this run. A question it asked is pending, unless an
    /// answer reached the helper while that turn ran.
    fn keep_turn(&mut self, c: Completion) {
        let answer = self.answer_in_turn.take();
        if let Some(q) = &c.question {
            match answer {
                Some(a) => self.questions.answered.push((q.clone(), a)),
                // The same question asked again is the one already open.
                None if self.questions.pending.iter().any(|p| p.text == q.text) => {}
                None => self.questions.pending.push(q.clone()),
            }
        }
        self.earlier_reports.push(c);
    }

    /// An answer reached the helper: it settles the open question it
    /// names (the newest when it names none). With none open and a turn
    /// that was already running when it came, it settles the question that
    /// turn asks. `running_before` is whether a turn ran before this send:
    /// a send that starts the turn is never carried into it.
    fn answered(&mut self, answer: &tools::orchestrator::Answer, running_before: bool) {
        if !self.questions.answer(&answer.text, answer.question.as_deref()) && running_before && answer.question.is_none() {
            self.answer_in_turn = Some(answer.text.clone());
        }
    }

    /// The run's report so far: its turns and pending question, held to its
    /// report limit and read back the way its parent reads files
    /// ([`collect::assemble`]). The one report a parent, a row and a
    /// restart see.
    /// `fresh_only` for a report its parent gets: a pending question it
    /// was already given is named, not repeated.
    fn assembled(&self, spill_dir: &std::path::Path, fresh_only: bool) -> Completion {
        let reader = tools::result_shape::Reader::of(&self.parent_seat.door);
        collect::assemble(&self.earlier_reports, &self.questions, fresh_only, spill_dir, self.report_limit, reader)
    }
}

/// The `helper_kind` a background work run is stored under: it runs code,
/// not a model turn, so it takes no messages.
const WORK_KIND: &str = "research";

#[derive(Default)]
struct State {
    helpers: HashMap<String, Helper>,
    /// One stop token per session. Every helper's token derives from its
    /// parent session's, so the owner's Stop reaches every descendant,
    /// including background helpers from earlier turns.
    stops: HashMap<String, CancellationToken>,
    /// Launches in progress per helper tree (its top session), counted
    /// with the running helpers against [`MAX_RUNNING`].
    starting: HashMap<String, usize>,
}

/// A launch's place in its tree, given back when the launch ends: by then
/// the helper is running (and counted as such) or was never started.
struct Place<'a> {
    helpers: &'a Helpers,
    root: String,
}

impl Drop for Place<'_> {
    fn drop(&mut self) {
        self.helpers.state().release_place(&self.root);
    }
}

impl State {
    fn session_token(&mut self, key: &str) -> CancellationToken {
        self.stops.entry(key.to_string()).or_default().clone()
    }

    /// Whether work the session `key` started is still going, at any
    /// depth: a helper or research run that runs, or one that finished its
    /// turn and holds its report for work of its own.
    fn work_below(&self, key: &str) -> bool {
        self.helpers.values().any(|h| h.parent_key == key && (h.running || h.held.is_some() || self.work_below(&h.session_key)))
    }

    /// Stop everything under the session `key` (not `key` itself): every
    /// helper and research run below it, whatever turn started it. Each is
    /// marked stopped, so a report it holds goes up as stopped and nothing
    /// runs it again. Returns their ids.
    fn stop_below(&mut self, key: &str) -> Vec<String> {
        let mut stopped = Vec::new();
        for (id, h) in self.helpers.iter_mut().filter(|(_, h)| h.session_key != key && under(&h.session_key, key)) {
            if h.running || h.held.is_some() {
                h.stop();
                stopped.push(id.clone());
            }
        }
        stopped
    }

    /// Take a place for a helper launching in the tree `key` belongs to, and
    /// return the tree's top session; `Err` with the count when its running
    /// helpers and the launches in progress already reach [`MAX_RUNNING`]. Counting and taking under one lock keeps
    /// several delegate calls of one response from all passing the count.
    fn take_place(&mut self, key: &str) -> Result<String, usize> {
        let root = root_of(key).to_string();
        let running = self.helpers.values().filter(|h| h.running && root_of(&h.session_key) == root).count();
        let starting = self.starting.entry(root.clone()).or_default();
        if running + *starting >= MAX_RUNNING {
            return Err(running + *starting);
        }
        *starting += 1;
        Ok(root)
    }

    fn release_place(&mut self, root: &str) {
        if let Some(n) = self.starting.get_mut(root) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                self.starting.remove(root);
            }
        }
    }

    /// Drop a helper that is done with nothing running under it, and the
    /// finished helpers it started. Its row stays: `read_output` and a
    /// resuming `send` read that.
    fn forget(&mut self, task_id: &str) {
        let Some(h) = self.helpers.remove(task_id) else {
            return;
        };
        self.stops.remove(&h.session_key);
        let children: Vec<String> = self
            .helpers
            .iter()
            .filter(|(_, c)| c.parent_key == h.session_key && !c.running)
            .map(|(id, _)| id.clone())
            .collect();
        for id in children {
            self.forget(&id);
        }
    }
}

/// What a crashed helper's panic said.
fn panic_text(panic: &(dyn std::any::Any + Send)) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "it stopped unexpectedly".to_string())
}

/// The helper registry.
pub struct Helpers {
    store: Arc<db::Store>,
    sessions: Arc<SessionManager>,
    starter: Arc<dyn TurnStarter>,
    /// The server's wake rail: an owner session with a new notification
    /// row on the wake table is sent here to be delivered.
    wake: Option<mpsc::UnboundedSender<String>>,
    /// Where helper progress goes for the owner's screen.
    ui: Option<mpsc::UnboundedSender<HelperEvent>>,
    foreground_budget: Duration,
    state: Mutex<State>,
}

impl Helpers {
    pub fn new(
        store: Arc<db::Store>,
        sessions: Arc<SessionManager>,
        starter: Arc<dyn TurnStarter>,
        wake: Option<mpsc::UnboundedSender<String>>,
        ui: Option<mpsc::UnboundedSender<HelperEvent>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            sessions,
            starter,
            wake,
            ui,
            foreground_budget: FOREGROUND_BUDGET,
            state: Mutex::new(State::default()),
        })
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The stop token of session `key`. Each turn's cancel token is its
    /// child, and so is every helper the session starts.
    pub fn session_token(&self, key: &str) -> CancellationToken {
        self.state().session_token(key)
    }

    /// The owner's Stop on session `key` (`None`: on every session): every
    /// helper under it stops, whichever turn started it. The session's next
    /// turn gets a fresh token.
    pub fn stop_session(&self, key: Option<&str>) {
        let mut state = self.state();
        let stopped: Vec<CancellationToken> = match key {
            Some(key) => state.stops.remove(key).into_iter().collect(),
            None => state.stops.drain().map(|(_, token)| token).collect(),
        };
        drop(state);
        for token in stopped {
            token.cancel();
        }
    }

    /// The `delegate` tool: parse the call, launch, and word the result.
    pub async fn delegate(
        self: &Arc<Self>,
        turn: &TurnRequest,
        grant: Option<&Grant>,
        run_taint: &[ProvenanceClass],
        input: &serde_json::Value,
    ) -> Result<String, String> {
        let spec = HelperSpec::from_input(input)?;
        Ok(match self.launch(turn, grant, run_taint, spec).await? {
            Launch::Background { task_id } => launch_result(&task_id),
            Launch::Finished(c) => notify::render_foreground(&c),
        })
    }

    /// Start a helper for the running turn `turn`, which holds `grant`. A
    /// background helper returns at once; a foreground one returns its
    /// completion, or moves to the background when it outlasts the
    /// foreground budget.
    pub async fn launch(
        self: &Arc<Self>,
        turn: &TurnRequest,
        grant: Option<&Grant>,
        run_taint: &[ProvenanceClass],
        spec: HelperSpec,
    ) -> Result<Launch, String> {
        let background = spec.background;
        let (task_id, mut rx) = self.start(turn, grant, run_taint, spec).await?;
        if background {
            return Ok(Launch::Background { task_id });
        }
        match tokio::time::timeout(self.foreground_budget, &mut rx).await {
            Ok(Ok(c)) => Ok(Launch::Finished(c)),
            Ok(Err(_)) => Ok(Launch::Background { task_id }),
            Err(_) => {
                // Past the budget: take the waiter back so the helper reports
                // by notification. If it finished in the same instant, its
                // completion is already on the way to us.
                let reclaimed = self
                    .state()
                    .helpers
                    .get_mut(&task_id)
                    .and_then(|h| h.waiter.take())
                    .is_some();
                if reclaimed {
                    info!(task_id = %task_id, "foreground helper moved to the background");
                    return Ok(Launch::Background { task_id });
                }
                match rx.await {
                    Ok(c) => Ok(Launch::Finished(c)),
                    Err(_) => Ok(Launch::Background { task_id }),
                }
            }
        }
    }

    /// Admit and spawn a helper; its completion comes back on the receiver
    /// unless it runs in the background.
    async fn start(
        self: &Arc<Self>,
        turn: &TurnRequest,
        grant: Option<&Grant>,
        run_taint: &[ProvenanceClass],
        spec: HelperSpec,
    ) -> Result<(String, oneshot::Receiver<Completion>), String> {
        let parent_key = turn.session_key.as_str();
        if !on_surface(&turn.mode, "delegate") || depth_of(parent_key) >= MAX_DEPTH {
            return Err(format!(
                "This helper can't start helpers of its own (the limit is {MAX_DEPTH} levels, and \
                 explore and plan helpers never can). Do this part yourself with your own tools."
            ));
        }
        let taken = self.state().take_place(parent_key);
        let _place = match taken {
            Ok(root) => Place { helpers: self, root },
            Err(running) => {
                warn!(parent = %parent_key, running, "helper refused: the conversation's running-helper limit");
                return Err(format!(
                    "{MAX_RUNNING} helpers are already running in this conversation, the most at once. Don't \
                     retry: do this part yourself with your own tools, or wait for one to report."
                ));
            }
        };
        let task_id = format!("h-{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
        let session_key = helper_key(parent_key, &task_id);
        let parent_seat = &turn.seat;

        let isolation = match spec.isolation {
            Some(Isolation::Worktree) => Some(isolate(parent_seat, grant, &task_id).await?),
            None => None,
        };
        let mut brief = spec.prompt.clone();
        if let Some(iso) = &isolation {
            brief = format!("{}{brief}", crate::worktree::preamble(iso));
        }
        let copy = isolation.as_ref().map(|iso| iso.path().to_string_lossy().into_owned());

        let inputs = serde_json::json!({
            "prompt": spec.prompt,
            "description": spec.description,
            "user_id": parent_seat.user_id,
            "helper_kind": spec.kind.as_str(),
            "speed": spec.speed,
            "report_limit": spec.report_limit,
        })
        .to_string();
        let created = self
            .store
            .engine_create_run(&db::NewRun {
                id: &task_id,
                kind: ROW_KIND,
                session_key: &session_key,
                agent_id: &parent_seat.agent_id,
                lane: "subagent",
                inputs: Some(&inputs),
                ..Default::default()
            })
            .map_err(|e| format!("Could not start the helper: {e}"));
        if let Err(e) = created {
            if let Some(iso) = &isolation {
                crate::worktree::merge_all(std::slice::from_ref(iso), "nebo: helper not started").await;
            }
            return Err(e);
        }
        let _ = self.store.update_task_running(&task_id);

        let (tx, rx) = oneshot::channel();
        let req = {
            let mut state = self.state();
            let cancel = state.session_token(parent_key).child_token();
            state.stops.insert(session_key.clone(), cancel.clone());
            let parent = Parent {
                session_key: parent_key,
                seat: parent_seat,
                grant,
                run_taint,
                cancel: cancel.clone(),
                platform: turn.delivery.platform.as_deref(),
            };
            let req = child::child_request(&parent, &task_id, &spec, copy.as_deref(), TurnInput::Platform { text: brief });
            state.helpers.insert(
                task_id.clone(),
                Helper {
                    parent_key: parent_key.to_string(),
                    session_key,
                    description: spec.description.clone(),
                    kind: spec.kind,
                    speed: spec.speed.clone(),
                    report_limit: collect::report_limit(spec.report_limit),
                    parent_seat: parent_seat.clone(),
                    parent_grant: grant.cloned(),
                    parent_platform: turn.delivery.platform.clone(),
                    running: true,
                    cancel,
                    waiter: (!spec.background).then_some(tx),
                    held: None,
                    earlier_reports: Vec::new(),
                    questions: Default::default(),
                    answer_in_turn: None,
                    stopped: false,
                    call: None,
                    activity: String::new(),
                    started_at: chrono::Utc::now().timestamp(),
                },
            );
            req
        };
        if !spec.skills.is_empty() {
            self.preload_skills(&helper_key(parent_key, &task_id), &parent_seat.user_id, spec.skills);
        }
        info!(task_id = %task_id, parent = %parent_key, background = spec.background, "helper launched");
        self.spawn_turn(task_id.clone(), req, isolation);
        Ok((task_id, rx))
    }

    /// Start `work` as a background helper of the running turn `turn`: code
    /// that is not a model turn (the deep-research pipeline). It is one of
    /// the caller's helpers like any other: its row, its progress on the
    /// owner's screen, the session's stop token (the owner's Stop and
    /// stop_task reach it), and one notification when it ends. It takes no
    /// messages. Returns the launch receipt.
    pub fn start_work(self: &Arc<Self>, turn: &TurnRequest, description: &str, work: tools::orchestrator::Work) -> Result<(String, String), String> {
        let parent_key = turn.session_key.clone();
        let task_id = format!("h-{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
        let session_key = helper_key(&parent_key, &task_id);
        let inputs = serde_json::json!({ "description": description, "helper_kind": WORK_KIND }).to_string();
        self.store
            .engine_create_run(&db::NewRun {
                id: &task_id,
                kind: ROW_KIND,
                session_key: &session_key,
                agent_id: &turn.seat.agent_id,
                lane: "subagent",
                inputs: Some(&inputs),
                ..Default::default()
            })
            .map_err(|e| format!("Could not start {description}: {e}"))?;
        let _ = self.store.update_task_running(&task_id);
        let cancel = {
            let mut state = self.state();
            let cancel = state.session_token(&parent_key).child_token();
            state.stops.insert(session_key.clone(), cancel.clone());
            state.helpers.insert(
                task_id.clone(),
                Helper {
                    parent_key: parent_key.clone(),
                    session_key,
                    description: description.to_string(),
                    kind: HelperKind::General,
                    speed: None,
                    report_limit: collect::REPORT_LIMIT,
                    parent_seat: turn.seat.clone(),
                    parent_grant: None,
                    parent_platform: turn.delivery.platform.clone(),
                    running: true,
                    cancel: cancel.clone(),
                    waiter: None,
                    held: None,
                    earlier_reports: Vec::new(),
                    questions: Default::default(),
                    answer_in_turn: None,
                    stopped: false,
                    call: None,
                    activity: String::new(),
                    started_at: chrono::Utc::now().timestamp(),
                },
            );
            cancel
        };
        info!(task_id = %task_id, parent = %parent_key, "background work launched");
        let this = Arc::clone(self);
        let (id, description) = (task_id.clone(), description.to_string());
        tokio::spawn(async move {
            let run = std::panic::AssertUnwindSafe(this.run_work(&id, &parent_key, &description, cancel, work));
            let completion = match futures::FutureExt::catch_unwind(run).await {
                Ok(completion) => completion,
                Err(panic) => this.crashed(&id, &panic_text(&*panic)),
            };
            this.finish(&id, completion);
        });
        let receipt = launch_result(&task_id);
        Ok((task_id, receipt))
    }

    /// One helper of background work (the research pipeline) whose final
    /// answer is read as data: a turn on the one loop, under the work's own
    /// seat (the caller's, as its helpers run), stopped with it. `schema` is
    /// the answer's shape (the turn can't end on another,
    /// `turn_end::AnswerShapeCheck`), `tools` the only tools it may call.
    /// `node` names it under the work's session (`sa-<node>`, its browser
    /// tab too). Every event of its turn bumps `activity`. Returns the
    /// answer's JSON object.
    pub async fn answer_as_data(
        self: &Arc<Self>,
        work_key: &str,
        node: &str,
        prompt: String,
        schema: serde_json::Value,
        tools: Vec<String>,
        activity: Option<Arc<std::sync::atomic::AtomicU64>>,
    ) -> Result<serde_json::Value, String> {
        let (seat, grant, platform, cancel) = {
            let state = self.state();
            let work = state
                .helpers
                .values()
                .find(|h| h.session_key == work_key)
                .ok_or_else(|| format!("{work_key} is not running work"))?;
            (work.parent_seat.clone(), work.parent_grant.clone(), work.parent_platform.clone(), work.cancel.child_token())
        };
        let spec = HelperSpec {
            description: node.to_string(),
            prompt: prompt.clone(),
            kind: HelperKind::General,
            background: false,
            isolation: None,
            skills: Vec::new(),
            speed: None,
            report_limit: None,
        };
        let parent = Parent {
            session_key: work_key,
            seat: &seat,
            grant: grant.as_ref(),
            run_taint: &[],
            cancel: cancel.clone(),
            platform: platform.as_deref(),
        };
        let mut req = child::child_request(&parent, &format!("sa-{node}"), &spec, None, TurnInput::Platform { text: prompt });
        let allowed: std::collections::HashSet<String> = tools.into_iter().collect();
        req.seat.tool_allowlist = Some(match req.seat.tool_allowlist.take() {
            Some(inherited) => inherited.intersection(&allowed).cloned().collect(),
            None => allowed,
        });
        if let TurnMode::Helper { answer, .. } = &mut req.mode {
            *answer = Some(Arc::new(schema.clone()));
        }
        let waiting = waits_of(&req);
        let collected = match self.starter.start_turn(req).await {
            Ok(handle) => {
                collect::collect(handle.events, &cancel, INACTIVITY_LIMIT, &waiting, |_| {
                    if let Some(a) = &activity {
                        a.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                })
                .await
            }
            Err(e) => return Err(e.to_string()),
        };
        if let Some(error) = collected.error {
            return Err(error);
        }
        if collected.cancelled {
            return Err("stopped".into());
        }
        let issues = super::turn_end::answer_issues(&schema, &collected.final_message);
        if !issues.is_empty() {
            return Err(format!("its answer didn't match the schema: {}", issues.join("; ")));
        }
        super::turn_end::answer_object(&collected.final_message).ok_or_else(|| "it gave no answer".to_string())
    }

    /// Run background work to its end: its progress goes to the owner's
    /// screen as the helper's activity, and a stop ends it as stopped.
    async fn run_work(
        self: &Arc<Self>,
        task_id: &str,
        parent_key: &str,
        description: &str,
        cancel: CancellationToken,
        work: tools::orchestrator::Work,
    ) -> Completion {
        self.emit(parent_key, ai::StreamEvent::subagent_start(task_id, description));
        let (progress_tx, mut progress_rx) = mpsc::channel::<ai::StreamEvent>(64);
        let forward = {
            let this = Arc::clone(self);
            let (parent_key, task_id) = (parent_key.to_string(), task_id.to_string());
            tokio::spawn(async move {
                while let Some(mut ev) = progress_rx.recv().await {
                    let mut widgets = ev.widgets.take().unwrap_or_else(|| serde_json::json!({}));
                    widgets["task_id"] = serde_json::json!(task_id);
                    if !ev.text.is_empty() {
                        widgets["current_operation"] = serde_json::json!(ev.text);
                    }
                    ev.widgets = Some(widgets);
                    ev.event_type = ai::StreamEventType::SubagentProgress;
                    this.emit(&parent_key, ev);
                }
            })
        };
        let outcome = tokio::select! {
            r = work(cancel.clone(), progress_tx, helper_key(parent_key, task_id)) => Some(r),
            _ = cancel.cancelled() => None,
        };
        let _ = forward.await;
        let (status, result) = match outcome {
            None => (CompletionStatus::Stopped, String::new()),
            Some(_) if cancel.is_cancelled() => (CompletionStatus::Stopped, String::new()),
            Some(Ok(report)) => (CompletionStatus::Done, report),
            Some(Err(error)) => (CompletionStatus::Failed { error }, String::new()),
        };
        match &status {
            CompletionStatus::Done | CompletionStatus::Partial { .. } => {
                let _ = self.store.update_task_completed(task_id, Some(&result));
            }
            CompletionStatus::Failed { error } => {
                let _ = self.store.update_task_failed(task_id, error);
            }
            CompletionStatus::Stopped => {
                let _ = self.store.cancel_task(task_id);
            }
        }
        self.emit(parent_key, ai::StreamEvent::subagent_complete(task_id, description, status == CompletionStatus::Done));
        Completion {
            task_id: task_id.to_string(),
            description: description.to_string(),
            status,
            result,
            question: None,
            lead: String::new(),
            usage: ai::UsageInfo::default(),
            taint: Vec::new(),
        }
    }

    /// Write the skills the parent loaded into the helper's thread, before
    /// its first step: the row a checkpoint restores skills with.
    fn preload_skills(&self, session_key: &str, user_id: &str, skills: Vec<(String, String)>) {
        let written = self.sessions.get_or_create(session_key, user_id).map_err(|e| e.to_string()).and_then(|s| {
            let mut reminders = super::reminders::Reminders::default();
            reminders.add(&super::events::TurnEvent::InvokedSkills(skills));
            reminders.write(&self.sessions, &s.id).map_err(|e| e.to_string())
        });
        if let Err(e) = written {
            warn!(session = %session_key, error = %e, "the parent's skills could not be written for the helper");
        }
    }

    /// `send_message` to one of the caller's helpers. A running helper hears
    /// it at its next step; a finished one runs again from its history with
    /// the message as its next input, in the background.
    pub async fn send(
        self: &Arc<Self>,
        turn: &TurnRequest,
        grant: Option<&Grant>,
        run_taint: &[ProvenanceClass],
        task_id: &str,
        message: &str,
        answer: Option<tools::orchestrator::Answer>,
    ) -> Result<String, String> {
        let caller = turn.session_key.as_str();
        let row = self.own_row(caller, task_id, false)?;
        let answer = answer.filter(|a| !a.text.trim().is_empty());
        // An answer settles the open question it names, the newest when it
        // names none; one that names a question the helper doesn't have
        // open goes nowhere.
        let asked = match &answer {
            Some(a) => {
                let open = match self.state().helpers.get(task_id) {
                    Some(h) => h.questions.clone(),
                    None => questions_of_row(&self.store, task_id),
                };
                match (open.open(a.question.as_deref()), &a.question) {
                    (Some(q), _) => Some(q.text.clone()),
                    (None, Some(id)) => {
                        return Err(format!(
                            "Helper {task_id} has no open question {id}. Nothing was sent. Its open questions are in \
                             its latest report; send again with one of their ids, or without `question` for the newest."
                        ));
                    }
                    (None, None) => None,
                }
            }
            None => None,
        };
        // An answer reads as the owner's answer in the helper's thread; it
        // and only it settles the helper's pending questions.
        let text = match (&answer, asked) {
            (Some(a), Some(q)) => format!("The owner's answer to your question \"{q}\": {}", a.text.trim()),
            (Some(a), None) => format!("The owner's answer to your question: {}", a.text.trim()),
            (None, _) => message.to_string(),
        };
        let text = match &answer {
            Some(a) if !message.trim().is_empty() && message.trim() != a.text.trim() => format!("{text}\n\n{message}"),
            _ => text,
        };
        let message = text.as_str();
        if spec_kind_of_row(&self.store, &row).as_deref() == Some(WORK_KIND) {
            return Err(format!(
                "{task_id} is a research run, not a helper you can talk to: it takes no messages. \
                 Its report comes as a notification; for a follow-up, start a new one."
            ));
        }
        let from = crate::harness::conversation::MidTurnFrom::Parent {
            session_key: caller.to_string(),
            task_id: task_id.to_string(),
            taint: run_taint.to_vec(),
        };
        let deliver = |sessions: &SessionManager| -> Result<(), String> {
            let id = sessions
                .get_or_create(&row.session_key, &turn.seat.user_id)
                .map_err(|e| format!("Could not reach helper {task_id}: {e}"))?
                .id;
            sessions
                .append_message(&id, "user", message, None, None, Some(&from.metadata()))
                .map(|_| ())
                .map_err(|e| format!("Could not deliver the message to helper {task_id}: {e}"))
        };

        // Decide and deliver under the lock the helper's finish takes, so a
        // message either reaches a running turn or finds the helper finished.
        let resume = {
            let mut state = self.state();
            if let Some(h) = state.helpers.get_mut(task_id)
                && h.running
            {
                deliver(&self.sessions)?;
                if let Some(a) = &answer {
                    h.answered(a, true);
                    // Its row says so at once: a restart doesn't raise an
                    // answered question again.
                    self.keep_questions(task_id, &h.questions);
                    if !h.earlier_reports.is_empty() {
                        let c = h.assembled(&self.spill_dir(&h.parent_key), false);
                        self.keep_on_row(task_id, &c, false, false);
                    } else {
                        self.tag_row(task_id, false, !h.questions.pending.is_empty());
                    }
                }
                info!(task_id = %task_id, "message delivered into a running helper");
                return Ok(format!(
                    "Helper {task_id} has your message and hears it at its next step. Its report \
                     comes as a notification."
                ));
            }
            deliver(&self.sessions)?;
            let spec = spec_of_row(&self.store, &row);
            let cancel = state.session_token(caller).child_token();
            state.stops.insert(row.session_key.clone(), cancel.clone());
            let parent = Parent {
                session_key: caller,
                seat: &turn.seat,
                grant,
                run_taint,
                cancel: cancel.clone(),
                platform: turn.delivery.platform.as_deref(),
            };
            let req = child::child_request(&parent, task_id, &spec, None, TurnInput::None);
            let helper = state.helpers.entry(task_id.to_string()).or_insert_with(|| Helper {
                parent_key: caller.to_string(),
                session_key: row.session_key.clone(),
                description: spec.description.clone(),
                kind: spec.kind,
                speed: spec.speed.clone(),
                report_limit: collect::report_limit(spec.report_limit),
                parent_seat: turn.seat.clone(),
                parent_grant: grant.cloned(),
                parent_platform: turn.delivery.platform.clone(),
                running: false,
                cancel: cancel.clone(),
                waiter: None,
                held: None,
                earlier_reports: Vec::new(),
                // Its questions outlive the run that asked them: a plain
                // message never clears one.
                questions: questions_of_row(&self.store, task_id),
                answer_in_turn: None,
                stopped: false,
                call: None,
                activity: String::new(),
                started_at: chrono::Utc::now().timestamp(),
            });
            // The answer is for a question it already asked: settled before
            // the turn it starts, never carried into it.
            if let Some(a) = &answer {
                helper.answered(a, false);
                self.keep_questions(task_id, &helper.questions);
            }
            // A message from the conversation that started it runs it again,
            // stopped or not: it asked for more.
            helper.stopped = false;
            helper.running = true;
            helper.started_at = chrono::Utc::now().timestamp();
            helper.cancel = cancel;
            helper.parent_seat = turn.seat.clone();
            helper.parent_grant = grant.cloned();
            helper.parent_platform = turn.delivery.platform.clone();
            // An answer settles its pending questions; any other message
            // leaves them pending. A report held while its own helpers ran
            // is never dropped: its turns stay in the run, and the held copy
            // goes.
            helper.held = None;
            let restore = (answer.is_some() && !helper.earlier_reports.is_empty())
                .then(|| helper.assembled(&self.spill_dir(&helper.parent_key), false));
            if let Some(c) = restore {
                self.keep_on_row(task_id, &c, false, false);
            }
            req
        };
        self.running_again(task_id);
        self.spawn_turn(task_id.to_string(), resume, None);
        Ok(launch_result(task_id))
    }

    /// `read_output` for one of the caller's helpers.
    pub fn read_output(&self, caller: &str, task_id: &str) -> Result<String, String> {
        let row = self.own_row(caller, task_id, true)?;
        let description = row.description.clone().unwrap_or_default();
        match self.state().helpers.get(task_id) {
            Some(h) if h.running => return Ok(format!("Helper {task_id} \"{description}\" is still running.")),
            Some(h) if h.held.is_some() => {
                return Ok(format!(
                    "Helper {task_id} \"{description}\" finished its turn and waits on work it started; its report \
                     comes when that work is done."
                ));
            }
            _ => {}
        }
        let mut out = format!("Helper {task_id} \"{description}\": {}", row.status);
        if let Some(output) = row.output.as_deref() {
            out.push('\n');
            out.push_str(output);
        }
        if let Some(error) = row.last_error.as_deref() {
            out.push_str(&format!("\nError: {error}"));
        }
        Ok(out)
    }

    /// `stop_task` for a helper or research run the caller started, at any
    /// depth: it stops, and so does everything under it. A stopped helper's
    /// row says so, so a restart doesn't report it as done.
    pub fn stop(&self, caller: &str, task_id: &str) -> Result<String, String> {
        let row = self.own_row(caller, task_id, true)?;
        let mut state = self.state();
        let mut stopped = state.stop_below(&row.session_key);
        let below = stopped.len();
        let this = match state.helpers.get_mut(task_id) {
            Some(h) if h.running || h.held.is_some() => {
                h.stop();
                stopped.push(task_id.to_string());
                true
            }
            _ => false,
        };
        drop(state);
        for id in &stopped {
            let _ = self.store.cancel_task(id);
        }
        Ok(match (this, below) {
            (true, 0) => format!("Stopping helper {task_id}."),
            (true, n) => format!("Stopping helper {task_id} and {n} under it."),
            (false, n) if n > 0 => format!("Helper {task_id} had finished; stopping the {n} still running under it."),
            _ => format!("Helper {task_id} is not running."),
        })
    }

    /// Helpers a restart interrupted: each one fails, and an owner session
    /// that started one is told, with the report its row keeps (assembled
    /// and held to its limit, its pending question first). A nested
    /// helper's parent that was holding its report for it reports now
    /// ([`Helpers::release_held_after_restart`]).
    pub fn recover(self: &Arc<Self>) {
        let runs = match self.store.engine_live_runs_of_kind(ROW_KIND) {
            Ok(runs) => runs,
            Err(e) => {
                warn!(error = %e, "helpers interrupted by a restart could not be read");
                return;
            }
        };
        for run in runs {
            if self.state().helpers.contains_key(&run.id) {
                continue;
            }
            let error = "interrupted by a restart; send_message continues it from where it stopped";
            if let Err(e) = self.store.engine_set_run_state(&run.id, "failed", chrono::Utc::now().timestamp(), Some(error)) {
                warn!(task_id = %run.id, error = %e, "an interrupted helper could not be marked failed");
                continue;
            }
            let Some(parent_key) = split_helper_key(&run.session_key).filter(|(_, id)| *id == run.id).map(|(p, _)| p) else {
                continue;
            };
            info!(task_id = %run.id, parent = %parent_key, "a helper was interrupted by a restart");
            if depth_of(parent_key) > 0 {
                self.release_held_after_restart(parent_key, &run.id);
                continue;
            }
            // It stays failed; one that was waiting on the owner says so,
            // its questions leading the report its row keeps.
            let error = if run.summary.split_whitespace().any(|t| t == ROW_ASKS_OWNER) {
                format!("{error}; it was waiting on the owner's decision (its question leads its report)")
            } else {
                error.to_string()
            };
            let c = self.from_row(&run, CompletionStatus::Failed { error }, "It was interrupted by a restart.");
            self.deliver(parent_key, c, 0);
        }
    }

    fn description_of(&self, task_id: &str) -> String {
        self.store.get_pending_task(task_id).ok().flatten().and_then(|t| t.description).unwrap_or_default()
    }

    /// A helper's report as its row keeps it: assembled and held to its
    /// limit, its pending questions first, with `note` after it.
    /// Its row's questions are now with its parent.
    fn from_row(&self, run: &db::EngineRun, status: CompletionStatus, note: &str) -> Completion {
        let mut questions = questions_of_row(&self.store, &run.id);
        questions.reported();
        self.keep_questions(&run.id, &questions);
        let report = run.result.clone().unwrap_or_default();
        Completion {
            task_id: run.id.clone(),
            description: self.description_of(&run.id),
            status,
            result: if report.trim().is_empty() { format!("({note})") } else { format!("{report}\n\n({note})") },
            question: None,
            lead: String::new(),
            usage: ai::UsageInfo::default(),
            taint: Vec::new(),
        }
    }

    /// Helper `child` of the helper at `parent_key` was interrupted by a
    /// restart. Up the tree, bottom-up, every helper that had finished and
    /// was holding its report for its own helper (its row says `held`)
    /// reports now, from its row, to its own parent: into a helper parent's
    /// thread, and to an owner, scheduled or workflow session the usual way.
    /// A released report carries the one below it, so a question asked at
    /// any depth reaches the owner's conversation even when the helpers
    /// between never run again, and a pending question anywhere below makes
    /// the report a question for the owner.
    fn release_held_after_restart(self: &Arc<Self>, parent_key: &str, child: &str) {
        let mut key = parent_key.to_string();
        let mut child = child.to_string();
        let mut below: Option<(String, bool)> = None;
        while let Some((up, pid)) = split_helper_key(&key).map(|(u, p)| (u.to_string(), p.to_string())) {
            if self.state().helpers.contains_key(&pid) {
                return;
            }
            let Some(row) = self.store.engine_get_run(&pid).ok().flatten() else {
                return;
            };
            let tags: Vec<&str> = row.summary.split_whitespace().collect();
            if row.state != "done" || !tags.contains(&ROW_HELD) {
                return;
            }
            let asks_owner = tags.contains(&ROW_ASKS_OWNER) || below.as_ref().is_some_and(|(_, asks)| *asks);
            self.tag_row(&pid, false, tags.contains(&ROW_ASKS_OWNER));
            let note = match &below {
                None => format!("It was holding this report for its helper {child}, which was interrupted by a restart."),
                Some((report, _)) => {
                    format!("It was holding this report for its helper {child}, which reported after a restart:\n{report}")
                }
            };
            let status = if asks_owner {
                CompletionStatus::Partial { why: collect::NEEDS_OWNER_DECISION.to_string() }
            } else {
                CompletionStatus::Done
            };
            let c = self.from_row(&row, status, &note);
            info!(task_id = %pid, parent = %up, "a held helper report released after a restart");
            if depth_of(&up) == 0 {
                self.deliver(&up, c, 0);
                return;
            }
            self.append_notification(&up, &render_notification(&c), &[]);
            below = Some((c.report(), asks_owner));
            child = pid;
            key = up;
        }
    }

    /// Where a report to `parent_key` past its limit is saved: the parent's
    /// own results.
    fn spill_dir(&self, parent_key: &str) -> std::path::PathBuf {
        self.sessions
            .resolve_session_id_by_key(parent_key)
            .map(|id| tools::result_shape::results_dir(&id))
            .unwrap_or_else(|_| tools::result_shape::results_dir(&parent_key.replace(':', "_")))
    }

    /// Keep helper `task_id`'s assembled report `c` on its row with what it
    /// is (held, asking the owner), so `read_output` shows it and a restart
    /// keeps it; `done` when the run ended (a failed or stopped run keeps
    /// its state).
    fn keep_on_row(&self, task_id: &str, c: &Completion, held: bool, done: bool) {
        if done && matches!(c.status, CompletionStatus::Done | CompletionStatus::Partial { .. }) {
            let _ = self.store.update_task_completed(task_id, None);
        }
        if let Err(e) = self.store.engine_set_run_result(task_id, &c.report(), Some(&row_tag(held, c.status.awaits_owner()))) {
            warn!(task_id = %task_id, error = %e, "a helper's report could not be kept on its row");
        }
    }

    /// Keep helper `task_id`'s questions on its row
    /// (`engine_runs.pending_questions`), so they outlive the run that
    /// asked them and a restart.
    fn keep_questions(&self, task_id: &str, questions: &collect::Questions) {
        if let Ok(json) = serde_json::to_string(questions)
            && let Err(e) = self.store.engine_set_run_pending_questions(task_id, Some(&json))
        {
            warn!(task_id = %task_id, error = %e, "a helper's questions could not be kept on its row");
        }
    }

    /// Set what helper `task_id`'s row says its report is.
    fn tag_row(&self, task_id: &str, held: bool, asks_owner: bool) {
        let _ = self.store.engine_set_run_result_tag(task_id, &row_tag(held, asks_owner));
    }

    /// Helper `task_id` runs again: its row says running, no longer held,
    /// and still asking the owner while its question is pending.
    fn running_again(&self, task_id: &str) {
        let _ = self.store.update_task_running(task_id);
        let pending = self.state().helpers.get(task_id).is_some_and(|h| !h.questions.pending.is_empty());
        self.tag_row(task_id, false, pending);
    }

    /// Before a turn of helper `task_id` with a question still pending: a
    /// reminder in its thread that the owner's answer hasn't come. One
    /// reminder at a time: an earlier one is replaced.
    fn remind_pending(&self, task_id: &str) {
        let key = {
            let state = self.state();
            match state.helpers.get(task_id) {
                Some(h) if !h.questions.pending.is_empty() && h.answer_in_turn.is_none() => h.session_key.clone(),
                _ => return,
            }
        };
        if let Ok(id) = self.sessions.resolve_session_id_by_key(&key)
            && let Ok(rows) = self.store.get_chat_messages(&self.sessions.active_chat_id(&id))
        {
            for row in rows.iter().filter(|m| notify::is_notification_row(m) && m.content.contains(STILL_OPEN)) {
                let _ = self.store.delete_chat_message(&row.id);
            }
        }
        let text = notify::render_update(
            STILL_OPEN,
            "No answer has reached you yet. It comes as a message that starts \"The owner's answer to your \
             question\"; any other message leaves the question open. Don't ask it again. Go on with what doesn't \
             depend on it, and don't decide it yourself.",
        );
        self.append_notification(&key, &text, &[]);
    }

    /// The caller's own helpers still in hand: running, or finished and
    /// waiting on helpers of their own; every session's with `None`.
    /// Running first.
    pub fn list(&self, caller: Option<&str>, tree: bool) -> Vec<HelperStatus> {
        let mut out: Vec<HelperStatus> = self
            .state()
            .helpers
            .iter()
            .filter(|(_, h)| {
                caller.is_none_or(|c| if tree { h.session_key != c && under(&h.session_key, c) } else { h.parent_key == c })
            })
            .map(|(id, h)| HelperStatus {
                task_id: id.clone(),
                description: h.description.clone(),
                running: h.running,
                waiting: h.held.is_some(),
                session_key: h.session_key.clone(),
                call: h.call.clone(),
                activity: h.activity.clone(),
                parent_key: h.parent_key.clone(),
                agent_id: h.parent_seat.agent_id.clone(),
                started_at: h.started_at,
            })
            .collect();
        out.sort_by(|a, b| b.running.cmp(&a.running).then_with(|| a.task_id.cmp(&b.task_id)));
        out
    }

    /// The row of helper `task_id`, when `caller` started it.
    /// With `tree`, any helper or research run under `caller`, at any depth.
    fn own_row(&self, caller: &str, task_id: &str, tree: bool) -> Result<db::models::PendingTask, String> {
        let not_yours = || format!("No helper {task_id} was started from this conversation.");
        let row = self
            .store
            .get_pending_task(task_id)
            .map_err(|e| format!("Could not read helper {task_id}: {e}"))?
            .ok_or_else(not_yours)?;
        let mine = row.session_key == helper_key(caller, task_id)
            || (tree && split_helper_key(&row.session_key).is_some_and(|(parent, id)| id == task_id && under(parent, caller)));
        if row.task_type != ROW_KIND || !mine {
            return Err(not_yours());
        }
        Ok(row)
    }

    /// Run one turn of helper `task_id` in the background and finish it.
    /// A turn that crashes still ends the helper as failed and tells its
    /// parent: no helper ends without a word.
    fn spawn_turn(self: &Arc<Self>, task_id: String, req: TurnRequest, isolation: Option<crate::worktree::Isolation>) {
        let this = Arc::clone(self);
        tokio::spawn(async move {
            let mut next = Some(req);
            let mut isolation = isolation;
            while let Some(req) = next.take() {
                this.remind_pending(&task_id);
                let turn = std::panic::AssertUnwindSafe(this.run_turn(&task_id, req, isolation.take()));
                let completion = match futures::FutureExt::catch_unwind(turn).await {
                    Ok(completion) => completion,
                    Err(panic) => this.crashed(&task_id, &panic_text(&*panic)),
                };
                next = this.finish(&task_id, completion);
            }
        });
    }

    /// Helper `task_id` crashed mid-run: its row fails with why, the owner's
    /// screen hears it ended, and the completion its parent is told says so.
    fn crashed(&self, task_id: &str, why: &str) -> Completion {
        let (parent_key, description) = {
            let state = self.state();
            let h = state.helpers.get(task_id);
            (h.map(|h| h.parent_key.clone()).unwrap_or_default(), h.map(|h| h.description.clone()).unwrap_or_default())
        };
        let error = format!("the helper crashed: {why}");
        warn!(task_id = %task_id, parent = %parent_key, error = %error, "helper crashed");
        if let Err(e) = self.store.engine_set_run_state(task_id, "failed", chrono::Utc::now().timestamp(), Some(&error)) {
            warn!(task_id = %task_id, error = %e, "a crashed helper could not be marked failed");
        }
        self.emit(&parent_key, ai::StreamEvent::subagent_complete(task_id, description.as_str(), false));
        Completion {
            task_id: task_id.to_string(),
            description,
            status: CompletionStatus::Failed { error },
            result: String::new(),
            question: None,
            lead: String::new(),
            usage: ai::UsageInfo::default(),
            taint: Vec::new(),
        }
    }

    async fn run_turn(
        self: &Arc<Self>,
        task_id: &str,
        req: TurnRequest,
        isolation: Option<crate::worktree::Isolation>,
    ) -> Completion {
        let cancel = req.cancel.clone();
        let (parent_key, description) = {
            let state = self.state();
            let h = state.helpers.get(task_id);
            (
                h.map(|h| h.parent_key.clone()).unwrap_or_default(),
                h.map(|h| h.description.clone()).unwrap_or_default(),
            )
        };
        self.emit(&parent_key, ai::StreamEvent::subagent_start(task_id, description.as_str()));

        let progress = self.progress_forwarder(&parent_key, task_id);
        let waiting = waits_of(&req);
        let collected = match self.starter.start_turn(req).await {
            Ok(handle) => {
                collect::collect(handle.events, &cancel, INACTIVITY_LIMIT, &waiting, |e| {
                    if let Some(tx) = &progress
                        && let Some(tc) = &e.tool_call
                    {
                        let _ = tx.send(tc.clone());
                    }
                })
                .await
            }
            Err(e) => collect::Collected::failed(e.to_string()),
        };
        let mut completion = collected.into_completion(task_id, &description);
        if let Some(iso) = isolation {
            let merged = crate::worktree::merge_all(std::slice::from_ref(&iso), &format!("nebo: helper {task_id}")).await;
            for (_, outcome) in &merged {
                completion.result.push_str("\n\n");
                completion.result.push_str(&crate::worktree::render_outcome(&description, outcome));
            }
        }
        match &completion.status {
            // The run's report is assembled and kept on its row when it
            // ends (`finish`).
            CompletionStatus::Done | CompletionStatus::Partial { .. } => {}
            CompletionStatus::Failed { error } => {
                let _ = self.store.update_task_failed(task_id, error);
            }
            CompletionStatus::Stopped => {
                let _ = self.store.cancel_task(task_id);
            }
        }
        let success = matches!(completion.status, CompletionStatus::Done);
        self.emit(&parent_key, ai::StreamEvent::subagent_complete(task_id, description.as_str(), success));
        completion
    }

    /// Progress for the owner's screen: each tool call the helper makes is
    /// named by its structured name. `None` without a screen to show it.
    fn progress_forwarder(self: &Arc<Self>, parent_key: &str, task_id: &str) -> Option<mpsc::UnboundedSender<ai::ToolCall>> {
        self.ui.as_ref()?;
        let (tx, mut rx) = mpsc::unbounded_channel::<ai::ToolCall>();
        let this = Arc::clone(self);
        let parent_key = parent_key.to_string();
        let task_id = task_id.to_string();
        tokio::spawn(async move {
            let mut steps = 0usize;
            while let Some(tc) = rx.recv().await {
                steps += 1;
                let call = tools::humanize::call(&tc.name, &tc.input);
                if let Some(helper) = this.state().helpers.get_mut(task_id.as_str()) {
                    helper.call = Some(call.clone());
                    helper.activity.clear();
                }
                let mut ev = ai::StreamEvent::subagent_start(task_id.as_str(), "");
                ev.event_type = ai::StreamEventType::SubagentProgress;
                ev.widgets = Some(serde_json::json!({
                    "task_id": task_id,
                    "tool_count": steps,
                    "call": call,
                }));
                this.emit(&parent_key, ev);
            }
        });
        Some(tx)
    }

    fn emit(&self, parent_key: &str, event: ai::StreamEvent) {
        // Background work's latest progress line is kept with it, so a
        // screen that opens after the event still reads what it is doing.
        if event.event_type == ai::StreamEventType::SubagentProgress
            && let Some(widgets) = event.widgets.as_ref()
            && let (Some(task_id), Some(activity)) = (widgets["task_id"].as_str(), widgets["current_operation"].as_str())
            && let Some(helper) = self.state().helpers.get_mut(task_id)
        {
            helper.activity = activity.to_string();
            helper.call = None;
        }
        if let Some(ui) = &self.ui {
            let _ = ui.send(HelperEvent { parent_session_key: parent_key.to_string(), event });
        }
    }

    /// A helper's turn ended. A message or notification that reached its
    /// thread after its last step gets one more turn (returned). Otherwise a
    /// foreground launch still waiting gets the completion, or it notifies
    /// its parent, unless its own helpers still run: then it notifies once
    /// they are done and it has heard them.
    fn finish(self: &Arc<Self>, task_id: &str, completion: Completion) -> Option<TurnRequest> {
        let (parent_key, completion, depth) = {
            let mut state = self.state();
            let h = state.helpers.get_mut(task_id)?;
            let continues = matches!(completion.status, CompletionStatus::Done | CompletionStatus::Partial { .. })
                && !h.cancel.is_cancelled()
                && self
                    .sessions
                    .resolve_session_id_by_key(&h.session_key)
                    .and_then(|id| self.store.get_chat_messages(&self.sessions.active_chat_id(&id)))
                    .is_ok_and(|messages| input_unheard(&messages));
            h.keep_turn(completion);
            self.keep_questions(task_id, &h.questions);
            let spill_dir = self.spill_dir(&h.parent_key);
            if continues {
                info!(task_id = %task_id, "input reached the helper as its turn ended: running a turn to hear it");
                self.keep_on_row(task_id, &h.assembled(&spill_dir, false), false, false);
                let spec = HelperSpec {
                    description: h.description.clone(),
                    prompt: String::new(),
                    kind: h.kind,
                    background: true,
                    isolation: None,
                    skills: Vec::new(),
                    speed: h.speed.clone(),
                    report_limit: Some(h.report_limit),
                };
                let parent = Parent {
                    session_key: &h.parent_key,
                    seat: &h.parent_seat,
                    grant: h.parent_grant.as_ref(),
                    run_taint: &[],
                    cancel: h.cancel.clone(),
                    platform: h.parent_platform.as_deref(),
                };
                return Some(child::child_request(&parent, task_id, &spec, None, TurnInput::None));
            }
            h.running = false;
            // The row keeps the whole report; the parent gets each pending
            // question once.
            let row = h.assembled(&spill_dir, false);
            let mut completion = h.assembled(&spill_dir, true);
            if h.stopped {
                completion.status = CompletionStatus::Stopped;
            }
            let (session_key, parent_key, depth) = (h.session_key.clone(), h.parent_key.clone(), h.parent_seat.handoff_depth);
            // A run that failed or was stopped takes what it started with
            // it: nothing is left running with nowhere to report.
            if matches!(completion.status, CompletionStatus::Failed { .. } | CompletionStatus::Stopped) {
                let stopped = state.stop_below(&session_key);
                for id in &stopped {
                    let _ = self.store.cancel_task(id);
                }
                if !stopped.is_empty() {
                    info!(task_id = %task_id, stopped = stopped.len(), "a helper that ended stopped the work it started");
                }
            }
            // It doesn't finish while work it started still goes (helpers,
            // research runs, at any depth), whether a launch waits on it or
            // it reports by notification: its report waits for theirs.
            if matches!(completion.status, CompletionStatus::Done | CompletionStatus::Partial { .. }) && state.work_below(&session_key) {
                // Held on its row too: a restart before its helpers report
                // still delivers it (`release_held_after_restart`).
                self.keep_on_row(task_id, &row, true, true);
                if let Some(h) = state.helpers.get_mut(task_id) {
                    h.held = Some(completion);
                }
                return None;
            }
            self.keep_on_row(task_id, &row, false, true);
            // The parent has its questions now: a later report names them.
            let waiter = state.helpers.get_mut(task_id).and_then(|h| {
                h.questions.reported();
                self.keep_questions(task_id, &h.questions);
                h.earlier_reports.clear();
                h.waiter.take()
            });
            let completion = match waiter {
                Some(waiter) => match waiter.send(completion) {
                    Ok(()) => {
                        state.forget(task_id);
                        return None;
                    }
                    Err(c) => c,
                },
                None => completion,
            };
            state.forget(task_id);
            (parent_key, completion, depth)
        };
        self.deliver(&parent_key, completion, depth);
        None
    }

    /// Hand `c` to the session `parent_key`, whose run is `depth` hops down
    /// a coworker chain. A running parent hears it at its next step (a
    /// notification row); an idle helper parent gets a notification turn; an
    /// owner session goes through the wake rail, which does the same for it.
    /// Every path carries what the helper read. A stopped helper never
    /// starts a turn: its parent, or the owner, stopped it.
    fn deliver(self: &Arc<Self>, parent_key: &str, c: Completion, depth: u8) {
        let text = render_notification(&c);
        let wakes = c.status != CompletionStatus::Stopped;
        let mut release: Option<(String, Completion, u8, Option<oneshot::Sender<Completion>>)> = None;
        let mut resume: Option<(String, TurnRequest)> = None;
        {
            let mut state = self.state();
            let parent_id = state
                .helpers
                .iter()
                .find(|(_, h)| h.session_key == parent_key)
                .map(|(id, _)| id.clone());
            match parent_id {
                Some(pid) => {
                    let running = state.helpers.get(&pid).is_some_and(|p| p.running);
                    // A stopped helper never runs again on its own: what
                    // reaches it is kept in its thread.
                    let stopped = state.helpers.get(&pid).is_some_and(|p| p.stopped);
                    if running || !wakes || stopped {
                        self.append_notification(parent_key, &text, &c.taint);
                        if !running && !state.work_below(parent_key)
                            && let Some(p) = state.helpers.get_mut(&pid)
                            && let Some(held) = p.held.take()
                        {
                            // The parent has its questions now.
                            p.questions.reported();
                            self.keep_questions(&pid, &p.questions);
                            self.tag_row(&pid, false, held.status.awaits_owner());
                            release = Some((p.parent_key.clone(), held, p.parent_seat.handoff_depth, p.waiter.take()));
                            state.forget(&pid);
                        }
                    } else {
                        resume = State::resume(&mut state, &pid, TurnInput::Notification(c.clone())).map(|req| (pid, req));
                    }
                }
                None if wakes => {
                    let taint = serde_json::to_string(&c.taint).unwrap_or_else(|_| "[]".to_string());
                    let queued = self.store.engine_enqueue_wake(parent_key, notify::WAKE_KIND, &text, &taint, depth);
                    match queued {
                        Ok(_) => {
                            if let Some(wake) = &self.wake {
                                let _ = wake.send(parent_key.to_string());
                            }
                        }
                        Err(e) => {
                            warn!(error = %e, session = %parent_key, "helper notification could not be queued; writing it as a row");
                            self.append_notification(parent_key, &text, &c.taint);
                        }
                    }
                }
                None => self.append_notification(parent_key, &text, &c.taint),
            }
        }
        if let Some((pid, req)) = resume {
            self.running_again(&pid);
            self.spawn_turn(pid, req, None);
        }
        // A held report goes to the launch still waiting on it, else by
        // notification.
        if let Some((grandparent, held, depth, waiter)) = release {
            let held = match waiter {
                Some(waiter) => match waiter.send(held) {
                    Ok(()) => return,
                    Err(held) => held,
                },
                None => held,
            };
            self.deliver(&grandparent, held, depth);
        }
    }

    fn append_notification(&self, session_key: &str, text: &str, taint: &[ProvenanceClass]) {
        if let Err(e) = notify::append_row(&self.sessions, session_key, text, taint) {
            warn!(error = %e, session = %session_key, "helper notification row could not be written");
        }
    }

    /// Any other update for a helper's session, already in the one format
    /// (`notify::render_update`, an ask's outcome): a coworker's reply, a
    /// team reply, a finished command. When this process still holds the
    /// helper, the row is written with the taint it carries: a running
    /// helper hears it at its next step, an idle one runs a turn that hears
    /// it. Returns false when no held helper has that session (it finished
    /// and was let go, or a restart forgot it): the caller takes the update
    /// to the helper's parent instead.
    pub fn notify(self: &Arc<Self>, session_key: &str, text: &str, taint: &[ProvenanceClass]) -> bool {
        let resume = {
            let mut state = self.state();
            let Some((pid, running)) =
                state.helpers.iter().find(|(_, h)| h.session_key == session_key).map(|(id, h)| (id.clone(), h.running))
            else {
                return false;
            };
            // Written under the lock the helper's finish takes, so a row
            // either reaches a running turn or finds the helper idle.
            if let Err(e) = notify::append_row(&self.sessions, session_key, text, taint) {
                warn!(error = %e, session = %session_key, "update for a helper could not be written");
                return true;
            }
            // A stopped helper keeps the update in its thread; it doesn't run.
            if running || state.helpers.get(&pid).is_some_and(|h| h.stopped) {
                return true;
            }
            State::resume(&mut state, &pid, TurnInput::None).map(|req| (pid, req))
        };
        if let Some((pid, req)) = resume {
            self.running_again(&pid);
            self.spawn_turn(pid, req, None);
        }
        true
    }
}

impl State {
    /// Mark idle helper `pid` running again and build the turn that hears
    /// `input`, under a fresh stop token from its parent's session.
    fn resume(&mut self, pid: &str, input: TurnInput) -> Option<TurnRequest> {
        let grandparent = self.helpers.get(pid)?.parent_key.clone();
        let cancel = self.session_token(&grandparent).child_token();
        let session_key = self.helpers.get(pid)?.session_key.clone();
        self.stops.insert(session_key, cancel.clone());
        let p = self.helpers.get_mut(pid)?;
        let spec = HelperSpec {
            description: p.description.clone(),
            prompt: String::new(),
            kind: p.kind,
            background: true,
            isolation: None,
            skills: Vec::new(),
            speed: p.speed.clone(),
            report_limit: Some(p.report_limit),
        };
        let parent = Parent {
            session_key: &p.parent_key,
            seat: &p.parent_seat,
            grant: p.parent_grant.as_ref(),
            run_taint: &[],
            cancel: cancel.clone(),
            platform: p.parent_platform.as_deref(),
        };
        let req = child::child_request(&parent, pid, &spec, None, input);
        p.running = true;
        p.cancel = cancel;
        // A report held while its own helpers ran is never dropped: its
        // turns stay in the run, and the held copy goes.
        p.held = None;
        Some(req)
    }
}

/// True when the last message or notification that reached a helper's
/// thread (as stored: notification rows are wrapped and meta, which the
/// legacy history loader drops) has no model step after it: it landed after the turn's last step.
fn input_unheard(messages: &[db::models::ChatMessage]) -> bool {
    let Some(at) = messages.iter().rposition(|m| {
        m.role == "user"
            && (notify::is_notification_row(m)
                || matches!(crate::harness::conversation::arrived_mid_turn(m), Some(crate::harness::conversation::MidTurnFrom::Parent { .. })))
    }) else {
        return false;
    };
    !messages[at + 1..].iter().any(|m| m.role == "assistant")
}

/// Fence an isolated helper to its own copy of the parent's project. A
/// fenced parent may only isolate a project inside its fence: the copy is
/// merged back there.
async fn isolate(
    parent: &SeatRequest,
    grant: Option<&Grant>,
    task_id: &str,
) -> Result<crate::worktree::Isolation, String> {
    let workspace = match parent.cwd.as_deref() {
        Some(cwd) => std::path::PathBuf::from(cwd),
        None => std::env::current_dir().map_err(|e| format!("No project folder to isolate: {e}"))?,
    };
    let strings = |v: &[std::path::PathBuf]| -> Vec<String> { v.iter().map(|p| p.to_string_lossy().into_owned()).collect() };
    let target = [workspace.to_string_lossy().into_owned()];
    let folders = grant.map(|g| g.folders()).unwrap_or_default();
    let fence = grant.and_then(|g| g.fence.clone()).unwrap_or_default();
    if let Some(blocked) = tools::safeguard::outside_allowed("isolate", &target, &strings(&folders))
        .or_else(|| tools::safeguard::outside_allowed("isolate", &target, &strings(&fence)))
    {
        return Err(blocked);
    }
    crate::worktree::create(&workspace, task_id)
        .await
        .map_err(|e| format!("Could not isolate {}: {e}", workspace.display()))
}

/// The inputs a helper's row was stored with.
fn row_inputs(store: &db::Store, row: &db::models::PendingTask) -> Option<serde_json::Value> {
    store
        .engine_get_run(&row.id)
        .ok()
        .flatten()
        .and_then(|run| run.inputs)
        .and_then(|i| serde_json::from_str::<serde_json::Value>(&i).ok())
}

/// The questions a helper's row keeps ([`Helpers::keep_questions`]).
fn questions_of_row(store: &db::Store, task_id: &str) -> collect::Questions {
    store
        .engine_get_run(task_id)
        .ok()
        .flatten()
        .and_then(|run| run.pending_questions)
        .and_then(|q| serde_json::from_str(&q).ok())
        .unwrap_or_default()
}

/// The `helper_kind` a helper's row was stored with.
fn spec_kind_of_row(store: &db::Store, row: &db::models::PendingTask) -> Option<String> {
    row_inputs(store, row).and_then(|v| v.get("helper_kind")?.as_str().map(str::to_string))
}

/// The spec a helper was started with, read back from its row.
fn spec_of_row(store: &db::Store, row: &db::models::PendingTask) -> HelperSpec {
    let inputs = row_inputs(store, row);
    let field = |key: &str| inputs.as_ref().and_then(|v| v.get(key)?.as_str().map(str::to_string));
    HelperSpec {
        description: row.description.clone().unwrap_or_default(),
        prompt: row.prompt.clone(),
        kind: field("helper_kind").as_deref().and_then(HelperKind::parse).unwrap_or(HelperKind::General),
        background: true,
        isolation: None,
        skills: Vec::new(),
        speed: field("speed"),
        report_limit: inputs.as_ref().and_then(|v| v.get("report_limit")?.as_u64()).map(|n| n as usize),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ai::StreamEvent;
    use types::permissions::{CallEffects, Target};

    /// A turn the scripted starter started; the test plays its events.
    struct Started {
        request: TurnRequest,
        events: mpsc::Sender<StreamEvent>,
    }

    impl Started {
        async fn answer(&self, text: &str) {
            let _ = self.events.send(StreamEvent::text(text)).await;
            let _ = self.events.send(StreamEvent::done()).await;
        }

        /// Answer as a turn that read untrusted content: its `Done` carries
        /// the run's provenance, as the turn driver sends it.
        async fn answer_having_read(&self, text: &str, taint: Vec<ProvenanceClass>) {
            let _ = self.events.send(StreamEvent::text(text)).await;
            let _ = self.events.send(StreamEvent::done().with_provenance(taint)).await;
        }
    }

    struct Scripted {
        started: mpsc::UnboundedSender<Started>,
    }

    #[async_trait::async_trait]
    impl TurnStarter for Scripted {
        async fn start_turn(&self, request: TurnRequest) -> Result<TurnHandle, HarnessError> {
            let (events, rx) = mpsc::channel(64);
            let _ = self.started.send(Started { request, events });
            Ok(TurnHandle { events: rx, turn_id: uuid::Uuid::new_v4().to_string() })
        }
    }

    /// A turn that crashes as it starts.
    struct Crashes;

    #[async_trait::async_trait]
    impl TurnStarter for Crashes {
        async fn start_turn(&self, _request: TurnRequest) -> Result<TurnHandle, HarnessError> {
            panic!("the model client fell over")
        }
    }

    struct Rig {
        helpers: Arc<Helpers>,
        store: Arc<db::Store>,
        sessions: Arc<SessionManager>,
        started: mpsc::UnboundedReceiver<Started>,
        wake: mpsc::UnboundedReceiver<String>,
        ui: mpsc::UnboundedReceiver<HelperEvent>,
        _dir: tempfile::TempDir,
    }

    impl Rig {
        fn new(budget: Duration) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let store = Arc::new(db::Store::new(&dir.path().join("t.db").to_string_lossy()).unwrap());
            Self::on(store, dir, budget)
        }

        /// A fresh registry on an existing store: a restart.
        fn on(store: Arc<db::Store>, dir: tempfile::TempDir, budget: Duration) -> Self {
            Self::starting_with(store, dir, budget, None)
        }

        /// A registry whose turns `starter` starts (the scripted one when
        /// `None`).
        fn starting_with(store: Arc<db::Store>, dir: tempfile::TempDir, budget: Duration, starter: Option<Arc<dyn TurnStarter>>) -> Self {
            let sessions = Arc::new(SessionManager::new(store.clone()));
            let (started_tx, started) = mpsc::unbounded_channel();
            let (wake_tx, wake) = mpsc::unbounded_channel();
            let (ui_tx, ui) = mpsc::unbounded_channel();
            let helpers = Arc::new(Helpers {
                store: store.clone(),
                sessions: sessions.clone(),
                starter: starter.unwrap_or_else(|| Arc::new(Scripted { started: started_tx })),
                wake: Some(wake_tx),
                ui: Some(ui_tx),
                foreground_budget: budget,
                state: Mutex::new(State::default()),
            });
            Self { helpers, store, sessions, started, wake, ui, _dir: dir }
        }

        /// An owner's chat turn on `key`, its session in the store.
        fn owner_turn(&self, key: &str) -> TurnRequest {
            self.sessions.get_or_create(key, "owner-1").unwrap();
            TurnRequest {
                session_key: key.to_string(),
                input: TurnInput::None,
                seat: child::tests::parent_seat(),
                mode: TurnMode::Chat,
                delivery: super::super::Delivery { channel: "web".into(), channel_ctx: None, platform: None, mention_briefing: None },
                cancel: self.helpers.session_token(key).child_token(),
                progress: None,
            }
        }

        async fn next_turn(&mut self) -> Started {
            let started = tokio::time::timeout(Duration::from_secs(5), self.started.recv())
                .await
                .expect("a turn starts")
                .unwrap();
            self.sessions.get_or_create(&started.request.session_key, "owner-1").unwrap();
            started
        }

        async fn next_wake(&mut self) -> String {
            tokio::time::timeout(Duration::from_secs(5), self.wake.recv()).await.expect("a wake").unwrap()
        }

        fn pending_notifications(&self, key: &str) -> Vec<String> {
            let (batch, _) = self.store.engine_claim_session_events(key, 0).unwrap();
            batch.into_iter().filter(|w| w.kind == notify::WAKE_KIND).map(|w| w.payload).collect()
        }

        fn notification_rows(&self, key: &str) -> Vec<String> {
            let id = self.sessions.resolve_session_id_by_key(key).unwrap();
            self.store
                .get_chat_messages(&self.sessions.active_chat_id(&id))
                .unwrap()
                .into_iter()
                .filter(notify::is_notification_row)
                .map(|m| m.content)
                .collect()
        }

        /// The model's step after a row landed: an assistant row.
        fn heard(&self, key: &str) {
            let id = self.sessions.resolve_session_id_by_key(key).unwrap();
            self.sessions.append_message(&id, "assistant", "Noted.", None, None, None).unwrap();
        }
    }

    fn call(prompt: &str, background: Option<bool>) -> serde_json::Value {
        let mut v = serde_json::json!({"description": "read the ledger", "prompt": prompt});
        if let Some(b) = background {
            v["background"] = b.into();
        }
        v
    }

    fn task_id_of(launch_text: &str) -> String {
        launch_text.split_whitespace().nth(1).unwrap().to_string()
    }

    /// Review 5.1: a helper that read the web returns tainted, whichever
    /// way its result travels: the wake an owner session is woken with, the
    /// row a running parent helper hears, and a foreground result.
    #[tokio::test]
    async fn a_helpers_result_carries_what_it_read() {
        let mut rig = Rig::new(FOREGROUND_BUDGET);
        let owner = rig.owner_turn("agent:buyer:web");
        let text = rig.helpers.delegate(&owner, None, &[], &call("Read the supplier's web page.", None)).await.unwrap();
        let parent = rig.next_turn().await;
        assert_eq!(parent.request.session_key, helper_key("agent:buyer:web", &task_id_of(&text)));

        // The helper starts a helper of its own, and reads the web.
        let nested = rig
            .helpers
            .delegate(&parent.request, None, &[], &call("Open the price list web page.", None))
            .await
            .unwrap();
        let child = rig.next_turn().await;
        assert_eq!(child.request.session_key, helper_key(&parent.request.session_key, &task_id_of(&nested)));
        child.answer_having_read("Widgets are 4.10 each.", vec![ProvenanceClass::Web]).await;
        let parent_key = parent.request.session_key.clone();
        for _ in 0..200 {
            if !rig.notification_rows(&parent_key).is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let id = rig.sessions.resolve_session_id_by_key(&parent_key).unwrap();
        let rows = rig.store.get_chat_messages(&rig.sessions.active_chat_id(&id)).unwrap();
        let row = rows.iter().find(|m| notify::is_notification_row(m)).expect("the running parent hears it as a row");
        assert_eq!(notify::row_taint(row), vec![ProvenanceClass::Web], "the row a running parent hears");

        // The parent heard it and finishes, having read the web itself: the
        // owner's wake carries it.
        rig.heard(&parent_key);
        parent.answer_having_read("Supplier prices are up 3%.", vec![ProvenanceClass::Web]).await;
        assert_eq!(rig.next_wake().await, "agent:buyer:web");
        let (batch, _) = rig.store.engine_claim_session_events("agent:buyer:web", 0).unwrap();
        let wake = batch.iter().find(|w| w.kind == notify::WAKE_KIND).expect("a notification wake");
        assert_eq!(wake.provenance, r#"["web"]"#, "the wake the owner's session is woken with");

        // A foreground helper's completion carries it too.
        let mut fg = HelperSpec::from_input(&call("Read the returns web page.", Some(false))).unwrap();
        fg.background = false;
        let launched = {
            let helpers = rig.helpers.clone();
            let owner = rig.owner_turn("agent:buyer:web");
            tokio::spawn(async move { helpers.launch(&owner, None, &[], fg).await })
        };
        rig.next_turn().await.answer_having_read("Returns take 30 days.", vec![ProvenanceClass::Web]).await;
        match launched.await.unwrap().unwrap() {
            Launch::Finished(c) => assert_eq!(c.taint, vec![ProvenanceClass::Web], "a foreground result"),
            Launch::Background { .. } => panic!("it finished inside the budget"),
        }
    }

    /// A helper whose turn crashes still ends: its row fails with why, and
    /// its parent is told it failed. No helper ends without a word.
    #[tokio::test]
    async fn a_crashed_helper_reports_its_failure_to_its_parent() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(db::Store::new(&dir.path().join("t.db").to_string_lossy()).unwrap());
        let mut rig = Rig::starting_with(store, dir, FOREGROUND_BUDGET, Some(Arc::new(Crashes)));
        let owner = rig.owner_turn("agent:bookkeeper:web");
        let text = rig.helpers.delegate(&owner, None, &[], &call("Find the invoice.", None)).await.unwrap();
        let id = task_id_of(&text);
        assert_eq!(rig.next_wake().await, "agent:bookkeeper:web");
        let pending = rig.pending_notifications("agent:bookkeeper:web");
        assert_eq!(pending.len(), 1);
        assert!(pending[0].contains(&format!("helper {id} \"read the ledger\": failed")), "{}", pending[0]);
        assert!(pending[0].contains("the model client fell over"), "{}", pending[0]);
        let row = rig.store.get_pending_task(&id).unwrap().unwrap();
        assert_eq!(row.status, "failed");
        assert!(!rig.helpers.list(None, false).iter().any(|h| h.task_id == id && h.running), "it is not left running");
    }

    #[tokio::test]
    async fn background_is_default_and_ends_the_turn() {
        let mut rig = Rig::new(FOREGROUND_BUDGET);
        let owner = rig.owner_turn("agent:bookkeeper:web");
        let text = rig.helpers.delegate(&owner, None, &[], &call("Find the invoice.", None)).await.unwrap();
        let id = task_id_of(&text);
        assert_eq!(text, launch_result(&id));
        assert!(text.contains("you know nothing about its result: don't report, guess or redo its work"));

        // It returned while the helper still runs; the result comes as a
        // notification through the wake rail.
        let child = rig.next_turn().await;
        assert!(matches!(child.request.input, TurnInput::Platform { ref text } if text == "Find the invoice."));
        child.answer("Invoice 12 is missing.").await;
        assert_eq!(rig.next_wake().await, "agent:bookkeeper:web");
        let pending = rig.pending_notifications("agent:bookkeeper:web");
        assert_eq!(pending.len(), 1);
        assert!(pending[0].starts_with("<system-reminder>\n[Notification: not a message from the owner]"));
        assert!(pending[0].contains(&format!("helper {id} \"read the ledger\": done\nInvoice 12 is missing.\n")));
    }

    #[tokio::test]
    async fn foreground_past_120s_moves_to_background() {
        let mut rig = Rig::new(Duration::from_millis(100));
        let owner = rig.owner_turn("agent:bookkeeper:web");

        // Within the budget: the result is the tool's answer, and nothing
        // is notified.
        let helpers = rig.helpers.clone();
        let quick = {
            let owner = rig.owner_turn("agent:bookkeeper:web");
            tokio::spawn(async move { helpers.delegate(&owner, None, &[], &call("Quick look.", Some(false))).await })
        };
        rig.next_turn().await.answer("Found it.").await;
        let text = quick.await.unwrap().unwrap();
        assert!(text.contains(": done\nFound it.\n"), "{text}");
        assert!(text.contains("not a message from the owner"), "{text}");
        assert!(rig.wake.try_recv().is_err());

        // Past it: moved to the background, not dropped.
        let helpers = rig.helpers.clone();
        let slow = tokio::spawn(async move { helpers.delegate(&owner, None, &[], &call("Slow job.", Some(false))).await });
        let child = rig.next_turn().await;
        let text = slow.await.unwrap().unwrap();
        let id = task_id_of(&text);
        assert_eq!(text, launch_result(&id));
        assert!(!child.request.cancel.is_cancelled(), "it keeps running");
        child.answer("Done at last.").await;
        assert_eq!(rig.next_wake().await, "agent:bookkeeper:web");
        assert!(rig.pending_notifications("agent:bookkeeper:web")[0].contains("done\nDone at last.\n"));
    }

    #[tokio::test]
    async fn no_parent_stream_on_background_helper() {
        let mut rig = Rig::new(FOREGROUND_BUDGET);
        let owner = rig.owner_turn("agent:bookkeeper:web");
        rig.helpers.delegate(&owner, None, &[], &call("Look.", None)).await.unwrap();
        let child = rig.next_turn().await;
        let _ = child.events.send(StreamEvent::text("narrating to nobody")).await;
        let _ = child
            .events
            .send(StreamEvent::tool_call(ai::ToolCall { id: "c".into(), name: "read_file".into(), input: serde_json::json!({}) }))
            .await;
        child.answer("Report.").await;
        rig.next_wake().await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        let mut kinds = Vec::new();
        while let Ok(e) = rig.ui.try_recv() {
            assert_eq!(e.parent_session_key, "agent:bookkeeper:web");
            assert!(!e.event.text.contains("narrating"), "helper text never reaches the parent's screen");
            kinds.push(e.event.event_type);
        }
        use ai::StreamEventType::*;
        assert!(kinds.iter().all(|k| matches!(k, SubagentStart | SubagentProgress | SubagentComplete)), "{kinds:?}");
        assert!(kinds.contains(&SubagentStart) && kinds.contains(&SubagentComplete));
    }

    #[tokio::test]
    async fn busy_parent_hears_notification_next_step() {
        let mut rig = Rig::new(FOREGROUND_BUDGET);
        let owner = rig.owner_turn("agent:bookkeeper:web");
        rig.helpers.delegate(&owner, None, &[], &call("Plan the close.", None)).await.unwrap();
        let parent = rig.next_turn().await;
        let parent_key = parent.request.session_key.clone();

        // The helper starts a helper of its own and keeps working.
        rig.helpers.delegate(&parent.request, None, &[], &call("Sum March.", None)).await.unwrap();
        let child = rig.next_turn().await;
        assert!(matches!(child.request.mode, TurnMode::Helper { depth: 2, .. }));
        child.answer("March is 4,210.").await;

        // The running parent gets a notification row in its thread, not a
        // wake, and the owner hears nothing yet.
        for _ in 0..50 {
            if !rig.notification_rows(&parent_key).is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let rows = rig.notification_rows(&parent_key);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].contains("done\nMarch is 4,210.\n"));
        assert!(rig.wake.try_recv().is_err());

        // Its next step reads it; its report then goes to the owner.
        rig.heard(&parent_key);
        parent.answer("Close plan ready; March is 4,210.").await;
        assert_eq!(rig.next_wake().await, "agent:bookkeeper:web");
    }

    #[tokio::test]
    async fn notification_reaches_nested_parent() {
        let mut rig = Rig::new(FOREGROUND_BUDGET);
        let owner = rig.owner_turn("agent:bookkeeper:web");
        rig.helpers.delegate(&owner, None, &[], &call("Plan the close.", None)).await.unwrap();
        let parent = rig.next_turn().await;
        let parent_key = parent.request.session_key.clone();
        rig.helpers.delegate(&parent.request, None, &[], &call("Sum March.", None)).await.unwrap();
        let child = rig.next_turn().await;

        // The parent ends its turn while its helper runs: it does not
        // notify yet.
        rig.heard(&parent_key);
        parent.answer("Waiting on March.").await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(rig.wake.try_recv().is_err(), "a helper with running helpers does not notify");

        // Its helper reports to IT: an idle parent gets a notification turn.
        child.answer("March is 4,210.").await;
        let resumed = rig.next_turn().await;
        assert_eq!(resumed.request.session_key, parent_key);
        assert!(matches!(
            resumed.request.input,
            TurnInput::Notification(ref c) if c.result == "March is 4,210."
        ));
        assert!(rig.wake.try_recv().is_err(), "the owner is not the one told");

        // Then the parent notifies its own parent, once. The report it held
        // while its helper ran is never dropped: it leads.
        rig.heard(&parent_key);
        resumed.answer("Close plan ready.").await;
        assert_eq!(rig.next_wake().await, "agent:bookkeeper:web");
        let pending = rig.pending_notifications("agent:bookkeeper:web");
        assert_eq!(pending.len(), 1);
        assert!(pending[0].contains("done\nEarlier:\nWaiting on March.\n\nClose plan ready.\n"), "{}", pending[0]);
    }

    #[tokio::test]
    async fn stop_reaches_earlier_turn_helpers() {
        let mut rig = Rig::new(FOREGROUND_BUDGET);
        let key = "agent:bookkeeper:web";
        let first_turn = rig.owner_turn(key);
        rig.helpers.delegate(&first_turn, None, &[], &call("Long job.", None)).await.unwrap();
        let child = rig.next_turn().await;
        drop(first_turn); // that turn is over

        let _second_turn = rig.owner_turn(key);
        rig.helpers.stop_session(Some(key));
        assert!(child.request.cancel.is_cancelled(), "the owner's Stop reaches it");

        // A stopped helper leaves a row, and starts nothing.
        for _ in 0..50 {
            if !rig.notification_rows(key).is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let rows = rig.notification_rows(key);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].contains("\": stopped"), "{}", rows[0]);
        assert!(rig.wake.try_recv().is_err());

        // The session's next turn is not born stopped.
        assert!(!rig.helpers.session_token(key).is_cancelled());
    }

    /// Work handed a oneshot to finish on and a slot to report its stop.
    fn gated_work(report: &str) -> (tools::orchestrator::Work, oneshot::Sender<()>, oneshot::Receiver<bool>) {
        let (go, wait) = oneshot::channel::<()>();
        let (stopped_tx, stopped) = oneshot::channel::<bool>();
        let report = report.to_string();
        let work: tools::orchestrator::Work = Box::new(move |cancel, progress, _session| {
            Box::pin(async move {
                let _ = progress.send(StreamEvent::text("Searching 4 angles")).await;
                tokio::select! {
                    _ = wait => {
                        let _ = stopped_tx.send(false);
                        Ok(report)
                    }
                    _ = cancel.cancelled() => {
                        let _ = stopped_tx.send(true);
                        Err("cancelled".into())
                    }
                }
            })
        });
        (work, go, stopped)
    }

    /// D13: the research pipeline's sub-agent (the fact-check) is a helper of
    /// the research run on the one loop: a turn started through the harness
    /// under the run's seat, its session a child of the run's (so Stop
    /// reaches it), limited to the tools it was given, its answer's shape
    /// held by the turn's end check; the answer comes back as data. Before:
    /// `StructuredRunner` drove the provider in a loop of its own.
    #[tokio::test]
    async fn a_research_sub_agent_is_a_helper_of_the_run_on_the_one_loop() {
        let mut rig = Rig::new(FOREGROUND_BUDGET);
        let key = "agent:analyst:web";
        let owner = rig.owner_turn(key);
        let schema = serde_json::json!({
            "type": "object",
            "properties": { "refuted": { "type": "boolean" } },
            "required": ["refuted"]
        });
        let (answers_tx, mut answers) = mpsc::unbounded_channel::<Result<serde_json::Value, String>>();
        let helpers = rig.helpers.clone();
        let work: tools::orchestrator::Work = Box::new(move |_cancel, _progress, session| {
            Box::pin(async move {
                for node in ["verify-1", "verify-2"] {
                    let answer = helpers
                        .answer_as_data(&session, node, format!("Refute claim {node}."), schema.clone(), vec!["search_web".into()], None)
                        .await;
                    let _ = answers_tx.send(answer);
                }
                Ok("done".to_string())
            })
        });
        let (id, _) = rig.helpers.start_work(&owner, "research: rents", work).unwrap();
        let run_session = helper_key(key, &id);

        let first = tokio::time::timeout(Duration::from_secs(5), rig.started.recv()).await.unwrap().unwrap();
        assert_eq!(first.request.session_key, format!("subagent:{run_session}:sa-verify-1"));
        match &first.request.mode {
            TurnMode::Helper { parent_session_key, answer, .. } => {
                assert_eq!(parent_session_key, &run_session);
                assert!(answer.as_deref().is_some_and(|s| s["required"][0] == "refuted"), "the answer's shape rides the turn");
            }
            _ => panic!("a helper turn"),
        }
        assert_eq!(first.request.seat.agent_id, "bookkeeper", "the run's seat");
        assert_eq!(first.request.seat.tool_allowlist, Some(["search_web".to_string()].into_iter().collect()));
        assert!(matches!(&first.request.input, TurnInput::Platform { text } if text == "Refute claim verify-1."));
        first.answer(r#"Checked. {"refuted": true}"#).await;
        let got = tokio::time::timeout(Duration::from_secs(5), answers.recv()).await.unwrap().unwrap();
        assert_eq!(got.unwrap(), serde_json::json!({"refuted": true}), "the answer comes back as data");

        let second = tokio::time::timeout(Duration::from_secs(5), rig.started.recv()).await.unwrap().unwrap();
        second.answer("I couldn't decide.").await;
        let got = tokio::time::timeout(Duration::from_secs(5), answers.recv()).await.unwrap().unwrap();
        assert!(got.unwrap_err().contains("didn't match the schema"), "a shapeless answer is an error, not data");
    }

    /// Deep research is one of the caller's background helpers: the launch
    /// returns at once, its progress is the helper's activity, and its report
    /// comes back as the one notification. Before: it ran inside the call for
    /// up to an hour.
    #[tokio::test]
    async fn background_work_returns_at_once_and_reports_by_notification() {
        let mut rig = Rig::new(FOREGROUND_BUDGET);
        let key = "agent:analyst:web";
        let owner = rig.owner_turn(key);
        let (work, go, _stopped) = gated_work("# Research: rents\nRents rose 4%.");
        let (id, receipt) = rig.helpers.start_work(&owner, "research: rents", work).unwrap();
        assert_eq!(receipt, launch_result(&id), "the receipt, while the work still runs");
        assert!(rig.helpers.list(Some(key), false).iter().any(|h| h.task_id == id && h.running));

        let mut activity = None;
        for _ in 0..3 {
            let ev = tokio::time::timeout(Duration::from_secs(5), rig.ui.recv()).await.unwrap().unwrap();
            if let Some(op) = ev.event.widgets.as_ref().and_then(|w| w["current_operation"].as_str()) {
                activity = Some((ev.parent_session_key.clone(), op.to_string()));
                break;
            }
        }
        assert_eq!(activity, Some((key.to_string(), "Searching 4 angles".to_string())), "progress is the helper's activity");

        let _ = go.send(());
        assert_eq!(rig.next_wake().await, key);
        let pending = rig.pending_notifications(key);
        assert_eq!(pending.len(), 1);
        assert!(pending[0].contains(&format!("helper {id} \"research: rents\": done\n# Research: rents\nRents rose 4%.")), "{}", pending[0]);

        let refused = rig.helpers.send(&owner, None, &[], &id, "also cover Tucson", None).await.unwrap_err();
        assert!(refused.contains("takes no messages"), "{refused}");
    }

    /// The owner's Stop reaches background work like any helper of the
    /// session. Before: the research ran on its own loop Stop could not reach.
    #[tokio::test]
    async fn stop_reaches_background_work() {
        let rig = Rig::new(FOREGROUND_BUDGET);
        let key = "agent:analyst:web";
        let owner = rig.owner_turn(key);
        let (work, _go, stopped) = gated_work("never");
        let (id, _) = rig.helpers.start_work(&owner, "research: rents", work).unwrap();
        rig.helpers.stop_session(Some(key));
        let ended = tokio::time::timeout(Duration::from_secs(5), stopped).await.expect("the stop ended the work");
        assert_ne!(ended, Ok(false), "it ended by the stop, not by finishing");
        for _ in 0..50 {
            if !rig.notification_rows(key).is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let rows = rig.notification_rows(key);
        assert!(rows.len() == 1 && rows[0].contains(&format!("helper {id} \"research: rents\": stopped")), "{rows:?}");
    }

    #[tokio::test]
    async fn status_sees_only_own_children() {
        let mut rig = Rig::new(FOREGROUND_BUDGET);
        let mine = rig.owner_turn("agent:bookkeeper:web");
        let text = rig.helpers.delegate(&mine, None, &[], &call("Look.", None)).await.unwrap();
        let id = task_id_of(&text);
        let _child = rig.next_turn().await;

        let list = rig.helpers.list(Some("agent:bookkeeper:web"), false);
        let started_at = list.first().map(|h| h.started_at).unwrap_or_default();
        assert!(started_at > 0, "the start is kept");
        assert_eq!(
            list,
            vec![HelperStatus {
                task_id: id.clone(),
                description: "read the ledger".into(),
                running: true,
                session_key: helper_key("agent:bookkeeper:web", &id),
                call: None,
                activity: String::new(),
                parent_key: "agent:bookkeeper:web".into(),
                agent_id: list[0].agent_id.clone(),
                started_at,
                waiting: false,
            }]
        );
        // Every session's helpers, for the owner's list of background work.
        assert!(rig.helpers.list(None, false).iter().any(|h| h.task_id == id));
        assert!(rig.helpers.list(Some("agent:ceo:web"), false).is_empty());
        assert!(rig.helpers.read_output("agent:ceo:web", &id).is_err());
        assert!(rig.helpers.stop("agent:ceo:web", &id).is_err());
        assert!(!_child.request.cancel.is_cancelled());
        assert!(rig.helpers.read_output("agent:bookkeeper:web", &id).unwrap().contains("still running"));
        assert_eq!(rig.helpers.stop("agent:bookkeeper:web", &id).unwrap(), format!("Stopping helper {id}."));
        assert!(_child.request.cancel.is_cancelled());
    }

    /// `stop_task` on a running helper: within a second the owner's screen
    /// hears it ended (not successfully), its parent's conversation says
    /// "stopped", and it no longer counts as running. Nothing waits out a
    /// timeout for a helper that was stopped.
    #[tokio::test]
    async fn a_stopped_helper_reports_stopped_within_a_second() {
        let mut rig = Rig::new(FOREGROUND_BUDGET);
        let key = "agent:bookkeeper:web";
        let owner = rig.owner_turn(key);
        let id = task_id_of(&rig.helpers.delegate(&owner, None, &[], &call("Long job.", None)).await.unwrap());
        let _child = rig.next_turn().await;
        assert_eq!(rig.helpers.stop(key, &id).unwrap(), format!("Stopping helper {id}."));
        let done = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let ev = rig.ui.recv().await.expect("the helper's events");
                if ev.event.event_type == ai::StreamEventType::SubagentComplete {
                    return ev;
                }
            }
        })
        .await
        .expect("the stop is heard within a second");
        assert_eq!(done.parent_session_key, key);
        assert_eq!(done.event.widgets.as_ref().map(|w| w["success"].clone()), Some(serde_json::json!(false)), "{:?}", done.event);
        // The row follows the event at once.
        for _ in 0..20 {
            if !rig.notification_rows(key).is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let rows = rig.notification_rows(key);
        assert!(rows.len() == 1 && rows[0].contains(&format!("helper {id} \"read the ledger\": stopped")), "{rows:?}");
        assert!(!rig.helpers.list(Some(key), false).iter().any(|h| h.running));
        assert!(rig.wake.try_recv().is_err(), "a stopped helper starts no turn");
    }

    #[tokio::test]
    async fn a_running_helper_hears_a_message_and_one_landing_late_gets_a_turn() {
        let mut rig = Rig::new(FOREGROUND_BUDGET);
        let owner = rig.owner_turn("agent:bookkeeper:web");
        let id = task_id_of(&rig.helpers.delegate(&owner, None, &[], &call("Sum Q1.", None)).await.unwrap());
        let child = rig.next_turn().await;
        let text = rig.helpers.send(&owner, None, &[], &id, "Include April too.", None).await.unwrap();
        assert!(text.contains("hears it at its next step"), "{text}");
        let child_id = rig.sessions.resolve_session_id_by_key(&child.request.session_key).unwrap();
        let row = rig.sessions.get_messages(&child_id).unwrap().pop().unwrap();
        assert_eq!(row.content, "Include April too.");
        assert!(matches!(crate::harness::conversation::arrived_mid_turn(&row), Some(crate::harness::conversation::MidTurnFrom::Parent { .. })));

        // The turn ended before a step read it: one more turn hears it, and
        // the parent gets both reports, the earlier one marked so.
        child.answer("Q1 is 12,000.").await;
        let again = rig.next_turn().await;
        assert_eq!(again.request.session_key, child.request.session_key);
        assert!(matches!(again.request.input, TurnInput::None));
        rig.heard(&again.request.session_key);
        again.answer("With April, 16,100.").await;
        rig.next_wake().await;
        let pending = rig.pending_notifications("agent:bookkeeper:web");
        assert!(pending[0].contains("done\nEarlier:\nQ1 is 12,000.\n\nWith April, 16,100.\n"), "{}", pending[0]);
    }

    /// A coworker's reply to a helper that asked reaches that helper, not a
    /// chat turn on its session: a running helper hears it (with the
    /// taint it carries) and one that ended before a step read it runs a
    /// turn that does. Once the helper is let go, the caller is told, so
    /// the update goes to the helper's parent instead.
    #[tokio::test]
    async fn an_update_reaches_the_helper_that_asked() {
        let mut rig = Rig::new(FOREGROUND_BUDGET);
        let owner = rig.owner_turn("agent:bookkeeper:web");
        task_id_of(&rig.helpers.delegate(&owner, None, &[], &call("Ask the clerk.", None)).await.unwrap());
        let child = rig.next_turn().await;
        let key = child.request.session_key.clone();
        let reply = notify::render_update("A coworker replied to your message", "[Reply from Clerk]\nFiled.");
        assert!(rig.helpers.notify(&key, &reply, &[ProvenanceClass::Coworker]));
        let rows: Vec<_> = rig.sessions.get_messages(&rig.sessions.resolve_session_id_by_key(&key).unwrap()).unwrap();
        let row = rows.iter().find(|m| notify::is_notification_row(m)).expect("the reply is a row in the helper's thread");
        assert_eq!(notify::row_taint(row), vec![ProvenanceClass::Coworker]);

        child.answer("Asked the clerk.").await;
        let again = rig.next_turn().await;
        assert_eq!(again.request.session_key, key, "the helper runs a turn that hears the reply");
        rig.heard(&key);
        again.answer("The clerk filed it.").await;
        rig.next_wake().await;
        assert!(!rig.helpers.notify(&key, &reply, &[]), "a helper let go takes no update");
    }

    #[tokio::test]
    async fn finished_helper_resumes_from_its_row() {
        let mut rig = Rig::new(FOREGROUND_BUDGET);
        let owner = rig.owner_turn("agent:bookkeeper:web");
        let input = serde_json::json!({"description": "scan logs", "prompt": "Scan.", "helper_type": "explore"});
        let id = task_id_of(&rig.helpers.delegate(&owner, None, &[], &input).await.unwrap());
        let child = rig.next_turn().await;
        rig.heard(&child.request.session_key);
        child.answer("Three errors.").await;
        rig.next_wake().await;
        assert!(rig.helpers.read_output("agent:bookkeeper:web", &id).unwrap().contains("Three errors."));

        // A restart: a fresh registry knows the helper only by its row.
        let Rig { store, _dir, .. } = rig;
        let mut rig = Rig::on(store, _dir, FOREGROUND_BUDGET);
        let owner = rig.owner_turn("agent:bookkeeper:web");
        let text = rig.helpers.send(&owner, None, &[], &id, "Which job threw them?", None).await.unwrap();
        assert_eq!(text, launch_result(&id));
        let resumed = rig.next_turn().await;
        assert_eq!(resumed.request.session_key, helper_key("agent:bookkeeper:web", &id));
        assert!(matches!(resumed.request.mode, TurnMode::Helper { kind: HelperKind::Explore, depth: 1, .. }));
        assert!(matches!(resumed.request.input, TurnInput::None), "the message is already in its thread");

        // Only the helper's own parent can resume it.
        let other = rig.owner_turn("agent:ceo:web");
        assert!(rig.helpers.send(&other, None, &[], &id, "hi", None).await.is_err());
    }

    fn target(key: &str, read_only: bool) -> Target {
        Target {
            tool: key.into(),
            key: key.into(),
            operation: None,
            capability: None,
            field: None,
            subject: None,
            read_only,
            effects: CallEffects::none(),
        }
    }

    fn helper_mode(kind: HelperKind, depth: u8) -> TurnMode {
        TurnMode::Helper { parent_session_key: "agent:x:web".into(), kind, depth, answer: None }
    }

    #[test]
    fn explore_has_no_write_tools() {
        for kind in [HelperKind::Explore, HelperKind::Plan] {
            let mode = helper_mode(kind, 1);
            assert!(permits(&mode, &target("read_file", true)).is_ok());
            assert!(permits(&mode, &target("grep", true)).is_ok());
            assert!(permits(&mode, &target("write_file", false)).is_err());
            assert!(permits(&mode, &target("run_command", false)).is_err(), "no state-changing shell");
            assert!(permits(&mode, &target("delegate", false)).is_err());
            assert!(!on_surface(&mode, "delegate"));
        }
        let general = helper_mode(HelperKind::General, 1);
        assert!(permits(&general, &target("write_file", false)).is_ok());
        assert!(permits(&TurnMode::Chat, &target("write_file", false)).is_ok());
    }

    /// Every run declares `exit`; only a workflow activity is offered it
    /// and may call it.
    #[test]
    fn only_a_workflow_activity_exits() {
        let activity = TurnMode::Workflow(Box::default());
        assert!(on_surface(&activity, "exit") && permits(&activity, &target("exit", true)).is_ok());
        for mode in [TurnMode::Chat, helper_mode(HelperKind::General, 1)] {
            assert!(!on_surface(&mode, "exit"));
            let refused = permits(&mode, &target("exit", true)).unwrap_err();
            assert!(refused.contains("isn't one"), "{refused}");
        }
    }

    #[tokio::test]
    async fn depth_cap_removes_helper_tool() {
        assert!(on_surface(&helper_mode(HelperKind::General, 2), "delegate"));
        assert!(!on_surface(&helper_mode(HelperKind::General, 3), "delegate"));
        assert!(on_surface(&helper_mode(HelperKind::General, 3), "read_file"));
        assert!(permits(&helper_mode(HelperKind::General, 3), &target("delegate", false)).is_err());
        assert_eq!(depth_of("subagent:subagent:subagent:agent:x:web:a:b:c"), 3);

        // Launching from the cap is refused, whatever the mode claims.
        let rig = Rig::new(FOREGROUND_BUDGET);
        let mut deep = rig.owner_turn("subagent:subagent:subagent:agent:x:web:a:b:c");
        deep.mode = helper_mode(HelperKind::General, 3);
        assert!(rig.helpers.delegate(&deep, None, &[], &call("x", None)).await.is_err());
        deep.mode = TurnMode::Chat;
        assert!(rig.helpers.delegate(&deep, None, &[], &call("x", None)).await.is_err(), "the key counts too");
    }

    #[test]
    fn a_delegate_call_reads_the_interface() {
        let spec = HelperSpec::from_input(&serde_json::json!({"description": "d", "prompt": "p"})).unwrap();
        assert!(spec.background, "background by default");
        assert_eq!(spec.kind, HelperKind::General);
        let spec = HelperSpec::from_input(&serde_json::json!({
            "description": "d", "prompt": "p", "helper_type": "Plan", "background": false
        }))
        .unwrap();
        assert_eq!((spec.kind, spec.background), (HelperKind::Plan, false));
        assert_eq!(spec.report_limit, None, "the default report limit unless set");
        let spec = HelperSpec::from_input(&serde_json::json!({"description": "d", "prompt": "p", "report_limit": 12_000})).unwrap();
        assert_eq!(spec.report_limit, Some(12_000));
        let spec = HelperSpec::from_input(&serde_json::json!({"description": "d", "prompt": "p", "report_limit": 0})).unwrap();
        assert_eq!(spec.report_limit, None, "0 is no limit set");
        assert!(HelperSpec::from_input(&serde_json::json!({"description": "d"})).is_err());
        assert!(HelperSpec::from_input(&serde_json::json!({"description": "d", "prompt": "p", "helper_type": "coder"})).is_err());
    }

    /// D16: a run that can delegate is told each helper type and when it
    /// fits, explore's line naming wide searches; a run that can't
    /// delegate (explore, plan, the depth cap) is told none.
    #[test]
    fn the_helper_types_listing_says_when_each_type_fits() {
        let listed = helper_types(&TurnMode::Chat);
        assert_eq!(listed.keys().map(String::as_str).collect::<Vec<_>>(), ["explore", "general", "plan"]);
        assert!(listed["explore"].starts_with("Wide searches: when answering means going through many files"), "{}", listed["explore"]);
        assert!(listed["explore"].contains("it changes nothing and starts no helpers"));
        assert!(listed["general"].contains("(Tools: all of yours.)"));
        assert_eq!(helper_types(&helper_mode(HelperKind::General, 2)), listed, "a general helper under the cap");
        for mode in [helper_mode(HelperKind::Explore, 1), helper_mode(HelperKind::Plan, 1), helper_mode(HelperKind::General, 3)] {
            assert!(helper_types(&mode).is_empty());
        }
    }

    /// D18: a goal is the owner's. A helper is neither offered
    /// suggest_goal nor allowed to call it; the owner's own turn is.
    #[test]
    fn a_helper_never_proposes_a_goal() {
        for kind in HelperKind::ALL {
            let mode = helper_mode(kind, 1);
            assert!(!on_surface(&mode, "suggest_goal"), "{kind:?}");
            let refused = permits(&mode, &target("suggest_goal", true)).unwrap_err();
            assert!(refused.contains("a helper doesn't propose one"), "{refused}");
        }
        assert!(on_surface(&TurnMode::Chat, "suggest_goal"));
        assert!(permits(&TurnMode::Chat, &target("suggest_goal", true)).is_ok());
    }

    /// D17: at most MAX_RUNNING helpers run at once in one conversation's
    /// tree, nested ones included (the fixture run where one helper started
    /// about fifty). The next launch is refused and told not to retry;
    /// another conversation is untouched, and a finished helper frees its
    /// place.
    #[tokio::test]
    async fn a_conversation_runs_at_most_max_running_helpers_at_once() {
        let mut rig = Rig::new(FOREGROUND_BUDGET);
        let owner = rig.owner_turn("agent:ops:web");
        rig.helpers.delegate(&owner, None, &[], &call("Watch the others.", None)).await.unwrap();
        let babysitter = rig.next_turn().await;
        let mut started = Vec::new();
        for i in 1..MAX_RUNNING {
            rig.helpers
                .delegate(&babysitter.request, None, &[], &call(&format!("check {i}"), None))
                .await
                .unwrap_or_else(|e| panic!("helper {i} of {MAX_RUNNING}: {e}"));
            started.push(rig.next_turn().await);
        }
        let refused = rig.helpers.delegate(&babysitter.request, None, &[], &call("check again", None)).await.unwrap_err();
        assert!(refused.contains(&format!("{MAX_RUNNING} helpers are already running in this conversation")) && refused.contains("Don't retry"), "{refused}");
        assert!(rig.helpers.delegate(&owner, None, &[], &call("one more", None)).await.is_err(), "the owner's own launch counts the whole tree");

        let other = rig.owner_turn("agent:sales:web");
        rig.helpers.delegate(&other, None, &[], &call("elsewhere", None)).await.expect("another conversation has its own count");
        let _elsewhere = rig.next_turn().await;

        started.pop().unwrap().answer("done").await;
        for _ in 0..200 {
            if rig.helpers.delegate(&owner, None, &[], &call("now there's room", None)).await.is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("a finished helper never freed its place");
    }

    /// A delegate call's `report_limit`, and each piece's, is the helper's:
    /// held while it runs and kept on its row, so a resumed helper keeps it.
    /// A call that sets none gets the lean default.
    #[tokio::test]
    async fn a_report_limit_is_carried_per_helper() {
        let mut rig = Rig::new(FOREGROUND_BUDGET);
        let owner = rig.owner_turn("agent:agent1:web");
        let mut input = call("Research the 30 listings.", None);
        input["report_limit"] = 20_000.into();
        let id = task_id_of(&rig.helpers.delegate(&owner, None, &[], &input).await.unwrap());
        let plain = task_id_of(&rig.helpers.delegate(&owner, None, &[], &call("Check one listing.", None)).await.unwrap());
        assert_eq!(rig.helpers.state().helpers[&id].report_limit, 20_000);
        assert_eq!(rig.helpers.state().helpers[&plain].report_limit, collect::REPORT_LIMIT);
        let row = rig.store.get_pending_task(&id).unwrap().unwrap();
        assert_eq!(spec_of_row(&rig.store, &row).report_limit, Some(20_000), "a restart keeps it");
        let row = rig.store.get_pending_task(&plain).unwrap().unwrap();
        assert_eq!(spec_of_row(&rig.store, &row).report_limit, None);
        rig.next_turn().await;
        rig.next_turn().await;
    }

    /// A helper's question for the owner comes back to the conversation
    /// that started it as a partial report: the question, its options and
    /// the recommended one, and how to answer. The answer, sent with
    /// send_message, runs the helper again from where it stopped.
    #[tokio::test]
    async fn a_helpers_question_reaches_its_parent_and_the_answer_resumes_it() {
        let mut rig = Rig::new(FOREGROUND_BUDGET);
        let owner = rig.owner_turn("agent:sdr:web");
        let id = task_id_of(&rig.helpers.delegate(&owner, None, &[], &call("Draft the outreach batch.", None)).await.unwrap());
        let child = rig.next_turn().await;
        let q = types::OwnerQuestion { question: "Which vendor?".into(), options: vec!["Acme".into(), "Globex".into()], ..Default::default() };
        let _ = child.events.send(StreamEvent::text("Drafted 18 of 20.")).await;
        let _ = child.events.send(StreamEvent::tool_call(ai::ToolCall { id: "c1".into(), name: "ask_owner".into(), input: serde_json::json!({}) })).await;
        let _ = child.events.send(StreamEvent::owner_decision("Your question goes back.", &q)).await;
        let _ = child.events.send(StreamEvent::done_with_reason("terminal_tool_error")).await;
        assert_eq!(rig.next_wake().await, "agent:sdr:web");
        let pending = rig.pending_notifications("agent:sdr:web");
        for part in [
            format!("helper {id} \"read the ledger\": partial ({}", collect::NEEDS_OWNER_DECISION),
            "Drafted 18 of 20.".to_string(),
            "Question: Which vendor?".to_string(),
            "Recommended: Acme".to_string(),
            format!("send_message to {id} with the decision in `answer`"),
        ] {
            assert!(pending[0].contains(&part), "{part:?} missing from:\n{}", pending[0]);
        }
        let owner = rig.owner_turn("agent:sdr:web");
        assert_eq!(rig.helpers.send(&owner, None, &[], &id, "The owner picked Acme.", Some(ans("Acme"))).await.unwrap(), launch_result(&id));
        let resumed = rig.next_turn().await;
        assert_eq!(resumed.request.session_key, helper_key("agent:sdr:web", &id));
    }

    /// The question the scripted helper turn `t` asks: a few words, the
    /// ask_owner call, the decision notice and the turn's end.
    async fn ask(t: &Started, words: &str, q: &types::OwnerQuestion) {
        let _ = t.events.send(StreamEvent::text(words)).await;
        let _ = t.events.send(StreamEvent::tool_call(ai::ToolCall { id: "c1".into(), name: "ask_owner".into(), input: serde_json::json!({}) })).await;
        let _ = t.events.send(StreamEvent::owner_decision("Your question goes back.", q)).await;
        let _ = t.events.send(StreamEvent::done_with_reason(ai::OWNER_DECISION)).await;
    }

    /// Start owner → A → B, with B running; returns A's id, A's turn and B's turn.
    async fn a_with_running_child(rig: &mut Rig, owner: &TurnRequest) -> (String, Started, Started) {
        let a_id = task_id_of(&rig.helpers.delegate(owner, None, &[], &call("Plan the outreach.", None)).await.unwrap());
        let a = rig.next_turn().await;
        rig.helpers.delegate(&a.request, None, &[], &call("Find the contacts.", None)).await.unwrap();
        let b = rig.next_turn().await;
        (a_id, a, b)
    }

    async fn until_held(rig: &Rig, id: &str) {
        for _ in 0..200 {
            if rig.helpers.state().helpers.get(id).is_some_and(|h| h.held.is_some()) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("{id} was never held");
    }

    fn ans(text: &str) -> tools::orchestrator::Answer {
        tools::orchestrator::Answer { question: None, text: text.into() }
    }

    fn vendor() -> types::OwnerQuestion {
        types::OwnerQuestion { question: "Which vendor?".into(), options: vec!["Acme".into(), "Globex".into()], ..Default::default() }
    }

    /// A helper two levels down asks while a helper of its own still runs:
    /// its report is held. When that helper reports, the helper runs again,
    /// reminded that its question is still open, and reports progress
    /// without asking again: no answer was sent, so the question is still
    /// pending. The run needs the owner's decision and the question leads.
    #[tokio::test]
    async fn a_held_question_survives_the_turn_that_hears_its_child() {
        let mut rig = Rig::new(FOREGROUND_BUDGET);
        let owner = rig.owner_turn("agent:sdr:web");
        let (a_id, a, b) = a_with_running_child(&mut rig, &owner).await;
        ask(&a, "Drafted the plan.", &vendor()).await;
        until_held(&rig, &a_id).await;

        b.answer("40 contacts found.").await;
        let resumed = rig.next_turn().await;
        assert_eq!(resumed.request.session_key, a.request.session_key, "the helper hears its child");
        assert!(
            rig.notification_rows(&a.request.session_key).iter().any(|r| r.contains("Your question for the owner is still open")),
            "the resumed turn is reminded the question is open"
        );
        rig.heard(&resumed.request.session_key);
        resumed.answer("Contacts are in.").await;
        assert_eq!(rig.next_wake().await, "agent:sdr:web");
        let pending = rig.pending_notifications("agent:sdr:web");
        assert!(pending[0].contains(&format!("helper {a_id} \"read the ledger\": partial ({}", collect::NEEDS_OWNER_DECISION)), "{}", pending[0]);
        let question = pending[0].find("Question: Which vendor?").expect("the question");
        assert!(question < pending[0].find("Earlier:\nWhere it stands: Drafted the plan.").expect("the held turn"), "the question leads: {}", pending[0]);
        assert!(pending[0].contains("Contacts are in."), "{}", pending[0]);
        // read_output shows the whole report, the question first.
        let read = rig.helpers.read_output("agent:sdr:web", &a_id).unwrap();
        assert!(read.contains("Question: Which vendor?") && read.contains("Earlier:"), "{read}");
    }

    /// A helper that asks again before the answer comes: the question is
    /// shown once, leading.
    #[tokio::test]
    async fn a_question_asked_twice_is_shown_once() {
        let mut rig = Rig::new(FOREGROUND_BUDGET);
        let owner = rig.owner_turn("agent:sdr:web");
        let (a_id, a, b) = a_with_running_child(&mut rig, &owner).await;
        ask(&a, "Drafted the plan.", &vendor()).await;
        until_held(&rig, &a_id).await;
        b.answer("40 contacts found.").await;
        let resumed = rig.next_turn().await;
        rig.heard(&resumed.request.session_key);
        ask(&resumed, "Contacts are in.", &vendor()).await;
        assert_eq!(rig.next_wake().await, "agent:sdr:web");
        let pending = rig.pending_notifications("agent:sdr:web");
        assert_eq!(pending[0].matches("Question: Which vendor?").count(), 1, "{}", pending[0]);
        assert!(pending[0].contains(&format!("partial ({}", collect::NEEDS_OWNER_DECISION)), "{}", pending[0]);
    }

    /// The conversation that started a helper sends it the owner's answer
    /// while the helper holds a report for its own helper: the answer
    /// settles the question, and the held turn is kept in the run (only the
    /// assembled copy goes), so its next report still carries it, answered.
    #[tokio::test]
    async fn a_message_to_a_helper_holding_a_question_keeps_it() {
        let mut rig = Rig::new(FOREGROUND_BUDGET);
        let owner = rig.owner_turn("agent:sdr:web");
        let (a_id, a, _b) = a_with_running_child(&mut rig, &owner).await;
        ask(&a, "Drafted the plan.", &vendor()).await;
        until_held(&rig, &a_id).await;
        assert!(rig.store.engine_get_run(&a_id).unwrap().unwrap().summary.split_whitespace().any(|t| t == ROW_HELD));
        rig.helpers.send(&owner, None, &[], &a_id, "Acme.", Some(ans("Acme"))).await.unwrap();
        {
            let state = rig.helpers.state();
            let a_now = &state.helpers[&a_id];
            assert!(a_now.held.is_none() && a_now.questions.pending.is_empty(), "answered, and the held copy goes");
            assert_eq!(a_now.questions.answered.len(), 1);
            assert_eq!(a_now.earlier_reports.len(), 1, "the held turn is kept");
            assert!(a_now.earlier_reports[0].question.as_ref().is_some_and(|q| q.text == "Which vendor?"));
        }
        let row = rig.store.engine_get_run(&a_id).unwrap().unwrap();
        assert_eq!(row.summary, "", "running again: no longer held, no longer asking");
        let next = rig.next_turn().await;
        assert!(!rig.notification_rows(&next.request.session_key).iter().any(|r| r.contains("still open")), "no reminder once answered");
    }

    /// A restart while a helper holds its question for its own helper: the
    /// interrupted helper fails, and the held report, kept on its parent's
    /// row, still reaches the owner's conversation as a question.
    #[tokio::test]
    async fn a_held_question_survives_a_restart() {
        let mut rig = Rig::new(FOREGROUND_BUDGET);
        let owner = rig.owner_turn("agent:sdr:web");
        let (a_id, a, _b) = a_with_running_child(&mut rig, &owner).await;
        ask(&a, "Drafted the plan.", &vendor()).await;
        until_held(&rig, &a_id).await;
        let Rig { store, _dir, .. } = rig;
        let mut rig = Rig::on(store, _dir, FOREGROUND_BUDGET);
        rig.helpers.recover();
        assert_eq!(rig.next_wake().await, "agent:sdr:web");
        let pending = rig.pending_notifications("agent:sdr:web");
        let held = pending.iter().find(|p| p.contains(&format!("helper {a_id} "))).expect("the held report is delivered");
        assert!(held.contains(&format!("partial ({}", collect::NEEDS_OWNER_DECISION)) && held.contains("Question: Which vendor?"), "{held}");
        assert!(held.contains("was interrupted by a restart"), "{held}");
    }

    /// Nested holds, owner → A → B → C, with C running: B asked and holds
    /// for C, A holds for B. A restart releases them bottom-up, each to its
    /// own parent: B's report, its question first, lands in A's thread, and
    /// A's reaches the owner's conversation.
    #[tokio::test]
    async fn nested_held_reports_are_released_bottom_up_after_a_restart() {
        let mut rig = Rig::new(FOREGROUND_BUDGET);
        let owner = rig.owner_turn("agent:sdr:web");
        let (a_id, a, b) = a_with_running_child(&mut rig, &owner).await;
        let b_id = split_helper_key(&b.request.session_key).unwrap().1.to_string();
        rig.helpers.delegate(&b.request, None, &[], &call("Check the CRM.", None)).await.unwrap();
        let _c = rig.next_turn().await;
        a.answer("Waiting on the contacts.").await;
        until_held(&rig, &a_id).await;
        ask(&b, "Need a vendor first.", &vendor()).await;
        until_held(&rig, &b_id).await;
        let a_key = a.request.session_key.clone();

        let Rig { store, _dir, .. } = rig;
        let mut rig = Rig::on(store, _dir, FOREGROUND_BUDGET);
        rig.helpers.recover();
        assert_eq!(rig.next_wake().await, "agent:sdr:web");
        let pending = rig.pending_notifications("agent:sdr:web");
        let a_report = pending.iter().find(|p| p.contains(&format!("helper {a_id} "))).expect("A reports to the owner's conversation");
        assert!(
            a_report.contains("Waiting on the contacts.")
                && a_report.contains(&format!("It was holding this report for its helper {b_id}, which reported after a restart")),
            "{a_report}"
        );
        assert!(a_report.contains("Question: Which vendor?"), "the owner's conversation has B's question: {a_report}");
        let rows = rig.notification_rows(&a_key);
        let b_report = rows.iter().find(|r| r.contains(&format!("helper {b_id} "))).expect("B's report lands in A's thread");
        assert!(b_report.contains("Question: Which vendor?") && b_report.contains(&format!("partial ({}", collect::NEEDS_OWNER_DECISION)), "{b_report}");
        for id in [&a_id, &b_id] {
            assert!(!rig.store.engine_get_run(id).unwrap().unwrap().summary.split_whitespace().any(|t| t == ROW_HELD), "{id} released");
        }
    }

    /// A long report on the race path (input lands as the turn ends, so the
    /// run goes on) is kept on the row assembled and held to the run's
    /// limit, and a restart delivers that, not the raw text.
    #[test]
    fn a_report_kept_while_the_run_goes_on_is_held_to_its_limit() {
        crate::test_home::with_home(|_| {
            tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
                let mut rig = Rig::new(FOREGROUND_BUDGET);
                let owner = rig.owner_turn("agent:sdr:web");
                let mut input = call("Research the listings.", None);
                input["report_limit"] = 1_000.into();
                let id = task_id_of(&rig.helpers.delegate(&owner, None, &[], &input).await.unwrap());
                let child = rig.next_turn().await;
                rig.helpers.send(&owner, None, &[], &id, "Add Denver.", None).await.unwrap();
                child.answer(&"Listing row. ".repeat(6_000)).await;
                let _again = rig.next_turn().await;
                let kept = rig.store.engine_get_run(&id).unwrap().unwrap().result.unwrap_or_default();
                assert!(kept.contains(tools::result_shape::SAVED_OUTPUT) && kept.chars().count() < 3_000, "{} chars", kept.chars().count());

                let Rig { store, _dir, .. } = rig;
                let mut rig = Rig::on(store, _dir, FOREGROUND_BUDGET);
                rig.helpers.recover();
                assert_eq!(rig.next_wake().await, "agent:sdr:web");
                let pending = rig.pending_notifications("agent:sdr:web");
                assert!(pending[0].contains(tools::result_shape::SAVED_OUTPUT) && pending[0].chars().count() < 4_000, "{} chars", pending[0].chars().count());
            })
        });
    }

    /// A helper's question goes to the turn that started it, whatever its
    /// door. A scheduled run that started it is woken with it and told what
    /// to do when nobody can answer: decide or take it up the line, and
    /// send the helper the answer either way, never leaving it waiting.
    #[tokio::test]
    async fn an_unattended_parent_is_woken_and_must_answer_its_helper() {
        let mut rig = Rig::new(FOREGROUND_BUDGET);
        let mut owner = rig.owner_turn("agent:sdr:schedule:daily");
        owner.seat.door = types::permissions::Door::Schedule;
        owner.seat.origin = tools::Origin::System;
        let id = task_id_of(&rig.helpers.delegate(&owner, None, &[], &call("Draft the batch.", None)).await.unwrap());
        let child = rig.next_turn().await;
        let q = types::OwnerQuestion { question: "Send today or Monday?".into(), options: vec!["Monday".into(), "Today".into()], ..Default::default() };
        ask(&child, "Drafted 20.", &q).await;
        assert_eq!(rig.next_wake().await, "agent:sdr:schedule:daily", "the unattended parent is woken");
        let pending = rig.pending_notifications("agent:sdr:schedule:daily");
        for part in [
            "Question: Send today or Monday?".to_string(),
            "If no one can answer it in your conversation, decide it yourself or take it up the line as ask_owner says.".to_string(),
            format!("send_message to {id} with the decision in `answer`"),
            "Never leave it waiting.".to_string(),
        ] {
            assert!(pending[0].contains(&part), "{part:?} missing from:\n{}", pending[0]);
        }
        let row = rig.store.get_pending_task(&id).unwrap().unwrap();
        assert_eq!(row.status, "completed", "its row is readable and it can be resumed");
        assert_eq!(rig.helpers.send(&owner, None, &[], &id, "Monday.", Some(ans("Monday"))).await.unwrap(), launch_result(&id));
        assert_eq!(rig.next_turn().await.request.session_key, helper_key("agent:sdr:schedule:daily", &id));
    }

    /// An answer sent while the helper runs settles the question that same
    /// turn then asks (it may have asked before it read the answer): no
    /// reminder, and its report is not a question.
    #[tokio::test]
    async fn an_answer_sent_while_running_settles_the_question_that_turn_asks() {
        let mut rig = Rig::new(FOREGROUND_BUDGET);
        let owner = rig.owner_turn("agent:sdr:web");
        let id = task_id_of(&rig.helpers.delegate(&owner, None, &[], &call("Plan.", Some(true))).await.unwrap());
        let a = rig.next_turn().await;
        rig.helpers.send(&owner, None, &[], &id, "Use Acme.", Some(ans("Acme"))).await.unwrap();
        ask(&a, "Drafted.", &vendor()).await;
        let again = rig.next_turn().await;
        let reminded = rig.notification_rows(&again.request.session_key).iter().any(|r| r.contains(STILL_OPEN));
        rig.heard(&again.request.session_key);
        again.answer("Went with Acme.").await;
        assert_eq!(rig.next_wake().await, "agent:sdr:web");
        let p = rig.pending_notifications("agent:sdr:web");
        assert!(!reminded, "not reminded: the answer is in its thread");
        assert!(!p[0].contains(collect::NEEDS_OWNER_DECISION), "{}", p[0]);
        assert!(p[0].contains("Asked: Which vendor?\nAnswered: Acme"), "{}", p[0]);
        assert!(!p[0].contains("ask them with ask_owner"), "an answered question drops its instructions: {}", p[0]);
    }

    /// A message without `answer` never settles a question: it stays
    /// pending, reminders continue, and it reaches the owner's conversation.
    #[tokio::test]
    async fn a_message_without_an_answer_leaves_the_question_pending() {
        let mut rig = Rig::new(FOREGROUND_BUDGET);
        let owner = rig.owner_turn("agent:sdr:web");
        let (a_id, a, b) = a_with_running_child(&mut rig, &owner).await;
        ask(&a, "Drafted the plan.", &vendor()).await;
        until_held(&rig, &a_id).await;
        rig.helpers.send(&owner, None, &[], &a_id, "Also cover Denver.", None).await.unwrap();
        let a2 = rig.next_turn().await;
        assert!(rig.notification_rows(&a2.request.session_key).iter().any(|r| r.contains(STILL_OPEN)), "still reminded");
        rig.heard(&a2.request.session_key);
        a2.answer("Denver added.").await;
        until_held(&rig, &a_id).await;
        b.answer("40 contacts.").await;
        let a3 = rig.next_turn().await;
        rig.heard(&a3.request.session_key);
        a3.answer("All done.").await;
        assert_eq!(rig.next_wake().await, "agent:sdr:web");
        let p = rig.pending_notifications("agent:sdr:web");
        assert!(p.iter().any(|x| x.contains(collect::NEEDS_OWNER_DECISION) && x.contains("Question: Which vendor?")), "{}", p.join("\n---\n"));
    }

    /// A question at depth 3, held through a restart, reaches the owner's
    /// conversation through the released reports above it.
    #[tokio::test]
    async fn a_depth_three_question_reaches_the_owner_after_a_restart() {
        let mut rig = Rig::new(FOREGROUND_BUDGET);
        let owner = rig.owner_turn("agent:sdr:web");
        let (a_id, a, b) = a_with_running_child(&mut rig, &owner).await;
        let b_id = split_helper_key(&b.request.session_key).unwrap().1.to_string();
        rig.helpers.delegate(&b.request, None, &[], &call("Check the CRM.", None)).await.unwrap();
        let _c = rig.next_turn().await;
        a.answer("Waiting on the contacts.").await;
        until_held(&rig, &a_id).await;
        ask(&b, "Need a vendor first.", &vendor()).await;
        until_held(&rig, &b_id).await;
        let Rig { store, _dir, .. } = rig;
        let mut rig = Rig::on(store, _dir, FOREGROUND_BUDGET);
        rig.helpers.recover();
        assert_eq!(rig.next_wake().await, "agent:sdr:web");
        let p = rig.pending_notifications("agent:sdr:web");
        let a_report = p.iter().find(|x| x.contains(&format!("helper {a_id} "))).expect("A reports");
        assert!(a_report.contains("Which vendor?") && a_report.contains(&format!("partial ({}", collect::NEEDS_OWNER_DECISION)), "{a_report}");
    }

    /// A foreground launch gets the question; the run's later report, after
    /// a plain message, says a question is still waiting without giving it
    /// again, and the row keeps that the parent has it.
    #[tokio::test]
    async fn a_question_is_delivered_once() {
        let mut rig = Rig::new(FOREGROUND_BUDGET);
        let h = rig.helpers.clone();
        let o2 = rig.owner_turn("agent:sdr:web");
        let fg = tokio::spawn(async move { h.delegate(&o2, None, &[], &call("Plan.", Some(false))).await });
        let a = rig.next_turn().await;
        let id = split_helper_key(&a.request.session_key).unwrap().1.to_string();
        ask(&a, "Drafted.", &vendor()).await;
        let first = fg.await.unwrap().unwrap();
        assert!(first.contains("Which vendor?"), "{first}");
        assert!(!questions_of_row(&rig.store, &id).reported.is_empty(), "the row keeps that the parent has it");
        let owner = rig.owner_turn("agent:sdr:web");
        rig.helpers.send(&owner, None, &[], &id, "Also cover Denver.", None).await.unwrap();
        let a2 = rig.next_turn().await;
        rig.heard(&a2.request.session_key);
        a2.answer("Denver added.").await;
        assert_eq!(rig.next_wake().await, "agent:sdr:web");
        let p = rig.pending_notifications("agent:sdr:web");
        let later = p.iter().find(|x| x.contains("Denver added.")).expect("the later report");
        assert!(!later.contains("Which vendor?"), "given twice: {later}");
        assert!(later.contains("reported before still wait for an answer") && later.contains(collect::NEEDS_OWNER_DECISION), "{later}");
    }

    /// However many turns a helper with an open question runs, its thread
    /// holds one reminder.
    #[tokio::test]
    async fn reminders_never_accumulate() {
        let mut rig = Rig::new(FOREGROUND_BUDGET);
        let owner = rig.owner_turn("agent:sdr:web");
        let a_id = task_id_of(&rig.helpers.delegate(&owner, None, &[], &call("Plan.", Some(true))).await.unwrap());
        let a = rig.next_turn().await;
        let mut kids = Vec::new();
        for i in 0..4 {
            rig.helpers.delegate(&a.request, None, &[], &call(&format!("k{i}"), Some(true))).await.unwrap();
            kids.push(rig.next_turn().await);
        }
        ask(&a, "Drafted.", &vendor()).await;
        until_held(&rig, &a_id).await;
        for k in kids {
            k.answer("ok").await;
            let t = rig.next_turn().await;
            rig.heard(&t.request.session_key);
            t.answer("noted").await;
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let key = helper_key("agent:sdr:web", &a_id);
        let n = rig.notification_rows(&key).iter().filter(|r| r.contains(STILL_OPEN)).count();
        assert_eq!(n, 1, "one reminder at a time");
    }

    /// An answer sent to a running helper re-tags its row at once: a
    /// restart before the turn ends doesn't raise the answered question.
    #[tokio::test]
    async fn an_answer_to_a_running_helper_is_on_its_row_at_once() {
        let mut rig = Rig::new(FOREGROUND_BUDGET);
        let owner = rig.owner_turn("agent:sdr:web");
        let (a_id, a, b) = a_with_running_child(&mut rig, &owner).await;
        ask(&a, "Drafted the plan.", &vendor()).await;
        until_held(&rig, &a_id).await;
        b.answer("40 contacts.").await;
        let _a2 = rig.next_turn().await;
        rig.helpers.send(&owner, None, &[], &a_id, "Acme.", Some(ans("Acme"))).await.unwrap();
        let row = rig.store.engine_get_run(&a_id).unwrap().unwrap();
        assert!(!row.summary.split_whitespace().any(|t| t == ROW_ASKS_OWNER), "{:?}", row.summary);
        assert!(row.result.unwrap_or_default().contains("Answered: Acme"));
        let Rig { store, _dir, .. } = rig;
        let mut rig = Rig::on(store, _dir, FOREGROUND_BUDGET);
        rig.helpers.recover();
        assert_eq!(rig.next_wake().await, "agent:sdr:web");
        let p = rig.pending_notifications("agent:sdr:web");
        assert!(!p[0].contains(collect::NEEDS_OWNER_DECISION) && !p[0].contains("waiting on the owner"), "{}", p[0]);
    }

    /// A newer question doesn't replace an unanswered older one: both stay
    /// pending and both lead, the newest first. An interrupted run that was
    /// waiting stays failed, and says it was waiting.
    #[tokio::test]
    async fn two_open_questions_both_lead_and_an_interrupted_one_stays_failed() {
        let mut rig = Rig::new(FOREGROUND_BUDGET);
        let owner = rig.owner_turn("agent:sdr:web");
        let (a_id, a, b) = a_with_running_child(&mut rig, &owner).await;
        ask(&a, "Drafted.", &vendor()).await;
        until_held(&rig, &a_id).await;
        b.answer("40 contacts.").await;
        let a2 = rig.next_turn().await;
        rig.heard(&a2.request.session_key);
        let budget = types::OwnerQuestion { question: "What budget?".into(), options: vec!["$5k".into()], ..Default::default() };
        ask(&a2, "Need the budget too.", &budget).await;
        assert_eq!(rig.next_wake().await, "agent:sdr:web");
        let p = rig.pending_notifications("agent:sdr:web");
        let (budget_at, vendor_at) = (p[0].find("Question: What budget?").expect("newer"), p[0].find("Question: Which vendor?").expect("older"));
        assert!(budget_at < vendor_at, "both pending, the newest first: {}", p[0]);

        // Its next run is interrupted by a restart while still waiting.
        rig.helpers.send(&owner, None, &[], &a_id, "Keep going on the rest.", None).await.unwrap();
        let _a3 = rig.next_turn().await;
        let Rig { store, _dir, .. } = rig;
        let mut rig = Rig::on(store, _dir, FOREGROUND_BUDGET);
        rig.helpers.recover();
        assert_eq!(rig.next_wake().await, "agent:sdr:web");
        let p = rig.pending_notifications("agent:sdr:web");
        let failed = p.iter().find(|x| x.contains("failed (interrupted by a restart")).expect("the interrupted run reports");
        assert!(failed.contains("waiting on the owner's decision") && failed.contains("Question: What budget?"), "{failed}");
    }

    /// A nested helper that starts research doesn't finish while the
    /// research runs, and neither does the helper above it: B's report
    /// waits for the research, A's for B's, and the research's results reach
    /// B, whose report reaches A, whose report reaches the owner's
    /// conversation. Nothing reports with work still going below it.
    #[tokio::test]
    async fn a_helper_holds_until_the_research_it_started_reports() {
        let mut rig = Rig::new(FOREGROUND_BUDGET);
        let owner = rig.owner_turn("agent:sdr:web");
        let (a_id, a, b) = a_with_running_child(&mut rig, &owner).await;
        let b_id = split_helper_key(&b.request.session_key).unwrap().1.to_string();
        let (work, go, _stopped) = gated_work("Rents rose 4%.");
        let (r_id, _) = rig.helpers.start_work(&b.request, "research: rents", work).unwrap();
        b.answer("Research is running.").await;
        until_held(&rig, &b_id).await;
        a.answer("Waiting on the contacts.").await;
        until_held(&rig, &a_id).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(rig.wake.try_recv().is_err(), "nothing reaches the owner while the research runs");
        assert!(rig.helpers.read_output("agent:sdr:web", &a_id).unwrap().contains("waits on work it started"));

        let _ = go.send(());
        let b2 = rig.next_turn().await;
        assert_eq!(b2.request.session_key, b.request.session_key, "B hears the research");
        assert!(matches!(&b2.request.input, TurnInput::Notification(c) if c.task_id == r_id && c.result.contains("Rents rose 4%.")));
        rig.heard(&b2.request.session_key);
        b2.answer("Rents rose 4%; contacts found.").await;
        let a2 = rig.next_turn().await;
        assert_eq!(a2.request.session_key, a.request.session_key, "A hears B");
        rig.heard(&a2.request.session_key);
        a2.answer("Plan ready: rents up 4%.").await;
        assert_eq!(rig.next_wake().await, "agent:sdr:web");
        let p = rig.pending_notifications("agent:sdr:web");
        assert!(p[0].contains("Plan ready: rents up 4%."), "{}", p[0]);
    }

    /// Stopping a helper stops the research it started, and a helper that
    /// fails takes its research with it: nothing is left running with
    /// nowhere to report.
    #[tokio::test]
    async fn stopping_or_failing_a_helper_stops_its_research() {
        let mut rig = Rig::new(FOREGROUND_BUDGET);
        let owner = rig.owner_turn("agent:sdr:web");
        let a_id = task_id_of(&rig.helpers.delegate(&owner, None, &[], &call("Plan.", None)).await.unwrap());
        let a = rig.next_turn().await;
        let (work, _go, stopped) = gated_work("never");
        rig.helpers.start_work(&a.request, "research: rents", work).unwrap();
        a.answer("Research is running.").await;
        until_held(&rig, &a_id).await;
        let said = rig.helpers.stop("agent:sdr:web", &a_id).unwrap();
        assert!(said.contains("and 1 under it"), "{said}");
        let ended = tokio::time::timeout(Duration::from_secs(5), stopped).await.expect("the research stopped");
        assert_ne!(ended, Ok(false), "ended by the stop, not by finishing");

        // A helper whose turn fails: its research stops too.
        let b_id = task_id_of(&rig.helpers.delegate(&owner, None, &[], &call("Price it.", None)).await.unwrap());
        let b = rig.next_turn().await;
        let (work, _go, stopped) = gated_work("never");
        rig.helpers.start_work(&b.request, "research: prices", work).unwrap();
        let _ = b.events.send(StreamEvent::error("provider down")).await;
        let _ = b.events.send(StreamEvent::done()).await;
        let ended = tokio::time::timeout(Duration::from_secs(5), stopped).await.expect("the research stopped with it");
        assert_ne!(ended, Ok(false), "ended by the stop, not by finishing");
        assert!(rig.helpers.read_output("agent:sdr:web", &b_id).is_ok());
    }

    /// The conversation sees everything it started at any depth (helpers,
    /// theirs, research runs) and stops any of it, here a research run three
    /// levels down.
    #[tokio::test]
    async fn the_conversation_lists_and_stops_work_at_any_depth() {
        let mut rig = Rig::new(FOREGROUND_BUDGET);
        let owner = rig.owner_turn("agent:sdr:web");
        let (_a_id, _a, b) = a_with_running_child(&mut rig, &owner).await;
        let b_id = split_helper_key(&b.request.session_key).unwrap().1.to_string();
        let (work, _go, stopped) = gated_work("never");
        let (r_id, _) = rig.helpers.start_work(&b.request, "research: rents", work).unwrap();
        let tree = rig.helpers.list(Some("agent:sdr:web"), true);
        let research = tree.iter().find(|h| h.task_id == r_id).expect("listed from the top");
        assert!(research.running && research.parent_key == b.request.session_key);
        assert_eq!(tree.len(), 3, "A, B and the research");
        assert!(rig.helpers.list(Some("agent:sdr:web"), false).iter().all(|h| h.task_id != r_id), "not a direct child");
        assert!(rig.helpers.read_output("agent:sdr:web", &r_id).unwrap().contains("still running"));

        rig.helpers.stop("agent:sdr:web", &r_id).unwrap();
        let ended = tokio::time::timeout(Duration::from_secs(5), stopped).await.expect("the research stopped");
        assert_ne!(ended, Ok(false), "ended by the stop, not by finishing");
        // Another conversation can't reach it.
        assert!(rig.helpers.stop("agent:ceo:web", &r_id).is_err());
        // Messages still go only to the helpers it started itself.
        let _ = b_id;
    }

    /// A helper's open questions are kept in their own column of its row
    /// and come back after a restart: a plain message to it then leaves its
    /// question open (reminded, still leading its report), as before.
    #[tokio::test]
    async fn a_restart_restores_a_helpers_open_questions_from_its_row() {
        let mut rig = Rig::new(FOREGROUND_BUDGET);
        let owner = rig.owner_turn("agent:sdr:web");
        let id = task_id_of(&rig.helpers.delegate(&owner, None, &[], &call("Plan.", None)).await.unwrap());
        ask(&rig.next_turn().await, "Drafted.", &vendor()).await;
        assert_eq!(rig.next_wake().await, "agent:sdr:web");
        let run = rig.store.engine_get_run(&id).unwrap().unwrap();
        assert!(run.pending_questions.as_deref().is_some_and(|q| q.contains("Which vendor?")), "{:?}", run.pending_questions);
        assert_eq!(run.definition, None, "never the workflow definition");

        let Rig { store, _dir, .. } = rig;
        let mut rig = Rig::on(store, _dir, FOREGROUND_BUDGET);
        rig.helpers.recover();
        let owner = rig.owner_turn("agent:sdr:web");
        rig.helpers.send(&owner, None, &[], &id, "Also cover Denver.", None).await.unwrap();
        assert_eq!(rig.helpers.state().helpers[&id].questions.pending.len(), 1, "restored from its row");
        let again = rig.next_turn().await;
        assert!(rig.notification_rows(&again.request.session_key).iter().any(|r| r.contains(STILL_OPEN)), "reminded");
        rig.heard(&again.request.session_key);
        again.answer("Denver added.").await;
        assert_eq!(rig.next_wake().await, "agent:sdr:web");
        let p = rig.pending_notifications("agent:sdr:web");
        let report = p.iter().find(|x| x.contains("Denver added.")).expect("its report");
        assert!(report.contains(collect::NEEDS_OWNER_DECISION), "{report}");
    }

    /// Stopping a helper that holds its report for work below it sticks:
    /// the work below stops, its reports go up as stopped, and no helper in
    /// between runs again.
    #[tokio::test]
    async fn stopping_a_held_helper_sticks_at_every_depth() {
        let mut rig = Rig::new(FOREGROUND_BUDGET);
        let owner = rig.owner_turn("agent:sdr:web");
        let (a_id, a, b) = a_with_running_child(&mut rig, &owner).await;
        let b_id = split_helper_key(&b.request.session_key).unwrap().1.to_string();
        let (work, _go, stopped) = gated_work("never");
        rig.helpers.start_work(&b.request, "research: rents", work).unwrap();
        b.answer("Research is running.").await;
        until_held(&rig, &b_id).await;
        a.answer("Waiting on the contacts.").await;
        until_held(&rig, &a_id).await;
        let said = rig.helpers.stop("agent:sdr:web", &a_id).unwrap();
        eprintln!("STOP SAID: {said}");
        let _ = tokio::time::timeout(Duration::from_secs(5), stopped).await;
        let resumed = tokio::time::timeout(Duration::from_millis(800), rig.started.recv()).await;
        if let Ok(Some(t)) = &resumed {
            eprintln!("RESURRECTED: {} input={:?}", t.request.session_key, matches!(t.request.input, TurnInput::Notification(_)));
        }
        let wake = tokio::time::timeout(Duration::from_millis(300), rig.wake.recv()).await;
        if wake.is_ok() {
            for p in rig.pending_notifications("agent:sdr:web") { eprintln!("OWNER GOT: {}", &p[..p.len().min(300)]); }
        }
        assert!(resumed.is_err(), "a stopped helper started another turn");
    }

    /// An answer sent to a finished helper settles the question it asked,
    /// not one the turn it starts asks next.
    #[tokio::test]
    async fn an_answer_never_settles_the_next_question() {
        let mut rig = Rig::new(FOREGROUND_BUDGET);
        let owner = rig.owner_turn("agent:sdr:web");
        let id = task_id_of(&rig.helpers.delegate(&owner, None, &[], &call("Plan.", None)).await.unwrap());
        ask(&rig.next_turn().await, "Drafted.", &vendor()).await;
        assert_eq!(rig.next_wake().await, "agent:sdr:web");
        let _ = rig.pending_notifications("agent:sdr:web");
        rig.helpers.send(&owner, None, &[], &id, "", Some(ans("Acme"))).await.unwrap();
        let t = rig.next_turn().await;
        rig.heard(&t.request.session_key);
        let budget = types::OwnerQuestion { question: "What budget?".into(), options: vec!["$5k".into()], ..Default::default() };
        ask(&t, "Acme it is. Need the budget.", &budget).await;
        assert_eq!(rig.next_wake().await, "agent:sdr:web");
        let p = rig.pending_notifications("agent:sdr:web");
        eprintln!("REPORT2: {}", p.join("\n---\n"));
        assert!(p.iter().any(|x| x.contains("Question: What budget?")), "the new question never reached the owner");
    }

    /// A background helper's question, once given to its parent, is named
    /// in its later reports, not given again.
    #[tokio::test]
    async fn a_background_question_is_given_once() {
        let mut rig = Rig::new(FOREGROUND_BUDGET);
        let owner = rig.owner_turn("agent:sdr:web");
        let id = task_id_of(&rig.helpers.delegate(&owner, None, &[], &call("Plan.", None)).await.unwrap());
        ask(&rig.next_turn().await, "Drafted.", &vendor()).await;
        assert_eq!(rig.next_wake().await, "agent:sdr:web");
        let first = rig.pending_notifications("agent:sdr:web");
        assert!(first.iter().any(|x| x.contains("Question: Which vendor?")));
        rig.helpers.send(&owner, None, &[], &id, "Also cover Denver.", None).await.unwrap();
        let t = rig.next_turn().await;
        rig.heard(&t.request.session_key);
        t.answer("Denver added.").await;
        assert_eq!(rig.next_wake().await, "agent:sdr:web");
        let p = rig.pending_notifications("agent:sdr:web");
        let later = p.iter().find(|x| x.contains("Denver added.")).expect("the later report");
        assert_eq!(later.matches("Question: Which vendor?").count(), 0, "the question was given to the parent a second time: {later}");
    }

    /// An answer settles the one question it names, by the id its block
    /// shows; with none named, the newest. An id the helper has no open
    /// question for is refused, and nothing is sent.
    #[tokio::test]
    async fn an_answer_settles_the_question_it_names() {
        let mut rig = Rig::new(FOREGROUND_BUDGET);
        let owner = rig.owner_turn("agent:sdr:web");
        let id = task_id_of(&rig.helpers.delegate(&owner, None, &[], &call("Plan.", None)).await.unwrap());
        let t = rig.next_turn().await;
        rig.heard(&t.request.session_key);
        ask(&t, "Drafted.", &vendor()).await;
        assert_eq!(rig.next_wake().await, "agent:sdr:web");
        rig.helpers.send(&owner, None, &[], &id, "Keep going.", None).await.unwrap();
        let t = rig.next_turn().await;
        rig.heard(&t.request.session_key);
        let budget = types::OwnerQuestion { question: "What budget?".into(), options: vec!["$5k".into()], ..Default::default() };
        ask(&t, "Need the budget too.", &budget).await;
        assert_eq!(rig.next_wake().await, "agent:sdr:web");
        let (vendor_id, budget_id) = {
            let q = questions_of_row(&rig.store, &id);
            assert_eq!(q.pending.len(), 2);
            (q.pending[0].id.clone(), q.pending[1].id.clone())
        };
        let report = rig.helpers.read_output("agent:sdr:web", &id).unwrap();
        assert!(report.contains(&format!("question {vendor_id}")) && report.contains(&format!("`question: \"{budget_id}\"`")), "{report}");

        let refused = rig.helpers.send(&owner, None, &[], &id, "", Some(tools::orchestrator::Answer { question: Some("q-nope".into()), text: "x".into() })).await;
        assert!(refused.is_err_and(|e| e.contains("no open question q-nope")));

        let named = tools::orchestrator::Answer { question: Some(vendor_id.clone()), text: "Acme".into() };
        rig.helpers.send(&owner, None, &[], &id, "", Some(named)).await.unwrap();
        let q = rig.helpers.state().helpers[&id].questions.clone();
        assert_eq!(q.pending.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), [budget_id.as_str()], "only the vendor question settled");
        assert_eq!(q.answered[0].0.id, vendor_id);
        let t = rig.next_turn().await;
        let rows = rig.sessions.get_messages(&rig.sessions.resolve_session_id_by_key(&t.request.session_key).unwrap()).unwrap();
        assert!(rows.iter().any(|m| m.content.contains("The owner's answer to your question \"Which vendor?\": Acme")));
    }

    /// A launch that waits on a helper (foreground) gets its report only
    /// once the work it started is done, the same as one reporting by
    /// notification.
    #[tokio::test]
    async fn a_foreground_helper_holds_while_its_own_helpers_run() {
        let mut rig = Rig::new(FOREGROUND_BUDGET);
        let h = rig.helpers.clone();
        let o2 = rig.owner_turn("agent:sdr:web");
        let fg = tokio::spawn(async move { h.delegate(&o2, None, &[], &call("Plan.", Some(false))).await });
        let a = rig.next_turn().await;
        rig.helpers.delegate(&a.request, None, &[], &call("Find.", Some(true))).await.unwrap();
        let b = rig.next_turn().await;
        a.answer("Waiting on the contacts.").await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!fg.is_finished(), "no report while its helper runs");
        b.answer("40 contacts.").await;
        let a2 = rig.next_turn().await;
        rig.heard(&a2.request.session_key);
        a2.answer("Plan ready with 40 contacts.").await;
        let out = tokio::time::timeout(Duration::from_secs(5), fg).await.expect("it reports").unwrap().unwrap();
        assert!(out.contains("Plan ready with 40 contacts."), "{out}");
    }
}
