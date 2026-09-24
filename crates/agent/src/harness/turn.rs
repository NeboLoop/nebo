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

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex};

use ai::{ChatRequest, RequestTrace, StreamEvent};
use db::models::ChatMessage;
use tokio::sync::mpsc;
use tracing::{info, warn};

use super::conversation::{self, InputRow, MidTurnFrom};
use super::events::{self, TurnEvent};
use super::model_call::{self, CallOutcome, RetryWhy};
use super::prompt::{self, PromptInputs, SystemPrompt, sections};
use super::seat::{self, ApprovalMode, Seat};
use super::session_gate::{self, Admission, RunProgress, TurnGuard};
use super::tool_round::{self, RoundContext, RoundGuards, RoundOutcome, RunToolScope};
use super::tool_surface::{self, SurfaceInputs};
use super::turn_end::{self, EndVerdict};
use super::{Harness, HarnessError, TurnHandle, TurnInput, TurnMode, TurnRequest, compact, goal, reminders, usage};
use crate::pruning::{self, ContextThresholds};
use crate::runner::RunState;
use crate::selector;

/// Steps one turn takes before it ends with `MaxSteps` (Claude Code's
/// max-turns option).
pub const DEFAULT_MAX_STEPS: u32 = 100;
/// Context window assumed for a model that reports none.
const DEFAULT_CONTEXT_WINDOW: usize = 80_000;
/// Metadata of a notification row: the model reads it, the owner's thread
/// never shows it.
const NOTIFICATION_ROW_METADATA: &str = r#"{"notification":true,"isMeta":true}"#;

/// What one turn runs with, fixed for the turn.
pub struct TurnContext {
    pub harness: Harness,
    pub request: TurnRequest,
    pub seat: Seat,
    /// The employee's registry entry, when the turn has one.
    pub agent: Option<tools::ActiveAgent>,
    /// The session row id (the request carries the key).
    pub session_id: String,
    pub channel: String,
    /// The owner's IANA timezone, when set: the date is theirs.
    pub timezone: Option<String>,
    /// The model the owner or the employee chose (`provider/model`); empty
    /// means the selector's.
    pub model: String,
    /// Built once per turn.
    pub prompt: SystemPrompt,
    pub tx: mpsc::Sender<StreamEvent>,
    pub progress: RunProgress,
    pub max_steps: u32,
    /// The owner's spending limit for the run, microcents; 0 = none.
    pub spend_cap_microcents: i64,
    /// Declared on every step: the employee's `requires.tools`, and the
    /// plugin tool when its job needs plugins.
    pub always_load: HashSet<String>,
    /// The run's provenance: seeded by the input, grown by its tool calls,
    /// stamped on its last event.
    pub taint: Mutex<BTreeSet<types::provenance::ProvenanceClass>>,
    /// After the turn: memory extraction, personality and the chat title.
    pub after_turn: bool,
}

impl TurnContext {
    fn full_access(&self) -> bool {
        self.request.seat.approval_mode == ApprovalMode::FullAccess
    }

    fn approval_relay(&self) -> bool {
        self.request.seat.approval_mode == ApprovalMode::Relay
    }

    fn plan_mode(&self) -> bool {
        self.request.seat.approval_mode == ApprovalMode::Plan
    }

