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
/// from it is refused as a backstop.
pub const MAX_DEPTH: u8 = 3;

/// How long a foreground helper holds its parent's step. Past it the helper
/// moves to the background: it keeps running and reports by notification.
pub const FOREGROUND_BUDGET: Duration = Duration::from_secs(120);

/// A helper that emits nothing for this long is ended, and reports what it
/// had as partial.
pub const INACTIVITY_LIMIT: Duration = Duration::from_secs(10 * 60);

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
}

impl HelperSpec {
    /// A `delegate` call's input: `description` and `prompt` required,
    /// `helper_type` one of the listed types (default general), `background`
    /// default true.
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
        Ok(Self { description, prompt, kind, background, isolation: None, skills: Vec::new() })
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

/// The parent session and task id of a helper's session key, `None` for
/// any other session. A task id never holds a colon.
pub fn split_helper_key(key: &str) -> Option<(&str, &str)> {
    key.strip_prefix("subagent:")?.rsplit_once(':')
}

/// The session key of helper `task_id` of `parent_key`.
pub fn helper_key(parent_key: &str, task_id: &str) -> String {
    format!("subagent:{parent_key}:{task_id}")
}

/// Whether the helper tool `tool_name` is on the surface of a turn in
/// `mode`: never for Explore or Plan helpers, never at the depth cap.
pub fn on_surface(mode: &TurnMode, tool_name: &str) -> bool {
    match mode {
        TurnMode::Helper { kind, depth, .. } if tool_name == HELPER_TOOL => {
            !kind.read_only() && *depth < MAX_DEPTH
        }
        _ => true,
    }
}

/// Whether a turn in `mode` may make the call `target`. A helper's kind is
/// an enforced tool set, and a helper at the depth cap cannot delegate,
/// whatever tool shape the call arrives in.
pub fn permits(mode: &TurnMode, target: &types::permissions::Target) -> Result<(), String> {
    let TurnMode::Helper { kind, depth, .. } = mode else {
        return Ok(());
    };
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
}

/// A helper this process started.
struct Helper {
    parent_key: String,
    session_key: String,
    description: String,
    kind: HelperKind,
    /// The seat the helper was built from (its parent's), kept so a
    /// notification turn is built by the same constructor.
    parent_seat: SeatRequest,
    /// The parent's grant, the helper's ceiling.
    parent_grant: Option<types::permissions::Grant>,
    running: bool,
    cancel: CancellationToken,
    /// A foreground launch waiting on this helper's first completion.
    waiter: Option<oneshot::Sender<Completion>>,
    /// A completion held back while the helper's own helpers still run.
    held: Option<Completion>,
    /// Reports of earlier turns of this run, when input reached it as a
    /// turn ended; they lead the run's final result.
    earlier_reports: Vec<String>,
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
}

impl State {
    fn session_token(&mut self, key: &str) -> CancellationToken {
        self.stops.entry(key.to_string()).or_default().clone()
    }

