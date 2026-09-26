//! `drive_turn`: the turn state machine, the one loop.
//!
//! Admit (`session_gate`) → Prepare (seat, input row, system prompt, the
//! first step's events) → Step, until the turn ends:
//!
//! 1. the step's events become attachment rows (`reminders`, the one writer);
//! 2. the conversation loads since the last checkpoint, queued mid-turn rows
//!    and notification rows with it;
//! 3. old tool results are trimmed; past the window's threshold the
//!    conversation is checkpointed;
//! 4. the tool surface is built (core, always loaded, loaded deferred);
//! 5. the model is called.
//!
//! A reply with tool calls runs its tool round and takes another step. A
//! reply without tool calls passes the end checks (`turn_end`); one that
//! continues takes another step with its reminder, and when none does the
//! turn has answered. A transient failure, an overflow or an output cutoff
//! takes the step again: its attachments are already stored and nothing is
//! written twice. Finish records the usage, schedules the after-turn work
//! and hands input that arrived after the last step to the next turn.
//!
//! There is no per-call state block and no stream reminder: everything the
//! model reads is the system prompt or a stored row.

use std::collections::{BTreeSet, HashSet};
use std::sync::{Arc, Mutex};

use ai::{ChatRequest, RequestTrace, StreamEvent};
use db::models::ChatMessage;
use tokio::sync::mpsc;
use tracing::{info, warn};

use super::conversation::{self, InputRow, MidTurnFrom};
use super::events::{self, TurnEvent};
use super::model_call::{self, Block, CallOutcome, RetryWhy};
use super::text_fold;
use super::prompt::{self, Identity, sections};
use super::seat::{self, GrantRequest, Seat};
use types::permissions::{Grant, Mode};
use super::session_gate::{self, Admission, RunProgress, TurnGuard};
use super::tool_round::{self, RoundContext, RoundOutcome, RoundState, RunToolScope, ToolExecutor};
use super::tool_surface::{self, SurfaceInputs};
use super::turn_end::{self, EndVerdict};
use super::{Harness, HarnessError, TurnHandle, TurnInput, TurnMode, TurnRequest, compact, goal, reminders, usage};
use crate::pruning;
use super::usage::RunState;
use crate::selector;

/// Steps one turn takes before it ends with `MaxSteps`, so a model that
/// never stops calling tools can't run forever.
pub const DEFAULT_MAX_STEPS: u32 = 100;

/// What one turn runs with, fixed for the turn.
pub struct TurnContext {
    pub harness: Harness,
    pub request: TurnRequest,
    pub seat: Seat,
    /// The run's permissions: every tool call is decided against them.
    pub grant: Arc<Grant>,
    /// The employee's registry entry, when the turn has one.
    pub agent: Option<tools::ActiveAgent>,
    /// The session row id (the request carries the key).
    pub session_id: String,
    pub channel: String,
    /// The owner's IANA timezone, when set: the date is theirs.
    pub timezone: Option<String>,
    /// The model the owner, the job or the helper's speed chose
    /// (`provider/model`); empty when none did and the turn runs on the
    /// selector's default (`TurnState::model`).
    pub model: String,
    /// A linked employee's turn: its linked bot answers it, or nothing does.
    pub linked: bool,
    /// Who the turn is for, told as the `identity` row; built once per turn.
    pub identity: String,
    /// The employee's name, for its memory's heading.
    pub name: String,
    /// The session's environment fields after the date.
    pub environment: Vec<(String, String)>,
    pub mode_facts: events::ModeFacts,
    /// The workspace notes, the employee's own setup and the tools its job
    /// uses.
    pub session_context: String,
    /// How to write for the channel; empty for a channel with none.
    pub channel_rules: String,
    /// The limit on a coworker without shared memory; empty otherwise.
    pub coworker_access: String,
    pub tx: mpsc::Sender<StreamEvent>,
    pub progress: RunProgress,
    pub max_steps: u32,
    /// The owner's spending limit for the run, microcents; 0 = none.
    pub spend_cap_microcents: i64,
    /// The run's provenance: seeded by the input, grown by its tool calls,
    /// stamped on its last event.
    pub taint: Mutex<BTreeSet<types::provenance::ProvenanceClass>>,
    /// After the turn: memory extraction, personality, the chat title and
    /// the self-improvement review.
    pub after_turn: bool,
    /// A review fork's limits: it can only save skills, into its employee's
    /// learned tree.
    pub review_fork: Option<crate::review_fork::ReviewForkCtx>,
    /// The employee's own tools the run's tool scope leaves out.
    pub withheld_tools: Arc<HashSet<String>>,
}

impl TurnContext {
    fn plan_mode(&self) -> bool {
        self.grant.mode == Mode::Plan
    }

    fn workflow(&self) -> Option<&super::WorkflowMode> {
        match &self.request.mode {
            TurnMode::Workflow(m) => Some(m),
            _ => None,
        }
    }

    fn agent_id(&self) -> &str {
        &self.request.seat.agent_id
    }

    fn trace(&self, purpose: &'static str) -> RequestTrace {
        RequestTrace {
            agent_id: self.agent_id().to_string(),
            ..RequestTrace::new(purpose)
        }
    }
}

/// A turn's state across its steps.
pub struct TurnState {
    pub step: u32,
    pub transition: Transition,
    pub reminders: reminders::Reminders,
    /// Failover position, retry counters and output-cap escalation.
    pub call: model_call::CallState,
    /// Tokens, cost and the context thresholds, as the calls report them.
    pub(crate) usage: RunState,
    /// Memory ids this session was already shown; seeded at Prepare from
    /// `memory_context::surfaced_memories`.
    pub surfaced_memories: HashSet<i64>,
    pub end_checks_this_turn: u8,
    pub frozen_renderings: compact::trim::Frozen,
    /// The relevant-memories search started at Prepare.
    pub recall: super::memory_context::RecallPrefetch,
    /// The conversation the last step sent: input stored after it is heard
    /// by the next turn.
    pub seen: Vec<ChatMessage>,
    /// The model every step of the turn runs on (`provider/model`), chosen
    /// once at Prepare: the one the owner, the job or the helper's speed
    /// chose, else the configured default (`ModelSelector::resolve`). It
    /// never changes, mid-turn or after an error: a failed call is retried
    /// on it (`model_call`): switching models mid-turn would break the
    /// cached prefix and change the voice of the answer, and Nebo has no
    /// configured fallback model to switch to.
    pub model: String,
    /// Checkpoints taken this turn.
    pub checkpoints: usize,
    /// The last call that got a reply: the recap forks it.
    last_call: Option<LastCall>,
    persisted_renderings: HashSet<String>,
    /// Stored calls whose tool was asked whether its result may be cleared,
    /// and those it said may.
    trim_checked: HashSet<String>,
    clearable: compact::trim::Clearable,
    /// When the turn checkpoints for itself, with its failure breaker.
    trigger: compact::checkpoint::Trigger,
    round: RoundCarry,
    /// The turn's text segments and their verdicts (`text_fold`).
    folds: text_fold::TurnFolds,
}

/// The last call that got a reply, as the recap forks it.
struct LastCall {
    request: ChatRequest,
    /// The provider that answered it.
    provider: Arc<dyn ai::Provider>,
    /// The last stored row the request was built from: what was stored after
    /// it (the reply, its results) extends the request as the next step's
    /// would.
    heard_through: Option<String>,
}

/// What the tool round keeps from one round to the next within a turn.
#[derive(Default)]
struct RoundCarry {
    called_tools: Vec<String>,
    plan_touch: Option<(usize, String)>,
    edits_since_check: usize,
    last_desktop_act: Option<String>,
}

/// Why the loop is taking its next step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transition {
    First,
    AfterTools,
    MidTurnInput,
    CutoffResume { attempt: u8 },
    OutputEscalated,
    OverflowCleared,
    OverflowCheckpointed,
    TransientRetry { attempt: u8 },
    EndCheckContinue { check: &'static str, reason: String },
}

/// How a turn ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnExit {
    Answered,
    Cancelled,
    MaxSteps {
        steps: u32,
    },
    /// The owner's spending limit was reached.
    BudgetReached,
    /// An app's `agent.should_continue` hook said stop, with its reason.
    AppHalted {
        reason: String,
    },
    /// A tool ended the turn, with what only the owner can supply when the
    /// tool named it.
    TerminalTool {
        notice: String,
        need: Option<types::OwnerNeed>,
    },
    /// A workflow primitive ended the turn (`workflow_exit:…`,
    /// `suspension_failed:…`); the engine reads the reason.
    WorkflowEnded(String),
    ProviderFailed(String),
    Refused(String),
    AwaitingApproval,
    /// The owner's `/compact` wrote its checkpoint.
    Compacted,
    GoalMet {
        reason: String,
    },
    GoalImpossible {
        reason: String,
    },
    GoalPaused(goal::Pause),
}

impl TurnExit {
    /// The word stored on the run's usage row and read by the workflow
    /// engine and `test runs`.
    pub fn label(&self) -> String {
        match self {
            TurnExit::Answered => "text_response".into(),
            // What a helper's collector reads as stopped.
            TurnExit::Cancelled => super::delegation::collect::STOP_CANCELLED.into(),
            // What a helper's collector reads as a partial result.
            TurnExit::MaxSteps { .. } => super::delegation::collect::STOP_MAX_STEPS.into(),
            TurnExit::BudgetReached => super::delegation::collect::STOP_SPEND_CAP.into(),
            TurnExit::AppHalted { .. } => "app_halted".into(),
            TurnExit::TerminalTool { .. } => "terminal_tool_error".into(),
            TurnExit::WorkflowEnded(reason) => reason.clone(),
            TurnExit::ProviderFailed(_) => "provider_failed".into(),
            TurnExit::Refused(_) => "refused".into(),
            TurnExit::AwaitingApproval => "awaiting_approval".into(),
            TurnExit::Compacted => "compacted".into(),
            TurnExit::GoalMet { .. } => "goal_met".into(),
            TurnExit::GoalImpossible { .. } => "goal_impossible".into(),
            TurnExit::GoalPaused(_) => "goal_paused".into(),
        }
    }
}

// ── Admit ────────────────────────────────────────────────────────────────

/// Admit `req` on its session and drive it on its own task. A busy
/// session takes the input as a mid-turn row and the handle carries the
/// busy line.
pub(crate) async fn start(h: Harness, mut req: TurnRequest) -> Result<TurnHandle, HarnessError> {
    if h.providers.read().await.is_empty() {
        return Err(HarnessError::Failed(
            "No AI providers configured. Add API keys in Settings > Providers.".into(),
        ));
    }
    if req.session_key.is_empty() {
        req.session_key = "default".into();
    }
    let session = h
        .sessions
        .get_or_create(&req.session_key, &req.seat.user_id)
        .map_err(|e| HarnessError::Failed(format!("session error: {e}")))?;
    let progress = req.progress.clone().unwrap_or_else(|| RunProgress {
        run_id: uuid::Uuid::new_v4().to_string(),
        iteration_count: Default::default(),
        tool_call_count: Default::default(),
        current_tool: Default::default(),
        waiting: Default::default(),
        stalled: Default::default(),
    });
    let turn_id = progress.run_id.clone();
    if owner_speaks(&req) {
        resume_goal(&h, &session.id);
    }

    let admission = if matches!(req.input, TurnInput::Compact { .. }) {
        match session_gate::admit_when_free(&h.active_turns, &req.session_key, progress.clone(), req.cancel.clone()).await {
            Some(guard) => Admission::Admitted(guard),
            None => return Err(HarnessError::Failed("The compact was stopped before it started.".into())),
        }
    } else {
        let queue = || queue_input(&h, &session.id, &req);
        session_gate::admit_or_queue(&h.active_turns, &req.session_key, progress.clone(), req.cancel.clone(), queue).await
    };
    let (tx, rx) = mpsc::channel(100);
    match admission {
        Admission::Queued { status } => {
            info!(session_id = %session.id, "input on a busy session queued into the running turn");
            // A send fails only when the caller dropped the receiver.
            let _ = tx
                .send(StreamEvent::control_notice(status, session_gate::QUEUED_INTO_RUNNING_TURN))
                .await;
            let _ = tx.send(StreamEvent::done()).await;
        }
        Admission::Admitted(guard) => {
            tokio::spawn(run(h, req, session.id, progress, guard, tx));
        }
    }
    Ok(TurnHandle { events: rx, turn_id })
}

/// Whether the owner wrote this turn's input in their own chat: an owner
/// chat turn from the owner's app, not a chat channel (Slack, Discord, a
/// loop), a coworker, a visitor or a caller. Only such input is stored as
/// the owner's word (a consent reads nothing else).
fn owner_speaks(req: &TurnRequest) -> bool {
    matches!(req.mode, TurnMode::Chat)
        && matches!(req.input, TurnInput::Owner { .. })
        && req.seat.origin == tools::Origin::User
        && req.seat.audience.is_none()
}

/// Whether the owner is in this turn's conversation: an owner chat turn in
/// the owner's own chat (the app, the phone, the owner's loop), started by
/// the owner's message, a message queued behind it, or a result the owner's
/// work brought back. Not a scheduled or other unattended turn, a coworker,
/// a chat channel, a visitor or a caller, and not a prompt the platform
/// wrote (an introduction). Only these turns get a recap.
fn owner_in_turn(req: &TurnRequest) -> bool {
    matches!(req.mode, TurnMode::Chat)
        && !matches!(req.input, TurnInput::Platform { .. } | TurnInput::Compact { .. })
        && req.seat.origin == tools::Origin::User
        && req.seat.audience.is_none()
}

/// The owner's message resumes a paused goal, and the owner sees it resume.
fn resume_goal(h: &Harness, session_id: &str) {
    match goal::GoalStore::new(&h.sessions, session_id).resume() {
        Ok(Some(goal)) => {
            info!(session_id, "the owner's message resumed the paused goal");
            if let Some(observer) = h.goal_observer() {
                observer.status(&goal);
            }
        }
        Ok(None) => {}
        Err(e) => warn!(session_id, error = %e, "a paused goal could not be resumed"),
    }
}

/// Write a busy session's input where its running turn hears it at the
/// next step. Runs under the admission lock.
fn queue_input(h: &Harness, session_id: &str, req: &TurnRequest) {
    let written = match &req.input {
        TurnInput::Owner { text, .. } => {
            let via = if req.delivery.channel.is_empty() {
                "chat"
            } else {
                &req.delivery.channel
            };
            let mut meta = MidTurnFrom::Owner {
                via: via.to_string(),
            }
            .value();
            if owner_speaks(req) {
                conversation::mark_owner(&mut meta);
            }
            h.sessions
                .append_message(
                    session_id,
                    "user",
                    text,
                    None,
                    None,
                    Some(&meta.to_string()),
                )
                .map(|_| ())
        }
        TurnInput::Platform { text } => h
            .sessions
            .append_message(session_id, "user", text, None, None, Some(r#"{"isMeta":true,"hiddenPrompt":true}"#))
            .map(|_| ()),
        TurnInput::Coworker { from, text } => h
            .sessions
            .append_message(
                session_id,
                "user",
                text,
                None,
                None,
                Some(&MidTurnFrom::Coworker { from: from.clone() }.metadata()),
            )
            .map(|_| ()),
        TurnInput::Notification(c) => h
            .sessions
            .append_message(
                session_id,
                "user",
                &super::delegation::render_notification(c),
                None,
                None,
                Some(&super::delegation::notify::row_metadata(&c.taint)),
            )
            .map(|_| ()),
        TurnInput::None | TurnInput::Compact { .. } => Ok(()),
    };
    if let Err(e) = written {
        warn!(session_id, error = %e, "could not queue input into the running turn");
    }
    if let Some(briefing) = req.delivery.mention_briefing.as_deref() {
        let mut r = reminders::Reminders::default();
        r.add(&TurnEvent::RunBriefing(briefing.to_string()));
        if let Err(e) = r.write(&h.sessions, session_id) {
            warn!(session_id, error = %e, "could not queue the briefing into the running turn");
        }
    }
}

/// The admitted turn's task: prepare, drive, finish; then, while input
/// arrived after the last step, the next turn on the same slot.
async fn run(
    h: Harness,
    req: TurnRequest,
    session_id: String,
    progress: RunProgress,
    guard: TurnGuard,
    tx: mpsc::Sender<StreamEvent>,
) {
    let mut req = Some(req);
    let mut taint = BTreeSet::new();
    let mut exit = TurnExit::Answered;
    while let Some(next) = req.take() {
        let (cx, mut st) = match prepare(&h, next, &session_id, progress.clone(), tx.clone()).await {
            Ok(prepared) => prepared,
            Err(e) => {
                let _ = tx.send(StreamEvent::error(format!("Agent error: {e}"))).await;
                exit = TurnExit::ProviderFailed(e);
                break;
            }
        };
        exit = drive_turn(&cx, &mut st).await;
        finish(&cx, &mut st, &exit).await;
        taint.extend(cx.taint.lock().unwrap_or_else(|p| p.into_inner()).iter().copied());

        // Input stored after the last step was not in any call. The slot
        // closes first: input arriving from here on waits for it and starts
        // its own turn, so everything before is visible now.
        guard.close();
        if exit != TurnExit::Cancelled && heard_nothing_since(&h, &session_id, &st.seen) {
            info!(session_id, "input arrived after the last step: the next turn hears it");
            guard.reopen();
            req = Some(follow_up(cx.request));
        }
    }
    let _ = tx
        .send(StreamEvent::done_with_reason(exit.label()).with_provenance(taint.into_iter().collect()))
        .await;
    drop(guard);
}

/// Whether a mid-turn or notification row landed after `seen`.
fn heard_nothing_since(h: &Harness, session_id: &str, seen: &[ChatMessage]) -> bool {
    let Ok(fresh) = h.sessions.get_messages_since_checkpoint(session_id) else {
        return false;
    };
    let last_seen = seen.last().map(|m| m.id.as_str());
    fresh
        .iter()
        .rev()
        .take_while(|m| last_seen != Some(m.id.as_str()))
        .any(|m| m.role == "user" && (conversation::arrived_mid_turn(m).is_some() || queued_row(m)))
}


/// A notification or a platform prompt (a goal kickoff) written into a
/// running turn.
fn queued_row(msg: &ChatMessage) -> bool {
    super::delegation::notify::is_notification_row(msg)
        || msg
            .metadata
            .as_deref()
            .and_then(|m| serde_json::from_str::<serde_json::Value>(m).ok())
            .and_then(|v| v.get("hiddenPrompt").and_then(|b| b.as_bool()))
            == Some(true)
}

/// The next turn on the same session and seat: its input is already in the
/// conversation.
fn follow_up(req: TurnRequest) -> TurnRequest {
    TurnRequest {
        input: TurnInput::None,
        delivery: super::Delivery {
            mention_briefing: None,
            ..req.delivery
        },
        ..req
    }
}

// ── Prepare ──────────────────────────────────────────────────────────────

/// Resolve the seat, store the input, build the system prompt and queue the
/// first step's events.
pub(crate) async fn prepare(
    h: &Harness,
    mut req: TurnRequest,
    session_id: &str,
    progress: RunProgress,
    tx: mpsc::Sender<StreamEvent>,
) -> Result<(TurnContext, TurnState), String> {
    {
        let s = &mut req.seat;
        seat::restrict_outside_origin(s.origin, &mut s.tool_allowlist, &mut s.tool_denial_hint);
    }
    let grant = Arc::new(seat::run_grant(
        &h.store,
        GrantRequest {
            agent_id: &req.seat.agent_id,
            origin: req.seat.origin,
            mode: req.seat.mode,
            ceiling: req.seat.ceiling.as_ref(),
            cwd: req.seat.cwd.as_deref(),
        },
    ));
    let agent = if req.seat.agent_id.is_empty() {
        None
    } else {
        h.agent_registry.read().await.get(&req.seat.agent_id).cloned()
    };
    let channel = if !req.delivery.channel.is_empty() {
        req.delivery.channel.clone()
    } else {
        let info = types::keyparser::parse_session_key(&req.session_key);
        if info.channel.is_empty() { "web".to_string() } else { info.channel }
    };
    let seat = seat::resolve_seat(
        &h.store,
        &req.session_key,
        seat::SeatInputs {
            agent: agent.as_ref(),
            agent_id: &req.seat.agent_id,
            user_id: &req.seat.user_id,
            session_id,
            origin: req.seat.origin,
            channel: &channel,
            audience: req.seat.audience.as_deref(),
        },
    );
    // The employee's own model preference, from its hire or its settings,
    // for every run kind (the owner's chat, a workflow, a schedule, a
    // coworker, the phone) when the request names none. A linked employee
    // runs as its linked agent: that is not a model choice, so a request's
    // model never replaces it and nothing else answers for it.
    let employee = employee_model(&h.store, &req.seat.agent_id);
    let raw_model = if employee.linked {
        employee.preference.clone().unwrap_or_default()
    } else if !req.seat.model_override.is_empty() {
        req.seat.model_override.clone()
    } else {
        req.seat.model_preference.clone().or(employee.preference).unwrap_or_default()
    };
    let model = if raw_model.is_empty() {
        String::new()
    } else {
        h.selector.resolve_fuzzy(&raw_model).unwrap_or(raw_model)
    };

    store_input(h, session_id, &req).await?;

    let name = agent
        .as_ref()
        .map(|a| a.name.clone())
        .or_else(|| h.store.get_agent(&req.seat.agent_id).ok().flatten().map(|a| a.name))
        .unwrap_or_else(|| "Nebo".to_string());
    let memory = super::memory_context::load_employee_memory(
        &h.store,
        &seat.memory.user_id,
        &req.seat.agent_id,
        &seat.inherit_scopes,
        &name,
    );
    super::memory_context::record_access(&h.store, memory.identity_ids.clone());
    let memory_timezone = memory.timezone.clone();
    let role = match &req.mode {
        TurnMode::Helper { parent_session_key, kind, .. } => {
            prompt::Role::Helper { parent: parent_name(h, parent_session_key).await, kind: *kind }
        }
        _ => prompt::Role::Employee,
    };
    let mut withheld = match agent.as_ref() {
        Some(a) => tool_surface::scope_withheld(a, req.seat.tool_scope.as_deref(), &h.tools).await,
        None => HashSet::new(),
    };
    if seat.company_memory_sealed {
        withheld.extend(seat::company_memory_tools(&h.store, &h.tools, &req.seat.agent_id).await);
    }
    let withheld_tools = Arc::new(withheld);
    let job_tools = match agent.as_ref() {
        Some(a) => {
            prompt::inputs::job_tools(
                a,
                req.seat.tool_scope.as_deref(),
                &h.tools,
                &h.store,
                &withheld_tools,
            )
            .await
        }
        None => String::new(),
    };
    let session_context = [
        prompt::inputs::workspace_notes().map(|n| sections::workspace_notes(&n)).unwrap_or_default(),
        agent.as_ref().map(prompt::inputs::self_context).unwrap_or_default(),
        agent
            .as_ref()
            .map(|a| {
                prompt::inputs::plugin_context(
                    a,
                    req.seat.tool_scope.as_deref(),
                    h.skill_loader.as_deref(),
                    &h.store,
                )
            })
            .unwrap_or_default(),
        job_tools,
    ]
    .into_iter()
    .filter(|p| !p.trim().is_empty())
    .collect::<Vec<_>>()
    .join("\n\n");
    let turn_model = h.selector.resolve(&model);
    let mode_facts = events::ModeFacts {
        model: turn_model.clone(),
        permission_mode: permission_mode_name(grant.mode).to_string(),
    };
    let environment = sections::environment_fields(req.seat.cwd.as_deref(), &channel, seat.execution_mode.into());
    let channel_plugin = h.tools.get(&format!("{}{channel}", tools::plugin_tools::PLUGIN_PREFIX)).await.is_some();
    let files_dir = config::data_dir()
        .map(|d| d.join("files").to_string_lossy().into_owned())
        .unwrap_or_else(|_| "~/Documents".to_string());
    let channel_rules = sections::channel_rules(&channel, channel_plugin, &files_dir);
    let coworker_access = sections::coworker_access(seat.audience_restricted);
    let identity = Identity {
        name: name.clone(),
        role,
        personality_snippet: req.seat.personality_snippet.clone(),
        soul: agent.as_ref().and_then(|a| a.soul.clone()),
        rules: agent.as_ref().and_then(|a| a.rules.clone()),
        persona: agent.as_ref().map(|a| prompt::inputs::persona_body(&a.agent_md)),
    }
    .text();

    // The relevant-memories search runs while the steps go on; a step lands
    // it once it has finished.
    let history = h.sessions.get_messages_since_checkpoint(session_id).unwrap_or_default();
    let mut surfaced = super::memory_context::surfaced_memories(&history);
    // The words someone wrote are searched for (a coworker's recall under
    // its audience limit); other input recalls nothing.
    let recall_prompt = match &req.input {
        TurnInput::Owner { text, .. } | TurnInput::Coworker { text, .. } => text.as_str(),
        _ => "",
    };
    let recall = super::memory_context::RecallPrefetch::start(
        h.hybrid_searcher.as_ref(),
        &h.store,
        super::memory_context::RecallRequest {
            prompt: recall_prompt,
            user_id: &seat.memory.user_id,
            tacit_only: seat.audience_restricted,
            skip: surfaced.iter().copied().chain(memory.identity_ids.iter().copied()).collect(),
        },
    );
    surfaced.extend(memory.identity_ids.iter().copied());

    let (max_steps, spend_cap_microcents) = match &req.mode {
        TurnMode::Workflow(m) => (if m.max_steps > 0 { m.max_steps } else { DEFAULT_MAX_STEPS }, m.spend_cap_microcents),
        TurnMode::Fork(_) => (crate::review_fork::REVIEW_MAX_ITERATIONS as u32, 0),
        _ => (DEFAULT_MAX_STEPS, 0),
    };
    let after_turn = matches!(req.mode, TurnMode::Chat)
        && !matches!(req.input, TurnInput::Compact { .. })
        && !seat.memory.writes_disabled;
    let taint = Mutex::new(req.seat.seed_taint.iter().copied().collect());

    let mut st = TurnState {
        step: 0,
        transition: Transition::First,
        reminders: reminders::Reminders::default(),
        call: model_call::CallState::default(),
        usage: RunState::default(),
        surfaced_memories: surfaced,
        recall,
        end_checks_this_turn: 0,
        frozen_renderings: h
            .store
            .get_chat_renderings(&h.store.resolve_session_chat_id(session_id))
            .unwrap_or_default(),
        seen: Vec::new(),
        model: turn_model,
        checkpoints: 0,
        last_call: None,
        persisted_renderings: HashSet::new(),
        trim_checked: HashSet::new(),
        clearable: compact::trim::Clearable::new(),
        trigger: compact::checkpoint::Trigger::default(),
        round: RoundCarry::default(),
        folds: text_fold::TurnFolds::default(),
    };
    st.persisted_renderings = st.frozen_renderings.keys().cloned().collect();

    // The first step's events: when the turn starts, then its briefing.
    st.reminders.add(&TurnEvent::TurnTime(sections::owner_now(memory_timezone.as_deref())));
    if let Some(briefing) = req.delivery.mention_briefing.as_deref() {
        st.reminders.add(&TurnEvent::RunBriefing(briefing.to_string()));
    }
    if let Some(notice) = seat::restricted_run_notice(
        req.seat.tool_allowlist.as_ref().is_some_and(|wl| wl.is_empty()),
        req.seat.tool_allowlist.as_ref(),
        req.seat.tool_denial_hint.as_deref(),
    ) {
        st.reminders.add(&TurnEvent::RestrictedRun(notice));
    }

    let review_fork = match &req.mode {
        TurnMode::Fork(super::ForkKind::Review { staged }) => {
            Some(crate::review_fork::ReviewForkCtx::new(req.seat.agent_id.clone(), *staged))
        }
        _ => None,
    };
    let cx = TurnContext {
        harness: h.clone(),
        request: req,
        seat,
        grant,
        agent,
        session_id: session_id.to_string(),
        channel,
        timezone: memory_timezone,
        model,
        linked: employee.linked,
        identity,
        name,
        environment,
        mode_facts,
        session_context,
        channel_rules,
        coworker_access,
        tx,
        progress,
        max_steps,
        spend_cap_microcents,
        taint,
        after_turn,
        review_fork,
        withheld_tools,
    };
    // Entering and leaving Plan mode are rows, told once each (the
    // plan_mode and plan_mode_exit attachments), so the prompt stays
    // cacheable and the model isn't reminded every step.
    let announced = plan_mode_announced(&h.sessions, session_id);
    if cx.plan_mode() && !announced {
        st.reminders.add(&TurnEvent::PlanMode { entered: true });
    } else if !cx.plan_mode() && announced {
        st.reminders.add(&TurnEvent::PlanMode { entered: false });
    }
    Ok((cx, st))
}

/// Store the turn's input as its row.
async fn store_input(h: &Harness, session_id: &str, req: &TurnRequest) -> Result<(), String> {
    let (text, images, attachments, hidden, coworker): (&str, &[ai::ImageContent], &[comm::wire::Attachment], bool, Option<&str>) =
        match &req.input {
            TurnInput::Owner { text, images, attachments } => (text, images, attachments, false, None),
            TurnInput::Platform { text } => (text, &[], &[], true, None),
            TurnInput::Coworker { from, text } => (text, &[], &[], false, Some(from.as_str())),
            TurnInput::Notification(c) => {
                return h
                    .sessions
                    .append_message(
                        session_id,
                        "user",
                        &super::delegation::render_notification(c),
                        None,
                        None,
                        Some(&super::delegation::notify::row_metadata(&c.taint)),
                    )
                    .map(|_| ())
                    .map_err(|e| format!("failed to store the notification: {e}"));
            }
            TurnInput::None | TurnInput::Compact { .. } => return Ok(()),
        };
    if text.is_empty() {
        return Ok(());
    }
    conversation::persist_input(
        &h.sessions,
        session_id,
        InputRow {
            text,
            images,
            attachments,
            hidden,
            by_owner: !hidden && owner_speaks(req),
            coworker,
        },
    )
}

/// The name of the employee a helper works for.
async fn parent_name(h: &Harness, parent_key: &str) -> String {
    let agent_id = types::keyparser::extract_agent_id(parent_key);
    if !agent_id.is_empty()
        && let Some(a) = h.agent_registry.read().await.get(&agent_id)
    {
        return a.name.clone();
    }
    "the employee".to_string()
}

/// The permission mode as the owner sees it.
fn permission_mode_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Automatic => "Automatic",
        Mode::Ask => "Ask",
        Mode::Plan => "Plan",
        Mode::FullAccess => "Full Access",
    }
}