    fn workflow(&self) -> Option<&crate::runner::WorkflowMode> {
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
    /// Deferred tools loaded in the conversation, derived from it each step.
    pub loaded_tools: BTreeSet<String>,
    pub surfaced_memories: HashSet<String>,
    pub end_checks_this_turn: u8,
    pub frozen_renderings: compact::trim::Frozen,
    pub read_ledger: crate::read_ledger::ReadLedger,
    /// The conversation the last step sent: input stored after it is heard
    /// by the next turn.
    pub seen: Vec<ChatMessage>,
    /// The model the last call ran on (`provider/model` or the name).
    pub model: String,
    /// The date the prompt was built with, moved by a `DateChanged` row.
    pub date: chrono::NaiveDate,
    /// Checkpoints taken this turn.
    pub checkpoints: usize,
    plan_approved: bool,
    persisted_renderings: HashSet<String>,
    trim_spec: pruning::TrimSpec,
    round: RoundCarry,
}

/// What the tool round keeps from one round to the next within a turn.
#[derive(Default)]
struct RoundCarry {
    called_tools: Vec<String>,
    identical_call_budget: ai::call_budget::CallBudget,
    runaway_wrap_up: Option<String>,
    runaway_wrap_up_issued: bool,
    read_failures: HashMap<String, usize>,
    action_call_counts: HashMap<String, usize>,
    spiral_escalator: crate::guardrails::Escalator,
    error_streak: crate::guardrails::ErrorStreak,
    files_read_this_session: HashSet<String>,
    recent_result_content_hashes: Vec<u64>,
    readonly_result_hash_by_call: HashMap<(u64, u64), u64>,
    tool_doc_cache: Vec<(String, String)>,
    plan_touch: Option<(usize, String)>,
    edits_since_check: usize,
    last_desktop_act: Option<String>,
    spilled_results: usize,
}

/// Why the loop is taking its next step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transition {
    First,
    AfterTools,
    MidTurnInput,
    CutoffResume { attempt: u8 },
    OutputEscalated,
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
    PlanProposed,
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
            TurnExit::Cancelled => "cancelled".into(),
            TurnExit::MaxSteps { steps } => format!("max_iterations_reached({steps})"),
            TurnExit::BudgetReached => "spend_cap_reached".into(),
            TurnExit::TerminalTool { .. } => "terminal_tool_error".into(),
            TurnExit::WorkflowEnded(reason) => reason.clone(),
            TurnExit::ProviderFailed(_) => "provider_failed".into(),
            TurnExit::Refused(_) => "refused".into(),
            TurnExit::AwaitingApproval => "awaiting_approval".into(),
            TurnExit::PlanProposed => "plan_proposed".into(),
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
    });
    let turn_id = progress.run_id.clone();

    let queue = || queue_input(&h, &session.id, &req);
    let admission =
        session_gate::admit_or_queue(&h.active_turns, &req.session_key, progress.clone(), req.cancel.clone(), queue)
            .await;
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