    fn running_children(&self, key: &str) -> bool {
        self.helpers.values().any(|h| h.parent_key == key && h.running)
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

/// The helper registry.
pub struct Helpers {
    store: Arc<db::Store>,
    sessions: Arc<SessionManager>,
    registry: Arc<tools::Registry>,
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
        registry: Arc<tools::Registry>,
        starter: Arc<dyn TurnStarter>,
        wake: Option<mpsc::UnboundedSender<String>>,
        ui: Option<mpsc::UnboundedSender<HelperEvent>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            sessions,
            registry,
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
            let parent = Parent { session_key: parent_key, seat: parent_seat, grant, run_taint, cancel: cancel.clone() };
            let req = child::child_request(&parent, &task_id, &spec, copy.as_deref(), TurnInput::Platform { text: brief });
            state.helpers.insert(
                task_id.clone(),
                Helper {
                    parent_key: parent_key.to_string(),
                    session_key,
                    description: spec.description.clone(),
                    kind: spec.kind,
                    parent_seat: parent_seat.clone(),
                    parent_grant: grant.cloned(),
                    running: true,
                    cancel,
                    waiter: (!spec.background).then_some(tx),
                    held: None,
                    earlier_reports: Vec::new(),
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
                    parent_seat: turn.seat.clone(),
                    parent_grant: None,
                    running: true,
                    cancel: cancel.clone(),
                    waiter: None,
                    held: None,
                    earlier_reports: Vec::new(),
                },
            );
            cancel
        };
        info!(task_id = %task_id, parent = %parent_key, "background work launched");
        let this = Arc::clone(self);
        let (id, description) = (task_id.clone(), description.to_string());
        tokio::spawn(async move {
            let completion = this.run_work(&id, &parent_key, &description, cancel, work).await;
            this.finish(&id, completion);
        });
        let receipt = launch_result(&task_id);
        Ok((task_id, receipt))
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
            r = work(cancel.clone(), progress_tx) => Some(r),
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
            let spec = spec_of_row(&self.store, &row);
            let cancel = state.session_token(caller).child_token();
            state.stops.insert(row.session_key.clone(), cancel.clone());
            let parent = Parent { session_key: caller, seat: &turn.seat, grant, run_taint, cancel: cancel.clone() };
            let req = child::child_request(&parent, task_id, &spec, None, TurnInput::None);
            let helper = state.helpers.entry(task_id.to_string()).or_insert_with(|| Helper {
                parent_key: caller.to_string(),
                session_key: row.session_key.clone(),
                description: spec.description.clone(),
                kind: spec.kind,
                parent_seat: turn.seat.clone(),
                parent_grant: grant.cloned(),
                running: false,
                cancel: cancel.clone(),
                waiter: None,
                held: None,
                earlier_reports: Vec::new(),
            });
            helper.running = true;
            helper.cancel = cancel;
            helper.parent_seat = turn.seat.clone();
            helper.parent_grant = grant.cloned();
            helper.held = None;
            req
        };
        let _ = self.store.update_task_running(task_id);
        self.spawn_turn(task_id.to_string(), resume, None);
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
                    status: CompletionStatus::Failed { error: error.to_string() },
                    result: String::new(),
                    usage: ai::UsageInfo::default(),
                    taint: Vec::new(),
                },
                0,
            );
        }
    }

    /// The caller's own helpers still in hand: running, or finished and
    /// waiting on helpers of their own. Running first.
    pub fn list(&self, caller: &str) -> Vec<HelperStatus> {
        let mut out: Vec<HelperStatus> = self
            .state()
            .helpers
            .iter()
            .filter(|(_, h)| h.parent_key == caller)
            .map(|(id, h)| HelperStatus { task_id: id.clone(), description: h.description.clone(), running: h.running })
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
    fn spawn_turn(self: &Arc<Self>, task_id: String, req: TurnRequest, isolation: Option<crate::worktree::Isolation>) {
        let this = Arc::clone(self);
        tokio::spawn(async move {
            let mut next = Some(req);
            let mut isolation = isolation;
            while let Some(req) = next.take() {
                let completion = this.run_turn(&task_id, req, isolation.take()).await;
                next = this.finish(&task_id, completion);
            }
        });
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
        let collected = match self.starter.start_turn(req).await {
            Ok(handle) => {
                collect::collect(handle.events, &cancel, INACTIVITY_LIMIT, |e| {
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
        if let Some(iso) = isolation {
            let merged = crate::worktree::merge_all(std::slice::from_ref(&iso), &format!("nebo: helper {task_id}")).await;
            for (_, outcome) in &merged {
                completion.result.push_str("\n\n");
                completion.result.push_str(&crate::worktree::render_outcome(&description, outcome));
            }
        }
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

    /// Progress lines for the owner's screen: each tool call the helper
    /// makes becomes its activity line. `None` without a screen to show it.
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
                let (activity, _) = this.registry.labels(&tc.name, &tc.input).await;
                let mut ev = ai::StreamEvent::subagent_start(task_id.as_str(), activity.as_str());
                ev.event_type = ai::StreamEventType::SubagentProgress;
                ev.widgets = Some(serde_json::json!({
                    "task_id": task_id,
                    "tool_count": steps,
                    "current_operation": activity,
                }));
                this.emit(&parent_key, ev);
            }
        });
        Some(tx)
    }

    fn emit(&self, parent_key: &str, event: ai::StreamEvent) {
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
            let mut state = self.state();
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
                };
                let parent = Parent {
                    session_key: &h.parent_key,
                    seat: &h.parent_seat,
                    grant: h.parent_grant.as_ref(),
                    run_taint: &[],
                    cancel: h.cancel.clone(),
                };
                return Some(child::child_request(&parent, task_id, &spec, None, TurnInput::None));
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
            self.spawn_turn(pid, req, None);
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
        };
        let parent = Parent {
            session_key: &p.parent_key,
            seat: &p.parent_seat,
            grant: p.parent_grant.as_ref(),
            run_taint: &[],
            cancel: cancel.clone(),
        };
        let req = child::child_request(&parent, pid, &spec, None, input);
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

/// The spec a helper was started with, read back from its row.
/// The `helper_kind` a helper's row was stored with.
fn spec_kind_of_row(store: &db::Store, row: &db::models::PendingTask) -> Option<String> {
    store
        .engine_get_run(&row.id)
        .ok()
        .flatten()
        .and_then(|run| run.inputs)
        .and_then(|i| serde_json::from_str::<serde_json::Value>(&i).ok())
        .and_then(|v| v.get("helper_kind")?.as_str().map(str::to_string))
}

fn spec_of_row(store: &db::Store, row: &db::models::PendingTask) -> HelperSpec {
    HelperSpec {
        description: row.description.clone().unwrap_or_default(),
        prompt: row.prompt.clone(),
        kind: spec_kind_of_row(store, row)
            .as_deref()
            .and_then(HelperKind::parse)
            .unwrap_or(HelperKind::General),
        background: true,
        isolation: None,
        skills: Vec::new(),
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
            let sessions = Arc::new(SessionManager::new(store.clone()));
            let (started_tx, started) = mpsc::unbounded_channel();
            let (wake_tx, wake) = mpsc::unbounded_channel();
            let (ui_tx, ui) = mpsc::unbounded_channel();
            let helpers = Arc::new(Helpers {
                store: store.clone(),
                sessions: sessions.clone(),
                registry: Arc::new(tools::Registry::new(Arc::new(crate::harness::permissions::Check::new(store.clone())))),
                starter: Arc::new(Scripted { started: started_tx }),
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
                delivery: super::super::Delivery { channel: "web".into(), channel_ctx: None, mention_briefing: None },
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

    /// Parity 5.1: a helper that read the web returns tainted, whichever
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
        let work: tools::orchestrator::Work = Box::new(move |cancel, progress| {
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
        assert!(rig.helpers.list(key).iter().any(|h| h.task_id == id && h.running));

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

        let list = rig.helpers.list("agent:bookkeeper:web");
        assert_eq!(list, vec![HelperStatus { task_id: id.clone(), description: "read the ledger".into(), running: true }]);
        assert!(rig.helpers.list("agent:ceo:web").is_empty());
        assert!(rig.helpers.read_output("agent:ceo:web", &id).is_err());
        assert!(rig.helpers.stop("agent:ceo:web", &id).is_err());
        assert!(!_child.request.cancel.is_cancelled());
        assert!(rig.helpers.read_output("agent:bookkeeper:web", &id).unwrap().contains("still running"));
        assert_eq!(rig.helpers.stop("agent:bookkeeper:web", &id).unwrap(), format!("Stopping helper {id}."));
        assert!(_child.request.cancel.is_cancelled());
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
        TurnMode::Helper { parent_session_key: "agent:x:web".into(), kind, depth }
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
}