/// Whether the conversation's latest plan-mode row says plan mode is on.
fn plan_mode_announced(sessions: &crate::session::SessionManager, session_id: &str) -> bool {
    let entered = events::attachment_for(&TurnEvent::PlanMode { entered: true }).map(|a| reminders::wrap(&a.text));
    sessions
        .get_messages_since_checkpoint(session_id)
        .unwrap_or_default()
        .iter()
        .rev()
        .find(|m| reminders::attachment_kind(m).as_deref() == Some("plan_mode"))
        .is_some_and(|m| Some(&m.content) == entered.as_ref())
}

// ── Step ─────────────────────────────────────────────────────────────────

/// Drive one turn to its exit.
pub async fn drive_turn(cx: &TurnContext, st: &mut TurnState) -> TurnExit {
    let h = &cx.harness;
    let sessions = &h.sessions;
    let sid = cx.session_id.as_str();
    let side_trace = |purpose: &'static str| cx.trace(purpose);

    loop {
        if cx.request.cancel.is_cancelled() {
            return TurnExit::Cancelled;
        }
        if cx.tx.is_closed() {
            warn!(session_id = sid, "event receiver dropped: ending the turn");
            return TurnExit::Cancelled;
        }
        if st.step >= cx.max_steps {
            let _ = cx
                .tx
                .send(StreamEvent::control_notice(
                    format!("Stopped after {} steps, the most one turn takes. Ask me to continue and I'll pick it up.", st.step),
                    "max_steps",
                ))
                .await;
            return TurnExit::MaxSteps { steps: st.step };
        }
        if let Some(exit) = budget_reached(cx, st).await {
            return exit;
        }
        if let Some(exit) = app_halted(cx, st).await {
            return exit;
        }
        st.step += 1;
        cx.progress.iteration_count.store(st.step, std::sync::atomic::Ordering::Relaxed);
        info!(session_id = sid, step = st.step, transition = ?st.transition, "turn step");

        // 1-2. The step's events, then the conversation with them.
        let mut conversation = match sessions.get_messages_since_checkpoint(sid) {
            Ok(c) => c,
            Err(e) => return TurnExit::ProviderFailed(format!("failed to load the conversation: {e}")),
        };
        if conversation::mid_turn_message_landed(&conversation, &st.seen) && st.step > 1 {
            st.transition = Transition::MidTurnInput;
        }
        let surface_seat = SurfaceInputs {
            agent_id: cx.agent_id(),
            allowlist: cx.request.seat.tool_allowlist.as_ref(),
            workflow: cx.workflow(),
            mode: &cx.request.mode,
            withheld: &cx.withheld_tools,
            desktop: tools::desktop_available(),
        };
        let surface = tool_surface::surface(&h.tools, &conversation, &surface_seat).await;
        // Attachments are Nebo's context for Nebo's model. A linked
        // employee's runtime owns its own context, so a linked turn writes
        // none: no identity, roster, environment, time or listing row ever
        // reaches another runtime.
        if !cx.linked {
            step_events(cx, st, &conversation, surface.listing.clone(), &surface.declared).await;
            if st.reminders.has_queued() {
                if let Err(e) = st.reminders.write(sessions, sid) {
                    warn!(session_id = sid, error = %e, "could not write this step's attachments");
                }
                conversation = match sessions.get_messages_since_checkpoint(sid) {
                    Ok(c) => c,
                    Err(e) => return TurnExit::ProviderFailed(format!("failed to load the conversation: {e}")),
                };
            }
        }
        cx.taint
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .extend(conversation::received_taint(&conversation));

        // 3. Trim; past the threshold, clear old results, else checkpoint.
        let context_window = cx.harness.selector.context_window(&st.model);
        st.usage.system_overhead_tokens = overhead_tokens(&surface.declared);
        let window = trim(st, &conversation);
        st.usage.last_request_estimate = pruning::estimate_total_tokens(&window);
        let window = conversation::sanitize_message_order(conversation::order_as_heard(window));
        // Rows stored after this one arrive while the step runs: a
        // checkpoint's summary never reads them.
        let heard_through = conversation.last().map(|m| m.id.as_str());

        // 4-5. The request and the call.
        let selected = st.model.clone();
        let (provider_id, mut model_name) = model_parts(&selected);
        let loaded = h.providers.read().await.iter().any(|p| p.id() == provider_id);
        // A linked employee is answered by its linked bot or not at all:
        // any other provider answering would speak as the employee.
        if cx.linked && !(loaded && provider_id == ai::providers::linked::ID) {
            warn!(session_id = sid, model = %selected, "a linked employee's provider can't take the turn");
            let message = format!("Could not connect to {}. Try again.", cx.name);
            let _ = cx.tx.send(StreamEvent::error(message.clone())).await;
            return TurnExit::ProviderFailed(message);
        }
        // The model's provider isn't loaded: the call goes to the first
        // provider with that provider's own model (`model_call`), and the
        // request says so, so what forks it (the recap, a checkpoint) sends
        // what was sent.
        if !loaded {
            model_name.clear();
        }
        let request = build_request(cx, st, &window, surface.declared, &model_name);
        let request_tokens =
            st.usage.last_request_estimate + st.usage.system_overhead_tokens + st.usage.estimate_correction;
        let max_output = usize::try_from(request.max_tokens).unwrap_or_default();
        // The owner's `/compact`: this step's request is what the summary
        // forks, and the turn ends with the checkpoint.
        if let TurnInput::Compact { instructions } = &cx.request.input {
            let why = compact::checkpoint::CheckpointReason::OwnerAsked;
            return match checkpoint(cx, st, &window, heard_through, &request, why, Some(instructions)).await {
                Ok(()) => TurnExit::Compacted,
                Err(e) => {
                    let _ = cx.tx.send(StreamEvent::error(format!("The conversation could not be compacted: {e}"))).await;
                    TurnExit::ProviderFailed(e)
                }
            };
        }
        // A linked employee's runtime keeps its own transcript and is sent
        // only the newest message: there is nothing here to checkpoint, and a
        // checkpoint forks the turn's provider.
        if !cx.linked && st.trigger.due(request_tokens, context_window, max_output) {
            if clear_old_results(cx, st, &conversation).await {
                st.step -= 1;
                continue;
            }
            match checkpoint(cx, st, &window, heard_through, &request, compact::checkpoint::CheckpointReason::Threshold, None).await {
                Ok(()) => continue,
                Err(e) => warn!(session_id = sid, error = %e, "checkpoint failed; sending the conversation as it is"),
            }
        }
        st.seen = conversation.clone();

        let declared_names: Arc<HashSet<String>> = Arc::new(request.tools.iter().map(|t| t.name.clone()).collect());
        let memory_user_id = cx.seat.memory.user_id.clone();
        let tool_scope = RunToolScope {
            sessions,
            tx: &cx.tx,
            session_id: sid,
            origin: cx.request.seat.origin,
            cancel_token: &cx.request.cancel,
            progress: Some(&cx.progress),
            ask_channels: h.ask_channels.as_ref(),
            handoff_depth: cx.request.seat.handoff_depth,
            grant: &cx.grant,
            door: &cx.request.seat.door,
            owner_request: owner_speaks(&cx.request),
            untrusted_input: cx.workflow().is_some_and(|m| m.tainted),
            run_cwd: cx.request.seat.cwd.as_deref(),
            channel_ctx: cx.request.delivery.channel_ctx.as_ref(),
            model_override: &cx.model,
            memory_user_id: &memory_user_id,
            memory_topics: &cx.seat.memory_topics,
            memory_writes_disabled: cx.seat.memory.writes_disabled,
            memory_write_bar: &cx.seat.write_bar,
            audience_restricted: cx.seat.audience_restricted,
            audience: cx.request.seat.audience.as_deref(),
            memory_matter: &cx.seat.memory_matter,
            run_taint: &cx.taint,
            review_fork: cx.review_fork.as_ref(),
            tool_allowlist: cx.request.seat.tool_allowlist.as_ref(),
            tool_denial_hint: &cx.request.seat.tool_denial_hint,
            declared_tools: &declared_names,
            withheld_tools: &cx.withheld_tools,
        };
        let issue_credential = h.tool_credentials.as_ref().map(|credentials| {
            let tool_scope = &tool_scope;
            move || {
                credentials.issue(crate::tool_credentials::RunGrant {
                    ctx: tool_scope.tool_context(),
                    agent_id: cx.agent_id().to_string(),
                })
            }
        });
        let fork_of = request.clone();
        // What the permission judge reads beside a call it is asked about
        // (PRD-Permissions §4.7): the owner's latest message and the goal.
        let owner_words = h
            .store
            .latest_owner_message(&sessions.active_chat_id(sid))
            .ok()
            .flatten()
            .unwrap_or_default();
        let goal = goal::GoalStore::new(sessions, sid)
            .active()
            .ok()
            .flatten()
            .map(|g| g.condition)
            .unwrap_or_default();
        let round_cx = RoundContext {
            scope: &tool_scope,
            tools: &h.tools,
            providers: &h.providers,
            concurrency: &h.concurrency,
            hooks: &h.hooks,
            user_prompt: &owner_words,
            iteration: st.step as usize,
            workflow_mode: cx.workflow(),
            decide: h.decide.as_ref(),
            active_task: &goal,
            turn_mode: Some(&cx.request.mode),
            side_trace: &side_trace,
        };
        // The streaming executor: safe calls start as their input completes
        // in the stream, while the reply is still arriving, so a read costs
        // no wait for the rest of the reply.
        let mut executor = ToolExecutor::new(&round_cx);
        let (tool_calls_out, streamed_calls) = mpsc::unbounded_channel();
        let call = model_call::call_model(
            model_call::ModelCall {
                request,
                providers: &h.providers,
                selector: &h.selector,
                concurrency: &h.concurrency,
                priority: if owner_in_turn(&cx.request) {
                    crate::concurrency::Priority::Owner
                } else {
                    crate::concurrency::Priority::Work
                },
                sessions,
                cancel: &cx.request.cancel,
                tx: &cx.tx,
                session_id: sid,
                step: st.step as usize,
                step_started: std::time::Instant::now(),
                selected_provider_id: &provider_id,
                selected_model: &selected,
                model_override: &cx.model,
                context_limit: compact::checkpoint::Trigger::threshold(context_window, max_output),
                tool_credential: issue_credential
                    .as_ref()
                    .map(|issue| issue as &(dyn Fn() -> crate::tool_credentials::CredentialGuard + Send + Sync)),
                tool_calls_out,
                folds: &mut st.folds,
            },
            &mut st.call,
            &mut st.usage,
        );
        let (outcome, ()) = tokio::join!(call, executor.stream(streamed_calls));
        let reply = match outcome {
            CallOutcome::Reply(reply) => reply,
            CallOutcome::Retry(RetryWhy::Overflow) => {
                // The provider refused the window: clear old results when
                // that saves enough, the cheapest way to fit the window;
                // else checkpoint, unless the breaker
                // has tripped. The model call gives up after its own
                // overflow retries.
                let outcome = if clear_old_results(cx, st, &conversation).await {
                    Ok(Transition::OverflowCleared)
                } else if st.trigger.tripped() {
                    Err("the checkpoint breaker has tripped".to_string())
                } else {
                    checkpoint(cx, st, &window, heard_through, &fork_of, compact::checkpoint::CheckpointReason::Overflow, None)
                        .await
                        .map(|()| Transition::OverflowCheckpointed)
                };
                match outcome {
                    Ok(transition) => st.transition = transition,
                    Err(e) => {
                        warn!(session_id = sid, error = %e, "overflow checkpoint failed; retrying as it is");
                        st.transition = Transition::TransientRetry { attempt: st.call.overflow_retries as u8 };
                    }
                }
                st.step -= 1;
                continue;
            }
            CallOutcome::Retry(RetryWhy::StreamCut) => {
                st.reminders.add(&TurnEvent::StreamCut);
                st.transition = Transition::TransientRetry {
                    attempt: (st.call.transient_retries + st.call.retryable_retries) as u8,
                };
                st.step -= 1;
                continue;
            }
            CallOutcome::Retry(RetryWhy::Transient) => {
                st.transition = Transition::TransientRetry {
                    attempt: (st.call.transient_retries + st.call.retryable_retries) as u8,
                };
                st.step -= 1;
                continue;
            }
            CallOutcome::Cancelled | CallOutcome::CancelledInBackoff => return TurnExit::Cancelled,
            CallOutcome::Exhausted => return TurnExit::ProviderFailed("the provider's retries ran out".into()),
            CallOutcome::Failed(e) => {
                let _ = cx.tx.send(StreamEvent::error(format!("Agent error: {e}"))).await;
                return TurnExit::ProviderFailed(e);
            }
        };
        let model_call::ModelReply {
            text,
            mut tool_calls,
            stop,
            stream_error,
            block_order,
            provider,
            thinking,
            thinking_model,
        } = reply;
        st.last_call = Some(LastCall {
            request: fork_of.clone(),
            provider: provider.clone(),
            heard_through: st.seen.last().map(|m| m.id.clone()),
        });
        let text = post_receive(cx, text, tool_calls.len()).await;
        if stream_error.is_some() {
            // Calls that arrived on a broken stream are not run or stored.
            tool_calls.clear();
        }
        let heard_through = st.seen.last().map(|m| m.id.as_str());
        save_reply(cx, &mut st.folds, &text, &tool_calls, &block_order, (&thinking, &thinking_model), heard_through).await;
        // A stream error the call did not retry has been shown to the owner
        // once, and the call's ladder decided it is final: the step ends
        // with it. Taking the step again as an empty reply would send what
        // the ladder chose not to send, and show the error again (a linked
        // bot that can't be reached was dialled again after each 20 s
        // connect timeout).
        if let Some(error) = stream_error {
            return TurnExit::ProviderFailed(error);
        }

        if !tool_calls.is_empty() {
            // A CLI provider ran its tools itself over /agent/mcp.
            if provider.handles_tools() {
                return TurnExit::Answered;
            }
            match tool_round(cx, st, &round_cx, executor, &text, &mut tool_calls).await {
                Some(exit) => return exit,
                None => {
                    st.transition = Transition::AfterTools;
                    continue;
                }
            }
        }

        // No tool calls. The output cap cut the reply off: retry at the
        // escalated cap, then continue in place.
        match model_call::output_cutoff(&mut st.call, stop.as_deref(), st.step as usize, sid) {
            Some(model_call::StepRetry::Same) => {
                st.transition = Transition::OutputEscalated;
                continue;
            }
            Some(model_call::StepRetry::Resume) => {
                st.reminders.add(&TurnEvent::CutoffResume);
                st.transition = Transition::CutoffResume {
                    attempt: st.call.output_recovery_attempts as u8,
                };
                continue;
            }
            None => {}
        }
        // The provider said tools were called and none arrived: the
        // transport lost them; take the step again.
        if model_call::lost_tool_calls(&mut st.call, stop.as_deref(), &tool_calls, st.step as usize, sid) {
            st.transition = Transition::TransientRetry {
                attempt: st.call.lost_toolcall_retries as u8,
            };
            st.step -= 1;
            continue;
        }
        if text.trim().is_empty() {
            if model_call::retry_empty_reply(&mut st.call, st.step as usize, sid) {
                st.reminders.add(&TurnEvent::EmptyReply);
                st.transition = Transition::TransientRetry {
                    attempt: st.call.empty_content_retries as u8,
                };
                st.step -= 1;
                continue;
            }
            let _ = cx.tx.send(StreamEvent::error("The model returned an empty reply.")).await;
            return TurnExit::ProviderFailed("empty reply".into());
        }
        st.call.empty_content_retries = 0;

        // Turn end: every end check may continue the turn or end it.
        if let Some(next) = end_checks(cx, st).await {
            match next {
                Ok(()) => continue,
                Err(exit) => return exit,
            }
        }
        return TurnExit::Answered;
    }
}

/// The owner's spending limit, checked before each step.
async fn budget_reached(cx: &TurnContext, st: &TurnState) -> Option<TurnExit> {
    if cx.spend_cap_microcents <= 0 {
        return None;
    }
    let h = &cx.harness;
    let spent = usage::run_spend_so_far(&h.store, &h.selector, &cx.request.session_key, &st.model, &st.usage);
    if spent < cx.spend_cap_microcents {
        return None;
    }
    const PER_DOLLAR: f64 = 100_000_000.0;
    warn!(session_id = %cx.session_id, spent, cap = cx.spend_cap_microcents, "spending limit reached");
    let _ = cx
        .tx
        .send(StreamEvent::control_notice(
            format!(
                "Stopped: this run reached its spending limit (${:.2} of ${:.2}).",
                spent as f64 / PER_DOLLAR,
                cx.spend_cap_microcents as f64 / PER_DOLLAR
            ),
            "spend_cap_reached",
        ))
        .await;
    Some(TurnExit::BudgetReached)
}

/// An app's `agent.should_continue` answer, asked before each step: `false`
/// ends the turn there, and the owner sees why.
async fn app_halted(cx: &TurnContext, st: &TurnState) -> Option<TurnExit> {
    let h = &cx.harness;
    if !h.hooks.has_subscribers("agent.should_continue") {
        return None;
    }
    let has_active_task = h
        .store
        .list_task_items(&format!("session:{}", cx.session_id))
        .unwrap_or_default()
        .iter()
        .any(|t| t.status == "pending" || t.status == "in_progress");
    let reason =
        turn_end::app_halt(&h.hooks, &cx.session_id, st.step + 1, &st.round.called_tools, has_active_task).await?;
    info!(session_id = %cx.session_id, step = st.step + 1, reason = %reason, "an app halted the turn");
    let notice = if reason.is_empty() {
        "Stopped: an app asked for this work to stop.".to_string()
    } else {
        format!("Stopped: an app asked for this work to stop ({reason}).")
    };
    let _ = cx.tx.send(StreamEvent::control_notice(notice, "app_halted")).await;
    Some(TurnExit::AppHalted { reason })
}

/// Queue what happened since the last step: files changed outside the
/// turn, new diagnostics, the date rolling over, a changed tool or skill
/// listing, the task reminder, the app hook's text.
async fn step_events(
    cx: &TurnContext,
    st: &mut TurnState,
    conversation: &[ChatMessage],
    listing: Option<tool_surface::ListingDelta>,
    declared: &[ai::ToolDefinition],
) {
    let h = &cx.harness;
    let tools = h.tools.clone();
    let key = cx.request.session_key.clone();
    match tokio::task::spawn_blocking(move || (tools.external_edit_notes(&key), tools.new_diagnostics_note())).await {
        Ok((changed, diagnostics)) => {
            st.reminders.add(&TurnEvent::FilesChanged(changed));
            st.reminders.add(&TurnEvent::Diagnostics(diagnostics.into_iter().collect()));
        }
        Err(e) => warn!(error = %e, "the outside-edit sweep panicked; skipped this step"),
    }

    // The session's facts: the whole snapshot when the conversation was told
    // nothing since its boundary, then one row per fact that changed.
    let memory = super::memory_context::load_employee_memory(
        &h.store,
        &cx.seat.memory.user_id,
        cx.agent_id(),
        &cx.seat.inherit_scopes,
        &cx.name,
    );
    let facts = events::SessionFacts {
        identity: cx.identity.clone(),
        activity: cx.workflow().map(|m| m.instructions.clone()).unwrap_or_default(),
        date: sections::owner_today(cx.timezone.as_deref()),
        timezone: cx.timezone.clone(),
        environment: cx.environment.clone(),
        mode: cx.mode_facts.clone(),
        employee_memory: memory.section,
        session_context: cx.session_context.clone(),
        channel_rules: cx.channel_rules.clone(),
        coworker_access: cx.coworker_access.clone(),
    };
    for event in events::session_fact_events(&facts, conversation) {
        st.reminders.add(&event);
    }
    // The owner's phone position, read every step: a reading shared with
    // this employee is told when it is new, and turning sharing off is told
    // at the next step. Never shared into a turn a stranger or another
    // program started.
    let shared = cx
        .request
        .seat
        .origin
        .is_trusted()
        .then(|| h.phone_locations.reading_for(&cx.request.seat.agent_id, chrono::Utc::now().timestamp()))
        .flatten();
    if let Some(event) = events::phone_location_event(shared, conversation) {
        st.reminders.add(&event);
    }
    let team: events::Listing = h
        .store
        .list_agents(100, 0)
        .unwrap_or_default()
        .into_iter()
        .filter(|a| a.is_enabled == 1 && a.name != cx.name)
        .map(|a| (a.name, a.description))
        .collect();
    if let Some(delta) = events::LinedDelta::between(&events::announced("agents_listing", conversation), &team) {
        st.reminders.add(&TurnEvent::AgentsListing(delta));
    }
    let teams: events::Listing = h
        .store
        .list_teams()
        .unwrap_or_default()
        .into_iter()
        .map(|t| {
            let roster = tools::team::member_roster(&h.store, &t);
            let lead = tools::team::lead_of(&t).and_then(|id| roster.iter().find(|(m, _)| *m == id).map(|(_, name)| name.as_str()));
            let members: Vec<String> = roster.iter().map(|(_, name)| name.clone()).collect();
            let line = events::team_line(&t.mission, lead, &members);
            (t.name, line)
        })
        .collect();
    if let Some(delta) = events::LinedDelta::between(&events::announced("teams_listing", conversation), &teams) {
        st.reminders.add(&TurnEvent::TeamsListing(delta));
    }
    st.recall.land(&mut st.reminders, &mut st.surfaced_memories, &h.store);

    if let Some(delta) = listing {
        st.reminders.add(&TurnEvent::ToolsAvailable(delta));
    }
    if let (Some(loader), None) = (h.skill_loader.as_ref(), cx.workflow()) {
        let scope = (!cx.agent_id().is_empty()).then_some(cx.agent_id());
        let now: events::Listing = loader.listing(scope).await;
        let announced = events::announced("skill_listing", conversation);
        if let Some(delta) = events::LinedDelta::between(&announced, &now) {
            st.reminders.add(&TurnEvent::SkillListing(delta));
        }
    }
    // The helper types this run can start and when each fits: the helper
    // listing, a delta row rather than tool text so the tools array stays
    // the same everywhere and the cached prefix survives a change to it.
    let helper_types = super::delegation::helper_types(&cx.request.mode);
    if let Some(delta) = events::LinedDelta::between(&events::announced("helper_types", conversation), &helper_types) {
        st.reminders.add(&TurnEvent::HelperTypes(delta));
    }

    let task_tools_declared = declared.iter().any(|d| events::TASK_TOOLS.contains(&d.name.as_str()));
    if task_tools_declared && events::task_reminder_due(conversation) {
        let tasks = h
            .store
            .list_task_items(&format!("session:{}", cx.session_id))
            .unwrap_or_default()
            .into_iter()
            .map(|t| events::WorkTaskLine {
                subject: t.description.unwrap_or(t.prompt),
                status: t.status,
            })
            .collect();
        st.reminders.add(&TurnEvent::TasksIdle(tasks));
    }

    if h.hooks.has_subscribers("steering.generate") {
        let payload = serde_json::to_vec(&crate::hooks::SteeringGeneratePayload {
            session_id: cx.session_id.clone(),
            iteration: st.step as usize,
        })
        .unwrap_or_default();
        let (result, _) = h.hooks.apply_filter("steering.generate", payload).await;
        if let Ok(resp) = serde_json::from_slice::<crate::hooks::SteeringGenerateResponse>(&result) {
            for d in resp.directives {
                st.reminders.add(&TurnEvent::AppHook { label: d.label, text: d.content });
            }
        }
    }
}