/// Write a busy session's input where its running turn hears it at the
/// next step. Runs under the admission lock.
fn queue_input(h: &Harness, session_id: &str, req: &TurnRequest) {
    let written = match &req.input {
        TurnInput::Owner { text, .. } => {
            let via = if req.delivery.channel.is_empty() { "chat" } else { &req.delivery.channel };
            let meta = MidTurnFrom::Owner { via: via.to_string() }.metadata();
            h.sessions.append_message(session_id, "user", text, None, None, Some(&meta)).map(|_| ())
        }
        TurnInput::Platform { text } => h
            .sessions
            .append_message(session_id, "user", text, None, None, Some(r#"{"isMeta":true,"hiddenPrompt":true}"#))
            .map(|_| ()),
        TurnInput::Notification(c) => h
            .sessions
            .append_message(
                session_id,
                "user",
                &super::delegation::render_notification(c),
                None,
                None,
                Some(NOTIFICATION_ROW_METADATA),
            )
            .map(|_| ()),
        TurnInput::None => Ok(()),
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
        .any(|m| m.role == "user" && (conversation::arrived_mid_turn(m).is_some() || is_notification_row(m)))
}

fn is_notification_row(msg: &ChatMessage) -> bool {
    msg.metadata
        .as_deref()
        .and_then(|m| serde_json::from_str::<serde_json::Value>(m).ok())
        .and_then(|v| v.get("notification").and_then(|b| b.as_bool()))
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
        let mut full_access = s.approval_mode == ApprovalMode::FullAccess;
        seat::restrict_outside_origin(s.origin, &mut full_access, &mut s.tool_allowlist, &mut s.tool_denial_hint);
        if !full_access && s.approval_mode == ApprovalMode::FullAccess {
            s.approval_mode = ApprovalMode::Ask;
        }
    }
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
    let raw_model = if !req.seat.model_override.is_empty() {
        req.seat.model_override.clone()
    } else {
        req.seat.model_preference.clone().unwrap_or_default()
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
    let date = sections::owner_today(memory.timezone.as_deref());
    let memory_timezone = memory.timezone.clone();
    let role = match &req.mode {
        TurnMode::Helper { parent_session_key, .. } => prompt::Role::Helper { parent: parent_name(h, parent_session_key).await },
        _ => prompt::Role::Employee,
    };
    let self_context = agent
        .as_ref()
        .map(|a| {
            [
                prompt::inputs::self_context(a),
                prompt::inputs::plugin_context(a, req.seat.tool_scope.as_deref(), h.skill_loader.as_deref()),
            ]
            .into_iter()
            .filter(|p| !p.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n\n")
        })
        .unwrap_or_default();
    let team = h
        .store
        .list_agents(100, 0)
        .unwrap_or_default()
        .into_iter()
        .filter(|a| a.is_enabled == 1)
        .map(|a| (a.name, a.description))
        .collect();
    let prompt = SystemPrompt::build(&PromptInputs {
        name,
        role,
        personality_snippet: req.seat.personality_snippet.clone(),
        soul: agent.as_ref().and_then(|a| a.soul.clone()),
        rules: agent.as_ref().and_then(|a| a.rules.clone()),
        persona: agent.as_ref().map(|a| prompt::inputs::persona_body(&a.agent_md)),
        environment: sections::Environment {
            date,
            timezone: memory.timezone.clone(),
            model: if model.is_empty() { h.selector.select(&[]) } else { model.clone() },
            cwd: req.seat.cwd.clone(),
            channel: channel.clone(),
            watching: seat.execution_mode.into(),
            permission_mode: permission_mode_name(req.seat.approval_mode).to_string(),
        },
        employee_memory: memory.section,
        team,
        workspace_notes: prompt::inputs::workspace_notes(),
        self_context,
    });

    let always_load = agent.as_ref().and_then(|a| a.config.as_ref()).map(|cfg| {
        let mut set: HashSet<String> = cfg.requires.tools.iter().cloned().collect();
        let scope_plugins = req
            .seat
            .tool_scope
            .as_deref()
            .and_then(|s| cfg.scopes.get(s))
            .is_some_and(|s| !s.plugins.is_empty());
        if !cfg.requires.plugins.is_empty() || scope_plugins {
            set.insert("plugin".to_string());
        }
        set
    });

    let (max_steps, spend_cap_microcents) = match &req.mode {
        TurnMode::Workflow(m) => (DEFAULT_MAX_STEPS, m.spend_cap_microcents),
        TurnMode::Fork(_) => (crate::review_fork::REVIEW_MAX_ITERATIONS as u32, 0),
        _ => (DEFAULT_MAX_STEPS, 0),
    };
    let after_turn = matches!(req.mode, TurnMode::Chat) && !seat.memory.writes_disabled;
    let taint = Mutex::new(req.seat.seed_taint.iter().copied().collect());

    let mut st = TurnState {
        step: 0,
        transition: Transition::First,
        reminders: reminders::Reminders::default(),
        call: model_call::CallState::default(),
        usage: RunState::new(),
        loaded_tools: BTreeSet::new(),
        surfaced_memories: HashSet::new(),
        end_checks_this_turn: 0,
        frozen_renderings: h
            .store
            .get_chat_renderings(&h.store.resolve_session_chat_id(session_id))
            .unwrap_or_default(),
        read_ledger: Default::default(),
        seen: Vec::new(),
        model: model.clone(),
        date,
        checkpoints: 0,
        plan_approved: false,
        persisted_renderings: HashSet::new(),
        trim_spec: pruning::TrimSpec::new(),
        round: RoundCarry::default(),
    };
    st.persisted_renderings = st.frozen_renderings.keys().cloned().collect();

    // The first step's events.
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

    let cx = TurnContext {
        harness: h.clone(),
        request: req,
        seat,
        agent,
        session_id: session_id.to_string(),
        channel,
        timezone: memory_timezone,
        model,
        prompt,
        tx,
        progress,
        max_steps,
        spend_cap_microcents,
        always_load: always_load.unwrap_or_default(),
        taint,
        after_turn,
    };
    if cx.plan_mode() && !plan_mode_announced(&h.sessions, session_id) {
        st.reminders.add(&TurnEvent::PlanMode { entered: true });
    }
    Ok((cx, st))
}

/// Store the turn's input as its row.
async fn store_input(h: &Harness, session_id: &str, req: &TurnRequest) -> Result<(), String> {
    let (text, images, attachments, hidden): (&str, &[ai::ImageContent], &[comm::wire::Attachment], bool) =
        match &req.input {
            TurnInput::Owner { text, images, attachments } => (text, images, attachments, false),
            TurnInput::Platform { text } => (text, &[], &[], true),
            TurnInput::Notification(c) => {
                return h
                    .sessions
                    .append_message(
                        session_id,
                        "user",
                        &super::delegation::render_notification(c),
                        None,
                        None,
                        Some(NOTIFICATION_ROW_METADATA),
                    )
                    .map(|_| ())
                    .map_err(|e| format!("failed to store the notification: {e}"));
            }
            TurnInput::None => return Ok(()),
        };
    if text.is_empty() {
        return Ok(());
    }
    conversation::persist_input(
        &h.sessions,
        &h.providers,
        &h.selector,
        &req.seat.agent_id,
        session_id,
        InputRow { text, images, attachments, hidden },
    )
    .await
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
fn permission_mode_name(mode: ApprovalMode) -> &'static str {
    match mode {
        ApprovalMode::Ask => "Ask",
        ApprovalMode::AcceptEdits => "Accept edits",
        ApprovalMode::Plan => "Plan",
        ApprovalMode::FullAccess => "Full Access",
        ApprovalMode::NeverAsk => "Never ask",
        ApprovalMode::Automatic => "Automatic",
        ApprovalMode::Relay => "Relay",
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
    let guard_cfg = crate::guardrails::GuardrailConfig::from_json(&h.store.get_guardrails().unwrap_or_else(|_| "{}".into()))
        .sanitized();
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
            always_load: &cx.always_load,
            allowlist: cx.request.seat.tool_allowlist.as_ref(),
            company_memory_sealed: cx.seat.company_memory_sealed,
            workflow: cx.workflow(),
        };
        let surface = tool_surface::surface(&h.tools, &h.store, &conversation, &surface_seat).await;
        st.loaded_tools = surface.loaded.clone();
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
        cx.taint
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .extend(conversation::parent_taint(&conversation));

        // 3. Trim, and checkpoint past the threshold.
        let thresholds = thresholds(cx, st, &surface.declared);
        let window = trim(cx, st, &conversation, thresholds.warning).await;
        st.usage.last_request_estimate = pruning::estimate_total_tokens(&window);
        let window = conversation::sanitize_message_order(window);

        // 4-5. The request and the call.
        let (provider_id, model_name, selected) = select_model(cx, &window);
        st.model = selected.clone();
        let request = build_request(cx, st, &window, surface.declared, &model_name);
        if pruning::estimate_total_tokens(&window) > thresholds.auto_compact && st.checkpoints == 0 {
            match checkpoint(cx, st, &conversation, &request, compact::checkpoint::CheckpointReason::Threshold).await {
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
            full_access: cx.full_access(),
            handoff_depth: cx.request.seat.handoff_depth,
            entity_permissions: cx.request.seat.permissions.as_ref(),
            operation_policy: cx.request.seat.operation_policy.as_ref(),
            entity_resource_grants: cx.request.seat.resource_grants.as_ref(),
            allowed_paths: &cx.request.seat.allowed_paths,
            run_cwd: cx.request.seat.cwd.as_deref(),
            channel_ctx: cx.request.delivery.channel_ctx.as_ref(),
            model_override: &cx.model,
            memory_user_id: &memory_user_id,
            memory_topics: &cx.seat.memory_topics,
            memory_writes_disabled: cx.seat.memory.writes_disabled,
            memory_write_bar: &cx.seat.write_bar,
            audience_restricted: cx.seat.audience_restricted,
            memory_matter: &cx.seat.memory_matter,
            run_taint: &cx.taint,
            review_fork: None,
            tool_allowlist: cx.request.seat.tool_allowlist.as_ref(),
            tool_denial_hint: &cx.request.seat.tool_denial_hint,
            declared_tools: &declared_names,
        };
        let issue_credential = h.tool_credentials.as_ref().map(|credentials| {
            let tool_scope = &tool_scope;
            move || {
                credentials.issue(crate::tool_credentials::RunGrant {
                    ctx: tool_scope.tool_context(),
                    agent_id: cx.agent_id().to_string(),
                    approval: h.approval_channels.as_ref().map(|channels| crate::tool_credentials::OwnedApprovalDoor {
                        channels: channels.clone(),
                        tx: cx.tx.clone(),
                        cancel_token: cx.request.cancel.clone(),
                    }),
                    approval_relay: cx.approval_relay(),
                    workflow_mode: cx.workflow().cloned(),
                    sessions: Some(sessions.clone()),
                })
            }
        });
        let fork_of = request.clone();
        // A call never carries a stream reminder; what `call_model` would
        // queue for a cut stream is dropped and the step is taken again.
        let mut no_stream_reminders = Vec::new();
        let outcome = model_call::call_model(
            model_call::ModelCall {
                request,
                providers: &h.providers,
                selector: &h.selector,
                concurrency: &h.concurrency,
                sessions,
                cancel: &cx.request.cancel,
                tx: &cx.tx,
                session_id: sid,
                step: st.step as usize,
                step_started: std::time::Instant::now(),
                selected_provider_id: &provider_id,
                selected_model: &selected,
                model_override: &cx.model,
                context_limit: thresholds.auto_compact,
                tool_credential: issue_credential
                    .as_ref()
                    .map(|issue| issue as &(dyn Fn() -> crate::tool_credentials::CredentialGuard + Send + Sync)),
            },
            &mut st.call,
            &mut st.usage,
            &mut no_stream_reminders,
        )
        .await;
        let reply = match outcome {
            CallOutcome::Reply(reply) => reply,
            CallOutcome::Retry(RetryWhy::Overflow) => {
                match checkpoint(cx, st, &conversation, &fork_of, compact::checkpoint::CheckpointReason::Overflow).await {
                    Ok(()) => st.transition = Transition::OverflowCheckpointed,
                    Err(e) => {
                        warn!(session_id = sid, error = %e, "overflow checkpoint failed; trimming harder");
                        st.transition = Transition::TransientRetry { attempt: st.call.overflow_retries as u8 };
                    }
                }
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
        } = reply;
        let text = post_receive(cx, text, tool_calls.len()).await;
        if stream_error.is_some() {
            // Calls that arrived on a broken stream are not run or stored.
            tool_calls.clear();
        }
        save_reply(cx, &text, &tool_calls, &block_order).await;

        if !tool_calls.is_empty() {
            if cx.plan_mode() && !st.plan_approved {
                match approve_plan(cx, &text, &tool_calls).await {
                    Some(true) => {
                        st.plan_approved = true;
                        st.reminders.add(&TurnEvent::PlanMode { entered: false });
                    }
                    Some(false) => return TurnExit::PlanProposed,
                    None => {}
                }
            }
            // A CLI provider ran its tools itself over /agent/mcp.
            if provider.handles_tools() {
                return TurnExit::Answered;
            }
            match tool_round(cx, st, &tool_scope, &guard_cfg, &side_trace, &text, &mut tool_calls).await {
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
            Some(model_call::StepRetry::WithReminder(_)) => {
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
        if model_call::lost_tool_calls(&mut st.call, stop.as_deref(), &tool_calls, st.step as usize, sid).is_some() {
            st.transition = Transition::TransientRetry {
                attempt: st.call.lost_toolcall_retries as u8,
            };
            st.step -= 1;
            continue;
        }
        if text.trim().is_empty() {
            if model_call::retry_empty_reply(&mut st.call, st.step as usize, sid) {
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

    let today = sections::owner_today(cx.timezone.as_deref());
    if today != st.date {
        st.date = today;
        st.reminders.add(&TurnEvent::DateChanged(today));
    }

    if let Some(delta) = listing {
        st.reminders.add(&TurnEvent::ToolsAvailable(delta));
    }
    if let (Some(loader), None) = (h.skill_loader.as_ref(), cx.workflow()) {
        let scope = (!cx.agent_id().is_empty()).then_some(cx.agent_id());
        let now: events::Listing = loader
            .list_summaries(scope)
            .await
            .into_iter()
            .filter(|s| s.enabled)
            .map(|s| (s.name, s.description))
            .collect();
        let announced = events::announced("skill_listing", conversation);
        if let Some(delta) = events::LinedDelta::between(&announced, &now) {
            st.reminders.add(&TurnEvent::SkillListing(delta));
        }
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

/// The context thresholds for this turn's model, tightened by the observed
/// undercount of the local estimate.
fn thresholds(cx: &TurnContext, st: &mut TurnState, declared: &[ai::ToolDefinition]) -> ContextThresholds {
    let correction = st.usage.estimate_correction;
    let h = &cx.harness;
    let prompt_chars = cx.prompt.text().len();
    let schema_chars: usize = declared.iter().map(|t| t.description.len() + t.input_schema.to_string().len()).sum();
    st.usage.system_overhead_tokens = (prompt_chars + schema_chars) / crate::CHARS_PER_TOKEN;
    let overhead = st.usage.system_overhead_tokens + 4_000;
    st.usage
        .thresholds
        .get_or_insert_with(|| {
            let model = if cx.model.is_empty() { h.selector.select(&[]) } else { cx.model.clone() };
            let window = h
                .selector
                .get_model_info(&model)
                .map(|m| m.context_window as usize)
                .filter(|&w| w > 0)
                .unwrap_or(DEFAULT_CONTEXT_WINDOW);
            ContextThresholds::from_context_window(window, overhead)
        })
        .adjusted(correction)
}

/// Old tool results trimmed to stubs, each rendering frozen the first time
/// it is chosen and persisted for the chat.
async fn trim(cx: &TurnContext, st: &mut TurnState, conversation: &[ChatMessage], budget: usize) -> Vec<ChatMessage> {
    let h = &cx.harness;
    crate::runner::extend_trim_spec(&h.tools, conversation, &mut st.trim_spec).await;
    let (working, _) = pruning::time_based_micro_compact(
        conversation,
        pruning::TIME_BASED_KEEP_RECENT,
        pruning::TIME_BASED_GAP_THRESHOLD_SECS,
        budget,
        &mut st.frozen_renderings,
        &st.trim_spec,
    );
    let (working, _) = pruning::micro_compact(&working, budget, &mut st.frozen_renderings, &st.trim_spec);
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
    working
}

/// The provider, model name and full model id for this step.
fn select_model(cx: &TurnContext, window: &[ChatMessage]) -> (String, String, String) {
    let selected = if cx.model.is_empty() { cx.harness.selector.select(window) } else { cx.model.clone() };
    if selected.is_empty() {
        return (String::new(), String::new(), selected);
    }
    let (provider, name) = selector::parse_model_id(&selected);
    (provider.to_string(), name.to_string(), selected)
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
    let h = &cx.harness;
    let enable_thinking = cx.workflow().is_none()
        && !model_name.is_empty()
        && h.selector.classify_task(window) == selector::TaskType::Reasoning
        && h.selector.supports_thinking(&st.model);
    ChatRequest {
        tool_credential: None,
        tool_choice: Default::default(),
        messages: conversation::convert_messages(window),
        tools: declared,
        max_tokens: st.call.max_output_tokens(),
        temperature: if cx.workflow().is_some() { 0.0 } else { 0.7 },
        system: cx.prompt.text(),
        static_system: format!("{}\n\n{}", cx.prompt.fixed, cx.prompt.employee),
        model: model_name.to_string(),
        enable_thinking,
        metadata: st.call.sticky_metadata.clone(),
        cache_breakpoints: cx.prompt.cache_breakpoints(),
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

/// Checkpoint the conversation (WP2.6 writes the boundary).
async fn checkpoint(
    cx: &TurnContext,
    st: &mut TurnState,
    _conversation: &[ChatMessage],
    _fork_of: &ChatRequest,
    why: compact::checkpoint::CheckpointReason,
) -> Result<(), String> {
    compact::checkpoint::checkpoint(cx, why).await?;
    st.checkpoints += 1;
    st.seen.clear();
    Ok(())
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

/// Store the reply with its tool calls and its block order.
async fn save_reply(cx: &TurnContext, text: &str, tool_calls: &[ai::ToolCall], block_order: &[(&'static str, Option<usize>)]) {
    if text.is_empty() && tool_calls.is_empty() {
        return;
    }
    let calls = (!tool_calls.is_empty()).then(|| serde_json::to_string(tool_calls).ok()).flatten();
    let metadata = (block_order.len() > 1 || block_order.first().is_some_and(|b| b.0 == "tool")).then(|| {
        let blocks: Vec<serde_json::Value> = block_order
            .iter()
            .map(|(kind, idx)| match (*kind, idx) {
                ("tool", Some(i)) => serde_json::json!({"type": "tool", "toolCallIndex": i}),
                _ => serde_json::json!({"type": "text"}),
            })
            .collect();
        serde_json::json!({ "contentBlocks": blocks }).to_string()
    });
    let h = &cx.harness;
    if let Err(e) = h.sessions.append_message(&cx.session_id, "assistant", text, calls.as_deref(), None, metadata.as_deref()) {
        warn!(session_id = %cx.session_id, error = %e, "failed to save the reply");
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

/// Plan mode: the first tool calls wait for the owner's approval of the
/// plan. `None` when no one can answer (no ask channel).
async fn approve_plan(cx: &TurnContext, text: &str, tool_calls: &[ai::ToolCall]) -> Option<bool> {
    let channels = cx.harness.ask_channels.as_ref()?;
    let names: Vec<String> = tool_calls.iter().map(|tc| tc.name.clone()).collect();
    let plan = if text.is_empty() {
        format!("I'd like to run {} tool calls: {}", names.len(), names.join(", "))
    } else {
        text.to_string()
    };
    let request_id = uuid::Uuid::new_v4().to_string();
    let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
    channels.lock().await.insert(request_id.clone(), resp_tx);
    let _ = cx.tx.send(StreamEvent::plan_approval_request(&request_id, &plan, names)).await;
    let approved = tokio::select! {
        _ = cx.request.cancel.cancelled() => {
            channels.lock().await.remove(&request_id);
            return Some(false);
        }
        answer = resp_rx => answer.is_ok_and(|v| matches!(v.to_lowercase().as_str(), "approve" | "approved" | "yes" | "true")),
    };
    if !approved {
        const REJECTED: &str = "Plan was rejected. Let me know how you'd like to proceed.";
        let _ = cx.tx.send(StreamEvent::text(format!("\n\n{REJECTED}"))).await;
        let _ = cx.harness.sessions.append_message(&cx.session_id, "assistant", REJECTED, None, None, None);
    }
    Some(approved)
}

/// Run the reply's tool calls. `Some` ends the turn.
async fn tool_round(
    cx: &TurnContext,
    st: &mut TurnState,
    scope: &RunToolScope<'_>,
    guard_cfg: &crate::guardrails::GuardrailConfig,
    side_trace: &(dyn Fn(&'static str) -> RequestTrace + Sync),
    text: &str,
    tool_calls: &mut [ai::ToolCall],
) -> Option<TurnExit> {
    let h = &cx.harness;
    let no_objective = String::new();
    let carry = &mut st.round;
    let outcome = tool_round::run_tool_round(
        &RoundContext {
            scope,
            tools: &h.tools,
            store: &h.store,
            providers: &h.providers,
            concurrency: &h.concurrency,
            hooks: &h.hooks,
            agent_id: cx.agent_id(),
            user_prompt: "",
            iteration: st.step as usize,
            approval_channels: h.approval_channels.as_ref(),
            approval_relay: cx.approval_relay(),
            workflow_mode: cx.workflow(),
            decide: None,
            active_task: &no_objective,
            guard_cfg,
            side_trace,
        },
        RoundGuards {
            called_tools: &mut carry.called_tools,
            recent_tool_result_hashes: &[],
            identical_call_budget: &carry.identical_call_budget,
            runaway_wrap_up: &mut carry.runaway_wrap_up,
            runaway_wrap_up_issued: &mut carry.runaway_wrap_up_issued,
            read_failures: &mut carry.read_failures,
            action_call_counts: &mut carry.action_call_counts,
            spiral_escalator: &mut carry.spiral_escalator,
            error_streak: &mut carry.error_streak,
            files_read_this_session: &mut carry.files_read_this_session,
            recent_result_content_hashes: &mut carry.recent_result_content_hashes,
            readonly_result_hash_by_call: &mut carry.readonly_result_hash_by_call,
            read_ledger: &mut st.read_ledger,
            tool_doc_cache: &mut carry.tool_doc_cache,
            plan_touch: &mut carry.plan_touch,
            edits_since_check: &mut carry.edits_since_check,
            last_desktop_act: &mut carry.last_desktop_act,
            ctx_spilled_results: &mut carry.spilled_results,
        },
        tool_calls,
    )
    .await;
    let results = match outcome {
        RoundOutcome::Ran(results) => results,
        RoundOutcome::Cancelled => return Some(TurnExit::Cancelled),
        RoundOutcome::Ended(crate::guardrails::Exit::Workflow(reason)) if reason == "awaiting_approval" => {
            return Some(TurnExit::AwaitingApproval);
        }
        RoundOutcome::Ended(crate::guardrails::Exit::Workflow(reason)) => return Some(TurnExit::WorkflowEnded(reason)),
        // The round sent the owner its notice (with the need a tool named).
        RoundOutcome::Ended(exit) => {
            return Some(TurnExit::TerminalTool {
                notice: exit.label(),
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
    for check in turn_end::registry(&cx.request.mode) {
        match check.check(cx, st).await {
            EndVerdict::Stop => {}
            EndVerdict::Exit(exit) => return Some(Err(exit)),
            EndVerdict::Continue { reminder } => {
                st.end_checks_this_turn += 1;
                st.reminders.add(&TurnEvent::AppHook {
                    label: check.name().to_string(),
                    text: reminder.clone(),
                });
                st.transition = Transition::EndCheckContinue {
                    check: check.name(),
                    reason: reminder,
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
    if *exit == TurnExit::Cancelled {
        conversation::record_interrupt(&h.sessions, &cx.session_id);
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
    usage::send_context_stats(&cx.tx, st.read_ledger.stats(), 0, st.checkpoints, st.round.spilled_results, &st.usage).await;
    if !cx.after_turn || *exit == TurnExit::Cancelled {
        return;
    }
    super::after_turn::MemoryExtraction {
        sessions: &h.sessions,
        session_id: &cx.session_id,
        providers: &h.providers,
        store: &h.store,
        concurrency: &h.concurrency,
        embedding_provider: h.embedding_provider.as_ref(),
        decide: None,
        memory_user_id: &cx.seat.memory.user_id,
        memory_topics: &cx.seat.memory_topics,
        memory_write_bar: &cx.seat.write_bar,
        run_taint: &cx.taint,
        objective: "",
        skip_memory: false,
        gate_trace: cx.trace("memory_gate"),
        trace: cx.trace("memory_extract"),
    }
    .schedule()
    .await;
    super::after_turn::spawn_personality_synthesis(&h.store, &h.providers, &cx.seat.memory.user_id, &h.concurrency).await;
    super::after_turn::spawn_chat_title_generation(
        h.providers.clone(),
        h.store.clone(),
        h.sessions.active_chat_id(&cx.session_id),
        cx.session_id.clone(),
        h.selector.get_cheapest_model(),
        h.title_sink.clone(),
    );
}
