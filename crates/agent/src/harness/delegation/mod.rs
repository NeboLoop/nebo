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
}

impl HelperSpec {
    /// A `delegate` call's input: `description` and `prompt` required,
    /// `helper_type` one of the listed types (default general), `background`
    /// default true, `speed` the parent's unless named.
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
        Ok(Self { description, prompt, kind, background, isolation: None, skills: Vec::new(), speed: text("speed") })
    }
}

/// A helper's type. Explore and Plan are enforced tool sets: no writes, no
/// mutating shell, no helper tool. Review is the verify loop's independent
/// reviewer: it only looks too, except that it may run commands (the
/// project's build and tests), fenced to the copy it reviews and its build
/// folder. The verify loop starts it; a model never names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelperKind {
    General,
    Explore,
    Plan,
    Review,
}

impl HelperKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "general" => Some(Self::General),
            "explore" => Some(Self::Explore),
            "plan" => Some(Self::Plan),
            "review" => Some(Self::Review),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Explore => "explore",
            Self::Plan => "plan",
            Self::Review => "review",
        }
    }

    /// Explore, Plan and Review only look.
    fn read_only(self) -> bool {
        matches!(self, Self::Explore | Self::Plan | Self::Review)
    }

    pub const ALL: [HelperKind; 3] = [Self::General, Self::Explore, Self::Plan];

    /// When the type fits, and the tools it has: its line in the helper
    /// types listing, rendered `- type: when to use (Tools: …)` so the model
    /// picks a type by the job in front of it and knows what it can do.
    pub fn when_to_use(self) -> &'static str {
        match self {
            Self::General => "Researching open questions, searching when you aren't sure the first few tries will find \
the match, and carrying out work of several steps. (Tools: all of yours.)",
            Self::Explore => "Wide searches: when answering means going through many files, folders or naming \
patterns and you need only the conclusion, not the file contents. It finds things; it doesn't review or audit \
them. Say how wide to search: \"medium\", or \"very thorough\" for several places and naming conventions. \
(Tools: yours that only look; it changes nothing and starts no helpers.)",
            Self::Plan => "Designing how to carry out a piece of work: it returns the steps, the files that matter \
and the trade-offs. (Tools: yours that only look; it changes nothing and starts no helpers.)",
            Self::Review => "Reviewing a code change independently, through one lens, trying to break it. (Tools: \
yours that only look, and commands in the copy it reviews; it changes nothing and starts no helpers.)",
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
    pub result: String,
    pub usage: ai::UsageInfo,
    /// The untrusted content the helper read: its result carries it to
    /// whoever reads it, as a tool result, a notification row or a wake.
    pub taint: Vec<ProvenanceClass>,
}

/// A helper whose answer is read as data ([`Helpers::answer`]).
#[derive(Debug, Clone)]
pub struct Ask {
    /// Names its session under the parent's (`sa-<node>`).
    pub node: String,
    /// What the owner's screen shows while it runs.
    pub description: String,
    pub kind: HelperKind,
    /// Its whole brief: it sees nothing else.
    pub prompt: String,
    /// The shape its final answer must have.
    pub schema: serde_json::Value,
    /// The only tools it may call; `None`: what its kind allows.
    pub tools: Option<Vec<String>>,
    /// The folders it works in: the first is its working directory; all of
    /// them are its fence. Empty: the parent's.
    pub fence: Vec<String>,
    /// Shown on the owner's screen as one of the parent's helpers.
    pub shown: bool,
}

/// What a helper asked for data came back with.
#[derive(Debug, Clone)]
pub struct Answered {
    pub value: Result<serde_json::Value, String>,
    pub usage: ai::UsageInfo,
    /// Its session, where its steps (and their cost) are kept.
    pub session_key: String,
}

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
    // A reviewer runs the project's build and tests; its fence keeps every
    // command inside the copy it reviews and its build folder.
    let reviewer_command = *kind == HelperKind::Review && target.key == super::owner_command::RUN_COMMAND;
    if kind.read_only() && (delegating || !target.read_only) && !reviewer_command {
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
}

/// A helper this process started.
struct Helper {
    parent_key: String,
    session_key: String,
    description: String,
    kind: HelperKind,
    /// The speed it was started at; `None` = its parent's.
    speed: Option<String>,
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
    /// A completion held back while the helper's own helpers still run.
    held: Option<Completion>,
    /// Reports of earlier turns of this run, when input reached it as a
    /// turn ended; they lead the run's final result.
    earlier_reports: Vec<String>,
    /// The call it is making now, as the owner's screen was sent it.
    call: Option<tools::humanize::Call>,
    /// Background work's latest progress line, as the owner's screen was
    /// sent it.
    activity: String,
    /// When its current run started (unix seconds).
    started_at: i64,
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
    /// The copies isolated helpers work in, by helper id: held from the
    /// helper's start until its changes are landed or discarded, across its
    /// turns (a follow-up works in the same copy), never merged on their own.
    copies: HashMap<String, crate::worktree::Isolation>,
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