/// The system prompt and the tool schemas, in tokens: what every request
/// carries besides the conversation.
fn overhead_tokens(declared: &[ai::ToolDefinition]) -> usize {
    let schema_chars: usize = declared.iter().map(|t| t.description.len() + t.input_schema.to_string().len()).sum();
    (prompt::system_prompt().len() + schema_chars) / crate::CHARS_PER_TOKEN
}

/// The per-step trim: every frozen rendering applied, all but the newest
/// screenshots dropped.
fn trim(st: &TurnState, conversation: &[ChatMessage]) -> Vec<ChatMessage> {
    compact::trim::trim(conversation, &st.frozen_renderings).0
}

/// Under context pressure, clear old results their tools let be cleared,
/// each saved through the one spill path, when that saves at least 20k
/// tokens (below that, breaking the cache costs more than it saves). Each
/// rendering is frozen and persisted for
/// the chat. Returns whether anything was cleared.
async fn clear_old_results(cx: &TurnContext, st: &mut TurnState, conversation: &[ChatMessage]) -> bool {
    let h = &cx.harness;
    extend_clearable(&h.tools, conversation, &mut st.trim_checked, &mut st.clearable).await;
    let dir = tools::result_shape::results_dir(&cx.session_id);
    let saved = compact::trim::clear_old_results(conversation, &st.clearable, &mut st.frozen_renderings, |text| {
        tools::result_shape::persist_cleared(&dir, text)
    });
    if saved == 0 {
        return false;
    }
    info!(session_id = %cx.session_id, tokens = saved, "cleared old tool results under context pressure");
    let fresh: Vec<(String, String)> = st
        .frozen_renderings
        .iter()
        .filter(|(k, _)| !st.persisted_renderings.contains(*k))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    if !fresh.is_empty() {
        let chat_id = h.store.resolve_session_chat_id(&cx.session_id);
        match h.store.insert_chat_renderings(&chat_id, &fresh) {
            Ok(()) => st.persisted_renderings.extend(fresh.into_iter().map(|(k, _)| k)),
            Err(e) => warn!(error = %e, "could not persist frozen renderings"),
        }
    }
    true
}

/// The provider and model name of the turn's model.
/// An employee's own model, as its entity config stores it.
struct EmployeeModel {
    /// Its model preference (`provider/model`, or a linked agent's id).
    preference: Option<String>,
    /// A linked employee (hired from a linked bot, or whose preference names
    /// a linked agent): its linked bot answers its turns, or nothing does.
    linked: bool,
}

fn employee_model(store: &db::Store, agent_id: &str) -> EmployeeModel {
    if agent_id.is_empty() {
        return EmployeeModel { preference: None, linked: false };
    }
    let preference = store
        .get_entity_config("agent", agent_id)
        .ok()
        .flatten()
        .and_then(|c| c.model_preference)
        .filter(|m| !m.trim().is_empty());
    let hired_linked = store
        .get_agent(agent_id)
        .ok()
        .flatten()
        .is_some_and(|a| a.kind.as_deref() == Some(ai::providers::linked::ID));
    let linked = hired_linked || preference.as_deref().is_some_and(|m| ai::LinkedProvider::target(m).is_some());
    EmployeeModel { preference, linked }
}

fn model_parts(model: &str) -> (String, String) {
    if model.is_empty() {
        return (String::new(), String::new());
    }
    let (provider, name) = selector::parse_model_id(model);
    (provider.to_string(), name.to_string())
}

/// The step's request: the turn's system prompt, the conversation, the
/// surface.
fn build_request(
    cx: &TurnContext,
    st: &TurnState,
    window: &[ChatMessage],
    declared: Vec<ai::ToolDefinition>,
    model_name: &str,
) -> ChatRequest {
    ChatRequest {
        tool_credential: None,
        // The conversation and the ask door, for a provider that keeps one
        // remote chat per Nebo chat and relays its runtime's own questions
        // (the linked provider).
        chat_id: cx.harness.store.resolve_session_chat_id(&cx.session_id),
        ask_channels: cx.harness.ask_channels.clone(),
        // How much the employee may do without asking, for a runtime that
        // runs its own tools under modes of its own (the linked provider).
        permission_mode: Some(cx.grant.mode),
        tool_choice: Default::default(),
        messages: conversation::convert_messages(window, &st.model),
        tools: declared,
        max_tokens: st.call.max_output_tokens(),
        temperature: if cx.workflow().is_some() { 0.0 } else { 0.7 },
        system: prompt::system_prompt().to_string(),
        model: model_name.to_string(),
        // Set by the turn's model (its speed), so it holds for every step and
        // never changes the cached request mid-turn: on where the model
        // thinks: thinking is on for every model that supports it, since it
        // improves tool use and costs nothing on the cache. The blocks come
        // back with their
        // turn (`conversation::convert_messages`).
        enable_thinking: cx.harness.selector.thinks(&st.model),
        metadata: st.call.sticky_metadata.clone(),
        cache_breakpoints: prompt::cache_breakpoints(),
        cancel_token: Some(cx.request.cancel.clone()),
        trace: match cx.workflow() {
            Some(m) => m.trace.clone(),
            None => RequestTrace {
                agent_id: cx.agent_id().to_string(),
                run_id: cx.progress.run_id.clone(),
                ..RequestTrace::new("agent_turn")
            },
        },
    }
}

/// Checkpoint the conversation: the pre-checkpoint memory flush, the
/// summary forked from the step's request, the boundary row and the restore
/// rows. `heard_through` is the last row the step's conversation was
/// loaded through. The next step loads from the boundary, with every row
/// the summary never read, and is told the session's facts again.
async fn checkpoint(
    cx: &TurnContext,
    st: &mut TurnState,
    conversation: &[ChatMessage],
    heard_through: Option<&str>,
    fork_of: &ChatRequest,
    why: compact::checkpoint::CheckpointReason,
    instructions: Option<&str>,
) -> Result<(), String> {
    let h = &cx.harness;
    let provider = match &st.last_call {
        Some(last) => last.provider.clone(),
        None => ai::default_provider(&h.providers.read().await).ok_or("no provider to checkpoint with")?,
    };
    let taint: Vec<types::provenance::ProvenanceClass> =
        cx.taint.lock().unwrap_or_else(|p| p.into_inner()).iter().copied().collect();
    // Both write under the memory scope, so a run that may not write
    // memory (an isolated employee with no derivable matter, a helper) runs
    // neither.
    let mut hooks: Vec<Box<dyn compact::checkpoint::PreCheckpointHook>> = Vec::new();
    if !cx.seat.memory.writes_disabled {
        hooks.push(Box::new(compact::checkpoint::MemoryFlush {
            // Housekeeping: the background pool, like every memory write.
            provider: h.concurrency.background(provider.clone()),
            store: h.store.clone(),
            user_id: cx.seat.memory.user_id.clone(),
            topics: cx.seat.memory_topics.clone(),
            embedding: h.embedding_provider.clone(),
            barred: taint.iter().any(|c| cx.seat.write_bar.contains(c)),
            taint,
            window_tokens: h.selector.context_window(&st.model),
        }));
        if let Some(embedding) = h.embedding_provider.clone() {
            hooks.push(Box::new(compact::checkpoint::TranscriptIndex {
                store: h.store.clone(),
                embedding,
                user_id: cx.seat.memory.user_id.clone(),
            }));
        }
    }
    let goal = goal::GoalStore::new(&h.sessions, &cx.session_id).active().ok().flatten();
    let running = running_work(cx).await;
    let outcome = compact::checkpoint::checkpoint(
        &compact::checkpoint::CheckpointContext {
            sessions: &h.sessions,
            provider: provider.as_ref(),
            session_id: &cx.session_id,
            conversation,
            heard_through,
            fork_of,
            hooks: &hooks,
            restore: compact::restore::RestoreState {
                goal: goal.as_ref(),
                running: &running,
                plan_mode: cx.plan_mode(),
            },
            instructions,
            fit_under: (why != compact::checkpoint::CheckpointReason::OwnerAsked).then(|| {
                compact::checkpoint::Trigger::threshold(
                    h.selector.context_window(&st.model),
                    usize::try_from(fork_of.max_tokens).unwrap_or_default(),
                )
            }),
            overhead_tokens: st.usage.system_overhead_tokens,
        },
        why,
    )
    .await;
    st.trigger.record(&outcome);
    outcome?;
    st.checkpoints += 1;
    st.seen.clear();
    Ok(())
}

/// The work this session started that is still running, told again after a
/// checkpoint so the model doesn't start it twice: its helpers and its
/// background commands.
async fn running_work(cx: &TurnContext) -> Vec<compact::restore::RunningWork> {
    let h = &cx.harness;
    let mut running = h.goal_observer().map(|o| o.background(&cx.session_id)).unwrap_or_default();
    for (session, caller) in h.tools.process_registry().running_for(&cx.request.session_key).await {
        running.push(compact::restore::RunningWork {
            id: session.id.clone(),
            description: caller.description,
            kind: compact::restore::WorkKind::Command { command: session.command.clone() },
        });
    }
    running
}

/// The app hook that may rewrite the reply before it is stored.
async fn post_receive(cx: &TurnContext, text: String, tool_calls: usize) -> String {
    let hooks = &cx.harness.hooks;
    if !hooks.has_subscribers("message.post_receive") {
        return text;
    }
    let payload = serde_json::to_vec(&crate::hooks::PostReceivePayload {
        response_text: text.clone(),
        tool_calls_count: tool_calls,
    })
    .unwrap_or_default();
    let (result, _) = hooks.apply_filter("message.post_receive", payload).await;
    serde_json::from_slice::<crate::hooks::PostReceiveResponse>(&result)
        .ok()
        .and_then(|r| r.response_text)
        .unwrap_or(text)
}

/// Store the reply with its tool calls, its block order (each text block
/// with its verdict) and its thinking blocks with the model that wrote them.
async fn save_reply(
    cx: &TurnContext,
    folds: &mut text_fold::TurnFolds,
    text: &str,
    tool_calls: &[ai::ToolCall],
    block_order: &[Block],
    (thinking, thinking_model): (&[ai::ThinkingBlock], &str),
    heard_through: Option<&str>,
) {
    if text.is_empty() && tool_calls.is_empty() {
        return;
    }
    let calls = (!tool_calls.is_empty()).then(|| serde_json::to_string(tool_calls).ok()).flatten();
    let mut metadata = serde_json::Map::new();
    if block_order.len() > 1 || matches!(block_order.first(), Some(Block::Tool(_))) {
        let blocks: Vec<serde_json::Value> = block_order
            .iter()
            .map(|block| match block {
                Block::Tool(i) => serde_json::json!({"type": "tool", "toolCallIndex": i}),
                Block::Text(Some(fold)) => serde_json::json!({"type": "text", "fold": fold.as_str()}),
                Block::Text(None) => serde_json::json!({"type": "text"}),
            })
            .collect();
        metadata.insert("contentBlocks".into(), serde_json::json!(blocks));
    }
    conversation::mark_thinking(&mut metadata, thinking, thinking_model);
    if let Some(id) = heard_through {
        metadata.insert(conversation::HEARD_THROUGH.into(), serde_json::json!(id));
    }
    let metadata = (!metadata.is_empty()).then(|| serde_json::Value::Object(metadata).to_string());
    let h = &cx.harness;
    match h.sessions.append_message(&cx.session_id, "assistant", text, calls.as_deref(), None, metadata.as_deref()) {
        Ok(row) => {
            // The segment this reply's tool call closed is stored here.
            if let Some(block) = block_order.iter().rposition(|b| matches!(b, Block::Text(Some(_)))) {
                folds.stored(text_fold::StoredRow { message_id: row.id, block });
            }
        }
        Err(e) => warn!(session_id = %cx.session_id, error = %e, "failed to save the reply"),
    }
    if h.hooks.has_subscribers("session.message_append") {
        let payload = serde_json::to_vec(&crate::hooks::MessageAppendPayload {
            session_id: cx.session_id.clone(),
            role: "assistant".to_string(),
            content: text.to_string(),
        })
        .unwrap_or_default();
        h.hooks.do_action("session.message_append", payload).await;
    }
}

/// Run the reply's tool calls. `Some` ends the turn.
async fn tool_round(
    cx: &TurnContext,
    st: &mut TurnState,
    round_cx: &RoundContext<'_>,
    executor: ToolExecutor<'_>,
    text: &str,
    tool_calls: &mut [ai::ToolCall],
) -> Option<TurnExit> {
    let h = &cx.harness;
    let carry = &mut st.round;
    let outcome = tool_round::run_tool_round(
        round_cx,
        RoundState {
            called_tools: &mut carry.called_tools,
            plan_touch: &mut carry.plan_touch,
            edits_since_check: &mut carry.edits_since_check,
            last_desktop_act: &mut carry.last_desktop_act,
        },
        executor,
        tool_calls,
    )
    .await;
    let results = match outcome {
        RoundOutcome::Ran(results) => results,
        RoundOutcome::Cancelled => return Some(TurnExit::Cancelled),
        RoundOutcome::Workflow(reason) if reason == "awaiting_approval" => {
            return Some(TurnExit::AwaitingApproval);
        }
        RoundOutcome::Workflow(reason) => return Some(TurnExit::WorkflowEnded(reason)),
        // The round sent the owner its notice (with the need a tool named).
        RoundOutcome::Terminal => {
            return Some(TurnExit::TerminalTool {
                notice: "terminal_tool_error".into(),
                need: None,
            });
        }
    };
    for tc in tool_calls.iter() {
        if let Some(class) = h.tools.get(&tc.name).await.and_then(|t| t.taint(&tc.input)) {
            cx.taint.lock().unwrap_or_else(|p| p.into_inner()).insert(class);
        }
    }
    if let Ok(mut current) = cx.progress.current_tool.lock() {
        current.clear();
    }
    if h.hooks.has_subscribers("agent.turn") {
        let payload = serde_json::to_vec(&crate::hooks::TurnPayload {
            session_id: cx.session_id.clone(),
            turn: st.step as usize,
            tool_calls: tool_calls.iter().map(|tc| tc.name.clone()).collect(),
            total_tool_calls: st.round.called_tools.clone(),
            has_active_task: false,
        })
        .unwrap_or_default();
        h.hooks.do_action("agent.turn", payload).await;
    }
    super::after_turn::hand_off_tool_summary(
        &cx.session_id,
        &h.providers,
        &cx.tx,
        text,
        results.summary_tool_calls,
        results.summary_tool_results,
        cx.trace("tool_summary"),
    )
    .await;
    None
}

/// Turn end: `None` when every check lets the turn end, `Ok` to take
/// another step, `Err` to end it with that exit.
async fn end_checks(cx: &TurnContext, st: &mut TurnState) -> Option<Result<(), TurnExit>> {
    let h = &cx.harness;
    let goal = match (&cx.request.mode, h.goal_observer()) {
        (TurnMode::Chat, Some(observer)) => Some(goal::GoalCheck {
            sessions: h.sessions.clone(),
            session_id: cx.session_id.clone(),
            judge: goal::DoneJudge::for_providers(&h.providers.read().await, &h.selector),
            trace: cx.trace("done_check"),
            observer,
            check_ins: h.goal_check_ins.clone(),
        }),
        _ => None,
    };
    let workflow_contract = cx.workflow().map(|m| m.contract.clone());
    let checks = turn_end::registry(&cx.request.mode, turn_end::EndChecks { goal, workflow_contract });
    if checks.is_empty() {
        return None;
    }
    // The conversation the model just answered, the answer included.
    let transcript =
        conversation::convert_messages(&h.sessions.get_messages_since_checkpoint(&cx.session_id).unwrap_or_default(), "");
    let end = turn_end::TurnEnd {
        transcript: &transcript,
        step: st.step,
        checks_this_turn: st.end_checks_this_turn,
    };
    for check in checks {
        match check.check(&end).await {
            EndVerdict::Stop => {}
            EndVerdict::Exit(exit) => return Some(Err(exit)),
            EndVerdict::Continue(event) => {
                st.end_checks_this_turn += 1;
                let reason = events::attachment_for(&event).map(|a| a.text).unwrap_or_default();
                st.reminders.add(&event);
                st.transition = Transition::EndCheckContinue {
                    check: check.name(),
                    reason,
                };
                return Some(Ok(()));
            }
        }
    }
    None
}

// ── Finish ───────────────────────────────────────────────────────────────

/// After the turn: the interrupt record, the usage row, the context line and
/// the background work.
pub(crate) async fn finish(cx: &TurnContext, st: &mut TurnState, exit: &TurnExit) {
    let h = &cx.harness;
    show_a_folded_answer(cx, &mut st.folds).await;
    if *exit == TurnExit::Cancelled {
        let why = if cx.progress.stalled.load(std::sync::atomic::Ordering::SeqCst) {
            conversation::Interrupt::Stalled
        } else {
            conversation::Interrupt::Owner
        };
        conversation::record_interrupt(&h.sessions, &cx.session_id, why);
    }
    info!(
        session_id = %cx.session_id,
        exit = %exit.label(),
        steps = st.step,
        checkpoints = st.checkpoints,
        attachments = %st.reminders.tally(),
        "turn ended"
    );
    usage::record_run_usage(&h.store, &h.selector, cx.agent_id(), &cx.request.session_key, &st.model, &st.usage, &exit.label());
    // The chat is named after its first exchange and again at its third,
    // however the turn ended: one the owner stopped is named too.
    if matches!(cx.request.mode, TurnMode::Chat) {
        super::after_turn::spawn_chat_title_generation(
            h.providers.clone(),
            h.store.clone(),
            h.sessions.active_chat_id(&cx.session_id),
            cx.session_id.clone(),
            h.selector.background_model(),
            h.title_sink(),
        );
    }
    if *exit == TurnExit::Cancelled {
        return;
    }
    // A provider that takes no call it did not build (the linked one: its
    // runtime would read the recap's instruction as the owner's message)
    // gets no recap.
    if owner_in_turn(&cx.request)
        && let Some(last) = st.last_call.take()
        && last.provider.retryable()
    {
        spawn_recap(cx, last);
    }
    if !cx.after_turn {
        return;
    }
    // The goal this turn worked under: still pursued, or settled by this
    // turn's own done check.
    let goal = goal::GoalStore::new(&h.sessions, &cx.session_id)
        .get()
        .ok()
        .flatten()
        .filter(|g| match g.status {
            goal::GoalStatus::Active | goal::GoalStatus::Paused(_) => true,
            goal::GoalStatus::Met | goal::GoalStatus::Impossible => {
                matches!(exit, TurnExit::GoalMet { .. } | TurnExit::GoalImpossible { .. })
            }
            goal::GoalStatus::Cleared => false,
        })
        .map(|g| g.condition);
    super::after_turn::MemoryExtraction {
        sessions: &h.sessions,
        session_id: &cx.session_id,
        providers: &h.providers,
        store: &h.store,
        concurrency: &h.concurrency,
        selector: &h.selector,
        embedding_provider: h.embedding_provider.as_ref(),
        tools: &h.tools,
        memory_user_id: &cx.seat.memory.user_id,
        memory_topics: &cx.seat.memory_topics,
        memory_write_bar: &cx.seat.write_bar,
        run_taint: &cx.taint,
        goal: goal.as_deref(),
        skip_memory: false,
        trace: cx.trace("memory_extract"),
    }
    .schedule()
    .await;
    super::after_turn::spawn_personality_synthesis(&h.store, &h.providers, &cx.seat.memory.user_id, &h.concurrency).await;
    if !matches!(exit, TurnExit::ProviderFailed(_)) {
        super::after_turn::start_review(h, &cx.request, &cx.session_id);
    }
}

/// The turn ended with every text segment folded and no answer after them:
/// the longest is shown, on the stream and where it is stored, so the turn
/// leaves something to read.
async fn show_a_folded_answer(cx: &TurnContext, folds: &mut text_fold::TurnFolds) {
    let Some((segment, row)) = folds.safety_net() else {
        return;
    };
    let shown = text_fold::Fold::Shown.as_str();
    let _ = cx.tx.send(StreamEvent::text_verdict(segment, shown)).await;
    if let Some(row) = row {
        let path = format!("$.contentBlocks[{}].fold", row.block);
        if let Err(e) = cx.harness.store.set_chat_message_metadata(&row.message_id, &path, &serde_json::json!(shown)) {
            warn!(session_id = %cx.session_id, error = %e, "failed to store the shown segment");
        }
    }
}

/// Write the recap of the turn just finished, in the background. The call
/// forks the turn's last request, extended by what was stored after it (the
/// answer), so it reads the turn's cached prefix: the same system prompt,
/// tools, conversation and model, so the recap costs a cache read. A
/// checkpoint taken after the last call replaced the conversation it was
/// built from; that turn has no cached prefix to fork and gets no recap.
fn spawn_recap(cx: &TurnContext, last: LastCall) {
    let h = &cx.harness;
    let stored = conversation::order_as_heard(h.sessions.get_messages_since_checkpoint(&cx.session_id).unwrap_or_default());
    let Some(after) = last
        .heard_through
        .as_deref()
        .and_then(|id| stored.iter().position(|m| m.id == id))
    else {
        info!(session_id = %cx.session_id, "a checkpoint followed the turn's last call: no recap");
        return;
    };
    let mut fork_of = last.request;
    let model = format!("{}/{}", last.provider.id(), fork_of.model);
    fork_of.messages.extend(conversation::convert_messages(&stored[after + 1..], &model));
    let recap = super::recap::RecapRequest {
        chat_id: h.sessions.active_chat_id(&cx.session_id),
        turn_id: cx.progress.run_id.clone(),
        fork_of,
        provider: last.provider,
    };
    tokio::spawn(super::recap::write_recap(h.store.clone(), h.concurrency.clone(), h.broadcast(), recap));
}