    fn running_children(&self, key: &str) -> bool {
        self.helpers.values().any(|h| h.parent_key == key && h.running)
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
            "isolation": isolation.as_ref().map(|_| "worktree"),
            "copy": isolation.as_ref().map(|iso| iso.path().to_string_lossy().into_owned()),
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
                // Never used: nothing in it.
                crate::worktree::put_away(iso, &spec.description).await;
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
            if let Some(iso) = isolation {
                state.copies.insert(task_id.clone(), iso);
            }
            state.helpers.insert(
                task_id.clone(),
                Helper {
                    parent_key: parent_key.to_string(),
                    session_key,
                    description: spec.description.clone(),
                    kind: spec.kind,
                    speed: spec.speed.clone(),
                    parent_seat: parent_seat.clone(),
                    parent_grant: grant.cloned(),
                    parent_platform: turn.delivery.platform.clone(),
                    running: true,
                    cancel,
                    waiter: (!spec.background).then_some(tx),
                    held: None,
                    earlier_reports: Vec::new(),
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
        self.spawn_turn(task_id.clone(), req);
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
                    parent_seat: turn.seat.clone(),
                    parent_grant: None,
                    parent_platform: turn.delivery.platform.clone(),
                    running: true,
                    cancel: cancel.clone(),
                    waiter: None,
                    held: None,
                    earlier_reports: Vec::new(),
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
        let parent = Parent {
            session_key: work_key,
            seat: &seat,
            grant: grant.as_ref(),
            run_taint: &[],
            cancel,
            platform: platform.as_deref(),
        };
        let ask = Ask {
            node: node.to_string(),
            description: node.to_string(),
            kind: HelperKind::General,
            prompt,
            schema,
            tools: Some(tools),
            fence: Vec::new(),
            shown: false,
        };
        self.answer(&parent, ask, activity).await.value
    }

    /// A helper whose final answer is read as data, for the turn or work
    /// `parent`: a turn on the one loop under the parent's seat, stopped
    /// with it, its answer held to `ask.schema` by the turn's end check
    /// (`turn_end::AnswerShapeCheck`). Its session is the parent's
    /// `sa-<node>`. The research pipeline's sub-agents and the verify loop's
    /// reviewers both come through here.
    pub async fn answer(self: &Arc<Self>, parent: &Parent<'_>, ask: Ask, activity: Option<Arc<std::sync::atomic::AtomicU64>>) -> Answered {
        let spec = HelperSpec {
            description: ask.description.clone(),
            prompt: ask.prompt.clone(),
            kind: ask.kind,
            background: false,
            isolation: None,
            skills: Vec::new(),
            speed: None,
        };
        let copy = ask.fence.first().cloned();
        let mut req = child::child_request(parent, &format!("sa-{}", ask.node), &spec, copy.as_deref(), TurnInput::Platform { text: ask.prompt });
        let session_key = req.session_key.clone();
        if ask.fence.len() > 1
            && let Some(types::permissions::Ceiling::Parent { grant }) = req.seat.ceiling.as_mut()
        {
            grant.fence = Some(ask.fence.iter().map(std::path::PathBuf::from).collect());
        }
        if let Some(tools) = ask.tools {
            let allowed: std::collections::HashSet<String> = tools.into_iter().collect();
            req.seat.tool_allowlist = Some(match req.seat.tool_allowlist.take() {
                Some(inherited) => inherited.intersection(&allowed).cloned().collect(),
                None => allowed,
            });
        }
        if let TurnMode::Helper { answer, .. } = &mut req.mode {
            *answer = Some(Arc::new(ask.schema.clone()));
        }
        let task_id = format!("sa-{}", ask.node);
        if ask.shown {
            self.emit(parent.session_key, ai::StreamEvent::subagent_start(&task_id, ask.description.as_str()));
        }
        let waiting = waits_of(&req);
        let cancel = parent.cancel.clone();
        let collected = match self.starter.start_turn(req).await {
            Ok(handle) => {
                collect::collect(handle.events, &cancel, INACTIVITY_LIMIT, &waiting, |_| {
                    if let Some(a) = &activity {
                        a.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                })
                .await
            }
            Err(e) => collect::Collected::failed(e.to_string()),
        };
        let value = if let Some(error) = collected.error.clone() {
            Err(error)
        } else if collected.cancelled {
            Err("stopped".into())
        } else {
            let issues = super::turn_end::answer_issues(&ask.schema, &collected.final_message);
            if issues.is_empty() {
                super::turn_end::answer_object(&collected.final_message).ok_or_else(|| "it gave no answer".to_string())
            } else {
                Err(format!("its answer didn't match the schema: {}", issues.join("; ")))
            }
        };
        if ask.shown {
            self.emit(parent.session_key, ai::StreamEvent::subagent_complete(&task_id, ask.description.as_str(), value.is_ok()));
        }
        Answered { value, usage: collected.usage, session_key }
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
    ) -> Result<String, String> {
        let caller = turn.session_key.as_str();
        let row = self.own_row(caller, task_id)?;
        if spec_kind_of_row(&self.store, &row).as_deref() == Some(WORK_KIND) {
            return Err(format!(
                "{task_id} is a research run, not a helper you can talk to: it takes no messages. \
                 Its report comes as a notification; for a follow-up, start a new one."
            ));
        }
        let spec = spec_of_row(&self.store, &row);
        if spec.kind == HelperKind::Review {
            return Err(format!(
                "{task_id} was an independent reviewer: a reviewer is always fresh and never continued. Its findings \
                 came back with its review."
            ));
        }
        // An isolated helper continues in its own copy. One whose copy is
        // gone (Nebo restarted, and its work was put away) gets a fresh one:
        // a follow-up never writes into the owner's own files.
        let needs_copy = spec.isolation.is_some() && !self.state().copies.contains_key(task_id);
        if needs_copy {
            let iso = isolate(&turn.seat, grant, task_id).await?;
            self.state().copies.insert(task_id.to_string(), iso);
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
            if state.helpers.get(task_id).is_some_and(|h| h.running) {
                deliver(&self.sessions)?;
                info!(task_id = %task_id, "message delivered into a running helper");
                return Ok(format!(
                    "Helper {task_id} has your message and hears it at its next step. Its report \
                     comes as a notification."
                ));
            }
            deliver(&self.sessions)?;
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
            let copy = state.copies.get(task_id).map(|iso| iso.path().to_string_lossy().into_owned());
            let req = child::child_request(&parent, task_id, &spec, copy.as_deref(), TurnInput::None);
            let helper = state.helpers.entry(task_id.to_string()).or_insert_with(|| Helper {
                parent_key: caller.to_string(),
                session_key: row.session_key.clone(),
                description: spec.description.clone(),
                kind: spec.kind,
                speed: spec.speed.clone(),
                parent_seat: turn.seat.clone(),
                parent_grant: grant.cloned(),
                parent_platform: turn.delivery.platform.clone(),
                running: false,
                cancel: cancel.clone(),
                waiter: None,
                held: None,
                earlier_reports: Vec::new(),
                call: None,
                activity: String::new(),
                started_at: chrono::Utc::now().timestamp(),
            });
            helper.running = true;
            helper.started_at = chrono::Utc::now().timestamp();
            helper.cancel = cancel;
            helper.parent_seat = turn.seat.clone();
            helper.parent_grant = grant.cloned();
            helper.parent_platform = turn.delivery.platform.clone();
            helper.held = None;
            req
        };
        let _ = self.store.update_task_running(task_id);
        self.spawn_turn(task_id.to_string(), resume);
        Ok(launch_result(task_id))
    }

    /// `read_output` for one of the caller's helpers.
    pub fn read_output(&self, caller: &str, task_id: &str) -> Result<String, String> {
        let row = self.own_row(caller, task_id)?;
        let description = row.description.clone().unwrap_or_default();
        if self.state().helpers.get(task_id).is_some_and(|h| h.running) {
            return Ok(format!("Helper {task_id} \"{description}\" is still running."));
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

    /// `stop_task` for one of the caller's helpers.
    pub fn stop(&self, caller: &str, task_id: &str) -> Result<String, String> {
        self.own_row(caller, task_id)?;
        let state = self.state();
        match state.helpers.get(task_id) {
            Some(h) if h.running => {
                h.cancel.cancel();
                Ok(format!("Stopping helper {task_id}."))
            }
            _ => Ok(format!("Helper {task_id} is not running.")),
        }
    }

    /// Helpers a restart interrupted: each one fails, and an owner session
    /// that started one is told as it is of any failed helper. A nested
    /// helper's parent was interrupted too and reports for its own work.
    pub async fn recover(self: &Arc<Self>) {
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
            let mut error = "interrupted by a restart; send_message continues it from where it stopped".to_string();
            if let Some(kept) = put_away_after_restart(&run.id).await {
                error.push_str(&format!("; {kept}, and a follow-up works in a fresh copy"));
            }
            if let Err(e) = self.store.engine_set_run_state(&run.id, "failed", chrono::Utc::now().timestamp(), Some(&error)) {
                warn!(task_id = %run.id, error = %e, "an interrupted helper could not be marked failed");
                continue;
            }
            let Some(parent_key) = split_helper_key(&run.session_key).filter(|(_, id)| *id == run.id).map(|(p, _)| p) else {
                continue;
            };
            if depth_of(parent_key) > 0 {
                continue;
            }
            let description = self
                .store
                .get_pending_task(&run.id)
                .ok()
                .flatten()
                .and_then(|t| t.description)
                .unwrap_or_default();
            info!(task_id = %run.id, parent = %parent_key, "a helper was interrupted by a restart");
            self.deliver(
                parent_key,
                Completion {
                    task_id: run.id.clone(),
                    description,
                    status: CompletionStatus::Failed { error: error.clone() },
                    result: String::new(),
                    usage: ai::UsageInfo::default(),
                    taint: Vec::new(),
                },
                0,
            );
        }
    }

    /// The caller's own helpers still in hand: running, or finished and
    /// waiting on helpers of their own; every session's with `None`.
    /// Running first.
    pub fn list(&self, caller: Option<&str>) -> Vec<HelperStatus> {
        let mut out: Vec<HelperStatus> = self
            .state()
            .helpers
            .iter()
            .filter(|(_, h)| caller.is_none_or(|c| h.parent_key == c))
            .map(|(id, h)| HelperStatus {
                task_id: id.clone(),
                description: h.description.clone(),
                running: h.running,
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
    fn own_row(&self, caller: &str, task_id: &str) -> Result<db::models::PendingTask, String> {
        let not_yours = || format!("No helper {task_id} was started from this conversation.");
        let row = self
            .store
            .get_pending_task(task_id)
            .map_err(|e| format!("Could not read helper {task_id}: {e}"))?
            .ok_or_else(not_yours)?;
        if row.task_type != ROW_KIND || row.session_key != helper_key(caller, task_id) {
            return Err(not_yours());
        }
        Ok(row)
    }

    /// Run one turn of helper `task_id` in the background and finish it.
    /// A turn that crashes still ends the helper as failed and tells its
    /// parent: no helper ends without a word.
    fn spawn_turn(self: &Arc<Self>, task_id: String, req: TurnRequest) {
        let this = Arc::clone(self);
        tokio::spawn(async move {
            let mut next = Some(req);
            while let Some(req) = next.take() {
                let turn = std::panic::AssertUnwindSafe(this.run_turn(&task_id, req));
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
            usage: ai::UsageInfo::default(),
            taint: Vec::new(),
        }
    }

    async fn run_turn(self: &Arc<Self>, task_id: &str, req: TurnRequest) -> Completion {
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
        let spill_dir = self
            .sessions
            .resolve_session_id_by_key(&parent_key)
            .map(|id| tools::result_shape::results_dir(&id))
            .unwrap_or_else(|_| tools::result_shape::results_dir(&parent_key.replace(':', "_")));
        let mut completion = collected.into_completion(task_id, &description, &spill_dir);
        self.report_copy(task_id, &description, &mut completion).await;
        match &completion.status {
            CompletionStatus::Done | CompletionStatus::Partial { .. } => {
                let _ = self.store.update_task_completed(task_id, Some(&completion.result));
            }
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

    /// After an isolated helper's turn: its changes are held in its copy
    /// for the primary (or the owner) to land or discard, never merged on
    /// their own. A stopped helper's work is put away (on its branch, or in
    /// its copy) and the copy freed; a copy with nothing in it is removed.
    async fn report_copy(&self, task_id: &str, description: &str, completion: &mut Completion) {
        let Some(iso) = self.state().copies.get(task_id).cloned() else { return };
        if completion.status == CompletionStatus::Stopped {
            self.state().copies.remove(task_id);
            if let Some(where_) = crate::worktree::put_away(&iso, &format!("Unfinished: {description}")).await {
                completion.result.push_str(&format!("\n\nIt was stopped before its changes landed: {where_}. Nothing of the owner's changed."));
            }
            return;
        }
        let files = crate::worktree::changed_files(&iso).await;
        if files.is_empty() {
            self.state().copies.remove(task_id);
            crate::worktree::put_away(&iso, description).await;
            return;
        }
        completion.result.push_str(&format!("\n\n{}", changes_ready(task_id, &iso, &files)));
    }

    /// Land or discard the changes an isolated helper of `caller`'s holds
    /// in its copy: the primary's (or the owner's) decision, the one way an
    /// isolated helper's work reaches the owner's files. Landing catches the
    /// copy up with the project, re-tests it with `retest`, and lands it as
    /// one commit ([`crate::worktree::merge_all`]); anything short of that
    /// keeps the copy and says what is needed. A land asked for after a
    /// conflict was handed back counts as the word that it is resolved.
    pub async fn settle(&self, caller: &str, task_id: &str, decision: Decision, retest: Option<crate::worktree::Retest<'_>>) -> Result<String, String> {
        let row = self.own_row(caller, task_id)?;
        if self.state().helpers.get(task_id).is_some_and(|h| h.running) {
            return Err(format!("Helper {task_id} is still working; its changes are settled once it reports."));
        }
        let Some(mut iso) = self.state().copies.remove(task_id) else {
            return Err(format!("Helper {task_id} holds no changes to land or discard."));
        };
        let description = row.description.clone().unwrap_or_default();
        match decision {
            Decision::Discard => match crate::worktree::discard(&iso).await {
                Ok(()) => Ok(format!("Helper {task_id}'s changes were discarded. Nothing of the owner's changed.")),
                Err(e) => {
                    self.state().copies.insert(task_id.to_string(), iso);
                    Err(format!("Could not discard helper {task_id}'s changes: {e}"))
                }
            },
            Decision::Land => {
                iso.mark_resolved();
                let message = landing_message(&description);
                let mut isos = [iso];
                let outcome = crate::worktree::merge_all(&mut isos, &message, retest).await.remove(0).1;
                let [iso] = isos;
                let text = crate::worktree::render_outcome(&description, &outcome);
                if outcome.landed() {
                    return Ok(text);
                }
                self.state().copies.insert(task_id.to_string(), iso);
                Ok(format!(
                    "{text}\nThe changes stay in helper {task_id}'s copy. Send them back to the helper to fix \
                     (send_message to {task_id}), ask the owner, or discard them."
                ))
            }
        }
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
    fn finish(self: &Arc<Self>, task_id: &str, mut completion: Completion) -> Option<TurnRequest> {
        let (parent_key, completion, depth) = {
            let mut guard = self.state();
            let state = &mut *guard;
            let copy = state.copies.get(task_id).map(|iso| iso.path().to_string_lossy().into_owned());
            let h = state.helpers.get_mut(task_id)?;
            let continues = matches!(completion.status, CompletionStatus::Done | CompletionStatus::Partial { .. })
                && !h.cancel.is_cancelled()
                && self
                    .sessions
                    .resolve_session_id_by_key(&h.session_key)
                    .and_then(|id| self.store.get_chat_messages(&self.sessions.active_chat_id(&id)))
                    .is_ok_and(|messages| input_unheard(&messages));
            if continues {
                info!(task_id = %task_id, "input reached the helper as its turn ended: running a turn to hear it");
                h.earlier_reports.push(completion.result);
                let spec = HelperSpec {
                    description: h.description.clone(),
                    prompt: String::new(),
                    kind: h.kind,
                    background: true,
                    isolation: None,
                    skills: Vec::new(),
                    speed: h.speed.clone(),
                };
                let parent = Parent {
                    session_key: &h.parent_key,
                    seat: &h.parent_seat,
                    grant: h.parent_grant.as_ref(),
                    run_taint: &[],
                    cancel: h.cancel.clone(),
                    platform: h.parent_platform.as_deref(),
                };
                return Some(child::child_request(&parent, task_id, &spec, copy.as_deref(), TurnInput::None));
            }
            h.running = false;
            if !h.earlier_reports.is_empty() {
                h.earlier_reports.push(std::mem::take(&mut completion.result));
                completion.result = std::mem::take(&mut h.earlier_reports).join("\n\n");
            }
            let waiter = h.waiter.take();
            let (session_key, parent_key, depth) = (h.session_key.clone(), h.parent_key.clone(), h.parent_seat.handoff_depth);
            let completion = match waiter {
                Some(waiter) => match waiter.send(completion) {
                    Ok(()) => {
                        if !state.running_children(&session_key) {
                            state.forget(task_id);
                        }
                        return None;
                    }
                    Err(c) => c,
                },
                None => completion,
            };
            if completion.status != CompletionStatus::Stopped && state.running_children(&session_key) {
                if let Some(h) = state.helpers.get_mut(task_id) {
                    h.held = Some(completion);
                }
                return None;
            }
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
        let mut release: Option<(String, Completion, u8)> = None;
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
                    if running || !wakes {
                        self.append_notification(parent_key, &text, &c.taint);
                        if !running && !state.running_children(parent_key)
                            && let Some(p) = state.helpers.get_mut(&pid)
                            && let Some(held) = p.held.take()
                        {
                            release = Some((p.parent_key.clone(), held, p.parent_seat.handoff_depth));
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
            let _ = self.store.update_task_running(&pid);
            self.spawn_turn(pid, req);
        }
        if let Some((grandparent, held, depth)) = release {
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
            if running {
                return true;
            }
            State::resume(&mut state, &pid, TurnInput::None).map(|req| (pid, req))
        };
        if let Some((pid, req)) = resume {
            let _ = self.store.update_task_running(&pid);
            self.spawn_turn(pid, req);
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
        };
        let parent = Parent {
            session_key: &p.parent_key,
            seat: &p.parent_seat,
            grant: p.parent_grant.as_ref(),
            run_taint: &[],
            cancel: cancel.clone(),
            platform: p.parent_platform.as_deref(),
        };
        let copy = self.copies.get(pid).map(|iso| iso.path().to_string_lossy().into_owned());
        let req = child::child_request(&parent, pid, &spec, copy.as_deref(), input);
        let p = self.helpers.get_mut(pid)?;
        p.running = true;
        p.cancel = cancel;
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

/// What the primary (or the owner) decided about an isolated helper's
/// changes ([`Helpers::settle`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Land,
    Discard,
}

/// What a helper's report says about the changes it holds in its copy.
fn changes_ready(task_id: &str, iso: &crate::worktree::Isolation, files: &[String]) -> String {
    let shown: Vec<&str> = files.iter().take(12).map(String::as_str).collect();
    let more = files.len().saturating_sub(shown.len());
    format!(
        "Its changes are ready in its own copy at {} ({} file{}: {}{}), not in the owner's files yet. Land them with \
         send_message(to: \"{task_id}\", changes: \"land\"), or drop them with changes: \"discard\". A follow-up to the \
         helper works in the same copy.",
        iso.path().display(),
        files.len(),
        if files.len() == 1 { "" } else { "s" },
        shown.join(", "),
        if more > 0 { format!(" and {more} more") } else { String::new() }
    )
}

/// The one commit a helper's work lands as: its description, as a sentence.
fn landing_message(description: &str) -> String {
    let d = description.trim();
    if d.is_empty() {
        return "Changes made by a helper".to_string();
    }
    let mut chars = d.chars();
    let first = chars.next().map(|c| c.to_uppercase().collect::<String>()).unwrap_or_default();
    format!("{first}{}", chars.as_str())
}

/// An interrupted helper's copy after a restart: a worktree's work is
/// committed on its task branch and the worktree removed (nothing lost,
/// the disk freed); a scratch copy is kept where it is. Returns where its
/// work is, if it had a copy.
async fn put_away_after_restart(task_id: &str) -> Option<String> {
    let copies = config::data_dir().ok()?.join("worktrees");
    let path = std::fs::read_dir(&copies).ok()?.flatten().map(|project| project.path().join(task_id)).find(|p| p.is_dir())?;
    if !path.join(".git").exists() {
        return Some(format!("its work is kept in its copy at {}", path.display()));
    }
    let common = crate::worktree::git(&path, &["rev-parse", "--path-format=absolute", "--git-common-dir"]).await.ok()?;
    let root = std::path::PathBuf::from(common.trim()).parent()?.to_path_buf();
    let branch = format!("nebo/{task_id}");
    let wt = crate::worktree::Worktree {
        task_id: task_id_of(&path),
        root: root.clone(),
        path: path.clone(),
        branch: branch.clone(),
        start_branch: String::new(),
        base: String::new(),
        onto: None,
        resolved: false,
    };
    Some(match crate::worktree::shelve(&wt, "Unfinished work, put away when Nebo restarted").await {
        Ok(()) => format!("its work is kept on branch {branch} in {}", root.display()),
        Err(e) => {
            warn!(task_id, error = %e, "an interrupted helper's copy could not be put away; it is kept");
            format!("its work is kept in its copy at {}", path.display())
        }
    })
}

fn task_id_of(path: &std::path::Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
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
        isolation: (field("isolation").as_deref() == Some("worktree")).then_some(Isolation::Worktree),
        skills: Vec::new(),
        speed: field("speed"),
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
        assert!(!rig.helpers.list(None).iter().any(|h| h.task_id == id && h.running), "it is not left running");
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

        // Then the parent notifies its own parent, once.
        rig.heard(&parent_key);
        resumed.answer("Close plan ready.").await;
        assert_eq!(rig.next_wake().await, "agent:bookkeeper:web");
        let pending = rig.pending_notifications("agent:bookkeeper:web");
        assert_eq!(pending.len(), 1);
        assert!(pending[0].contains("done\nClose plan ready.\n"), "{}", pending[0]);
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
        assert!(rig.helpers.list(Some(key)).iter().any(|h| h.task_id == id && h.running));

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

        let refused = rig.helpers.send(&owner, None, &[], &id, "also cover Tucson").await.unwrap_err();
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

        let list = rig.helpers.list(Some("agent:bookkeeper:web"));
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
            }]
        );
        // Every session's helpers, for the owner's list of background work.
        assert!(rig.helpers.list(None).iter().any(|h| h.task_id == id));
        assert!(rig.helpers.list(Some("agent:ceo:web")).is_empty());
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
        assert!(!rig.helpers.list(Some(key)).iter().any(|h| h.running));
        assert!(rig.wake.try_recv().is_err(), "a stopped helper starts no turn");
    }

    #[tokio::test]
    async fn a_running_helper_hears_a_message_and_one_landing_late_gets_a_turn() {
        let mut rig = Rig::new(FOREGROUND_BUDGET);
        let owner = rig.owner_turn("agent:bookkeeper:web");
        let id = task_id_of(&rig.helpers.delegate(&owner, None, &[], &call("Sum Q1.", None)).await.unwrap());
        let child = rig.next_turn().await;
        let text = rig.helpers.send(&owner, None, &[], &id, "Include April too.").await.unwrap();
        assert!(text.contains("hears it at its next step"), "{text}");
        let child_id = rig.sessions.resolve_session_id_by_key(&child.request.session_key).unwrap();
        let row = rig.sessions.get_messages(&child_id).unwrap().pop().unwrap();
        assert_eq!(row.content, "Include April too.");
        assert!(matches!(crate::harness::conversation::arrived_mid_turn(&row), Some(crate::harness::conversation::MidTurnFrom::Parent { .. })));

        // The turn ended before a step read it: one more turn hears it, and
        // the parent gets both reports.
        child.answer("Q1 is 12,000.").await;
        let again = rig.next_turn().await;
        assert_eq!(again.request.session_key, child.request.session_key);
        assert!(matches!(again.request.input, TurnInput::None));
        rig.heard(&again.request.session_key);
        again.answer("With April, 16,100.").await;
        rig.next_wake().await;
        let pending = rig.pending_notifications("agent:bookkeeper:web");
        assert!(pending[0].contains("done\nQ1 is 12,000.\n\nWith April, 16,100.\n"), "{}", pending[0]);
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
        let text = rig.helpers.send(&owner, None, &[], &id, "Which job threw them?").await.unwrap();
        assert_eq!(text, launch_result(&id));
        let resumed = rig.next_turn().await;
        assert_eq!(resumed.request.session_key, helper_key("agent:bookkeeper:web", &id));
        assert!(matches!(resumed.request.mode, TurnMode::Helper { kind: HelperKind::Explore, depth: 1, .. }));
        assert!(matches!(resumed.request.input, TurnInput::None), "the message is already in its thread");

        // Only the helper's own parent can resume it.
        let other = rig.owner_turn("agent:ceo:web");
        assert!(rig.helpers.send(&other, None, &[], &id, "hi").await.is_err());
    }

    fn isolated_rig_test(test: impl FnOnce(std::path::PathBuf) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()>>>) {
        crate::test_home::with_home(|_| {
            let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
            let repo = tempfile::tempdir().unwrap();
            rt.block_on(async {
                for args in [vec!["init", "-q", "-b", "main"], vec!["config", "user.email", "t@t"], vec!["config", "user.name", "t"]] {
                    crate::worktree::git(repo.path(), &args).await.unwrap();
                }
                std::fs::write(repo.path().join("a.txt"), "base\n").unwrap();
                crate::worktree::git(repo.path(), &["add", "-A"]).await.unwrap();
                crate::worktree::git(repo.path(), &["commit", "-q", "-m", "base"]).await.unwrap();
                test(repo.path().to_path_buf()).await;
            });
        })
    }

    fn isolated_spec(description: &str) -> HelperSpec {
        HelperSpec {
            description: description.into(),
            prompt: "Add b.".into(),
            kind: HelperKind::General,
            background: true,
            isolation: Some(Isolation::Worktree),
            skills: Vec::new(),
            speed: None,
        }
    }

    /// An isolated helper's changes are held in its copy, never merged on
    /// their own; a follow-up continues in the same copy; the primary lands
    /// them (caught up and re-tested in the copy) or discards them.
    #[test]
    fn an_isolated_helper_holds_its_changes_and_a_follow_up_works_in_the_same_copy() {
        isolated_rig_test(|repo| {
            Box::pin(async move {
                let mut rig = Rig::new(FOREGROUND_BUDGET);
                let key = "agent:bookkeeper:web";
                let mut owner = rig.owner_turn(key);
                owner.seat.cwd = Some(repo.to_string_lossy().into_owned());
                let Launch::Background { task_id: id } = rig.helpers.launch(&owner, None, &[], isolated_spec("add b")).await.unwrap() else {
                    panic!("background")
                };
                let child = rig.next_turn().await;
                let copy = std::path::PathBuf::from(child.request.seat.cwd.clone().unwrap());
                assert_ne!(copy, repo, "it works in its own copy");
                std::fs::write(copy.join("b.txt"), "from the helper\n").unwrap();
                rig.heard(&child.request.session_key);
                child.answer("Added b.").await;
                rig.next_wake().await;
                let report = rig.helpers.read_output(key, &id).unwrap();
                assert!(report.contains("changes are ready in its own copy") && report.contains("b.txt"), "{report}");
                assert!(!repo.join("b.txt").exists(), "nothing reached the owner's files on its own");

                // A follow-up works in the same copy, with what it already did.
                rig.helpers.send(&owner, None, &[], &id, "Also add c.").await.unwrap();
                let resumed = rig.next_turn().await;
                assert_eq!(resumed.request.seat.cwd.as_deref(), Some(copy.to_string_lossy().as_ref()), "the same copy");
                assert!(copy.join("b.txt").exists());
                std::fs::write(copy.join("c.txt"), "c\n").unwrap();
                rig.heard(&resumed.request.session_key);
                resumed.answer("Added c.").await;
                rig.next_wake().await;

                let retested = std::sync::atomic::AtomicBool::new(false);
                let retest = |_: &crate::worktree::Isolation| -> futures::future::BoxFuture<'static, Result<(), String>> {
                    retested.store(true, std::sync::atomic::Ordering::SeqCst);
                    Box::pin(async { Ok(()) })
                };
                let landed = rig.helpers.settle(key, &id, Decision::Land, Some(&retest)).await.unwrap();
                assert!(landed.contains("landed as one commit"), "{landed}");
                assert!(retested.load(std::sync::atomic::Ordering::SeqCst), "re-tested before it landed");
                assert_eq!(std::fs::read_to_string(repo.join("b.txt")).unwrap(), "from the helper\n");
                assert!(repo.join("c.txt").exists());
                assert!(!copy.exists(), "the copy is gone once it landed");
                assert!(rig.helpers.settle(key, &id, Decision::Land, None).await.is_err(), "nothing left to land");
            })
        })
    }

    #[test]
    fn a_discarded_helpers_changes_never_reach_the_owner_and_its_copy_is_freed() {
        isolated_rig_test(|repo| {
            Box::pin(async move {
                let mut rig = Rig::new(FOREGROUND_BUDGET);
                let key = "agent:bookkeeper:web";
                let mut owner = rig.owner_turn(key);
                owner.seat.cwd = Some(repo.to_string_lossy().into_owned());
                let Launch::Background { task_id: id } = rig.helpers.launch(&owner, None, &[], isolated_spec("add b")).await.unwrap() else {
                    panic!("background")
                };
                let child = rig.next_turn().await;
                let copy = std::path::PathBuf::from(child.request.seat.cwd.clone().unwrap());
                std::fs::write(copy.join("b.txt"), "x\n").unwrap();
                rig.heard(&child.request.session_key);
                child.answer("Added b.").await;
                rig.next_wake().await;
                assert!(rig.helpers.settle("agent:ceo:web", &id, Decision::Discard, None).await.is_err(), "only its own parent decides");
                rig.helpers.settle(key, &id, Decision::Discard, None).await.unwrap();
                assert!(!copy.exists());
                assert!(!repo.join("b.txt").exists());
            })
        })
    }

    #[test]
    fn a_stopped_isolated_helpers_work_is_put_away_never_landed() {
        isolated_rig_test(|repo| {
            Box::pin(async move {
                let mut rig = Rig::new(FOREGROUND_BUDGET);
                let key = "agent:bookkeeper:web";
                let mut owner = rig.owner_turn(key);
                owner.seat.cwd = Some(repo.to_string_lossy().into_owned());
                let Launch::Background { task_id: id } = rig.helpers.launch(&owner, None, &[], isolated_spec("add b")).await.unwrap() else {
                    panic!("background")
                };
                let child = rig.next_turn().await;
                let copy = std::path::PathBuf::from(child.request.seat.cwd.clone().unwrap());
                std::fs::write(copy.join("b.txt"), "half\n").unwrap();
                rig.helpers.stop(key, &id).unwrap();
                tokio::time::timeout(Duration::from_secs(5), async {
                    while copy.exists() {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                })
                .await
                .expect("the copy is freed");
                assert!(!repo.join("b.txt").exists(), "a stopped helper's work never lands");
                let kept = crate::worktree::git(&repo, &["show", &format!("nebo/{id}:b.txt")]).await.unwrap();
                assert_eq!(kept, "half", "its work is kept on its branch");
            })
        })
    }

    #[test]
    fn a_follow_up_after_a_restart_never_writes_into_the_owners_files() {
        isolated_rig_test(|repo| {
            Box::pin(async move {
                let mut rig = Rig::new(FOREGROUND_BUDGET);
                let key = "agent:bookkeeper:web";
                let mut owner = rig.owner_turn(key);
                owner.seat.cwd = Some(repo.to_string_lossy().into_owned());
                let Launch::Background { task_id: id } = rig.helpers.launch(&owner, None, &[], isolated_spec("add b")).await.unwrap() else {
                    panic!("background")
                };
                let child = rig.next_turn().await;
                rig.heard(&child.request.session_key);
                child.answer("Looked around.").await;
                rig.next_wake().await;

                // A restart: the registry knows the helper only by its row.
                let Rig { store, _dir, .. } = rig;
                let mut rig = Rig::on(store, _dir, FOREGROUND_BUDGET);
                let mut owner = rig.owner_turn(key);
                owner.seat.cwd = Some(repo.to_string_lossy().into_owned());
                rig.helpers.send(&owner, None, &[], &id, "Now add b.").await.unwrap();
                let resumed = rig.next_turn().await;
                let cwd = std::path::PathBuf::from(resumed.request.seat.cwd.clone().unwrap());
                assert_ne!(cwd, repo, "a fresh copy, never the owner's folder");
                assert!(cwd.join("a.txt").exists());
            })
        })
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

    /// A reviewer only looks, except that it runs the project's build and
    /// tests; it never writes a file or starts a helper.
    #[test]
    fn a_reviewer_runs_commands_but_changes_nothing() {
        let mode = helper_mode(HelperKind::Review, 1);
        assert!(permits(&mode, &target("run_command", false)).is_ok(), "the build and tests");
        assert!(permits(&mode, &target("read_file", true)).is_ok());
        assert!(permits(&mode, &target("write_file", false)).is_err());
        assert!(permits(&mode, &target("edit_file", false)).is_err());
        assert!(permits(&mode, &target("delegate", false)).is_err());
        assert!(!on_surface(&mode, "delegate"));
        assert!(!HelperKind::ALL.contains(&HelperKind::Review), "never offered to a model");
    }

    /// The verify loop's reviewer: a helper of the turn on the one loop,
    /// fenced to the copy it reviews and its build folder, its answer held
    /// to the schema, shown on the owner's screen while it runs.
    #[tokio::test]
    async fn a_reviewer_answers_as_data_fenced_to_its_copy_and_shown_while_it_runs() {
        let mut rig = Rig::new(FOREGROUND_BUDGET);
        let key = "agent:dev:web";
        let owner = rig.owner_turn(key);
        let grant = child::tests::parent_grant(types::permissions::Mode::Automatic);
        let helpers = rig.helpers.clone();
        let seat = owner.seat.clone();
        let cancel = owner.cancel.clone();
        let task = tokio::spawn(async move {
            let parent = Parent { session_key: key, seat: &seat, grant: Some(&grant), run_taint: &[], cancel, platform: None };
            let ask = Ask {
                node: "verify-v1-r1-security".into(),
                description: "Reviewing: security".into(),
                kind: HelperKind::Review,
                prompt: "Refute it.".into(),
                schema: serde_json::json!({"type": "object", "required": ["findings"], "properties": {"findings": {"type": "array"}}}),
                tools: None,
                fence: vec!["/copy".into(), "/build".into()],
                shown: true,
            };
            helpers.answer(&parent, ask, None).await
        });
        let started = rig.next_turn().await;
        assert_eq!(started.request.session_key, "subagent:agent:dev:web:sa-verify-v1-r1-security");
        assert_eq!(started.request.seat.cwd.as_deref(), Some("/copy"));
        match started.request.seat.ceiling.as_ref() {
            Some(types::permissions::Ceiling::Parent { grant }) => {
                assert_eq!(grant.fence, Some(vec!["/copy".into(), "/build".into()]), "fenced to its copy and build folder")
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(started.request.mode, TurnMode::Helper { kind: HelperKind::Review, answer: Some(_), .. }));
        started.answer(r#"{"findings": []}"#).await;
        let answered = tokio::time::timeout(Duration::from_secs(5), task).await.unwrap().unwrap();
        assert_eq!(answered.value.unwrap(), serde_json::json!({"findings": []}));
        let shown = rig.ui.recv().await.unwrap();
        assert_eq!(shown.parent_session_key, key);
        assert_eq!(shown.event.event_type, ai::StreamEventType::SubagentStart);
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
}