/// Add every stored tool call not yet `checked` whose tool says its result
/// may be cleared under pressure (`DynTool::clearable`) to `clearable`.
/// A call to a tool no longer registered is never cleared.
async fn extend_clearable(tools: &tools::Registry, messages: &[ChatMessage], checked: &mut HashSet<String>, clearable: &mut compact::trim::Clearable) {
    for msg in messages.iter().filter(|m| m.role == "assistant") {
        let Some(calls) = msg
            .tool_calls
            .as_deref()
            .and_then(|j| serde_json::from_str::<Vec<ai::ToolCall>>(j).ok())
        else {
            continue;
        };
        for call in calls {
            if !checked.insert(call.id.clone()) {
                continue;
            }
            if let Some(tool) = tools.get(&call.name).await
                && tool.clearable(&call.input)
            {
                clearable.insert(call.id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! The turn end to end against a scripted model: every main-loop call is
    //! recorded and answered from the script; side calls answer "ok".

    use std::collections::VecDeque;
    use std::future::Future;
    use std::pin::Pin;

    use super::*;
    use crate::harness::{Delivery, SeatRequest};

    type Hook = Pin<Box<dyn Future<Output = ()> + Send>>;

    enum Step {
        Say(&'static str),
        Call(&'static str, serde_json::Value),
        /// Text, then a call to this tool.
        Narrated(&'static str, &'static str),
        /// Text the output cap cut off.
        Cut(&'static str),
        /// A dropped connection.
        Transient,
        /// The provider says the request is over the window.
        Overflow,
        /// The step, with what the call cost in microdollars.
        Paid(Box<Step>, i64),
        /// Run the hook while the call is in flight, then answer.
        During(Box<Step>, Hook),
        /// Stream these calls, then hold the reply open until a tool starts
        /// (or half a second passes) before ending it.
        Held(Vec<(&'static str, serde_json::Value)>, Arc<Probe>),
        /// The step, its first token this long in coming.
        Slow(Box<Step>, std::time::Duration),
        /// The step, after a whole thinking block.
        Thought(Box<Step>, ai::ThinkingBlock),
    }

    /// What the probe tools saw: each call's tool, and whether it started
    /// while the reply was still streaming.
    #[derive(Default)]
    struct Probe {
        reply_ended: std::sync::atomic::AtomicBool,
        started: tokio::sync::Notify,
        seen: Mutex<Vec<(&'static str, bool)>>,
    }

    struct Probed {
        name: &'static str,
        read_only: bool,
        probe: Arc<Probe>,
    }

    impl tools::registry::DynTool for Probed {
        fn name(&self) -> &str {
            self.name
        }
        fn description(&self) -> String {
            format!("{} things", self.name)
        }
        fn schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object", "properties": {}})
        }
        fn read_only(&self, _input: &serde_json::Value) -> bool {
            self.read_only
        }
        fn execute_dyn<'a>(
            &'a self,
            _ctx: &'a tools::ToolContext,
            _input: serde_json::Value,
        ) -> Pin<Box<dyn Future<Output = tools::ToolResult> + Send + 'a>> {
            Box::pin(async move {
                let streaming = !self.probe.reply_ended.load(std::sync::atomic::Ordering::SeqCst);
                self.probe.seen.lock().unwrap().push((self.name, streaming));
                self.probe.started.notify_one();
                tools::ToolResult::ok(format!("{} ran", self.name))
            })
        }
    }

    #[derive(Default)]
    struct Scripted {
        script: Mutex<VecDeque<Step>>,
        calls: Mutex<Vec<ChatRequest>>,
        /// The done check's answers, in order.
        verdicts: Mutex<VecDeque<&'static str>>,
        /// Every side call (title, recap, memory, …), in order.
        side: Mutex<Vec<ChatRequest>>,
        /// Run while the next checkpoint's summary call is in flight.
        during_checkpoint: Mutex<Option<Hook>>,
    }

    impl Scripted {
        fn new(steps: Vec<Step>) -> Arc<Self> {
            Arc::new(Self {
                script: Mutex::new(steps.into()),
                ..Default::default()
            })
        }

        fn calls(&self) -> Vec<ChatRequest> {
            self.calls.lock().unwrap().clone()
        }

        /// The first side call of `purpose`, waiting up to two seconds for
        /// it: side calls run in the background after the turn.
        async fn side_call(&self, purpose: &str) -> Option<ChatRequest> {
            for _ in 0..200 {
                if let Some(req) = self.side.lock().unwrap().iter().find(|r| r.trace.purpose == purpose) {
                    return Some(req.clone());
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            None
        }
    }

    #[async_trait::async_trait]
    impl ai::Provider for Scripted {
        fn id(&self) -> &str {
            "scripted"
        }

        async fn stream(&self, req: &ChatRequest) -> Result<ai::EventReceiver, ai::ProviderError> {
            if req.trace.purpose != "agent_turn" {
                self.side.lock().unwrap().push(req.clone());
            }
            if req.trace.purpose == "done_check" {
                let verdict = self.verdicts.lock().unwrap().pop_front().unwrap_or(r#"{"met": true, "reason": "done"}"#);
                return Ok(events(vec![StreamEvent::text(verdict)], None));
            }
            if req.trace.purpose == "owner_recap" {
                return Ok(events(vec![StreamEvent::text(RECAP)], None));
            }
            let during_checkpoint = (req.trace.purpose == "checkpoint")
                .then(|| self.during_checkpoint.lock().unwrap().take())
                .flatten();
            if let Some(hook) = during_checkpoint {
                hook.await;
            }
            if req.trace.purpose != "agent_turn" {
                return Ok(events(vec![StreamEvent::text("ok")], None));
            }
            self.calls.lock().unwrap().push(req.clone());
            let mut step = self.script.lock().unwrap().pop_front().expect("a call the script did not expect");
            if let Step::During(inner, hook) = step {
                hook.await;
                step = *inner;
            }
            if let Step::Held(calls, probe) = step {
                let (tx, rx) = mpsc::channel(calls.len() + 1);
                tokio::spawn(async move {
                    for (name, input) in calls {
                        let call = ai::ToolCall { id: format!("call-{name}"), name: name.into(), input };
                        let _ = tx.send(StreamEvent::tool_call(call)).await;
                    }
                    let _ = tokio::time::timeout(std::time::Duration::from_millis(500), probe.started.notified()).await;
                    probe.reply_ended.store(true, std::sync::atomic::Ordering::SeqCst);
                    let _ = tx.send(StreamEvent::done()).await;
                });
                return Ok(rx);
            }
            if let Step::Slow(inner, delay) = step {
                let (list, stop) = answer(*inner)?;
                let (tx, rx) = mpsc::channel(list.len() + 1);
                tokio::spawn(async move {
                    tokio::time::sleep(delay).await;
                    let mut replay = events(list, stop);
                    while let Some(e) = replay.recv().await {
                        let _ = tx.send(e).await;
                    }
                });
                return Ok(rx);
            }
            let (list, stop) = answer(step)?;
            Ok(events(list, stop))
        }
    }

    fn answer(step: Step) -> Result<(Vec<StreamEvent>, Option<&'static str>), ai::ProviderError> {
        Ok(match step {
            Step::Say(text) => (vec![StreamEvent::text(text)], None),
            Step::Call(name, input) => (
                vec![StreamEvent::tool_call(ai::ToolCall {
                    id: format!("call-{}", uuid::Uuid::new_v4()),
                    name: name.into(),
                    input,
                })],
                None,
            ),
            Step::Narrated(text, name) => (
                vec![
                    StreamEvent::text(text),
                    StreamEvent::tool_call(ai::ToolCall {
                        id: format!("call-{}", uuid::Uuid::new_v4()),
                        name: name.into(),
                        input: serde_json::json!({}),
                    }),
                ],
                None,
            ),
            Step::Cut(text) => (vec![StreamEvent::text(text)], Some("max_tokens")),
            Step::Transient => return Err(ai::ProviderError::Request("connection reset".into())),
            Step::Overflow => return Err(ai::ProviderError::ContextOverflow),
            Step::Paid(inner, microdollars) => {
                let (mut list, stop) = answer(*inner)?;
                list.push(StreamEvent::usage(ai::UsageInfo {
                    input_tokens: 10,
                    output_tokens: 5,
                    cost_microdollars: Some(microdollars),
                    ..Default::default()
                }));
                (list, stop)
            }
            Step::Thought(inner, block) => {
                let (mut list, stop) = answer(*inner)?;
                list.insert(0, StreamEvent::thinking_block(block));
                (list, stop)
            }
            Step::During(..) | Step::Held(..) | Step::Slow(..) => unreachable!("answered in stream"),
        })
    }

    fn events(mut list: Vec<StreamEvent>, stop: Option<&str>) -> ai::EventReceiver {
        list.push(match stop {
            Some(stop) => StreamEvent::done_with_reason(stop),
            None => StreamEvent::done(),
        });
        let (tx, rx) = mpsc::channel(list.len());
        for e in list {
            tx.try_send(e).expect("room for the scripted events");
        }
        rx
    }

    /// A read-only tool that echoes, and a deferred one `find_tools` loads.
    struct Echo {
        name: &'static str,
        deferred: bool,
        read_only: bool,
    }

    impl tools::registry::DynTool for Echo {
        fn name(&self) -> &str {
            self.name
        }
        fn description(&self) -> String {
            format!("{} things", self.name)
        }
        fn schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object", "properties": {}})
        }
        fn should_defer(&self) -> bool {
            self.deferred
        }
        fn read_only(&self, _input: &serde_json::Value) -> bool {
            self.read_only
        }
        fn execute_dyn<'a>(
            &'a self,
            _ctx: &'a tools::ToolContext,
            _input: serde_json::Value,
        ) -> Pin<Box<dyn Future<Output = tools::ToolResult> + Send + 'a>> {
            Box::pin(async move { tools::ToolResult::ok(format!("{} ran", self.name)) })
        }
    }

    async fn harness(model: &Arc<Scripted>) -> Harness {
        harness_with(model, Vec::new()).await
    }

    async fn harness_with(model: &Arc<Scripted>, extra: Vec<Box<dyn tools::registry::DynTool>>) -> Harness {
        harness_selecting(model, extra, crate::selector::ModelSelector::new(Default::default())).await
    }

    async fn harness_selecting(
        model: &Arc<Scripted>,
        extra: Vec<Box<dyn tools::registry::DynTool>>,
        selector: crate::selector::ModelSelector,
    ) -> Harness {
        let path = std::env::temp_dir().join(format!("nebo-turn-{}.db", uuid::Uuid::new_v4()));
        let store = Arc::new(db::Store::new(path.to_str().unwrap()).expect("store"));
        let registry = Arc::new(tools::Registry::new(Arc::new(crate::harness::permissions::Check::new(store.clone()))));
        registry.register(Box::new(Echo { name: "echo", deferred: false, read_only: true })).await;
        registry.register(Box::new(Echo { name: "weather", deferred: true, read_only: true })).await;
        registry.register(Box::new(Echo { name: "writer", deferred: false, read_only: false })).await;
        registry.register(Box::new(Echo { name: "delegate", deferred: false, read_only: true })).await;
        registry.register(Box::new(tools::find_tools::FindToolsTool::new(registry.clone()))).await;
        registry.register(Box::new(tools::ExitTool::new())).await;
        for tool in extra {
            registry.register(tool).await;
        }
        Harness::new(
            store,
            registry,
            vec![model.clone() as Arc<dyn ai::Provider>],
            selector,
            Arc::new(crate::concurrency::ConcurrencyController::new(Some(4))),
            Arc::new(napp::HookDispatcher::new()),
            None,
            Default::default(),
            None,
        )
    }

    const KEY: &str = "agent:ops:web";
    const RECAP: &str = "You asked for the plan; it is drafted. Next: review it.";

    fn owner(text: &str) -> TurnRequest {
        TurnRequest {
            session_key: KEY.into(),
            input: TurnInput::Owner {
                text: text.into(),
                images: Vec::new(),
                attachments: Vec::new(),
            },
            seat: SeatRequest {
                agent_id: String::new(),
                user_id: String::new(),
                origin: tools::Origin::User,
                door: types::permissions::Door::Chat,
                mode: Some(Mode::FullAccess),
                ceiling: None,
                cwd: None,
                seed_taint: Vec::new(),
                audience: None,
                tool_allowlist: None,
                tool_denial_hint: None,
                handoff_depth: 0,
                model_override: String::new(),
                model_preference: None,
                personality_snippet: None,
                tool_scope: None,
            },
            mode: TurnMode::Chat,
            delivery: Delivery {
                channel: "web".into(),
                channel_ctx: None,
                mention_briefing: None,
            },
            cancel: tokio_util::sync::CancellationToken::new(),
            progress: None,
        }
    }

    /// Run the turn to its last event; returns every event.
    async fn run_turn(h: &Harness, req: TurnRequest) -> Vec<StreamEvent> {
        let mut handle = h.start_turn(req).await.expect("start");
        let mut seen = Vec::new();
        while let Some(e) = handle.events.recv().await {
            seen.push(e);
        }
        for _ in 0..200 {
            if !h.is_session_busy(KEY) {
                return seen;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("the turn never released its session");
    }

    fn exit_of(events: &[StreamEvent]) -> String {
        let done: Vec<&StreamEvent> = events.iter().filter(|e| e.event_type == ai::StreamEventType::Done).collect();
        assert_eq!(done.len(), 1, "one Done per turn task");
        done[0].stop_reason.clone().unwrap_or_default()
    }

    fn stored(h: &Harness) -> Vec<ChatMessage> {
        let sid = h.sessions.resolve_session_id_by_key(KEY).expect("session");
        h.store.get_chat_messages(&h.sessions.active_chat_id(&sid)).expect("rows")
    }

    fn kinds(rows: &[ChatMessage]) -> Vec<String> {
        rows.iter().filter_map(reminders::attachment_kind).collect()
    }

    fn texts(req: &ChatRequest) -> Vec<String> {
        req.messages.iter().map(|m| m.content.clone()).collect()
    }

    /// The verdicts the stream gave, in order: (segment, fold).
    fn verdicts(events: &[StreamEvent]) -> Vec<(u64, String)> {
        events
            .iter()
            .filter(|e| e.event_type == ai::StreamEventType::TextVerdict)
            .map(|e| (e.payload.as_ref().and_then(|p| p["segment"].as_u64()).unwrap(), e.text.clone()))
            .collect()
    }

    /// The folds stored on the assistant rows' text blocks, in order.
    fn stored_folds(h: &Harness) -> Vec<String> {
        stored(h)
            .iter()
            .filter(|m| m.role == "assistant")
            .filter_map(|m| m.metadata.as_deref().and_then(|j| serde_json::from_str::<serde_json::Value>(j).ok()))
            .flat_map(|meta| {
                meta["contentBlocks"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|b| b["fold"].as_str().map(str::to_string))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// Text between calls gets its verdict before the call that closed it,
    /// on the stream and stored with its row: the owner's example stays in
    /// the reply deep in the turn, a short next step folds, the answer has
    /// none.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn text_between_calls_is_shown_or_folded_and_stored() {
        const REPORT: &str = "That worked — the workflow was created with 2 activities and proper steps. The problem is \
            that update_employee with automations stored them as metadata… I need to recreate all 10 workflows properly \
            through create_workflow. Let me delete the bad ones first, then rebuild";
        let model = Scripted::new(vec![
            Step::Narrated("Your workflows are stored as metadata.", "echo"),
            Step::Call("echo", serde_json::json!({})),
            Step::Call("echo", serde_json::json!({})),
            Step::Call("echo", serde_json::json!({})),
            Step::Narrated("Let me check the workflows.", "echo"),
            Step::Narrated(REPORT, "echo"),
            Step::Say("Done: all ten are rebuilt."),
        ]);
        let h = harness(&model).await;
        let events = run_turn(&h, owner("Fix my workflows")).await;
        assert_eq!(exit_of(&events), "text_response");
        assert_eq!(
            verdicts(&events),
            vec![(0, "shown".to_string()), (1, "folded".to_string()), (2, "shown".to_string())]
        );
        // Each verdict comes right before the call that closed its segment.
        for (i, e) in events.iter().enumerate() {
            if e.event_type == ai::StreamEventType::TextVerdict {
                assert_eq!(events[i + 1].event_type, ai::StreamEventType::ToolCall);
            }
        }
        assert_eq!(stored_folds(&h), vec!["shown", "folded", "shown"]);
    }

    /// A turn that folded every paragraph and ended with no answer shows the
    /// longest, on the stream and in its stored row.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_turn_that_folded_everything_shows_its_longest_paragraph() {
        let model = Scripted::new(vec![
            Step::Narrated("Let me check the first file.", "echo"),
            Step::Narrated("Now the second file, which holds the rest of the invoices.", "echo"),
            Step::Say(""),
            Step::Say(""),
            Step::Say(""),
            Step::Say(""),
        ]);
        let h = harness(&model).await;
        let events = run_turn(&h, owner("Check the invoices")).await;
        assert_eq!(
            verdicts(&events),
            vec![(0, "folded".to_string()), (1, "folded".to_string()), (1, "shown".to_string())]
        );
        assert_eq!(stored_folds(&h), vec!["folded", "shown"]);
    }

    /// A linked bot's stand-in: answers every turn and records it, or, when
    /// `offline`, says what the linked provider says when its bot can't be
    /// reached.
    #[derive(Default)]
    struct LinkedBot {
        calls: Mutex<Vec<ChatRequest>>,
        offline: bool,
        /// Stops first to ask the owner, the way a runtime asks permission.
        asks: bool,
    }

    #[async_trait::async_trait]
    impl ai::Provider for LinkedBot {
        fn id(&self) -> &str {
            ai::providers::linked::ID
        }
        fn handles_tools(&self) -> bool {
            true
        }
        fn retryable(&self) -> bool {
            false
        }
        async fn stream(&self, req: &ChatRequest) -> Result<ai::EventReceiver, ai::ProviderError> {
            self.calls.lock().unwrap().push(req.clone());
            if self.offline {
                return Ok(events(vec![StreamEvent::error("Could not connect to Hermes. Try again.")], None));
            }
            let mut said = Vec::new();
            if self.asks {
                let options = serde_json::json!([{ "type": "options", "multiSelect": false, "options": ["Allow once", "Deny"] }]);
                said.push(StreamEvent::ask_request("toolu_1", "git status\nShow git status", Some(options)));
            }
            said.push(StreamEvent::text("Hey, Hermes here."));
            Ok(events(said, None))
        }
    }

    const HERMES: &str = "9295191e-0f2d-4254-a3dc-8de2c52d975f";

    /// A harness on `providers`, with Hermes hired from a linked bot.
    fn hired_linked(providers: Vec<Arc<dyn ai::Provider>>) -> Harness {
        let path = std::env::temp_dir().join(format!("nebo-turn-{}.db", uuid::Uuid::new_v4()));
        let store = Arc::new(db::Store::new(path.to_str().unwrap()).expect("store"));
        store.create_agent(HERMES, Some("linked"), "Hermes", "", "---\nname: Hermes\n---\n", "{}", None, None).expect("hire");
        let brain = ai::LinkedProvider::model_id("a736730b-86e3-4a70-9a44-5e51724acf6e", "hermes");
        store.upsert_entity_config("agent", HERMES, &serde_json::json!({ "modelPreference": brain })).expect("brain");
        let registry = Arc::new(tools::Registry::new(Arc::new(crate::harness::permissions::Check::new(store.clone()))));
        let selector = crate::selector::ModelSelector::new(Default::default());
        Harness::new(
            store,
            registry,
            providers,
            selector,
            Arc::new(crate::concurrency::ConcurrencyController::new(Some(4))),
            Arc::new(napp::HookDispatcher::new()),
            None,
            Default::default(),
            None,
        )
    }

    /// The owner's chat with Hermes, with a composer pick that is not his.
    fn to_hermes(text: &str) -> TurnRequest {
        let mut req = owner(text);
        req.seat.agent_id = HERMES.into();
        req.seat.model_override = "janus/nebo-1".into();
        req
    }

    /// A linked employee's turn goes to its linked agent, whatever model the
    /// request named, and no other provider is asked.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_linked_employee_is_answered_by_its_linked_bot() {
        let other = Scripted::new(Vec::new());
        let bot = Arc::new(LinkedBot::default());
        let h = hired_linked(vec![other.clone() as Arc<dyn ai::Provider>, bot.clone() as Arc<dyn ai::Provider>]);
        let events = run_turn(&h, to_hermes("yo")).await;
        let calls = bot.calls.lock().unwrap().clone();
        let purposes: Vec<&str> = calls.iter().map(|c| &*c.trace.purpose).collect();
        assert_eq!(purposes, ["agent_turn"], "{events:?}");
        assert_eq!(calls[0].model, "a736730b-86e3-4a70-9a44-5e51724acf6e/hermes", "the provider's own model id");
        assert!(!calls[0].chat_id.is_empty(), "the conversation rides with the turn");
        assert!(other.calls().is_empty(), "nothing else answers as Hermes");
        assert!(events.iter().any(|e| e.text == "Hey, Hermes here."));
    }

    /// A linked runtime's question reaches the run's events as an ask, the
    /// card every surface (the phone included) shows and answers; the turn
    /// carries the run's ask door to the provider.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_linked_runtimes_question_is_an_ask() {
        let bot = Arc::new(LinkedBot { asks: true, ..LinkedBot::default() });
        let channels: tools::AskChannels = Default::default();
        let h = hired_linked(vec![bot.clone() as Arc<dyn ai::Provider>]).with_ask_channels(channels);
        let events = run_turn(&h, to_hermes("check the repo")).await;
        let ask = events
            .iter()
            .find(|e| e.event_type == ai::StreamEventType::AskRequest)
            .unwrap_or_else(|| panic!("no ask in {events:?}"));
        assert_eq!(ask.error.as_deref(), Some("toolu_1"));
        assert_eq!(ask.text, "git status\nShow git status");
        assert!(bot.calls.lock().unwrap()[0].ask_channels.is_some(), "the ask door rides with the turn");
    }

    /// A linked turn carries none of Nebo's attachments: the owner's words
    /// are the newest user message the linked bot is sent, and no
    /// `<system-reminder>` row is written or sent.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_linked_turn_carries_no_attachments() {
        let bot = Arc::new(LinkedBot::default());
        let h = hired_linked(vec![bot.clone() as Arc<dyn ai::Provider>]);
        run_turn(&h, to_hermes("Hey, how are you?")).await;
        let calls = bot.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
        let reminders: Vec<&str> = calls[0]
            .messages
            .iter()
            .map(|m| m.content.as_str())
            .filter(|c| c.contains("<system-reminder>"))
            .collect();
        assert!(reminders.is_empty(), "{reminders:?}");
        let newest = calls[0].messages.iter().rev().find(|m| m.role == "user").map(|m| m.content.as_str());
        assert_eq!(newest, Some("Hey, how are you?"));
    }

    /// A turn that names no model (a schedule, a coworker's post) still
    /// reaches a linked employee's linked bot: the turn reads the employee's
    /// own model, never the default.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_scheduled_turn_reaches_the_linked_bot_too() {
        let other = Scripted::new(Vec::new());
        let bot = Arc::new(LinkedBot::default());
        let h = hired_linked(vec![other.clone() as Arc<dyn ai::Provider>, bot.clone() as Arc<dyn ai::Provider>]);
        let mut req = owner("check the inbox");
        req.seat.agent_id = HERMES.into();
        req.seat.door = types::permissions::Door::Schedule;
        req.seat.origin = tools::Origin::System;
        run_turn(&h, req).await;
        assert_eq!(bot.calls.lock().unwrap().len(), 1);
        assert!(other.calls().is_empty());
    }

    /// Without its linked bot's provider a linked employee's turn fails in
    /// plain words, and nothing else is asked to answer: Nebo's own model
    /// answering as Hermes would be impersonation.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_linked_employee_without_its_bot_fails_plainly() {
        let other = Scripted::new(Vec::new());
        let h = hired_linked(vec![other.clone() as Arc<dyn ai::Provider>]);
        let events = run_turn(&h, to_hermes("yo")).await;
        let errors: Vec<&str> = events
            .iter()
            .filter(|e| e.event_type == ai::StreamEventType::Error)
            .filter_map(|e| e.error.as_deref())
            .collect();
        assert_eq!(errors, ["Could not connect to Hermes. Try again."], "{events:?}");
        assert!(other.calls().is_empty(), "no other provider answers as Hermes");
        let answered_as_hermes = other.side.lock().unwrap().iter().any(|r| r.trace.purpose == "agent_turn");
        assert!(!answered_as_hermes);
    }

    /// A linked bot that can't be reached is told once, in the linked
    /// provider's words, and the turn ends: no second dial (each one waits
    /// out a 20 s connect timeout), no "empty reply", and no other provider
    /// answers as Hermes.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_unreachable_linked_bot_is_told_once_and_ends_the_turn() {
        let other = Scripted::new(Vec::new());
        let bot = Arc::new(LinkedBot { offline: true, ..Default::default() });
        let h = hired_linked(vec![other.clone() as Arc<dyn ai::Provider>, bot.clone() as Arc<dyn ai::Provider>]);
        let events = run_turn(&h, to_hermes("yo")).await;
        let errors: Vec<&str> = events
            .iter()
            .filter(|e| e.event_type == ai::StreamEventType::Error)
            .filter_map(|e| e.error.as_deref())
            .collect();
        assert_eq!(errors, ["Could not connect to Hermes. Try again."], "{events:?}");
        assert_eq!(bot.calls.lock().unwrap().len(), 1, "the linked bot is dialled once");
        assert_eq!(exit_of(&events), "provider_failed");
        assert!(other.calls().is_empty(), "no other provider answers as Hermes");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn text_reply_ends_the_turn() {
        let model = Scripted::new(vec![Step::Say("Hello.")]);
        let h = harness(&model).await;
        let events = run_turn(&h, owner("Hi")).await;
        assert_eq!(exit_of(&events), "text_response");
        assert_eq!(model.calls().len(), 1, "one call, no continuation");
        let rows = stored(&h);
        let convo: Vec<(&str, &str)> = rows
            .iter()
            .filter(|m| reminders::attachment_kind(m).is_none())
            .map(|m| (m.role.as_str(), m.content.as_str()))
            .collect();
        assert_eq!(convo, [("user", "Hi"), ("assistant", "Hello.")]);
        let call = &model.calls()[0];
        let last_words = call.messages.iter().rev().find(|m| !m.content.starts_with("<system-reminder>")).unwrap();
        assert_eq!(last_words.content, "Hi", "the owner's words, then this step's attachments");
        assert_eq!(call.system, crate::harness::prompt::system_prompt(), "the one system prompt");
    }

    fn probed(probe: &Arc<Probe>) -> Vec<Box<dyn tools::registry::DynTool>> {
        vec![
            Box::new(Probed { name: "look", read_only: true, probe: probe.clone() }),
            Box::new(Probed { name: "change", read_only: false, probe: probe.clone() }),
        ]
    }

    /// The ids of the stored tool results, in the order they were saved.
    fn result_ids(h: &Harness) -> Vec<String> {
        stored(h)
            .iter()
            .filter(|m| m.role == "tool")
            .filter_map(|m| m.tool_results.as_deref())
            .filter_map(|r| serde_json::from_str::<serde_json::Value>(r).ok())
            .filter_map(|v| v[0]["tool_call_id"].as_str().map(str::to_string))
            .collect()
    }

    /// The streaming executor: a concurrency-safe call starts as
    /// soon as its input is complete, while the reply still streams; a call
    /// that changes things waits for the reply. Results keep call order.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_safe_call_starts_while_the_reply_streams() {
        let probe = Arc::new(Probe::default());
        let calls = vec![("look", serde_json::json!({})), ("change", serde_json::json!({}))];
        let model = Scripted::new(vec![Step::Held(calls, probe.clone()), Step::Say("Done.")]);
        let h = harness_with(&model, probed(&probe)).await;
        let events = run_turn(&h, owner("Look, then change it")).await;
        assert_eq!(exit_of(&events), "text_response");
        assert_eq!(*probe.seen.lock().unwrap(), [("look", true), ("change", false)]);
        assert_eq!(result_ids(&h), ["call-look", "call-change"], "results are saved in call order");
    }

    /// An unsafe call holds every call after it: nothing starts during the
    /// reply, and the safe call runs after the change.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_unsafe_call_holds_the_calls_after_it() {
        let probe = Arc::new(Probe::default());
        let calls = vec![("change", serde_json::json!({})), ("look", serde_json::json!({}))];
        let model = Scripted::new(vec![Step::Held(calls, probe.clone()), Step::Say("Done.")]);
        let h = harness_with(&model, probed(&probe)).await;
        let events = run_turn(&h, owner("Change it, then look")).await;
        assert_eq!(exit_of(&events), "text_response");
        assert_eq!(*probe.seen.lock().unwrap(), [("change", false), ("look", false)]);
        assert_eq!(result_ids(&h), ["call-change", "call-look"]);
    }

    /// A Jev served on a local port that answers every decision "stays
    /// inside" and keeps each request's `state`.
    async fn capturing_jev() -> (ai::DecideClient, Arc<Mutex<Vec<serde_json::Value>>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let kept = seen.clone();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let kept = kept.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 8192];
                    let start = loop {
                        let Ok(n) = sock.read(&mut chunk).await else { return };
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                        let text = String::from_utf8_lossy(&buf).into_owned();
                        if let Some(end) = text.find("\r\n\r\n") {
                            let len = text[..end]
                                .lines()
                                .find_map(|l| {
                                    let (k, v) = l.split_once(':')?;
                                    k.eq_ignore_ascii_case("content-length").then(|| v.trim().parse::<usize>().ok())?
                                })
                                .unwrap_or(0);
                            if buf.len() >= end + 4 + len {
                                break end + 4;
                            }
                        }
                    };
                    let req: serde_json::Value = serde_json::from_slice(&buf[start..]).unwrap_or_default();
                    kept.lock().unwrap().push(req["state"].clone());
                    let answers: serde_json::Map<String, serde_json::Value> = req["questions"]
                        .as_object()
                        .map(|q| q.keys().map(|k| (k.clone(), serde_json::json!({"type": "noul", "noul": 0.02}))).collect())
                        .unwrap_or_default();
                    let body = serde_json::json!({"model": "jev-test", "answers": answers, "usage": {}}).to_string();
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                });
            }
        });
        let client = ai::DecideClient::new(&format!("http://{addr}"), || Some(ai::Bearer { token: "t".into(), bot_id: None }));
        (client, seen)
    }

    /// D12 (PRD-Permissions §4.7): a call whose outward effect the code
    /// can't decide is judged by Jev first, reading the owner's message
    /// and the session's goal.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_permission_judge_asks_jev_with_the_owners_words_and_the_goal() {
        let model = Scripted::new(vec![Step::Call("writer", serde_json::json!({})), Step::Say("Done.")]);
        let (jev, seen) = capturing_jev().await;
        let h = harness(&model).await.with_decide(Arc::new(jev));
        let sid = h.sessions.get_or_create(KEY, "").unwrap().id;
        goal::GoalStore::new(&h.sessions, &sid)
            .set("the Rivera listing is updated", goal::GoalSource::OwnerCommand)
            .unwrap();
        let mut req = owner("OWNER-D12 update the Rivera listing");
        req.seat.mode = Some(Mode::Automatic);
        let events = run_turn(&h, req).await;
        assert_eq!(exit_of(&events), "text_response");
        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 1, "Jev was asked once, about the writer call: {seen:?}");
        assert_eq!(seen[0]["last_user_message"], "OWNER-D12 update the Rivera listing");
        assert_eq!(seen[0]["objective"], "the Rivera listing is updated");
        assert_eq!(seen[0]["calls"][0]["tool"], "writer");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn tool_round_then_answer() {
        let model = Scripted::new(vec![Step::Call("echo", serde_json::json!({})), Step::Say("It echoed.")]);
        let h = harness(&model).await;
        let events = run_turn(&h, owner("Echo something")).await;
        assert_eq!(exit_of(&events), "text_response");
        let calls = model.calls();
        assert_eq!(calls.len(), 2);
        assert!(calls[0].tools.iter().any(|t| t.name == "echo"), "the core tool is declared");
        let last = calls[1].messages.last().unwrap();
        assert_eq!(last.role, "tool");
        assert!(last.tool_results.as_ref().unwrap().to_string().contains("echo ran"), "the result reaches the next call");
        assert!(stored(&h).iter().any(|m| m.role == "assistant" && m.content == "It echoed."));
    }

    /// A dropped call is taken again with the same rows: the step's
    /// attachments were stored once, before the first call.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn transient_retry_resends_the_same_rows() {
        let model = Scripted::new(vec![Step::Transient, Step::Say("Back.")]);
        let h = harness(&model).await;
        let mut req = owner("Where were we?");
        req.delivery.mention_briefing = Some("Team Ops: Ava leads.".into());
        let events = run_turn(&h, req).await;
        assert_eq!(exit_of(&events), "text_response");
        let calls = model.calls();
        assert_eq!(calls.len(), 2, "the dropped call and its retry");
        assert_eq!(texts(&calls[0]), texts(&calls[1]), "the retry resends the same rows");
        assert!(texts(&calls[0]).iter().any(|t| t.contains("Team Ops: Ava leads.")), "the briefing is a row");
        assert_eq!(kinds(&stored(&h)).iter().filter(|k| *k == "run_briefing").count(), 1, "written once");
    }

    /// A dropped model connection is retried silently: nothing on the
    /// turn's stream reads as a stop, so no screen ends the turn early.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_reconnect_is_silent() {
        let model = Scripted::new(vec![Step::Transient, Step::Say("Back.")]);
        let h = harness(&model).await;
        let events = run_turn(&h, owner("Where were we?")).await;
        assert_eq!(exit_of(&events), "text_response");
        let notices: Vec<&StreamEvent> =
            events.iter().filter(|e| e.event_type == ai::StreamEventType::ControlNotice).collect();
        assert!(notices.is_empty(), "a reconnect said something: {notices:?}");
    }

    /// A model slow to answer is not a stop: nothing on the turn's stream
    /// reads as one, however long the first token takes, so no screen ends
    /// the turn or saves a wait as how it ended. The owner sees the turn is
    /// alive from the run's progress snapshot (A28).
    #[tokio::test(start_paused = true)]
    async fn a_slow_model_is_not_a_stop() {
        let model = Scripted::new(vec![Step::Slow(Box::new(Step::Say("Here it is.")), std::time::Duration::from_secs(95))]);
        let h = harness(&model).await;
        let events = run_turn(&h, owner("Take your time")).await;
        assert_eq!(exit_of(&events), "text_response");
        assert!(events.iter().any(|e| e.event_type == ai::StreamEventType::Text), "the reply arrived");
        let notices: Vec<&StreamEvent> =
            events.iter().filter(|e| e.event_type == ai::StreamEventType::ControlNotice).collect();
        assert!(notices.is_empty(), "a slow model said something: {notices:?}");
    }

    /// The output cap cuts a reply: the call is taken again at the higher
    /// cap, and a second cut continues in place from one resume row.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cutoff_resumes_once() {
        let model = Scripted::new(vec![Step::Cut("Part one"), Step::Cut("Part two"), Step::Say("Part three.")]);
        let h = harness(&model).await;
        let events = run_turn(&h, owner("Write it all")).await;
        assert_eq!(exit_of(&events), "text_response");
        let calls = model.calls();
        assert_eq!(calls.len(), 3);
        assert!(calls[1].max_tokens > calls[0].max_tokens, "the first cut escalates the cap");
        let resume = events::attachment_for(&TurnEvent::CutoffResume).unwrap();
        let resumes = |c: &ChatRequest| c.messages.iter().filter(|m| m.content.contains(&resume.text)).count();
        assert_eq!((resumes(&calls[1]), resumes(&calls[2])), (0, 1), "one resume row, after the second cut");
        assert_eq!(kinds(&stored(&h)).iter().filter(|k| *k == "cutoff_resume").count(), 1);
    }

    /// The owner speaks while a tool round runs: the next step's call
    /// carries their words, and their caller hears the turn is busy.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mid_turn_owner_message_heard_next_step() {
        let model = Arc::new(Scripted::default());
        let h = harness(&model).await;
        let (busy_tx, busy_rx) = tokio::sync::oneshot::channel();
        let h2 = h.clone();
        let hook: Hook = Box::pin(async move {
            let mut handle = h2.start_turn(owner("Also check the calendar")).await.expect("queued");
            let first = handle.events.recv().await.expect("status");
            let _ = busy_tx.send(first.stop_reason);
        });
        *model.script.lock().unwrap() = VecDeque::from(vec![
            Step::During(Box::new(Step::Call("echo", serde_json::json!({}))), hook),
            Step::Say("Done, and the calendar is clear."),
        ]);
        let events = run_turn(&h, owner("Echo something")).await;
        assert_eq!(exit_of(&events), "text_response");
        assert_eq!(busy_rx.await.unwrap().as_deref(), Some(session_gate::QUEUED_INTO_RUNNING_TURN));
        let calls = model.calls();
        assert_eq!(calls.len(), 2, "heard inside the same turn");
        assert!(!texts(&calls[0]).iter().any(|t| t.contains("Also check the calendar")));
        assert!(texts(&calls[1]).iter().any(|t| t.contains("Also check the calendar")), "heard at the next step");
    }

    /// The owner writes while the model is answering: the answer never saw
    /// their message, so the next call reads the message after that answer,
    /// as the newest thing in the conversation, never before an answer that
    /// reads as the reply to it (CI 2026-09-26: the owner's question sat
    /// above a "noted" meant for a coworker's update, and went unanswered).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_message_sent_while_the_model_answers_is_read_after_that_answer() {
        let model = Arc::new(Scripted::default());
        let h = harness(&model).await;
        let h2 = h.clone();
        let hook: Hook = Box::pin(async move {
            let mut handle = h2.start_turn(owner("What time do we open?")).await.expect("queued");
            let _ = handle.events.recv().await;
        });
        *model.script.lock().unwrap() = VecDeque::from(vec![
            Step::During(Box::new(Step::Say("Started on the books.")), hook),
            Step::Say("We open at nine."),
        ]);
        let events = run_turn(&h, owner("Start on the books")).await;
        assert_eq!(exit_of(&events), "text_response");
        let calls = model.calls();
        assert_eq!(calls.len(), 2, "the message gets a call of its own");
        let read = texts(&calls[1]);
        let answer = read.iter().position(|t| t == "Started on the books.").expect("the first answer is in the thread");
        let message = read.iter().position(|t| t.contains("What time do we open?")).expect("the message is in the thread");
        assert!(message > answer, "the message reads after the answer that never saw it: {read:#?}");
        let stored = stored(&h);
        let stored_at = |text: &str| stored.iter().position(|m| m.content == text).expect("stored");
        assert!(
            stored_at("What time do we open?") < stored_at("Started on the books."),
            "rows stay stored in the order they arrived"
        );
    }

    /// A message from someone who isn't the owner, as its door sends it.
    fn from_elsewhere(
        text: &str,
        origin: tools::Origin,
        door: types::permissions::Door,
        channel: &str,
    ) -> TurnRequest {
        let mut req = owner(text);
        req.seat.origin = origin;
        req.seat.door = door;
        req.delivery.channel = channel.into();
        req
    }

    /// The owner's messages in the session's conversation, as a consent
    /// reads them.
    fn owner_words(h: &Harness) -> usize {
        let sid = h.sessions.resolve_session_id_by_key(KEY).expect("session");
        h.store
            .owner_messages_after(&h.sessions.active_chat_id(&sid), 0)
            .expect("rows")
    }

    /// A consent is the owner's own word: only input the owner typed in
    /// their own app is stored as the owner's. A coworker's "yes", a Slack,
    /// Discord or loop message, a visitor's, stored through the same one
    /// path at a turn's start, never count.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn only_the_owners_own_input_is_stored_as_the_owners() {
        use tools::Origin;
        use types::permissions::Door;
        let not_the_owner = [
            from_elsewhere(
                "yes",
                Origin::Comm,
                Door::Coworker { from: "ops".into() },
                "coworker",
            ),
            from_elsewhere("yes", Origin::Comm, Door::Chat, "slack"),
            from_elsewhere("yes", Origin::Comm, Door::Chat, "discord"),
            from_elsewhere("yes", Origin::Comm, Door::Chat, "loop"),
            from_elsewhere("yes", Origin::Visitor, Door::Chat, "web"),
        ];
        for req in not_the_owner {
            let channel = req.delivery.channel.clone();
            let model = Scripted::new(vec![Step::Say("Noted.")]);
            let h = harness(&model).await;
            run_turn(&h, req).await;
            assert!(
                stored(&h)
                    .iter()
                    .any(|m| m.role == "user" && m.content == "yes"),
                "{channel}: stored"
            );
            assert_eq!(
                owner_words(&h),
                0,
                "{channel}: a message that isn't the owner's is never the owner's word"
            );
        }
        let model = Scripted::new(vec![Step::Say("Creating it.")]);
        let h = harness(&model).await;
        run_turn(&h, owner("yes")).await;
        assert_eq!(owner_words(&h), 1, "the owner's own message is");
    }

    /// The same holds for a message queued into a running turn: the owner's
    /// counts, a channel's arriving at the same moment doesn't.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_queued_message_is_the_owners_only_when_the_owner_sent_it() {
        let model = Arc::new(Scripted::default());
        let h = harness(&model).await;
        let h2 = h.clone();
        let hook: Hook = Box::pin(async move {
            let slack = from_elsewhere(
                "yes from slack",
                tools::Origin::Comm,
                types::permissions::Door::Chat,
                "slack",
            );
            let mut queued = h2.start_turn(slack).await.expect("queued");
            while queued.events.recv().await.is_some() {}
            let mut queued = h2
                .start_turn(owner("yes from the owner"))
                .await
                .expect("queued");
            while queued.events.recv().await.is_some() {}
        });
        *model.script.lock().unwrap() = VecDeque::from(vec![
            Step::During(Box::new(Step::Call("echo", serde_json::json!({}))), hook),
            Step::Say("Done."),
        ]);
        run_turn(&h, owner("Echo something")).await;
        let rows = stored(&h);
        assert!(
            rows.iter().any(|m| m.content == "yes from slack"),
            "the channel's message was queued"
        );
        assert_eq!(
            owner_words(&h),
            2,
            "the turn's own input and the owner's queued message; not the channel's"
        );
    }

    /// A call parked on the owner names its ask on the tool-result event, so
    /// the conversation the run came from can carry the card.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_parked_call_names_its_ask_on_the_result_event() {
        let model = Scripted::new(vec![
            Step::Call("echo", serde_json::json!({})),
            Step::Say("Waiting on you."),
        ]);
        let h = harness(&model).await;
        let rule = types::permissions::Rule {
            id: "ask-echo".into(),
            scope: types::permissions::Scope::Company,
            key: types::permissions::RuleKey::Tool("echo".into()),
            field: None,
            effect: types::permissions::Effect::Ask,
            money: None,
            source: types::permissions::RuleSource::Owner,
            locked: false,
            created_at: 0,
        };
        h.store
            .write_permission_rule(&rule, &types::permissions::Writer::Owner)
            .unwrap();
        let mut req = owner("Echo something");
        req.seat.mode = Some(Mode::Automatic);
        let events = run_turn(&h, req).await;
        let result = events
            .iter()
            .find(|e| e.event_type == ai::StreamEventType::ToolResult)
            .expect("the call's result event");
        let ask = result
            .widgets
            .as_ref()
            .and_then(|w| w["parked_ask"].as_str())
            .expect("the parked ask is named");
        assert_eq!(
            h.store
                .get_permission_ask(ask)
                .unwrap()
                .expect("the ask")
                .status,
            "open"
        );
    }

    /// An employee with its own tools `quote` and `refund`, and a tool scope
    /// `storefront` that lists only `quote`.
    async fn scoped_employee(h: &Harness) {
        for name in ["quote", "refund"] {
            h.tools
                .register_for_agent(
                    "ops",
                    Box::new(Echo {
                        name,
                        deferred: true,
                        read_only: true,
                    }),
                )
                .await;
        }
        let config =
            napp::agent::parse_agent_config(r#"{"scopes": {"storefront": {"tools": ["quote"]}}}"#)
                .unwrap();
        h.agent_registry.write().await.insert(
            "ops".into(),
            tools::ActiveAgent {
                agent_id: "ops".into(),
                name: "Ops".into(),
                agent_md: String::new(),
                config: Some(config),
                channel_id: None,
                degraded: None,
                soul: None,
                rules: None,
            },
        );
    }

    fn scoped(text: &str, scope: Option<&str>) -> TurnRequest {
        let mut req = owner(text);
        req.seat.agent_id = "ops".into();
        req.seat.tool_scope = scope.map(str::to_string);
        req
    }

    /// The text every request of the model's calls carried.
    fn request_text(model: &Scripted) -> String {
        model
            .calls()
            .iter()
            .flat_map(texts)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// A tool scope's `tools` narrow the employee's own tools in its
    /// conversations: one the scope leaves out is not listed, not named in
    /// the job's tools, can't be loaded, and a call to it is refused. The
    /// runtime's own tools stay.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_tool_scope_narrows_the_employees_own_tools() {
        let model = Scripted::new(vec![
            Step::Call(
                "find_tools",
                serde_json::json!({ "query": "select:refund" }),
            ),
            Step::Call("refund", serde_json::json!({})),
            Step::Say("I can't do that here."),
        ]);
        let h = harness(&model).await;
        scoped_employee(&h).await;
        run_turn(
            &h,
            scoped("Handle the storefront question", Some("storefront")),
        )
        .await;
        let calls = model.calls();
        assert_eq!(calls.len(), 3);
        let first = texts(&calls[0]).join("\n");
        assert!(first.contains("quote"), "the scope's own tool is listed");
        assert!(
            !first.contains("refund"),
            "the tool the scope leaves out is not listed or named"
        );
        assert!(
            calls
                .iter()
                .all(|c| c.tools.iter().all(|t| t.name != "refund")),
            "never declared"
        );
        let results: Vec<String> = stored(&h)
            .iter()
            .filter(|m| m.role == "tool")
            .filter_map(|m| m.tool_results.clone())
            .collect();
        assert!(
            results[0].contains("No deferred tool matches"),
            "it can't be loaded: {}",
            results[0]
        );
        assert!(
            results[1].contains("isn't one of the tools for this conversation"),
            "the call is refused: {}",
            results[1]
        );
        assert!(!results[1].contains("refund ran"));

        // The same employee with no scope has both.
        let model = Scripted::new(vec![Step::Say("Sure.")]);
        let h = harness(&model).await;
        scoped_employee(&h).await;
        run_turn(&h, scoped("Handle the storefront question", None)).await;
        let text = request_text(&model);
        assert!(
            text.contains("quote") && text.contains("refund"),
            "no scope narrows nothing"
        );
    }

    /// Input that lands during the last step was in no call: the next turn
    /// hears it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn message_after_last_step_starts_next_turn() {
        let model = Arc::new(Scripted::default());
        let h = harness(&model).await;
        let h2 = h.clone();
        let hook: Hook = Box::pin(async move {
            let mut handle = h2.start_turn(owner("One more thing")).await.expect("queued");
            while handle.events.recv().await.is_some() {}
        });
        *model.script.lock().unwrap() = VecDeque::from(vec![
            Step::During(Box::new(Step::Say("Here you go.")), hook),
            Step::Say("And the one more thing."),
        ]);
        let events = run_turn(&h, owner("First thing")).await;
        assert_eq!(exit_of(&events), "text_response");
        assert_eq!(events.iter().filter(|e| e.event_type == ai::StreamEventType::Done).count(), 1, "one Done");
        let calls = model.calls();
        assert_eq!(calls.len(), 2, "a second turn ran");
        assert!(!texts(&calls[0]).iter().any(|t| t.contains("One more thing")));
        assert!(texts(&calls[1]).iter().any(|t| t.contains("One more thing")));
    }

    /// `find_tools` loads a deferred tool: its schema joins the request from
    /// the next step, and the step before only listed its name.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn deferred_tool_loaded_mid_turn_is_callable_next_step() {
        let model = Scripted::new(vec![
            Step::Call(tools::find_tools::FIND_TOOLS, serde_json::json!({"query": "select:weather"})),
            Step::Call("weather", serde_json::json!({})),
            Step::Say("Sunny."),
        ]);
        let h = harness(&model).await;
        let events = run_turn(&h, owner("Weather?")).await;
        assert_eq!(exit_of(&events), "text_response");
        let calls = model.calls();
        assert_eq!(calls.len(), 3);
        let declared = |c: &ChatRequest| c.tools.iter().any(|t| t.name == "weather");
        assert!(!declared(&calls[0]), "deferred before it is loaded");
        assert!(texts(&calls[0]).iter().any(|t| t.contains("available through find_tools") && t.contains("weather")), "listed by name");
        assert!(declared(&calls[1]) && declared(&calls[2]), "declared from the next step on");
        let weather_result = calls[2].messages.last().unwrap().tool_results.as_ref().unwrap().to_string();
        assert!(weather_result.contains("weather ran"), "{weather_result}");
    }

    /// The tools array heads the cached prefix, so a load may only append
    /// to it. Over two turns of one session, with loads mid-turn and in the
    /// next turn, every request's serialized tools array is a byte prefix of
    /// the next one's: nothing earlier is reordered, dropped or re-rendered.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_tools_array_only_grows_at_its_end_across_steps_and_turns() {
        let model = Scripted::new(vec![
            Step::Call(tools::find_tools::FIND_TOOLS, serde_json::json!({"query": "select:weather"})),
            Step::Call(tools::find_tools::FIND_TOOLS, serde_json::json!({"query": "select:beta,alpha"})),
            Step::Call("alpha", serde_json::json!({})),
            Step::Say("Done."),
            Step::Call(tools::find_tools::FIND_TOOLS, serde_json::json!({"query": "select:gamma"})),
            Step::Say("Loaded."),
        ]);
        let extra: Vec<Box<dyn tools::registry::DynTool>> = ["alpha", "beta", "gamma"]
            .into_iter()
            .map(|name| Box::new(Echo { name, deferred: true, read_only: true }) as Box<dyn tools::registry::DynTool>)
            .collect();
        let h = harness_with(&model, extra).await;
        run_turn(&h, owner("Weather, then alpha.")).await;
        run_turn(&h, owner("Now gamma.")).await;
        let calls = model.calls();
        assert_eq!(calls.len(), 6, "four steps, then two");
        // The array as the request carries it: one JSON object per tool, in
        // order. Its open form (no closing bracket) is what a longer array
        // must start with.
        let open = |c: &ChatRequest| {
            let each: Vec<String> = c.tools.iter().map(|t| serde_json::to_string(t).unwrap()).collect();
            format!("[{}", each.join(","))
        };
        let names = |c: &ChatRequest| c.tools.iter().map(|t| t.name.clone()).collect::<Vec<_>>();
        for pair in calls.windows(2) {
            assert!(
                open(&pair[1]).starts_with(&open(&pair[0])),
                "the tools array changed before its end: {:?} then {:?}",
                names(&pair[0]),
                names(&pair[1])
            );
        }
        let counts: Vec<usize> = calls.iter().map(|c| c.tools.len()).collect();
        let core = counts[0];
        assert_eq!(counts, vec![core, core + 1, core + 3, core + 3, core + 3, core + 4], "each load appends");
        let loaded: Vec<String> = names(&calls[5]).split_off(core);
        assert_eq!(loaded, ["weather", "beta", "alpha", "gamma"], "in load order, as the model asked");
    }

    /// An app subscribed to `agent.should_continue` that answers `false`
    /// halts a running employee before its next step, as on main; `true`
    /// (or no answer) never keeps a finished turn going.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_app_answering_false_halts_the_running_turn() {
        /// Lets step 1 run, then says stop.
        struct StopAfterFirst(Mutex<Vec<crate::hooks::ShouldContinuePayload>>);
        #[async_trait::async_trait]
        impl napp::hooks::HookCaller for StopAfterFirst {
            async fn call_filter(&self, _hook: &str, payload: Vec<u8>) -> Result<(Vec<u8>, bool), String> {
                let asked: crate::hooks::ShouldContinuePayload = serde_json::from_slice(&payload).unwrap();
                let go_on = asked.turn < 2;
                self.0.lock().unwrap().push(asked);
                let answer = serde_json::json!({"should_continue": go_on, "reason": "the close is locked"});
                Ok((serde_json::to_vec(&answer).unwrap(), true))
            }
            async fn call_action(&self, _hook: &str, _payload: Vec<u8>) -> Result<(), String> {
                Ok(())
            }
        }
        let model = Scripted::new(vec![Step::Call("echo", serde_json::json!({})), Step::Say("Posted.")]);
        let h = harness(&model).await;
        let app = Arc::new(StopAfterFirst(Mutex::default()));
        h.hooks.register("agent.should_continue", "ledger-app", napp::hooks::HookType::Filter, 0, app.clone());

        let events = run_turn(&h, owner("Post the entries")).await;
        assert_eq!(exit_of(&events), "app_halted");
        assert_eq!(model.calls().len(), 1, "no step after the app said stop");
        let asked = app.0.lock().unwrap();
        assert_eq!(asked.iter().map(|p| p.turn).collect::<Vec<_>>(), [1, 2], "asked before each step");
        assert_eq!(asked[1].total_tool_calls, ["echo"], "with the tools called so far");
        let status = events
            .iter()
            .find(|e| e.stop_reason.as_deref() == Some("app_halted") && e.event_type != ai::StreamEventType::Done)
            .expect("the owner sees why the work stopped");
        assert!(status.text.contains("the close is locked"), "{}", status.text);

        // An app that says go on never adds a step to a finished answer.
        let model = Scripted::new(vec![Step::Say("Hello.")]);
        let h = harness(&model).await;
        h.hooks.register("agent.should_continue", "ledger-app", napp::hooks::HookType::Filter, 0, Arc::new(StopAfterFirst(Mutex::default())));
        let events = run_turn(&h, owner("Hi")).await;
        assert_eq!(exit_of(&events), "text_response");
        assert_eq!(model.calls().len(), 1);
    }

    /// The owner's spending limit ends the turn before the next step, with
    /// a status line that says so.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn budget_limit_ends_the_turn_with_a_status_line() {
        let model = Scripted::new(vec![Step::Paid(Box::new(Step::Call("echo", serde_json::json!({}))), 50_000)]);
        let h = harness(&model).await;
        let mut req = owner("Do the thing");
        req.mode = TurnMode::Workflow(Box::new(crate::harness::WorkflowMode {
            trace: RequestTrace::new("agent_turn"),
            advertised_tools: ["echo".to_string()].into(),
            spend_cap_microcents: 1_000_000,
            ..Default::default()
        }));
        let events = run_turn(&h, req).await;
        assert_eq!(exit_of(&events), super::super::delegation::collect::STOP_SPEND_CAP);
        assert_eq!(model.calls().len(), 1, "no step after the limit");
        let status = events
            .iter()
            .find(|e| e.stop_reason.as_deref() == Some("spend_cap_reached") && e.event_type != ai::StreamEventType::Done)
            .expect("a status line");
        assert!(status.text.contains("$0.05 of $0.01"), "{}", status.text);
    }

    /// Stop means stop: the open call gets an interrupted result and the
    /// thread records the interrupt.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancel_records_interrupt() {
        let model = Arc::new(Scripted::default());
        let h = harness(&model).await;
        let req = owner("Echo something");
        let cancel = req.cancel.clone();
        let hook: Hook = Box::pin(async move { cancel.cancel() });
        *model.script.lock().unwrap() =
            VecDeque::from(vec![Step::During(Box::new(Step::Call("echo", serde_json::json!({}))), hook)]);
        let events = run_turn(&h, req).await;
        assert_eq!(exit_of(&events), "cancelled");
        let rows = stored(&h);
        assert!(rows.iter().any(|m| m.content == conversation::INTERRUPT_MESSAGE), "the interrupt is recorded");
        let calls_open = rows.iter().filter(|m| m.role == "tool").all(|m| {
            m.tool_results.as_deref().is_some_and(|r| r.contains(conversation::INTERRUPTED_TOOL_RESULT) || r.contains("echo ran"))
        });
        assert!(calls_open, "every call has a result");
        assert_eq!(model.calls().len(), 1, "no step after the stop");
    }

    /// A run the dispatcher ended for going silent is recorded as a stall,
    /// never as the owner stopping it: the owner did nothing.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_stall_is_never_recorded_as_the_owners_stop() {
        let model = Arc::new(Scripted::default());
        let h = harness(&model).await;
        let mut req = owner("Echo something");
        let progress = RunProgress {
            run_id: "r-stall".into(),
            iteration_count: Default::default(),
            tool_call_count: Default::default(),
            current_tool: Default::default(),
            waiting: Default::default(),
            stalled: Default::default(),
        };
        req.progress = Some(progress.clone());
        let cancel = req.cancel.clone();
        let hook: Hook = Box::pin(async move {
            progress.stalled.store(true, std::sync::atomic::Ordering::SeqCst);
            cancel.cancel()
        });
        *model.script.lock().unwrap() =
            VecDeque::from(vec![Step::During(Box::new(Step::Call("echo", serde_json::json!({}))), hook)]);
        let events = run_turn(&h, req).await;
        assert_eq!(exit_of(&events), "cancelled");
        let rows = stored(&h);
        assert!(!rows.iter().any(|m| m.content.contains("The owner stopped this work")), "not the owner's stop");
        assert!(
            rows.iter().any(|m| m.content.starts_with("[Run ended: nothing happened for 15 minutes]") && m.content.contains("The owner did not stop it")),
            "the stall is recorded as a stall"
        );
        assert!(!rows.iter().any(|m| m.tool_results.as_deref().is_some_and(|r| r.contains(conversation::INTERRUPTED_TOOL_RESULT))));
    }

    /// The recap is written after a chat turn and stored, and no later
    /// request ever carries it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn recap_never_enters_a_request() {
        let model = Scripted::new(vec![Step::Say("Drafted."), Step::Say("Thanks!")]);
        let h = harness(&model).await;
        run_turn(&h, owner("Draft the plan")).await;
        let sid = h.sessions.resolve_session_id_by_key(KEY).unwrap();
        let chat_id = h.sessions.active_chat_id(&sid);
        let mut stored_recap = None;
        for _ in 0..200 {
            stored_recap = h.store.latest_chat_recap(&chat_id).unwrap();
            if stored_recap.is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(stored_recap.expect("the recap is stored").text, RECAP);
        run_turn(&h, owner("Thank you")).await;
        for call in model.calls() {
            assert!(!call.system.contains(RECAP) && !texts(&call).iter().any(|t| t.contains(RECAP)), "a recap entered a request");
        }
    }

    /// A recap is for the owner coming back to the thread: a scheduled
    /// turn and a coworker's request get none; the owner's own chat does.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn recaps_are_for_turns_the_owner_is_in() {
        let model = Scripted::new(vec![Step::Say("Checked."), Step::Say("Noted."), Step::Say("Nothing new.")]);
        let h = harness(&model).await;
        let mut job = owner("Check the overnight entries");
        job.seat.origin = tools::Origin::System;
        job.seat.door = types::permissions::Door::Schedule;
        run_turn(&h, job).await;
        let mut coworker = owner("Note the VAT rate");
        coworker.seat.origin = tools::Origin::Comm;
        coworker.seat.door = types::permissions::Door::Coworker { from: "supervisor".into() };
        run_turn(&h, coworker).await;
        assert!(model.side_call("owner_recap").await.is_none(), "no owner, no recap");
        run_turn(&h, owner("Anything new?")).await;
        assert!(model.side_call("owner_recap").await.is_some(), "the owner's turn is recapped");
    }

    /// A tool that replaces the owner's price list, which the employee
    /// didn't make, and counts the calls that ran.
    struct Replace(Arc<std::sync::atomic::AtomicUsize>);

    impl tools::registry::DynTool for Replace {
        fn name(&self) -> &str {
            "replace"
        }
        fn description(&self) -> String {
            "replaces a file".into()
        }
        fn schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object", "properties": {}})
        }
        fn should_defer(&self) -> bool {
            false
        }
        fn effects(&self, _input: &serde_json::Value) -> types::permissions::CallEffects {
            types::permissions::CallEffects {
                overwrites: vec!["file:/srv/owner/prices.md".into()],
                publishes: types::permissions::Knowable::No,
                ..Default::default()
            }
        }
        fn execute_dyn<'a>(
            &'a self,
            _ctx: &'a tools::ToolContext,
            _input: serde_json::Value,
        ) -> Pin<Box<dyn Future<Output = tools::ToolResult> + Send + 'a>> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async { tools::ToolResult::ok("replaced") })
        }
    }

    /// The edit the owner types in his own chat runs with no card: his
    /// message is his consent. The same edit asked by a coworker or a
    /// schedule parks on the owner.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_owners_chat_message_is_consent_to_the_edit_it_asks_for() {
        use std::sync::atomic::Ordering;
        let ran = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let automatic = |mut req: TurnRequest| {
            req.seat.mode = Some(Mode::Automatic);
            req
        };
        let mut coworker = automatic(owner("Fix the price list"));
        coworker.seat.origin = tools::Origin::Comm;
        coworker.seat.door = types::permissions::Door::Coworker { from: "sales".into() };
        let mut schedule = automatic(owner("Fix the price list"));
        schedule.seat.origin = tools::Origin::System;
        schedule.seat.door = types::permissions::Door::Schedule;
        for req in [coworker, schedule] {
            let model = Scripted::new(vec![Step::Call("replace", serde_json::json!({})), Step::Say("Asked.")]);
            let h = harness_with(&model, vec![Box::new(Replace(ran.clone()))]).await;
            run_turn(&h, req).await;
            let result = model.calls()[1].messages.last().unwrap().tool_results.as_ref().unwrap().to_string();
            assert!(result.contains("Waiting for the owner to allow"), "{result}");
        }
        assert_eq!(ran.load(Ordering::SeqCst), 0, "nobody but the owner consents");
        let model = Scripted::new(vec![Step::Call("replace", serde_json::json!({})), Step::Say("Fixed.")]);
        let h = harness_with(&model, vec![Box::new(Replace(ran.clone()))]).await;
        run_turn(&h, automatic(owner("Fix the price list"))).await;
        let result = model.calls()[1].messages.last().unwrap().tool_results.as_ref().unwrap().to_string();
        assert_eq!(ran.load(Ordering::SeqCst), 1, "{result}");
        assert!(result.contains("replaced"), "{result}");
    }

    /// The recap forks the turn's last request, extended by the answer, so
    /// it reads the turn's cached prefix, and it names the run it recaps.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_recap_forks_the_turns_last_request() {
        let model = Scripted::new(vec![Step::Call("echo", serde_json::json!({})), Step::Say("Drafted.")]);
        let h = harness(&model).await;
        run_turn(&h, owner("Draft the plan")).await;
        let recap = model.side_call("owner_recap").await.expect("a recap");
        let last = model.calls().last().cloned().expect("a main call");
        assert_eq!(recap.system, last.system);
        assert_eq!(recap.model, last.model);
        assert_eq!(recap.cache_breakpoints, last.cache_breakpoints);
        let names = |r: &ChatRequest| r.tools.iter().map(|t| t.name.clone()).collect::<Vec<_>>();
        assert!(!names(&last).is_empty());
        assert_eq!(names(&recap), names(&last), "the turn's tools");
        assert_eq!(recap.tool_choice, last.tool_choice);
        assert_eq!(
            (recap.max_tokens, recap.temperature, recap.enable_thinking),
            (last.max_tokens, last.temperature, last.enable_thinking),
            "the settings the cache keys on"
        );
        let n = last.messages.len();
        assert_eq!(recap.messages.len(), n + 2, "the last request, its answer, the instruction");
        for (a, b) in recap.messages.iter().zip(&last.messages) {
            assert_eq!((&a.role, &a.content), (&b.role, &b.content), "the last request is the prefix");
        }
        assert_eq!((recap.messages[n].role.as_str(), recap.messages[n].content.as_str()), ("assistant", "Drafted."));
        assert_eq!(recap.messages[n + 1].content, crate::harness::recap::RECAP_INSTRUCTION);
        assert!(!last.trace.run_id.is_empty());
        assert_eq!(recap.trace.run_id, last.trace.run_id, "the run it recaps");
    }

    /// A first turn the owner stops is named from the owner's words, and
    /// Nebo's own rows (the turn's facts, the interrupt line) are not in the
    /// transcript the title is written from.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_first_turn_the_owner_stops_is_titled() {
        let model = Arc::new(Scripted::default());
        let h = harness(&model).await;
        let req = owner("Reconcile the September invoices");
        let cancel = req.cancel.clone();
        let hook: Hook = Box::pin(async move { cancel.cancel() });
        *model.script.lock().unwrap() = VecDeque::from(vec![Step::During(Box::new(Step::Call("echo", serde_json::json!({}))), hook)]);
        let events = run_turn(&h, req).await;
        assert_eq!(exit_of(&events), "cancelled");
        let title = model.side_call("title").await.expect("the stopped first turn is titled");
        let transcript = texts(&title).join("\n");
        assert!(transcript.contains("Reconcile the September invoices"), "{transcript}");
        assert!(!transcript.contains("<system-reminder>"), "{transcript}");
        assert!(!transcript.contains(conversation::INTERRUPT_MESSAGE), "{transcript}");
    }

    /// The owner's phone position reaches the turn of an employee the
    /// owner shares it with, as a row; a turn a chat channel started never
    /// hears it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_shared_phone_position_is_a_row_for_the_owners_turns_only() {
        let model = Scripted::new(vec![Step::Say("On my way."), Step::Say("Hello.")]);
        let h = harness(&model).await;
        let now = chrono::Utc::now().timestamp();
        h.phone_locations()
            .update(
                crate::phone_location::PhoneReading {
                    account_id: "owner".into(),
                    device_id: "phone".into(),
                    revision: 1,
                    agent_ids: vec!["assistant".into()],
                    latitude: Some(40.7608),
                    longitude: Some(-111.891),
                    accuracy_metres: Some(12.0),
                    taken_at: Some(now),
                },
                now,
            )
            .unwrap();
        let mut channel = owner("Hi from Slack");
        channel.seat.origin = tools::Origin::Comm;
        run_turn(&h, channel).await;
        run_turn(&h, owner("How far am I from the office?")).await;
        let calls = model.calls();
        let heard = |c: &ChatRequest| texts(c).iter().any(|t| t.contains("40.760800, -111.891000"));
        assert!(!heard(&calls[0]), "a chat channel's turn never hears where the owner is");
        assert!(heard(&calls[1]), "the owner's turn does");
    }

    /// Turning location off reaches the conversation at once: the next step
    /// of the running turn and every later turn are told the readings they
    /// heard are withdrawn, and hear no new one. Sharing again tells the
    /// new reading (A29).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn turning_location_off_withdraws_it_at_once() {
        let model = Scripted::new(Vec::new());
        let h = harness(&model).await;
        let now = chrono::Utc::now().timestamp();
        let reading = |revision: i64, agent_ids: Vec<String>, latitude: f64| crate::phone_location::PhoneReading {
            account_id: "owner".into(),
            device_id: "phone".into(),
            revision,
            agent_ids,
            latitude: Some(latitude),
            longitude: Some(-111.891),
            accuracy_metres: Some(12.0),
            taken_at: Some(now),
        };
        h.phone_locations().update(reading(1, vec!["assistant".into()], 40.7608), now).unwrap();
        let locations = h.phone_locations.clone();
        let revoke = reading(2, Vec::new(), 40.7608);
        let off: Hook = Box::pin(async move { locations.update(revoke, now).unwrap() });
        *model.script.lock().unwrap() = VecDeque::from(vec![
            Step::During(Box::new(Step::Call("echo", serde_json::json!({}))), off),
            Step::Say("Done."),
            Step::Say("Hello."),
            Step::Say("Here."),
        ]);
        run_turn(&h, owner("Where am I?")).await;
        run_turn(&h, owner("And now?")).await;
        h.phone_locations().update(reading(3, vec!["assistant".into()], 40.5), now).unwrap();
        run_turn(&h, owner("Now?")).await;

        let calls = model.calls();
        let told = |c: &ChatRequest, what: &str| texts(c).iter().filter(|t| t.contains(what)).count();
        let withdrawn = "Earlier readings";
        assert_eq!(told(&calls[0], "40.760800, -111.891000"), 1, "shared: the turn hears it");
        assert_eq!(told(&calls[1], withdrawn), 1, "turned off mid-turn: the next step is told");
        assert_eq!(told(&calls[2], withdrawn), 1, "a later turn is told once, not again");
        assert_eq!(told(&calls[2], "40.500000"), 0, "and hears no reading");
        assert_eq!(told(&calls[3], "40.500000, -111.891000"), 1, "shared again: the new reading");
    }

    /// An explore helper declares the same tools as every run, the helper
    /// tool included, and the one check refuses what it may not do: a call
    /// that changes something, or starting a helper.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn explore_helper_only_looks() {
        let model = Scripted::new(vec![
            Step::Call("writer", serde_json::json!({})),
            Step::Call("delegate", serde_json::json!({"description": "look", "prompt": "Look."})),
            Step::Say("Found it."),
        ]);
        let h = harness(&model).await;
        let mut req = owner("Look around");
        req.session_key = "subagent:agent:ops:web:h-1".into();
        req.mode = TurnMode::Helper {
            parent_session_key: KEY.into(),
            kind: crate::harness::delegation::HelperKind::Explore,
            depth: 1,
            answer: None,
        };
        let mut handle = h.start_turn(req).await.expect("start");
        while handle.events.recv().await.is_some() {}
        let calls = model.calls();
        assert!(calls[0].tools.iter().any(|t| t.name == "delegate"), "the one tool list, helper tool included");
        let result = calls[1].messages.last().unwrap().tool_results.as_ref().unwrap().to_string();
        assert!(result.contains("only looks") && !result.contains("writer ran"), "{result}");
        let result = calls[2].messages.last().unwrap().tool_results.as_ref().unwrap().to_string();
        assert!(result.contains("only looks") && !result.contains("delegate ran"), "{result}");
    }

    /// The provider refuses the window: the conversation is checkpointed and
    /// the step is taken again from the boundary, with the session's facts
    /// told again after it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn overflow_checkpoints_then_retries() {
        let model = Scripted::new(vec![Step::Say("First answer."), Step::Overflow, Step::Say("Carried on.")]);
        let h = harness(&model).await;
        run_turn(&h, owner("Start the report")).await;
        let events = run_turn(&h, owner("Keep going")).await;
        assert_eq!(exit_of(&events), "text_response");
        let calls = model.calls();
        assert_eq!(calls.len(), 3, "the refused call and its retry");
        let retry = texts(&calls[2]);
        assert!(retry[0].starts_with(compact::checkpoint::BOUNDARY_LEAD), "the retry opens on the boundary: {}", retry[0]);
        assert!(!retry.iter().any(|t| t == "Start the report"), "history before the boundary is not sent");
        let rows = stored(&h);
        assert_eq!(rows.iter().filter(|m| m.content.starts_with(compact::checkpoint::BOUNDARY_LEAD)).count(), 1);
        assert_eq!(kinds(&rows).iter().filter(|k| *k == "environment").count(), 2, "the facts are told again after the boundary");
    }

    /// D10 (review 6.4): the owner's `/compact` is the turn's own
    /// checkpoint, not a weaker second path: the summary forks the step's
    /// request (the one system prompt and the tool list), carries the
    /// owner's instructions (added to the summary prompt), and
    /// the turn ends without a model turn. Sent while a turn runs, it waits
    /// for that turn instead of joining it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_owners_compact_is_the_turns_checkpoint_and_waits_for_a_running_turn() {
        let model = Scripted::new(vec![Step::Slow(Box::new(Step::Say("First answer.")), std::time::Duration::from_millis(400))]);
        let h = harness(&model).await;
        let mut first = h.start_turn(owner("Start the report")).await.unwrap();
        for _ in 0..100 {
            if h.is_session_busy(KEY) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        let mut compact = owner("");
        compact.input = TurnInput::Compact { instructions: "Keep the Rivera numbers exact.".into() };
        let events = run_turn(&h, compact).await;
        while first.events.recv().await.is_some() {}
        assert_eq!(exit_of(&events), "compacted");
        assert_eq!(model.calls().len(), 1, "the compact made no model turn of its own");
        let rows = stored(&h);
        let reply = rows.iter().position(|m| m.content == "First answer.").expect("the running turn's reply");
        let boundary = rows
            .iter()
            .position(|m| m.content.starts_with(compact::checkpoint::BOUNDARY_LEAD))
            .expect("a checkpoint was written");
        assert!(boundary > reply, "it waited for the running turn");
        assert!(rows[boundary].metadata.as_deref().unwrap_or("").contains("owner_asked"));
        let summary = model.side_call("checkpoint").await.expect("the summary call");
        assert_eq!(summary.system, crate::harness::prompt::system_prompt(), "it forks the step's request");
        assert!(!summary.tools.is_empty(), "with the step's tools");
        assert!(
            summary.messages.last().unwrap().content.ends_with("Additional instructions from the owner:\nKeep the Rivera numbers exact."),
            "{}",
            summary.messages.last().unwrap().content
        );
    }

    /// The owner writes while their `/compact` is being summarized: the
    /// compact makes no model turn and its summary never read the message,
    /// so the message waits for it and gets a turn of its own, read after
    /// the checkpoint and answered where the owner is listening. Proof
    /// fixture checkpoint-keeps-every-owner-message: the message was
    /// written into the compact, sat before the boundary, and no call ever
    /// read it (no reply for 180 s, every run).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_message_sent_during_the_owners_compact_gets_its_own_turn() {
        let model = Scripted::new(vec![Step::Say("First answer."), Step::Say("Saved them.")]);
        let h = harness(&model).await;
        run_turn(&h, owner("Read the fourteen logs")).await;
        let h2 = h.clone();
        let (sent_tx, sent_rx) = tokio::sync::oneshot::channel();
        let hook: Hook = Box::pin(async move {
            let message = tokio::spawn(async move {
                let mut handle = h2.start_turn(owner("Now save those ids to a file")).await.expect("start");
                let mut seen = Vec::new();
                while let Some(e) = handle.events.recv().await {
                    seen.push(e);
                }
                seen
            });
            // Long enough for the message to reach admission.
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            let _ = sent_tx.send(message);
        });
        *model.during_checkpoint.lock().unwrap() = Some(hook);
        let mut compact = owner("");
        compact.input = TurnInput::Compact { instructions: String::new() };
        let events = run_turn(&h, compact).await;
        assert_eq!(exit_of(&events), "compacted");
        let message = sent_rx.await.expect("the message was sent").await.expect("the message's turn");
        assert_eq!(exit_of(&message), "text_response", "the message was answered on its own turn");
        assert!(
            message.iter().any(|e| e.event_type == ai::StreamEventType::Text && e.text.contains("Saved them.")),
            "the answer streams to the one who sent it"
        );
        let calls = model.calls();
        assert_eq!(calls.len(), 2, "the first turn and the message's turn; the compact made none");
        let read = texts(&calls[1]);
        assert!(read[0].starts_with(compact::checkpoint::BOUNDARY_LEAD), "the call opens on the checkpoint: {}", read[0]);
        assert!(
            read.iter().any(|t| t.contains("Now save those ids to a file")),
            "the message is read after the checkpoint: {read:#?}"
        );
    }

    /// Runs a turn whose step overflows, so the turn checkpoints for
    /// itself, and sends `input` into it while the summary is written.
    /// Returns what the step after the checkpoint read.
    async fn input_during_a_checkpoint(input: TurnRequest) -> (Vec<StreamEvent>, Vec<String>, Vec<ChatMessage>) {
        let model = Scripted::new(vec![Step::Say("First answer."), Step::Overflow, Step::Say("Answered.")]);
        let h = harness(&model).await;
        run_turn(&h, owner("Start the report")).await;
        let h2 = h.clone();
        let hook: Hook = Box::pin(async move {
            let mut handle = h2.start_turn(input).await.expect("queued");
            let _ = handle.events.recv().await;
        });
        *model.during_checkpoint.lock().unwrap() = Some(hook);
        let events = run_turn(&h, owner("Keep going")).await;
        let calls = model.calls();
        assert_eq!(calls.len(), 3, "the refused call and the step after the checkpoint");
        (events, texts(&calls[2]), stored(&h))
    }

    /// The owner writes while a checkpoint the turn took for itself is being
    /// summarized: the summary never read the message, so the step after the
    /// checkpoint reads it after the boundary and the turn answers it. It sat
    /// before the boundary, which the step's load starts at, and no call
    /// ever read it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_message_sent_while_the_turn_checkpoints_is_read_after_the_boundary() {
        let (events, read, rows) = input_during_a_checkpoint(owner("What time do we open?")).await;
        assert_eq!(exit_of(&events), "text_response");
        assert!(read[0].starts_with(compact::checkpoint::BOUNDARY_LEAD), "the step opens on the boundary: {}", read[0]);
        assert!(read.iter().any(|t| t.contains("What time do we open?")), "the message is read after the boundary: {read:#?}");
        assert!(!read.iter().any(|t| t == compact::checkpoint::BOUNDARY_MARKER), "the owner's marker is never the model's");
        let at = |text: &str| rows.iter().position(|m| m.content.contains(text)).expect("stored");
        assert!(
            at("What time do we open?") < at(compact::checkpoint::BOUNDARY_LEAD),
            "rows stay stored in the order they arrived"
        );
    }

    /// A helper finishes while the turn's checkpoint is being summarized:
    /// its result is read after the boundary, never lost before it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_helper_result_that_lands_while_the_turn_checkpoints_is_read_after_the_boundary() {
        let mut notification = owner("");
        notification.input = TurnInput::Notification(crate::harness::delegation::Completion {
            task_id: "h-7".into(),
            description: "price the Rivera order".into(),
            status: crate::harness::delegation::CompletionStatus::Done,
            result: "The Rivera order comes to 4,210.".into(),
            usage: Default::default(),
            taint: Vec::new(),
        });
        let (events, read, _) = input_during_a_checkpoint(notification).await;
        assert_eq!(exit_of(&events), "text_response");
        assert!(read[0].starts_with(compact::checkpoint::BOUNDARY_LEAD), "the step opens on the boundary: {}", read[0]);
        assert!(
            read.iter().any(|t| t.contains("The Rivera order comes to 4,210.")),
            "the result is read after the boundary: {read:#?}"
        );
    }

    /// The session's running helpers, as the server lists them.
    struct Running(Vec<compact::restore::RunningWork>);

    impl goal::GoalObserver for Running {
        fn status(&self, _goal: &goal::AgreedGoal) {}
        fn kickoff(&self, _goal: &goal::AgreedGoal, _prompt: String) {}
        fn background(&self, _session_id: &str) -> Vec<compact::restore::RunningWork> {
            self.0.clone()
        }
    }

    /// D10 (review 6.3): after a checkpoint the model is told the work the
    /// session started that is still running (restored task rows): its
    /// helper and its background command, not
    /// another session's command.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_checkpoint_tells_the_work_still_running() {
        let model = Scripted::new(vec![Step::Say("First answer."), Step::Overflow, Step::Say("Carried on.")]);
        let h = harness(&model).await;
        h.bind(crate::harness::Outlets {
            goal_observer: Some(Arc::new(Running(vec![compact::restore::RunningWork::helper("h-7", "price the Rivera order")]))),
            ..Default::default()
        });
        let spawn = |key: &str, what: &str| {
            let mut cmd = tokio::process::Command::new("sleep");
            cmd.arg("30");
            let caller = tools::process::Caller { session_key: key.into(), description: what.into() };
            h.tools.process_registry().spawn(cmd, "sleep 30", tools::process::Spawn::Background(Some(caller)), false)
        };
        let ours = spawn(KEY, "wait for the build").await.unwrap().session.id.clone();
        let theirs = spawn("agent:other:web", "someone else's").await.unwrap().session.id.clone();
        run_turn(&h, owner("Start the report")).await;
        run_turn(&h, owner("Keep going")).await;
        let running: Vec<String> = stored(&h)
            .iter()
            .filter(|m| reminders::attachment_kind(m).as_deref() == Some("running_work"))
            .map(|m| m.content.clone())
            .collect();
        for id in [&ours, &theirs] {
            let _ = h.tools.process_registry().kill_session(id).await;
        }
        assert_eq!(running.len(), 2, "{running:?}");
        assert!(running[0].contains("Background helper \"price the Rivera order\" (h-7) is still running"), "{}", running[0]);
        assert!(running[1].contains(&format!("Background command {ours} (\"wait for the build\") is still running (command: `sleep 30`)")), "{}", running[1]);
    }

    /// A tool result that carries untrusted content (a helper's report of
    /// what it read).
    struct Relay;

    impl tools::registry::DynTool for Relay {
        fn name(&self) -> &str {
            "relay"
        }
        fn description(&self) -> String {
            "relays a helper's report".into()
        }
        fn schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object", "properties": {}})
        }
        fn read_only(&self, _input: &serde_json::Value) -> bool {
            true
        }
        fn execute_dyn<'a>(
            &'a self,
            _ctx: &'a tools::ToolContext,
            _input: serde_json::Value,
        ) -> Pin<Box<dyn Future<Output = tools::ToolResult> + Send + 'a>> {
            Box::pin(async move {
                tools::ToolResult::ok("the page says 40% off").with_taint(vec![types::provenance::ProvenanceClass::Web])
            })
        }
    }

    /// Review 5.1: a result that carries untrusted content taints the run
    /// that reads it — the turn's provenance names it at its end.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_result_that_carries_what_a_helper_read_taints_the_run() {
        let model = Scripted::new(vec![Step::Call("relay", serde_json::json!({})), Step::Say("It is 40% off.")]);
        let h = harness_with(&model, vec![Box::new(Relay)]).await;
        let events = run_turn(&h, owner("What does the sale page say?")).await;
        let done = events.iter().find(|e| e.event_type == ai::StreamEventType::Done).unwrap();
        assert_eq!(done.provenance.clone().unwrap_or_default(), vec![types::provenance::ProvenanceClass::Web]);
    }

    /// The text of the latest stored attachment row of `kind`.
    fn latest_row(h: &Harness, kind: &str) -> Option<String> {
        stored(h).into_iter().rev().find(|m| reminders::attachment_kind(m).as_deref() == Some(kind)).map(|m| m.content)
    }

    /// B10: the employee the owner talks to sees who owns which job — each
    /// employee's name and job, and each team's name, what it owns, its
    /// lead and its members — as roster rows (like the helper listing),
    /// never in the system prompt. A restaffed team is told again as a
    /// delta: only what changed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_roster_names_who_owns_which_job_and_follows_restaffing() {
        let model = Scripted::new(vec![Step::Say("Hi."), Step::Say("Noted."), Step::Say("Still here.")]);
        let h = harness(&model).await;
        for (id, name, job) in [
            ("bk", "Bookkeeper", "Keeps the books and reports the budget."),
            ("ava", "Ava", "Plans the marketing calendar."),
            ("bo", "Bo", "Writes the ads."),
        ] {
            h.store.create_agent(id, None, name, job, "", "", None, None).unwrap();
        }
        h.store
            .create_team("t-mkt", "Marketing", "every campaign we run", &[db::TeamMember::local("ava"), db::TeamMember::local("bo")], "ava", None)
            .unwrap();
        h.store
            .create_team("t-ops", "Ops", "", &[db::TeamMember::local("bk"), db::TeamMember::local("bo")], "", None)
            .unwrap();

        run_turn(&h, owner("Who handles what?")).await;
        let agents = latest_row(&h, "agents_listing").expect("the employees are listed");
        assert!(agents.contains("- Bookkeeper: Keeps the books and reports the budget."), "{agents}");
        let teams = latest_row(&h, "teams_listing").expect("the teams are listed");
        assert!(teams.contains("- Marketing: owns every campaign we run; lead: Ava; members: Ava, Bo"), "{teams}");
        assert!(teams.contains("- Ops: owns nothing stated yet; no lead set; members: Bookkeeper, Bo"), "{teams}");
        assert!(model.calls()[0].system == crate::harness::prompt::system_prompt(), "never in the system prompt");

        run_turn(&h, owner("Thanks")).await;
        assert_eq!(kinds(&stored(&h)).iter().filter(|k| *k == "teams_listing").count(), 1, "unchanged: not told again");

        // Restaffed: Bo leads Marketing now.
        h.store
            .update_team("t-mkt", "Marketing", "every campaign we run", &[db::TeamMember::local("ava"), db::TeamMember::local("bo")], "bo")
            .unwrap();
        run_turn(&h, owner("Bo leads marketing now")).await;
        assert_eq!(kinds(&stored(&h)).iter().filter(|k| *k == "teams_listing").count(), 2);
        let delta = latest_row(&h, "teams_listing").unwrap();
        assert!(delta.contains("- Marketing: owns every campaign we run; lead: Bo; members: Ava, Bo"), "{delta}");
        assert!(!delta.contains("Ops"), "only what changed: {delta}");
    }

    /// B12: the one permit pool serves the owner's turn before any work
    /// waiting for a permit. Both permits are out; a helper's call queues
    /// first, then the owner's. The first permit back goes to the owner's
    /// turn, which answers before the helper's call is made.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn work_waiting_for_a_permit_never_holds_the_owners_turn() {
        use crate::concurrency::Priority;
        let model = Scripted::new(vec![Step::Say("Owner answered."), Step::Say("Helper done.")]);
        let h = harness(&model).await;
        h.concurrency.set_ceiling(2);
        let mut busy = vec![
            h.concurrency.acquire_llm_permit(Priority::Work).await,
            h.concurrency.acquire_llm_permit(Priority::Work).await,
        ];
        let mut helper = owner("Count the till");
        helper.session_key = format!("subagent:{KEY}:h-1");
        helper.mode = TurnMode::Helper {
            parent_session_key: KEY.into(),
            kind: crate::harness::delegation::HelperKind::General,
            depth: 1,
            answer: None,
        };
        let mut helper_events = h.start_turn(helper).await.expect("helper starts").events;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let owner_turn = tokio::spawn({
            let h = h.clone();
            async move { run_turn(&h, owner("What time do we open?")).await }
        });
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(model.calls().is_empty(), "both calls wait for a permit");

        drop(busy.pop());
        let events = tokio::time::timeout(std::time::Duration::from_secs(10), owner_turn)
            .await
            .expect("the owner's turn finishes while the helper still waits")
            .unwrap();
        assert_eq!(exit_of(&events), "text_response");
        assert!(stored(&h).iter().any(|m| m.role == "assistant" && m.content == "Owner answered."), "the owner's call went first");
        drop(busy);
        while helper_events.recv().await.is_some() {}
        assert_eq!(model.calls().len(), 2, "the helper's call is made once the owner's is served");
    }

    /// B12, B15: the memory flush before a checkpoint is housekeeping: it
    /// waits for a background permit, never the pool the owner's turns use,
    /// and the turn never waits for it (extraction runs as a background
    /// fork). It reads the conversation the checkpoint
    /// summarized.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_checkpoint_memory_flush_runs_in_the_background_on_a_background_permit() {
        let model = Scripted::new(vec![Step::Say("First answer."), Step::Overflow, Step::Say("Carried on.")]);
        let h = harness(&model).await;
        h.concurrency.set_ceiling(4);
        assert_eq!(h.concurrency.background_permits(), 1);
        run_turn(&h, owner("The Zanzibar invoice is due on the ninth.")).await;
        let held = h.concurrency.acquire_background_permit().await;
        let events = tokio::time::timeout(std::time::Duration::from_secs(10), run_turn(&h, owner("Keep going")))
            .await
            .expect("the turn never waits on the flush");
        assert_eq!(exit_of(&events), "text_response");
        assert!(
            !model.side.lock().unwrap().iter().any(|r| r.trace.purpose == "memory_flush"),
            "no flush while housekeeping's only permit is taken"
        );
        drop(held);
        let flush = model.side_call("memory_flush").await.expect("the flush ran once a background permit was free");
        assert!(
            flush.messages[0].content.contains("The Zanzibar invoice is due on the ninth."),
            "it read the conversation before the boundary"
        );
    }

    /// The same vector for every text: a search through it finds whatever
    /// the index holds, and only that.
    struct ConstEmbedder;

    #[async_trait::async_trait]
    impl ai::EmbeddingProvider for ConstEmbedder {
        fn id(&self) -> &str {
            "const-test-embed"
        }
        fn dimensions(&self) -> usize {
            8
        }
        async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, ai::ProviderError> {
            Ok(texts.iter().map(|_| vec![1.0; 8]).collect())
        }
    }

    /// What a checkpoint summarizes away is indexed for recall: after the
    /// boundary, a search in the employee's memory scope finds an owner's
    /// detail from before it, and Nebo's own rows are not indexed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_checkpoint_indexes_the_conversation_it_replaces_for_recall() {
        let model = Scripted::new(vec![Step::Say("Noted."), Step::Overflow, Step::Say("Carried on.")]);
        let h = harness(&model).await.with_embedding_provider(Arc::new(ConstEmbedder));
        run_turn(&h, owner("The Zanzibar invoice is due on the ninth.")).await;
        run_turn(&h, owner("Keep going")).await;
        assert!(texts(&model.calls()[2])[0].starts_with(compact::checkpoint::BOUNDARY_LEAD), "a checkpoint was taken");

        let sid = h.sessions.resolve_session_id_by_key(KEY).expect("session");
        let scope = seat::resolve_seat(
            &h.store,
            KEY,
            seat::SeatInputs {
                agent: None,
                agent_id: "",
                user_id: "",
                session_id: &sid,
                origin: tools::Origin::User,
                channel: "web",
                audience: None,
            },
        )
        .memory
        .user_id;
        let mut found = Vec::new();
        for _ in 0..200 {
            found = crate::search::hybrid_search(
                &h.store,
                Some(&ConstEmbedder),
                "When is the Zanzibar invoice due?",
                &scope,
                &crate::search::SearchConfig::default(),
                None,
            )
            .await;
            if !found.is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let text: Vec<&str> = found.iter().map(|r| r.value.as_str()).collect();
        assert!(
            text.iter().any(|t| t.contains("user: The Zanzibar invoice is due on the ninth.")),
            "recall finds the detail from before the checkpoint: {text:?}"
        );
        assert!(!text.iter().any(|t| t.contains("<system-reminder>")), "Nebo's own rows are not indexed: {text:?}");
    }

    /// A tool whose 5,000-character result can be got again.
    struct Reader;

    impl tools::registry::DynTool for Reader {
        fn name(&self) -> &str {
            "reader"
        }
        fn description(&self) -> String {
            "reads things".into()
        }
        fn schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object", "properties": {}})
        }
        fn read_only(&self, _input: &serde_json::Value) -> bool {
            true
        }
        fn clearable(&self, _input: &serde_json::Value) -> bool {
            true
        }
        fn execute_dyn<'a>(
            &'a self,
            _ctx: &'a tools::ToolContext,
            _input: serde_json::Value,
        ) -> Pin<Box<dyn Future<Output = tools::ToolResult> + Send + 'a>> {
            Box::pin(async move { tools::ToolResult::ok("r".repeat(5_000)) })
        }
    }

    /// Context pressure clears old results before anything is summarised,
    /// because clearing is cheaper than a summary: the provider refuses the window, every
    /// clearable result but the five newest is saved to a file and replaced
    /// by where it was saved, and the step is taken again with no
    /// checkpoint.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pressure_clears_old_results_before_a_checkpoint() {
        let mut script: Vec<Step> = (0..30).map(|_| Step::Call("reader", serde_json::json!({}))).collect();
        script.extend([Step::Overflow, Step::Say("Carried on.")]);
        let model = Scripted::new(script);
        let h = harness_with(&model, vec![Box::new(Reader)]).await;
        let events = run_turn(&h, owner("Read everything")).await;
        let sid = h.sessions.resolve_session_id_by_key(KEY).expect("session");
        let _ = std::fs::remove_dir_all(tools::checkpoint::session_dir(&sid));
        assert_eq!(exit_of(&events), "text_response");
        let calls = model.calls();
        assert_eq!(calls.len(), 32, "the refused call and its retry");
        let results: Vec<String> = calls[31]
            .messages
            .iter()
            .filter_map(|m| m.tool_results.as_ref().map(|r| r.to_string()))
            .collect();
        assert_eq!(results.len(), 30);
        for r in &results[..25] {
            assert!(r.contains("This result was saved at:") && !r.contains("rrrrr"), "{r}");
        }
        for r in &results[25..] {
            assert!(r.contains(&"r".repeat(5_000)), "the five newest stay whole");
        }
        assert!(!texts(&calls[31]).iter().any(|t| t.starts_with(compact::checkpoint::BOUNDARY_LEAD)), "no checkpoint");
    }

    struct Watch(Mutex<Vec<String>>);

    impl goal::GoalObserver for Watch {
        fn status(&self, goal: &goal::AgreedGoal) {
            self.0.lock().unwrap().push(goal.status.as_str().to_string());
        }
        fn kickoff(&self, _goal: &goal::AgreedGoal, _prompt: String) {}
        fn background(&self, _session_id: &str) -> Vec<compact::restore::RunningWork> {
            Vec::new()
        }
    }

    /// An agreed goal holds the turn open: an unmet check continues with the
    /// check's reason as a row, and a met check ends the turn.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unmet_goal_continues_until_the_check_says_met() {
        let model = Scripted::new(vec![Step::Say("Tests written."), Step::Say("All tests pass now.")]);
        model.verdicts.lock().unwrap().extend([
            r#"{"met": false, "reason": "the transcript shows \"2 failing\""}"#,
            r#"{"met": true, "reason": "\"All tests pass now.\""}"#,
        ]);
        let h = harness(&model).await;
        let watch = Arc::new(Watch(Mutex::new(Vec::new())));
        h.bind(crate::harness::Outlets { goal_observer: Some(watch.clone()), ..Default::default() });
        let sid = h.sessions.get_or_create(KEY, "").unwrap().id;
        goal::GoalStore::new(&h.sessions, &sid).set("all tests pass", goal::GoalSource::OwnerCommand).unwrap();

        let events = run_turn(&h, owner("Fix the tests")).await;
        assert_eq!(exit_of(&events), "goal_met");
        let calls = model.calls();
        assert_eq!(calls.len(), 2, "one continuation");
        assert!(texts(&calls[1]).iter().any(|t| t.contains("The agreed goal isn't met yet") && t.contains("2 failing")));
        assert_eq!(kinds(&stored(&h)).iter().filter(|k| *k == "goal_check").count(), 1);
        assert!(watch.0.lock().unwrap().iter().any(|s| s == "met"), "the owner is told");
    }

    /// Main's prompt facts come back as rows, never as prompt text: the time
    /// the turn starts, the channel's rules, and a coworker's limit on
    /// shared memory.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn time_channel_rules_and_coworker_limit_are_rows() {
        let model = Scripted::new(vec![Step::Say("Done."), Step::Say("Sure.")]);
        let h = harness(&model).await;
        let mut voice = owner("Remind me in two hours");
        voice.delivery.channel = "voice".into();
        run_turn(&h, voice).await;
        let call = &model.calls()[0];
        assert_eq!(call.system, crate::harness::prompt::system_prompt(), "the one system prompt");
        let rows = texts(call);
        assert!(rows.iter().any(|t| t.contains("It is ") && t.contains(" on ")), "the time the turn starts: {rows:?}");
        assert!(rows.iter().any(|t| t.contains("# Channel rules") && t.contains("spoken aloud")), "{rows:?}");
        assert!(!rows.iter().any(|t| t.contains("shared memory")), "the owner has no limit");

        let asker = "agent:ops:coworker:thread-1";
        let mut coworker = seat_of(owner("What's the settlement figure?"), "", asker);
        coworker.seat.audience = Some("scout".into());
        run_turn(&h, coworker).await;
        let rows = texts(&model.calls()[1]);
        assert!(rows.iter().any(|t| t.contains("must not be passed on")), "{rows:?}");
    }

    /// Memory extraction after a turn reads the goal the turn worked under.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn memory_extraction_reads_the_goal_the_turn_worked_under() {
        let model = Scripted::new(vec![Step::Say("All four renewal letters are sent.")]);
        let h = harness(&model).await;
        h.bind(crate::harness::Outlets { goal_observer: Some(Arc::new(Watch(Mutex::new(Vec::new())))), ..Default::default() });
        let sid = h.sessions.get_or_create(KEY, "").unwrap().id;
        goal::GoalStore::new(&h.sessions, &sid)
            .set("every renewal letter is sent before Friday", goal::GoalSource::OwnerCommand)
            .unwrap();

        let events = run_turn(&h, owner("Send the renewal letters")).await;
        assert_eq!(exit_of(&events), "goal_met");
        let extraction = model.side_call("memory_extract").await.expect("memory extraction ran");
        let asked: String = extraction.messages.iter().map(|m| m.content.as_str()).collect::<Vec<_>>().join("\n");
        assert!(
            asked.contains("The agreed goal of this work: every renewal letter is sent before Friday"),
            "the extraction request names the goal: {asked}"
        );
    }

    fn employee(id: &str, name: &str, soul: &str, config: Option<napp::agent::AgentConfig>) -> tools::ActiveAgent {
        tools::ActiveAgent {
            agent_id: id.into(),
            name: name.into(),
            agent_md: format!("{name} does the {id} work."),
            config,
            channel_id: None,
            degraded: None,
            soul: Some(soul.into()),
            rules: Some(format!("{name}'s rules.")),
        }
    }

    fn seat_of(mut req: TurnRequest, agent_id: &str, key: &str) -> TurnRequest {
        req.seat.agent_id = agent_id.into();
        req.session_key = key.into();
        req
    }

    /// A company Memory search, proxied to the Memory integration; counts
    /// the calls that reached it.
    struct CompanyMemory {
        integration: String,
        ran: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl tools::registry::DynTool for CompanyMemory {
        fn name(&self) -> &str {
            "mcp__nebo_kb__memory_search"
        }
        fn description(&self) -> String {
            "Searches company memory".into()
        }
        fn schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object", "properties": {"query": {"type": "string"}}})
        }
        fn should_defer(&self) -> bool {
            true
        }
        fn read_only(&self, _input: &serde_json::Value) -> bool {
            true
        }
        fn mcp_proxy_info(&self) -> Option<(String, String)> {
            Some((self.integration.clone(), "memory_search".into()))
        }
        fn execute_dyn<'a>(
            &'a self,
            _ctx: &'a tools::ToolContext,
            _input: serde_json::Value,
        ) -> Pin<Box<dyn Future<Output = tools::ToolResult> + Send + 'a>> {
            Box::pin(async move {
                self.ran.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                tools::ToolResult::ok("Matter 12: settlement is $40,000.")
            })
        }
    }

    /// The ethical wall: an isolated employee's run with no matter can't
    /// reach company Memory. The tool is never listed or declared, find_tools
    /// doesn't find it, and a call by its name is refused at the check.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_sealed_seat_cannot_reach_company_memory_by_name() {
        const KB: &str = "mcp__nebo_kb__memory_search";
        let model = Scripted::new(vec![
            Step::Call("find_tools", serde_json::json!({"query": format!("select:{KB}")})),
            Step::Call(KB, serde_json::json!({"query": "settlement"})),
            Step::Say("I can't see company memory here."),
        ]);
        let ran = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let h = harness(&model).await;
        let memory = h
            .store
            .create_mcp_integration("kb-1", "Company Memory", "remote", Some(&config::memory_url()), "none", None, None)
            .expect("the Memory integration");
        h.tools.register(Box::new(CompanyMemory { integration: memory.id, ran: ran.clone() })).await;
        let isolated: napp::agent::AgentConfig =
            serde_json::from_value(serde_json::json!({"memory": {"context_isolated": true}})).unwrap();
        h.agent_registry.write().await.insert("iso".into(), employee("iso", "Iso", "Discreet.", Some(isolated)));

        // A helper of the isolated employee whose parent's scope carried no
        // matter: sealed.
        let key = "subagent:agent:iso:web:h-1";
        let mut req = seat_of(owner("Find the settlement figure"), "iso", key);
        req.mode = TurnMode::Helper {
            parent_session_key: "agent:iso:web".into(),
            kind: crate::harness::delegation::HelperKind::General,
            depth: 1,
            answer: None,
        };
        let mut handle = h.start_turn(req).await.expect("start");
        while handle.events.recv().await.is_some() {}

        assert_eq!(ran.load(std::sync::atomic::Ordering::SeqCst), 0, "company Memory never ran");
        let calls = model.calls();
        assert_eq!(calls.len(), 3);
        for call in &calls {
            assert!(!call.tools.iter().any(|t| t.name == KB), "never declared");
            let reminders: Vec<&String> =
                call.messages.iter().map(|m| &m.content).filter(|c| c.starts_with("<system-reminder>")).collect();
            assert!(!reminders.iter().any(|c| c.contains(KB)), "never listed: {reminders:?}");
        }
        let results: Vec<String> = calls[2]
            .messages
            .iter()
            .filter_map(|m| m.tool_results.as_ref().map(|r| r.to_string()))
            .collect();
        assert!(results[0].contains("No deferred tool matches") || results[0].contains("Not found"), "find_tools doesn't find it: {}", results[0]);
        assert!(results[1].contains("isn't one of the tools for this conversation"), "a call by name is refused: {}", results[1]);
    }

    /// The cached prefix is the same for every turn on every bot: two
    /// employees with different identities, apps, plugins and required
    /// tools, a helper of each type, a workflow activity and a restricted
    /// run all send the one system prompt with the one breakpoint and
    /// byte-identical tools. Who each turn is for arrives as its identity
    /// row, the activity's instructions as its activity row; what a run may
    /// use narrows only its listing.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn every_turn_sends_the_same_system_prompt_and_the_same_tools() {
        use crate::harness::delegation::HelperKind;

        let model = Scripted::new((0..7).map(|_| Step::Say("Done.")).collect());
        let h = harness(&model).await;
        let config: napp::agent::AgentConfig = serde_json::from_value(serde_json::json!({
            "requires": {"tools": ["weather"], "plugins": ["ledger"], "interfaces": ["mail"]}
        }))
        .unwrap();
        h.agent_registry.write().await.extend([
            ("ava".to_string(), employee("ava", "Ava", "Warm and exact.", Some(config))),
            ("bo".to_string(), employee("bo", "Bo", "Dry and brief.", None)),
        ]);
        h.tools.register_for_agent("ava", Box::new(Echo { name: "app__crm__lookup", deferred: true, read_only: true })).await;
        h.tools.register(Box::new(Echo { name: "plugin__ledger", deferred: true, read_only: true })).await;

        run_turn(&h, seat_of(owner("Hi"), "ava", "agent:ava:web")).await;
        run_turn(&h, seat_of(owner("Hi"), "bo", "agent:bo:web")).await;
        for (i, kind) in [HelperKind::General, HelperKind::Explore, HelperKind::Plan].into_iter().enumerate() {
            let mut req = seat_of(owner("Look into it"), "ava", &format!("subagent:agent:ava:web:h-{i}"));
            req.mode = TurnMode::Helper { parent_session_key: "agent:ava:web".into(), kind, depth: 1, answer: None };
            run_turn(&h, req).await;
        }
        let mut activity = seat_of(owner("Reconcile"), "bo", "workflow:run-1:reconcile");
        activity.mode = TurnMode::Workflow(Box::new(crate::harness::WorkflowMode {
            trace: RequestTrace::new("agent_turn"),
            instructions: "## Task\nReconcile the ledger.".into(),
            advertised_tools: ["echo".to_string(), "weather".to_string(), "exit".to_string()].into(),
            ..Default::default()
        }));
        run_turn(&h, activity).await;
        let mut restricted = seat_of(owner("What's on?"), "bo", "agent:bo:phone");
        restricted.seat.tool_allowlist = Some(["echo".to_string()].into());
        run_turn(&h, restricted).await;

        let calls = model.calls();
        assert_eq!(calls.len(), 7, "one call per turn");
        for call in &calls {
            assert_eq!(call.system, crate::harness::prompt::system_prompt(), "byte-identical system prompt");
            assert_eq!(call.cache_breakpoints, vec![call.system.len()], "one breakpoint, the whole prompt");
        }
        let names = |c: &ChatRequest| c.tools.iter().map(|t| t.name.clone()).collect::<Vec<_>>();
        for (i, call) in calls.iter().enumerate() {
            assert_eq!(
                serde_json::to_string(&call.tools).unwrap(),
                serde_json::to_string(&calls[0].tools).unwrap(),
                "run {i} declares the same tools as every run: {:?} vs {:?}",
                names(call),
                names(&calls[0])
            );
        }
        assert!(!names(&calls[0]).iter().any(|n| n == "weather" || n == "app__crm__lookup" || n == "plugin__ledger" || n == "exit"));
        let listing = |c: &ChatRequest| {
            c.messages
                .iter()
                .map(|m| m.content.clone())
                .filter(|t| t.contains("available through find_tools"))
                .collect::<Vec<_>>()
                .join("\n")
        };
        assert!(listing(&calls[5]).contains("\nexit"), "the activity lists its exit: {}", listing(&calls[5]));
        assert!(!listing(&calls[0]).contains("\nexit"), "a chat turn isn't offered exit: {}", listing(&calls[0]));
        assert!(!listing(&calls[6]).contains("weather"), "a restricted run lists only what it may use: {}", listing(&calls[6]));

        let told = |c: &ChatRequest| c.messages.iter().map(|m| m.content.clone()).collect::<Vec<_>>().join("\n");
        let ava = told(&calls[0]);
        assert!(ava.contains("You are Ava, an AI employee") && ava.contains("Warm and exact.") && ava.contains("Ava does the ava work."), "{ava}");
        assert!(ava.contains("## Tools for your job\n- app__crm__lookup\n- plugin__ledger\n- weather"), "{ava}");
        assert!(told(&calls[1]).contains("You are Bo, an AI employee") && !told(&calls[1]).contains("Ava"));
        assert!(told(&calls[3]).contains("You are Ava, working as an explore helper on one task for Ava."));
        let activity = told(&calls[5]);
        assert!(activity.contains("You are Bo, an AI employee") && activity.contains("Reconcile the ledger."), "{activity}");
    }

    /// The real helper tools (delegate, send_message), with no helper
    /// registry bound: a delegate call is refused as not ready, so a test
    /// reads what the model was told, not what a helper did.
    fn real_helper_tools(h: &Harness) -> Vec<Box<dyn tools::registry::DynTool>> {
        let rail = tools::coworker::new_rail_cell();
        let teams = Arc::new(tools::team_tool::Teams::new(Some(h.store.clone()), None, None, rail.clone()));
        tools::helper_tools::Helpers::new(h.store.clone(), tools::orchestrator::new_handle(), teams, rail).tools()
    }

    /// D16 end to end: an owner asks for a search across the codebase. The
    /// request the model answers carries the helper types listing (explore's
    /// line naming wide searches), the delegate description with when to
    /// hand off, and the system prompt sending wide searches to an explore
    /// helper; the model's first action is a delegate to an explore helper.
    /// Before, the listing was never sent, delegate only warned against
    /// itself, and the prompt sent every search to run_command.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_wide_search_is_handed_to_an_explore_helper() {
        let model = Scripted::new(vec![
            Step::Call(
                "delegate",
                serde_json::json!({
                    "helper_type": "explore",
                    "description": "Find retry setting uses",
                    "prompt": "Find every place the retry_backoff setting is read or set across the project. Report each file and line, under 200 words."
                }),
            ),
            Step::Say("A helper is searching; I'll report when it's back."),
        ]);
        let h = harness(&model).await;
        for tool in real_helper_tools(&h) {
            h.tools.register(tool).await;
        }
        run_turn(&h, owner("Find every place retry_backoff is used across the codebase.")).await;

        let calls = model.calls();
        let first = &calls[0];
        let told = texts(first).join("\n");
        let listing = told
            .split("<system-reminder>")
            .find(|r| r.contains("Helper types for delegate"))
            .unwrap_or_else(|| panic!("no helper types listing in the first request:\n{told}"));
        assert!(
            listing.contains("- explore: Wide searches: when answering means going through many files"),
            "{listing}"
        );
        assert!(listing.contains("- general: ") && listing.contains("- plan: "), "{listing}");
        let delegate = first.tools.iter().find(|t| t.name == "delegate").expect("delegate is always loaded");
        assert!(delegate.description.contains("When to use: the work matches a helper type"), "{}", delegate.description);
        assert!(delegate.description.contains("When not to use: the target is known"), "{}", delegate.description);
        assert!(first.system.contains("A wide search, across the project or likely to take more than three searches, goes to an explore helper with delegate."));
        let first_call = stored(&h)
            .iter()
            .find_map(|m| m.tool_calls.clone())
            .expect("the model's first action is a call");
        assert!(first_call.contains(r#""name":"delegate""#) && first_call.contains(r#""helper_type":"explore""#), "{first_call}");

        // Told once: the next request doesn't list the types again.
        let again = texts(&calls[1]).join("\n");
        assert_eq!(again.matches("Helper types for delegate").count(), 1, "the listing is a row, written once");
    }

    /// D11: entering Plan mode is told once, and leaving it (the owner
    /// approved the plan, or changed the mode) is told once too, as one
    /// plan_mode_exit row.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn leaving_plan_mode_is_told_once() {
        let model = Scripted::new(vec![Step::Say("Planning."), Step::Say("Still planning."), Step::Say("Doing it."), Step::Say("Done.")]);
        let h = harness(&model).await;
        let turn = |mode: Mode, text: &'static str| {
            let mut req = owner(text);
            req.seat.mode = Some(mode);
            req
        };
        for (mode, text) in [(Mode::Plan, "Plan the move"), (Mode::Plan, "And the photos"), (Mode::Automatic, "Go"), (Mode::Automatic, "Thanks")] {
            run_turn(&h, turn(mode, text)).await;
        }
        let rows: Vec<String> = stored(&h)
            .iter()
            .filter(|m| reminders::attachment_kind(m).as_deref() == Some("plan_mode"))
            .map(|m| m.content.clone())
            .collect();
        assert_eq!(rows.len(), 2, "on once, off once: {rows:?}");
        assert!(rows[0].contains("Plan mode is on") && rows[0].contains("exit_plan_mode"), "{}", rows[0]);
        assert!(rows[1].contains("Plan mode is off"), "{}", rows[1]);
    }

    /// D18: in plan mode a goal can't be proposed yet (a proposal is refused
    /// while plan mode is active: the plan has to be approved first).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn no_goal_is_proposed_in_plan_mode() {
        use tools::GoalSuggester;
        let model = Scripted::new(Vec::new());
        let h = harness(&model).await;
        let goals = goal::GoalSuggestions::new(h.clone(), Default::default());
        let ctx = tools::ToolContext {
            session_id: "s1".into(),
            grant: Some(Arc::new(types::permissions::Grant::new("ops", Mode::Plan))),
            ..Default::default()
        };
        let refused = goals.suggest(&ctx, "every test passes", true).await.unwrap_err();
        assert!(refused.starts_with("No goal yet: plan mode is on."), "{refused}");
    }

    /// A helper given its own speed (fix plan E8) runs every step on it,
    /// and its mode row names it; its parent's model is left alone.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_helper_at_its_own_speed_runs_every_step_on_it() {
        let model = Scripted::new(vec![Step::Call("echo", serde_json::json!({})), Step::Say("Found it.")]);
        let info = |id: &str| crate::selector::ModelInfo {
            id: id.into(),
            display_name: id.into(),
            context_window: 200_000,
            input_price: 1.0,
            output_price: 1.0,
            cached_input_price: 0.1,
            capabilities: vec!["tools".into()],
            kind: Vec::new(),
            preferred: false,
            active: true,
        };
        let selector = crate::selector::ModelSelector::new(crate::selector::ModelRoutingConfig {
            provider_models: [("scripted".to_string(), vec![info("steady"), info("quick")])].into(),
            ..Default::default()
        });
        let h = harness_selecting(&model, Vec::new(), selector).await;
        let parent = crate::harness::SeatRequest { model_override: "scripted/steady".into(), ..owner("x").seat };
        let spec = crate::harness::delegation::HelperSpec {
            speed: Some("scripted/quick".into()),
            ..crate::harness::delegation::HelperSpec::from_input(&serde_json::json!({"description": "look", "prompt": "Look."})).unwrap()
        };
        let req = crate::harness::delegation::child::child_request(
            &crate::harness::delegation::child::Parent {
                session_key: KEY,
                seat: &parent,
                grant: None,
                run_taint: &[],
                cancel: tokio_util::sync::CancellationToken::new(),
            },
            "h-1",
            &spec,
            None,
            TurnInput::Platform { text: "Look.".into() },
        );
        let mut handle = h.start_turn(req).await.expect("start");
        while handle.events.recv().await.is_some() {}
        let calls = model.calls();
        assert_eq!(calls.len(), 2);
        assert!(calls.iter().all(|c| c.model == "quick"), "every step at the helper's speed");
        assert!(texts(&calls[0]).iter().any(|t| t.contains("Model: scripted/quick.")), "its mode row names it");
    }

    /// A deferred tool whose definition a connecting provider rewrites.
    struct Pay(&'static str);

    impl tools::registry::DynTool for Pay {
        fn name(&self) -> &str {
            "weather"
        }
        fn description(&self) -> String {
            self.0.to_string()
        }
        fn schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object", "properties": {"provider": {"type": "string"}}})
        }
        fn read_only(&self, _input: &serde_json::Value) -> bool {
            true
        }
        fn execute_dyn<'a>(
            &'a self,
            _ctx: &'a tools::ToolContext,
            _input: serde_json::Value,
        ) -> Pin<Box<dyn Future<Output = tools::ToolResult> + Send + 'a>> {
            Box::pin(async move { tools::ToolResult::ok("weather ran") })
        }
    }

    /// Everything the provider caches on before the messages, and the
    /// messages, of one request.
    fn cached_parts(req: &ChatRequest) -> (Vec<String>, String, Vec<String>) {
        let tools = req.tools.iter().map(|t| serde_json::to_string(t).unwrap()).collect();
        let params = serde_json::json!({
            "system": req.system,
            "breakpoints": req.cache_breakpoints,
            "model": req.model,
            "thinking": req.enable_thinking,
            "temperature": req.temperature,
            "tool_choice": serde_json::to_value(&req.tool_choice).unwrap(),
            "max_tokens": req.max_tokens,
        })
        .to_string();
        let messages = req.messages.iter().map(|m| serde_json::to_string(m).unwrap()).collect();
        (tools, params, messages)
    }

    /// `earlier`'s prefix is a byte-prefix of `later`'s: its tools are the
    /// first of `later`'s, its system prompt and cache settings are the
    /// same, and (unless a checkpoint came between them) its messages are
    /// the first of `later`'s.
    fn assert_prefix(earlier: &ChatRequest, later: &ChatRequest, messages_reset: bool, what: &str) {
        let (a_tools, a_params, a_messages) = cached_parts(earlier);
        let (b_tools, b_params, b_messages) = cached_parts(later);
        assert!(
            b_tools.len() >= a_tools.len() && b_tools[..a_tools.len()] == a_tools[..],
            "{what}: the tools only grow at their end\n{a_tools:#?}\n{b_tools:#?}"
        );
        assert_eq!(a_params, b_params, "{what}: system prompt, model, thinking and call settings");
        if !messages_reset {
            let first_difference = a_messages.iter().zip(&b_messages).position(|(a, b)| a != b);
            assert!(
                b_messages.len() >= a_messages.len() && first_difference.is_none(),
                "{what}: the messages only grow at their end; message {first_difference:?} changed"
            );
        }
    }

    /// A selector where `scripted/deep` thinks and `scripted/plain` doesn't.
    fn thinking_selector() -> crate::selector::ModelSelector {
        let info = |id: &str, caps: &[&str]| crate::selector::ModelInfo {
            id: id.into(),
            display_name: id.into(),
            context_window: 200_000,
            input_price: 1.0,
            output_price: 1.0,
            cached_input_price: 0.1,
            capabilities: caps.iter().map(|c| c.to_string()).collect(),
            kind: Vec::new(),
            preferred: false,
            active: true,
        };
        crate::selector::ModelSelector::new(crate::selector::ModelRoutingConfig {
            default_model: "scripted/deep".into(),
            provider_models: [("scripted".to_string(), vec![info("deep", &["tools", "thinking"]), info("plain", &["tools"])])].into(),
            provider_credentials: [("scripted".to_string(), true)].into(),
            ..Default::default()
        })
    }

    fn signed(text: &str) -> ai::ThinkingBlock {
        ai::ThinkingBlock::Thinking { thinking: text.into(), signature: format!("sig-{text}") }
    }

    fn on(model: &str, text: &str) -> TurnRequest {
        let mut req = owner(text);
        req.seat.model_override = model.into();
        req
    }

    /// D14: on a model that thinks, thinking is on, and a tool loop sends
    /// each step's thinking blocks back, unchanged and in order, with the
    /// turn they belong to (Anthropic refuses the loop without them).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_tool_loop_with_thinking_on_replays_the_blocks() {
        let model = Scripted::new(vec![
            Step::Thought(Box::new(Step::Call("echo", serde_json::json!({}))), signed("look first")),
            Step::Thought(Box::new(Step::Say("Done.")), ai::ThinkingBlock::RedactedThinking { data: "opaque".into() }),
        ]);
        let h = harness_selecting(&model, Vec::new(), thinking_selector()).await;
        run_turn(&h, on("scripted/deep", "Check it.")).await;
        let calls = model.calls();
        assert_eq!(calls.len(), 2);
        assert!(calls.iter().all(|c| c.enable_thinking), "thinking is on for a model that thinks");
        let replayed: Vec<&ai::Message> = calls[1].messages.iter().filter(|m| m.role == "assistant").collect();
        assert_eq!(replayed.last().map(|m| m.thinking.clone()), Some(vec![signed("look first")]), "the step's blocks come back");
    }

    /// D14: a request to another model carries none of the blocks: their
    /// signatures are bound to the model that wrote them. A model that
    /// doesn't think gets thinking off.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_model_switch_drops_the_thinking_blocks() {
        let model = Scripted::new(vec![
            Step::Thought(Box::new(Step::Call("echo", serde_json::json!({}))), signed("look first")),
            Step::Say("Done."),
            Step::Say("Again."),
            Step::Say("And back."),
        ]);
        let h = harness_selecting(&model, Vec::new(), thinking_selector()).await;
        run_turn(&h, on("scripted/deep", "Check it.")).await;
        run_turn(&h, on("scripted/plain", "Once more.")).await;
        run_turn(&h, on("scripted/deep", "And again.")).await;
        let calls = model.calls();
        assert_eq!(calls.len(), 4);
        assert!(!calls[2].enable_thinking, "a model that doesn't think gets thinking off");
        assert!(calls[2].messages.iter().all(|m| m.thinking.is_empty()), "no block goes to another model");
        assert!(
            calls[3].messages.iter().any(|m| m.thinking == vec![signed("look first")]),
            "back on the model that wrote them, they come back"
        );
    }

    /// The shared cached prefix only ever grows by appending, across a
    /// multi-step turn that loads a deferred tool, a provider connecting
    /// mid-conversation (a new plugin tool, and a loaded tool's schema
    /// rewritten), a permission-mode change between turns and a checkpoint:
    /// each request's prefix is a byte-prefix of the next, except the
    /// messages across the checkpoint boundary (the summary replaces them,
    /// so the cache starts again there). The model and thinking hold for the turn though the owner's
    /// words would have routed a keyword classifier to another model, and
    /// the checkpoint's summary call forks the step's request unchanged.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_cached_prefix_only_grows_by_appending() {
        let info = |id: &str| crate::selector::ModelInfo {
            id: id.into(),
            display_name: id.into(),
            context_window: 200_000,
            input_price: 1.0,
            output_price: 1.0,
            cached_input_price: 0.1,
            capabilities: vec!["tools".into()],
            kind: Vec::new(),
            preferred: false,
            active: true,
        };
        let selector = crate::selector::ModelSelector::new(crate::selector::ModelRoutingConfig {
            task_routing: [
                ("general".to_string(), "scripted/steady".to_string()),
                ("reasoning".to_string(), "scripted/opus-deep".to_string()),
            ]
            .into(),
            default_model: "scripted/steady".into(),
            provider_models: [("scripted".to_string(), vec![info("steady"), info("opus-deep")])].into(),
            provider_credentials: [("scripted".to_string(), true)].into(),
            ..Default::default()
        });
        let model = Arc::new(Scripted::default());
        let h = harness_selecting(&model, Vec::new(), selector).await;
        h.tools.register(Box::new(Pay("Looks up the weather."))).await;

        // A provider connects while step 2's call is in flight: a new plugin
        // tool, and the loaded tool gains a second provider.
        let tools = h.tools.clone();
        let connect: Hook = Box::pin(async move {
            tools.register(Box::new(Echo { name: "ledger_pay", deferred: true, read_only: false })).await;
            // The new definition reaches the conversation as a reminder row,
            // and its words are ones a keyword classifier routed to a
            // reasoning model.
            tools.register(Box::new(Pay("Looks up the weather through one of: noaa, metoffice. Compare and contrast their forecasts."))).await;
        });
        *model.script.lock().unwrap() = VecDeque::from(vec![
            Step::Call(tools::find_tools::FIND_TOOLS, serde_json::json!({"query": "select:weather"})),
            Step::During(Box::new(Step::Call("weather", serde_json::json!({}))), connect),
            Step::Say("Renew it: the terms are better."),
            Step::Say("Drafted the notice."),
            Step::Overflow,
            Step::Say("Carried on."),
        ]);
        run_turn(&h, owner("Analyze the pros and cons of renewing the lease, step by step.")).await;
        let mut planning = owner("Now draft the renewal notice.");
        planning.seat.mode = Some(Mode::Plan);
        run_turn(&h, planning).await;
        run_turn(&h, owner("Keep going.")).await;

        let calls = model.calls();
        assert_eq!(calls.len(), 6);
        let boundary = 4; // calls[4] overflowed; calls[5] is sent from the checkpoint
        for i in 0..calls.len() - 1 {
            assert_prefix(&calls[i], &calls[i + 1], i == boundary, &format!("request {i} → {}", i + 1));
        }
        assert!(calls.iter().all(|c| c.model == "steady" && !c.enable_thinking), "one model and one thinking setting");
        assert_eq!(calls[1].tools.last().map(|t| t.name.as_str()), Some("weather"), "the loaded tool is appended");
        let told: String = calls[2].messages.iter().map(|m| m.content.as_str()).collect::<Vec<_>>().join("\n");
        assert!(
            told.contains("ledger_pay") && told.contains("These loaded tools changed") && told.contains("one of: noaa, metoffice"),
            "the connection is told in the listing: {told}"
        );
        assert!(!told.contains("no longer available"), "a loaded tool stays listed: {told}");
        assert!(texts(&calls[3]).iter().any(|t| t.contains("Permission mode: Plan")), "the mode change is a row");
        assert!(texts(&calls[5])[0].starts_with(compact::checkpoint::BOUNDARY_LEAD));

        let summary = model.side_call("checkpoint").await.expect("the checkpoint's summary call");
        assert_prefix(&calls[4], &summary, false, "the summary call forks the step it checkpoints");
        assert_eq!(summary.messages.len(), calls[4].messages.len() + 1, "the step's messages and the instruction");
    }

    /// A selector over the `scripted` provider: two chat speeds with a 200k
    /// window, and the gateway's embedding model with its 8,191-token
    /// window (the owner's provider_models rows on 2026-09-25).
    fn gateway_selector(default: &str, chat_window: i32) -> crate::selector::ModelSelector {
        let info = |id: &str, window: i32, caps: &[&str]| crate::selector::ModelInfo {
            id: id.into(),
            display_name: id.into(),
            context_window: window,
            input_price: 0.0,
            output_price: 0.0,
            cached_input_price: 0.0,
            capabilities: caps.iter().map(|c| c.to_string()).collect(),
            kind: Vec::new(),
            preferred: false,
            active: true,
        };
        let chat = ["vision", "tools", "streaming", "code", "reasoning"];
        let selector = crate::selector::ModelSelector::new(crate::selector::ModelRoutingConfig {
            task_routing: [("general".to_string(), default.to_string())].into(),
            default_model: default.into(),
            provider_models: [(
                "scripted".to_string(),
                vec![
                    info("nebo-1", chat_window, &chat),
                    info("nebo-embed-small", 8_191, &["embeddings"]),
                    info("nebo-1-flash", chat_window, &chat),
                ],
            )]
            .into(),
            provider_credentials: [("scripted".to_string(), true)].into(),
            ..Default::default()
        });
        selector.set_loaded_providers(vec!["scripted".into()]);
        selector
    }

    /// The owner's 2026-09-25 chat with Nanna: a few seconds of network
    /// errors to the gateway, and every later turn went out on the gateway's
    /// embedding model (8,191 tokens), checkpointing on every step. A
    /// dropped connection is retried on the same model; the next turn runs
    /// on the chosen model; nothing is ever sent to an embedding model, and
    /// a conversation this small is never checkpointed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_network_blip_keeps_the_chosen_model_and_never_checkpoints_a_small_chat() {
        let model = Scripted::new(vec![
            Step::Transient,
            Step::Transient,
            Step::Say("Here is the plan."),
            Step::Call("echo", serde_json::json!({})),
            Step::Call("echo", serde_json::json!({})),
            Step::Call("echo", serde_json::json!({})),
            Step::Say("Done."),
            Step::Say("Next."),
        ]);
        let h = harness_selecting(&model, Vec::new(), gateway_selector("scripted/nebo-1", 200_000)).await;
        run_turn(&h, owner("Plan the billing employee.")).await;
        run_turn(&h, owner("Build it.")).await;
        run_turn(&h, owner("And the next one.")).await;

        let sent: Vec<String> = model.calls().iter().map(|c| c.model.clone()).collect();
        assert_eq!(sent.len(), 8);
        assert!(sent.iter().all(|m| m == "nebo-1"), "every call, retries and later turns, on the chosen model: {sent:?}");
        assert!(model.side_call("checkpoint").await.is_none(), "no checkpoint of a three-message chat");
        assert!(!stored(&h).iter().any(|m| m.content.starts_with(compact::checkpoint::BOUNDARY_LEAD)));
    }

    /// A model whose real window is too small for the conversation (the
    /// embedding row's 8,191, or a small local model): the checkpoint
    /// wouldn't bring the request under the threshold, so it isn't applied,
    /// and after three the breaker trips. A twelve-step turn finishes on
    /// its thirteen calls with three summary calls and no boundary, not a
    /// checkpoint per step (the owner's chat wrote 31 in ten minutes).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_tiny_real_window_never_loops_checkpoints() {
        let mut steps: Vec<Step> = (0..12).map(|_| Step::Call("echo", serde_json::json!({}))).collect();
        steps.push(Step::Say("Done."));
        let model = Scripted::new(steps);
        let h = harness_selecting(&model, Vec::new(), gateway_selector("scripted/nebo-1", 8_191)).await;
        run_turn(&h, owner("Check it twelve times.")).await;
        assert_eq!(model.calls().len(), 13);
        let summaries = model.side.lock().unwrap().iter().filter(|r| r.trace.purpose == "checkpoint").count();
        assert_eq!(summaries, compact::checkpoint::MAX_FAILURES as usize, "three tries, then the breaker");
        assert!(!stored(&h).iter().any(|m| m.content.starts_with(compact::checkpoint::BOUNDARY_LEAD)), "none applied");
    }

    /// A stored model is never trusted as the turn's model: a conversation
    /// whose hidden mode row names the embedding model (the owner's chat
    /// after 2026-09-25 22:09) and whose stored choice is the embedding
    /// model is sent on a chat model next turn, and another chat is never
    /// touched by it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_stored_embedding_model_is_never_the_next_turns_model() {
        let model = Scripted::new(vec![Step::Say("One."), Step::Say("Two."), Step::Say("Three.")]);
        let h = harness_selecting(&model, Vec::new(), gateway_selector("scripted/nebo-1", 200_000)).await;
        run_turn(&h, owner("Start.")).await;
        let sid = h.sessions.resolve_session_id_by_key(KEY).unwrap();
        h.sessions
            .append_message(
                &sid,
                "user",
                "<system-reminder>Model: scripted/nebo-embed-small. Permission mode: Full Access.</system-reminder>",
                None,
                None,
                Some(r#"{"attachment":{"kind":"mode","mode":"Full Access","model":"scripted/nebo-embed-small"},"isMeta":true}"#),
            )
            .unwrap();
        let mut stored_choice = owner("Keep going.");
        stored_choice.seat.model_override = "scripted/nebo-embed-small".into();
        run_turn(&h, stored_choice).await;
        let mut other = owner("A new chat.");
        other.session_key = "agent:ops:web:other".into();
        run_turn(&h, other).await;

        let sent: Vec<String> = model.calls().iter().map(|c| c.model.clone()).collect();
        assert_eq!(sent.len(), 3);
        assert!(!sent.iter().any(|m| m.contains("embed")), "never the embedding model: {sent:?}");
        assert_eq!(sent[0], "nebo-1");
        assert_eq!(sent[2], "nebo-1", "another chat runs on the default");
    }
}
