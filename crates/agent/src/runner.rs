use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{RwLock, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use ai::{
    Answer, ChatRequest, Message, Question, Provider, ProviderError, RequestTrace, StreamEvent, StreamEventType,
};
use db::Store;
use db::models::ChatMessage;
use tools::{Origin, Registry};

use crate::concurrency::ConcurrencyController;
use crate::db_context;
use crate::harness::compact::trim;
use crate::harness::conversation::{
    InputRow, MidTurnFrom, convert_messages, mid_turn_message_landed, parent_taint, persist_input, record_interrupt, sanitize_message_order,
    unanswered_mid_turn_message,
};
use crate::harness::model_call::{self, prefer_non_gateway};
use crate::harness::seat;
use crate::harness::session_gate::{
    ActiveTurnStatus, ActiveTurns, Admission, QUEUED_INTO_RUNNING_TURN, RunProgress, active_turn_status,
    admit_or_queue, live_session_under, session_is_busy,
};
use crate::harness::{after_turn, usage};
use types::keyparser;
use crate::prompt;
use crate::pruning::{self, ContextThresholds};
use crate::selector::{self, ModelSelector};
use crate::session::SessionManager;
use crate::steering;
use crate::transcript;

/// Default maximum agentic loop iterations per run.
const DEFAULT_MAX_ITERATIONS: usize = 100;
/// Extended ceiling when agent is making genuine progress (successful tool calls, no loops).
const EXTENDED_MAX_ITERATIONS: usize = 200;
/// Default context token limit for models that don't report one.
const DEFAULT_CONTEXT_TOKEN_LIMIT: usize = 80_000;

/// Default max auto-continuations when agent stops mid-task (no work tasks).
#[allow(dead_code)] // used by max_auto_continuations, reserved for auto-continuation logic
const MAX_AUTO_CONTINUATIONS_DEFAULT: usize = 5;
/// Ceiling for auto-continuations even with many work tasks.
#[allow(dead_code)] // used by max_auto_continuations, reserved for auto-continuation logic
const MAX_AUTO_CONTINUATIONS_CEILING: usize = 50;

/// Evicted messages that must accumulate before another background LLM
/// compaction is spawned for a session.
///
/// The sliding window evicts whenever a conversation exceeds
/// `MAX_MESSAGE_COUNT` (80) — regardless of token budget — so a sustained
/// agentic session evicts on EVERY iteration, and the ungated spawn fired one
/// ~13.5s summary per iteration. Measured in the 2026-08-27 incident: 3,845
/// compactions against 3,981 agent turns (0.97 per turn), 376MB of input and
/// 14.4 hours of wall time — a third of all traffic, for a summary the next
/// iteration immediately superseded. The quick string-extraction fallback still
/// runs on every eviction, so nothing is lost between LLM passes; this only
/// throttles the expensive upgrade.
const SUMMARY_MIN_EVICTED: usize = 20;

/// Sessions with a background compaction in flight. The spawn is
/// fire-and-forget, so without this N summaries run concurrently — each taking
/// an LLM permit from the agent's own calls and racing on `update_summary`,
/// where the last writer wins in arbitrary order.
static SUMMARY_INFLIGHT: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashSet<String>>,
> = std::sync::LazyLock::new(Default::default);

/// Messages evicted for a session since its last spawned LLM summary.
static SUMMARY_EVICTED_SINCE: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, usize>>,
> = std::sync::LazyLock::new(Default::default);

/// Whether this eviction should spawn an LLM summary. Accumulates the evicted
/// count and returns true at most once per [`SUMMARY_MIN_EVICTED`] messages per
/// session, never while one is already running.
fn summary_due(session_id: &str, evicted: usize) -> bool {
    let mut since = SUMMARY_EVICTED_SINCE.lock().unwrap_or_else(|p| p.into_inner());
    let acc = since.entry(session_id.to_string()).or_insert(0);
    *acc += evicted;
    if *acc < SUMMARY_MIN_EVICTED {
        return false;
    }
    let mut inflight = SUMMARY_INFLIGHT.lock().unwrap_or_else(|p| p.into_inner());
    if !inflight.insert(session_id.to_string()) {
        return false; // one already running; keep accumulating
    }
    *acc = 0;
    true
}

/// Release the in-flight marker when a background summary finishes.
fn summary_done(session_id: &str) {
    SUMMARY_INFLIGHT
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .remove(session_id);
}
/// Max output length from forked command execution (bytes).
const FORK_OUTPUT_CAP: usize = 32_000;
/// Max iterations for forked command sub-agent.
const FORK_MAX_ITERATIONS: usize = 20;

/// Command prefixes eligible for forked (sub-agent) execution.
const FORK_COMMAND_PREFIXES: &[&str] = &["/research", "/analyze", "/deep-dive", "/investigate"];

/// Check whether a user prompt should be forked to a sub-agent context.
fn should_fork_command(prompt: &str) -> bool {
    let trimmed = prompt.trim().to_lowercase();
    FORK_COMMAND_PREFIXES
        .iter()
        .any(|prefix| trimmed.starts_with(prefix))
}

/// Cross-turn spiral memory. The per-turn counters reset every run, so a model
/// that resumed the same doomed strategy after each user message ("Let me read
/// the frames using sub-agents" x7, across turns, until the user gave up) never
/// tripped the backstop. Keys that ended a turn hot are carried into the next
/// turn at half strength: a resumed loop trips the nudge in half the calls, and
/// a third resumption almost immediately. Success on a key clears it.
/// Sessions remembered at once. Beyond it the least recently saved session
/// is forgotten, one at a time: a wholesale clear made every hot loop in
/// every session cold on the same tick.
const CROSS_TURN_SPIRAL_SESSIONS: usize = 512;

#[derive(Default)]
struct CrossTurnSpiral {
    hot: std::collections::HashMap<String, std::collections::HashMap<String, usize>>,
    /// Save order, oldest first; a re-save moves the session to the back.
    order: std::collections::VecDeque<String>,
}

impl CrossTurnSpiral {
    fn save(&mut self, session_id: &str, hot: std::collections::HashMap<String, usize>) {
        self.order.retain(|s| s != session_id);
        if hot.is_empty() {
            self.hot.remove(session_id);
            return;
        }
        self.hot.insert(session_id.to_string(), hot);
        self.order.push_back(session_id.to_string());
        while self.hot.len() > CROSS_TURN_SPIRAL_SESSIONS {
            match self.order.pop_front() {
                Some(oldest) => {
                    self.hot.remove(&oldest);
                }
                None => break,
            }
        }
    }
}

static CROSS_TURN_SPIRAL: std::sync::Mutex<Option<CrossTurnSpiral>> = std::sync::Mutex::new(None);

fn cross_turn_seed(session_id: &str) -> std::collections::HashMap<String, usize> {
    let mut guard = CROSS_TURN_SPIRAL.lock().unwrap_or_else(|p| p.into_inner());
    guard
        .get_or_insert_with(Default::default)
        .hot
        .get(session_id)
        .cloned()
        .unwrap_or_default()
}

fn cross_turn_save(
    session_id: &str,
    counts: &std::collections::HashMap<String, usize>,
    limit: usize,
) {
    let mut guard = CROSS_TURN_SPIRAL.lock().unwrap_or_else(|p| p.into_inner());
    let hot: std::collections::HashMap<String, usize> = counts
        .iter()
        .filter(|(_, c)| **c * 2 >= limit)
        .map(|(k, c)| (k.clone(), c / 2))
        .collect();
    guard.get_or_insert_with(Default::default).save(session_id, hot);
}

/// Workflow-mode configuration for a run — the ONE-loop convergence: workflow
/// activities execute through this same Runner instead of a second loop in
/// the engine. Config on the request, NOT a second run() (Rule 8).
#[derive(Clone)]
pub struct WorkflowMode {
    /// Janus attribution — workflow/action/step ids ride the request trace.
    pub trace: RequestTrace,
    /// What this step is for, in words: workflow name, activity and step
    /// instruction. A workflow turn has no session objective (detection is
    /// skipped for scratch sessions), so this is the objective the tool
    /// guardrail judges calls against.
    pub objective: String,
    /// The work order this turn was given (the seed's final user message).
    /// The run's prompt is empty — the seed carries it — so this stands in
    /// for the person's latest message in the guardrail's state.
    pub instruction: String,
    /// Schema-advertising filter (context scoping, not security): only these
    /// tools' schemas ship to the model. Dispatch still resolves through the
    /// full registry — the same roster fallback the engine loop had.
    pub advertised_tools: std::collections::HashSet<String>,
    /// The run's inputs carry untrusted content — a gated `Always` floors to
    /// Approval (WS2-R7), the same rule the engine checkpoint applied.
    pub tainted: bool,
    /// The owner's per-run spending limit in microcents (0 = none). A
    /// package's token_budget is an estimate, never enforced; this is the
    /// one ceiling. Reaching it earns one wrap-up turn (no tools: "report
    /// what you have"), then the turn ends `SpendCapReached`.
    pub spend_cap_microcents: i64,
    /// Park an Approval-gated operation instead of refusing it unattended:
    /// the closure persists the suspension (sync — rusqlite is sync) and the
    /// loop exits with reason "awaiting_approval". None = refuse (chat-style).
    #[allow(clippy::type_complexity)]
    pub park: Option<
        std::sync::Arc<dyn Fn(WorkflowPark<'_>) -> Result<(), String> + Send + Sync>,
    >,
}

impl std::fmt::Debug for WorkflowMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkflowMode")
            .field("trace_run", &self.trace.run_id)
            .field("advertised", &self.advertised_tools.len())
            .field("tainted", &self.tainted)
            .field("spend_cap_microcents", &self.spend_cap_microcents)
            .field("park", &self.park.is_some())
            .finish()
    }
}

/// What the workflow park closure receives — everything a suspension row needs.
pub struct WorkflowPark<'a> {
    /// The in-loop conversation at park time (session messages, converted).
    pub messages: Vec<Message>,
    pub call: &'a ai::ToolCall,
    /// The ask the call parked on: the owner's answer to it releases the run.
    pub ask_id: &'a str,
    /// Port-suffixed operation name + the owner-facing display sentence.
    pub operation: String,
    pub display: String,
}

/// Whether a restricted run's allowlist names this tool: by name, as the
/// tool of a `tool:resource` entry, or by a `prefix*` family.
pub(crate) fn allowlist_admits(allowlist: &HashSet<String>, name: &str) -> bool {
    allowlist.contains(name)
        || allowlist.iter().any(|e| {
            e.split_once(':').is_some_and(|(tool, _)| tool == name)
                || e.strip_suffix('*')
                    .is_some_and(|prefix| !prefix.is_empty() && name.starts_with(prefix))
        })
}

#[derive(Debug, Clone, Default)]
pub struct RunRequest {
    pub session_key: String,
    pub prompt: String,
    pub system: String,
    pub model_override: String,
    pub user_id: String,
    pub skip_memory_extract: bool,
    pub origin: Origin,
    /// The entry this run came through (chat, helper, workflow, schedule,
    /// heartbeat, coworker, voice, MCP). Recorded with every decision.
    pub door: types::permissions::Door,
    /// A run override of the employee's permission mode; `None` = its own.
    pub mode: Option<types::permissions::Mode>,
    /// The grant this run can only narrow: its parent's (a helper).
    pub ceiling: Option<types::permissions::Ceiling>,
    /// A hard fence for this run: an isolated helper's own copy.
    pub fence: Option<Vec<std::path::PathBuf>>,
    /// Agent-to-agent handoff depth for this run's outbound messages (0 = not
    /// a handoff). Stamped on loop-tool sends so receiving bots enforce the
    /// depth cap even on tool-authored messages, not just runner replies.
    pub handoff_depth: u8,
    pub channel: String,
    pub force_skill: String,
    /// Maximum agentic loop iterations (0 = default 100).
    pub max_iterations: usize,
    /// Cancellation token for cooperative shutdown of the agentic loop.
    pub cancel_token: CancellationToken,
    /// When set, this run executes as a specific agent (persona). The agent's persona
    /// replaces the default identity, and session history is isolated.
    pub agent_id: String,
    /// Per-entity model preference (fuzzy-resolved before provider selection).
    pub model_preference: Option<String>,
    /// Per-entity personality snippet prepended to system prompt.
    pub personality_snippet: Option<String>,
    /// Images attached to the user's message (base64-encoded).
    pub images: Vec<ai::ImageContent>,
    /// The files the owner attached, as uploaded (fileId, filename, mimeType,
    /// size, url). Kept on the user row so a reloaded transcript still shows
    /// them; the "[Attached: …]" note in the text is for the model.
    pub attachments: Vec<comm::wire::Attachment>,
    /// Default working directory for shell commands and relative file paths
    /// (an isolated sub-agent's worktree). None = the process cwd.
    pub cwd: Option<String>,
    /// User presence tracker (shared Arc, for live updates during the run).
    pub presence_tracker: Option<Arc<crate::proactive::PresenceTracker>>,
    /// Proactive inbox (shared Arc, drained once per run).
    pub proactive_inbox: Option<Arc<crate::proactive::ProactiveInbox>>,
    /// Minimum iterations before allowing the agent to stop naturally.
    /// When set, the runner forces continuation even on text-only responses
    /// until this many iterations have been reached.
    pub min_iterations: usize,
    /// Prompt assembly mode. Defaults to Full for interactive chat.
    /// Set to Minimal for sub-agents (drops memory docs, tool routing, etiquette, etc.).
    pub prompt_mode: prompt::PromptMode,
    /// Optional progress counters shared with the global RunRegistry.
    /// When set, the runner updates these atomics during run_loop() so
    /// external observers can see live iteration/tool counts.
    pub progress: Option<RunProgress>,
    /// Injected as a system-role message after the user prompt — visible to the
    /// LLM but not rendered in the frontend. Used for @mention routing context.
    pub mention_context: Option<String>,
    /// Tool scope name from agent.json for SDK-driven tool filtering.
    pub tool_scope: Option<String>,
    /// Explicit tool allowlist for restricted runs (phone callers). Entries
    /// are bare tool names ("use_skill") or `tool:resource` compounds
    /// ("agent:memory"). Enforced at the runner gate AND the registry choke
    /// point via `ToolContext::whitelist_allows`, and the declared schema is
    /// filtered to match. `None` = every normal run, unrestricted.
    pub tool_allowlist: Option<std::collections::HashSet<String>>,
    /// Denial text used when a whitelisted run calls an off-list tool.
    pub tool_denial_hint: Option<String>,
    /// Persist the prompt as isMeta (owner-invisible) — platform-authored
    /// prompts (christening intro) that must never render as the owner's words.
    pub hidden_prompt: bool,
    /// Skill names to pre-load into this run's context. Full SKILL.md content
    /// is injected into the system prompt so the agent has instructions without
    /// needing to discover/load them. Used by sub-agent spawning.
    pub preload_skills: Vec<String>,
    /// Tool names to pre-activate (bypass deferred-loading discovery).
    pub preactivate_tools: Vec<String>,
    /// When true, agent presents a plan before executing any tool calls.
    /// The plan is sent via a PlanApproval event for user approval.
    pub plan_mode: bool,
    /// Channel context (Slack/Discord/etc.) when this run was triggered by an
    /// inbound channel message. Surfaces on `ToolContext.channel` so the
    /// plugin tool can inject `NEBO_CHANNEL_*` env vars into plugin processes
    /// (e.g. for `slack upload`). See `docs/publishers-guide/channel-plugins.md`.
    pub channel_ctx: Option<tools::ChannelContext>,
    /// Provenance classes seeding this run's taint set — the taint of the
    /// TRIGGERING input (a coworker envelope's provenance, a remote channel
    /// message). The runner unions tool-derived classes on top and stamps the
    /// final set on the Done event.
    pub seed_taint: Vec<types::provenance::ProvenanceClass>,
    /// Recall-for-audience: the agent id this run is REPLYING TO (coworker
    /// messages only). When set and not granted by the target's
    /// `memory.share_with`, recall serves `tacit/` (working style) only and
    /// the memory tool refuses non-tacit reads — matter/project facts never
    /// surface in a reply to a non-granted colleague. `None` for owner runs.
    pub audience: Option<String>,
    /// Workflow-mode configuration (None = every normal chat run). See
    /// [`WorkflowMode`] — deterministic sampling, advertised-tools scoping,
    /// pending-call entry, approval parking, output budgets.
    pub workflow: Option<WorkflowMode>,
}

/// Per-run mutable state (prevents data races across concurrent runs).
pub(crate) struct RunState {
    prompt_overhead: usize,
    /// System prompt + tool-schema tokens (display estimate, no threshold fudge).
    pub(crate) system_overhead_tokens: usize,
    pub(crate) last_input_tokens: usize,
    /// Local estimate (chars/4) of the message tokens sent in the last request.
    /// Compared against API-reported usage to calibrate compaction thresholds.
    pub(crate) last_request_estimate: usize,
    /// Observed undercount of the local estimate vs API-reported usage
    /// (hybrid counting, expressed as a threshold adjustment:
    /// actual_prev + est(tail) > threshold  ⇔  est(prev) + est(tail) > threshold − undercount).
    pub(crate) estimate_correction: usize,
    /// Cumulative input tokens across all iterations in this run.
    pub(crate) total_input_tokens: i32,
    /// Cumulative output tokens across all iterations in this run.
    pub(crate) total_output_tokens: i32,
    /// Cumulative cache tokens. Read for calibration since forever but never
    /// kept — and cache reads are most of a long conversation's bill.
    pub(crate) total_cache_read_tokens: i32,
    pub(crate) total_cache_creation_tokens: i32,
    /// Provider-reported cost this run, microdollars (Janus prices the model it
    /// routed to). 0 when no provider said — then the price table is the only
    /// estimate, and for a routed alias it knows nothing.
    pub(crate) cost_microdollars: i64,
    pub(crate) thresholds: Option<ContextThresholds>,
    /// Janus quota warning string, populated when session or weekly usage exceeds 80%.
    pub(crate) quota_warning: Option<String>,
    /// Whether a quota warning WS event has already been sent this run (fire once).
    pub(crate) quota_warning_sent: bool,
}

impl RunState {
    pub(crate) fn new() -> Self {
        Self {
            prompt_overhead: 0,
            system_overhead_tokens: 0,
            last_input_tokens: 0,
            last_request_estimate: 0,
            estimate_correction: 0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cache_read_tokens: 0,
            total_cache_creation_tokens: 0,
            cost_microdollars: 0,
            thresholds: None,
            quota_warning: None,
            quota_warning_sent: false,
        }
    }
}

/// The main agentic loop runner.
///
/// Providers are wrapped in `Arc` so they can be shared across concurrent runs
/// spawned via `tokio::spawn`.
pub struct Runner {
    sessions: SessionManager,
    providers: Arc<RwLock<Vec<Arc<dyn Provider>>>>,
    tools: Arc<Registry>,
    store: Arc<Store>,
    selector: Arc<ModelSelector>,
    concurrency: Arc<ConcurrencyController>,
    hooks: Arc<napp::HookDispatcher>,
    /// Issues the credential a CLI provider's tool calls carry back over
    /// /agent/mcp (see `tool_credentials`).
    tool_credentials: Option<crate::tool_credentials::ToolCredentials>,
    agent_registry: tools::AgentRegistry,
    skill_loader: Option<Arc<tools::skills::Loader>>,
    ask_channels: Option<tools::AskChannels>,
    embedding_provider: Option<Arc<dyn ai::EmbeddingProvider>>,
    /// The typed-decision door (TypeSafe Jev through Janus). Present exactly
    /// when the Janus provider is; the judges use it instead of a chat turn.
    decide: Option<Arc<ai::DecideClient>>,
    /// The SAME hybrid-search adapter instance the memory tool uses (shared
    /// TurboVec index cache) — powers per-message prompt recall.
    hybrid_searcher: Option<Arc<dyn tools::HybridSearcher>>,
    /// Optional broadcast/loop-push sink for auto-generated chat titles.
    title_sink: std::sync::OnceLock<Arc<dyn after_turn::ChatTitleSink>>,
    active_turns: ActiveTurns,
}

impl Runner {
    pub fn new(
        store: Arc<Store>,
        tools: Arc<Registry>,
        providers: Vec<Arc<dyn Provider>>,
        selector: ModelSelector,
        concurrency: Arc<ConcurrencyController>,
        hooks: Arc<napp::HookDispatcher>,
        tool_credentials: Option<crate::tool_credentials::ToolCredentials>,
        agent_registry: tools::AgentRegistry,
        skill_loader: Option<Arc<tools::skills::Loader>>,
    ) -> Self {
        Self {
            sessions: SessionManager::new(store.clone()),
            providers: Arc::new(RwLock::new(providers)),
            tools,
            store,
            ask_channels: None,
            selector: Arc::new(selector),
            concurrency,
            hooks,
            tool_credentials,
            agent_registry,
            skill_loader,
            embedding_provider: None,
            decide: None,
            hybrid_searcher: None,
            title_sink: std::sync::OnceLock::new(),
            active_turns: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    /// Install the chat-title sink (broadcast + loop propagation). Set once at
    /// startup after AppState exists; no-op if already set.
    pub fn set_title_sink(&self, sink: Arc<dyn after_turn::ChatTitleSink>) {
        let _ = self.title_sink.set(sink);
    }

    /// Run the ONE chat-title generator for a chat that gained turns outside a
    /// Runner run (the voice loop persists turns directly). Same gates, same
    /// summarizer, same sink as the run-path call.
    pub fn spawn_title_generation(&self, session_id: &str, chat_id: &str) {
        after_turn::spawn_chat_title_generation(
            self.providers.clone(),
            self.store.clone(),
            chat_id.to_string(),
            session_id.to_string(),
            self.selector.get_cheapest_model(),
            self.title_sink.get().cloned(),
        );
    }

    /// Get the shared providers Arc (for workflow execution).
    pub fn providers(&self) -> Arc<RwLock<Vec<Arc<dyn Provider>>>> {
        self.providers.clone()
    }

    /// Set the shared ask channels so tools can prompt the user via `ctx.ask_user()`.
    /// Whether a turn is running on `session_key` (see `ActiveTurn`).
    pub fn is_session_busy(&self, session_key: &str) -> bool {
        session_is_busy(&self.active_turns, session_key)
    }

    /// The session a turn is live on under `session_key`, if any (see
    /// `live_session_under`) — the one steering must be addressed to.
    pub fn live_session_under(&self, session_key: &str) -> Option<String> {
        live_session_under(&self.active_turns, session_key)
    }

    /// The running turn's live counters for `session_key`, if any.
    pub fn active_turn_status(&self, session_key: &str) -> Option<ActiveTurnStatus> {
        active_turn_status(&self.active_turns, session_key)
    }

    pub fn set_ask_channels(mut self, channels: tools::AskChannels) -> Self {
        self.ask_channels = Some(channels);
        self
    }

    /// Set the embedding provider for transcript indexing and memory embedding.
    pub fn set_embedding_provider(mut self, provider: Arc<dyn ai::EmbeddingProvider>) -> Self {
        self.embedding_provider = Some(provider);
        self
    }

    /// Install the typed-decision client (Jev through Janus).
    pub fn set_decide(mut self, client: Arc<ai::DecideClient>) -> Self {
        self.decide = Some(client);
        self
    }

    /// The typed-decision client, if the Janus provider is present.
    pub fn decide(&self) -> Option<Arc<ai::DecideClient>> {
        self.decide.clone()
    }

    /// Set the hybrid searcher for per-message prompt memory recall — pass the
    /// same adapter instance wired into the memory tool so both share one
    /// pathway and one index cache.
    pub fn set_hybrid_searcher(mut self, searcher: Arc<dyn tools::HybridSearcher>) -> Self {
        self.hybrid_searcher = Some(searcher);
        self
    }

    /// Replace the active providers list (called when auth_profiles change).
    pub async fn reload_providers(&self, providers: Vec<Arc<dyn Provider>>) {
        let loaded_ids: Vec<String> = providers.iter().map(|p| p.id().to_string()).collect();
        let mut lock = self.providers.write().await;
        let count = providers.len();
        *lock = providers;
        drop(lock);
        // Sync selector with newly loaded provider IDs
        self.selector.set_loaded_providers(loaded_ids);
        self.selector
            .rebuild_fuzzy(&std::collections::HashMap::new());
        info!(count, "reloaded AI providers");
    }

    /// Access the model selector (e.g. to inject runtime-discovered models).
    pub fn selector(&self) -> &ModelSelector {
        &self.selector
    }

    /// Run the agentic loop: prompt -> stream -> tool calls -> loop.
    /// Returns a receiver of streaming events.
    pub async fn run(&self, mut req: RunRequest) -> Result<mpsc::Receiver<StreamEvent>, ProviderError> {
        seat::restrict_outside_origin(req.origin, &mut req.tool_allowlist, &mut req.tool_denial_hint);
        let t_run_entry = std::time::Instant::now();
        info!(
            session_key = %req.session_key,
            channel = %req.channel,
            "Runner.run() called"
        );
        {
            let lock = self.providers.read().await;
            if lock.is_empty() {
                warn!("No AI providers configured — rejecting run request");
                return Err(ProviderError::Request(
                    "No AI providers configured. Add API keys in Settings > Providers.".to_string(),
                ));
            }
            info!(provider_count = lock.len(), "providers available");
        }

        let session_key = if req.session_key.is_empty() {
            "default".to_string()
        } else {
            req.session_key.clone()
        };

        // Get or create session
        let session = self
            .sessions
            .get_or_create(&session_key, &req.user_id)
            .map_err(|e| {
                warn!(error = %e, "failed to get/create session");
                ProviderError::Request(format!("session error: {}", e))
            })?;

        let session_id = session.id.clone();
        info!(session_id = %session_id, ms = t_run_entry.elapsed().as_millis() as u64, "[telemetry] session ready");

        // One turn per session (see `ActiveTurn`). Callers without a registry
        // handle (voice, MCP) still get live counters for the status line.
        let progress = req.progress.clone().unwrap_or_else(|| RunProgress {
            run_id: uuid::Uuid::new_v4().to_string(),
            iteration_count: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            tool_call_count: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            current_tool: Arc::new(std::sync::Mutex::new(String::new())),
        });
        // The owner's words reach the running turn as its next message. They
        // are stored as typed (the chat shows them clean) and marked as
        // having arrived mid-work; the framing the model needs is added when
        // the window is built (`convert_messages`), the way Claude Code keeps
        // the transcript clean and frames the queued message for the model
        // only. Untrusted caller framing (phone lines) rides along in the
        // briefing below.
        // A coworker's run names its sender as the audience: its message is a
        // colleague's, never framed as the owner's.
        let from = match req.audience.as_deref() {
            Some(coworker) => MidTurnFrom::Coworker { from: coworker.to_string() },
            None => {
                let via = if req.channel.is_empty() { "chat" } else { req.channel.as_str() };
                MidTurnFrom::Owner { via: via.to_string() }
            }
        };
        let queue = || {
            let meta = from.metadata();
            if let Err(e) = self.sessions.append_message(&session_id, "user", &req.prompt, None, None, Some(&meta)) {
                warn!(session_id = %session_id, error = %e, "could not queue a message into the running turn");
            }
        };
        let turn_guard =
            match admit_or_queue(&self.active_turns, &session_key, progress.clone(), req.cancel_token.clone(), queue)
                .await
            {
                Admission::Admitted(guard) => guard,
                Admission::Queued { status } => {
                    // The briefing (team roster, turn rule) is steering: it rides
                    // the running turn's next call on the wake rail and is never
                    // written to the thread.
                    if let Some(ctx) = req.mention_context.as_deref() {
                        steering::push_wake(
                            &session_key,
                            steering::WakeEntry {
                                wake_id: None,
                                content: steering::wrap_system_reminder(ctx),
                                taint: Vec::new(),
                            },
                        );
                    }
                    info!(session_id = %session_id, channel = %req.channel, "second request on a busy session queued into the running turn");
                    // The running loop hears it at its next step, or before it
                    // ends the turn on a reply (`mid_turn_message_landed`).
                    let (tx, rx) = mpsc::channel(4);
                    // A send fails only if the caller already dropped the
                    // receiver; there is nobody left to tell.
                    let _ = tx
                        .send(StreamEvent::control_notice(status, QUEUED_INTO_RUNNING_TURN))
                        .await;
                    let _ = tx.send(StreamEvent::done()).await;
                    return Ok(rx);
                }
            };

        // Pre-load skills into the sub-agent's conversation.
        // Each skill becomes a user message with isMeta metadata so the UI doesn't
        // render it as real user input. Injected BEFORE the task prompt so the
        // sub-agent has instructions in its context from turn 1.
        //
        // Scoped to the seat this run belongs to: a sub-agent carries no persona
        // of its own (build_subagent_request never sets agent_id), and the
        // skills it preloads were named by the seat that spawned it. The ONE
        // extractor strips the `subagent:` wrappers and yields that seat, the
        // same scope the sub-agent's own later use_skill calls use
        // — without it a seat hands work to a helper and its own procedures go
        // along in name only.
        if !req.preload_skills.is_empty() {
            if let Some(ref loader) = self.skill_loader {
                let seat = keyparser::extract_agent_id(&session_key);
                let skill_scope = (!seat.is_empty()).then_some(seat.as_str());
                for skill_name in &req.preload_skills {
                    if let Some(skill) = loader.get(skill_name, skill_scope).await {
                        if skill.enabled {
                            let content = loader.expand_template(&skill, Some(&self.store));
                            if !content.is_empty() {
                                let meta = serde_json::json!({
                                    "isMeta": true,
                                    "skillPreload": skill_name,
                                })
                                .to_string();
                                let _ = self.sessions.append_message(
                                    &session_id,
                                    "user",
                                    &format!("[Loading skill: {}]\n\n{}", skill_name, content),
                                    None,
                                    None,
                                    Some(&meta),
                                );
                                info!(skill = %skill_name, len = content.len(),
                                      "pre-loaded skill into sub-agent context");
                            }
                        } else {
                            warn!(skill = %skill_name, "pre-load skill disabled, skipping");
                        }
                    } else {
                        warn!(skill = %skill_name, "pre-load skill not found");
                    }
                }
            }
        }

        // An auto-continuation is the house nudging the employee, not the
        // owner speaking. Steering is per turn: the nudge rides this run's
        // calls as a stream reminder (see `mention_context` below) and is
        // never written to the thread — a stored nudge was re-sent on every
        // later turn, telling the model to press on long after the work ended.
        let continuation = crate::goals::is_continuation_prompt(&req.prompt);

        // Append user message — large inputs are offloaded to a temp file and
        // replaced with an LLM-generated summary so the full document never
        // enters the main chat context.
        if !req.prompt.is_empty() && !continuation {
            persist_input(
                &self.sessions,
                &self.providers,
                &self.selector,
                &req.agent_id,
                &session_id,
                InputRow {
                    text: &req.prompt,
                    images: &req.images,
                    attachments: &req.attachments,
                    hidden: req.hidden_prompt,
                },
            )
            .await
            .map_err(ProviderError::Request)?;
            // @mention routing context rides the FIRST LLM call as an
            // ephemeral <system-reminder> (seeded into run_loop's pending
            // reminders) — never persisted to the session.
        }
        // The auto-continue nudge rides the same rail as the briefing.
        let mention_context = [req.mention_context.clone(), continuation.then(|| req.prompt.clone())]
            .into_iter()
            .flatten()
            .reduce(|a, b| format!("{a}\n\n{b}"));

        // Create result channel
        let (tx, rx) = mpsc::channel(100);

        // Clone refs for the spawned task (SessionManager shares cache via Arc)
        let session_mgr = self.sessions.clone();
        let store = self.store.clone();
        let tools = self.tools.clone();
        let providers = self.providers.clone();
        let decide = self.decide.clone();
        let concurrency = self.concurrency.clone();
        let selector = self.selector.clone();
        let hooks = self.hooks.clone();
        let agent_registry = self.agent_registry.clone();
        let agent_id = req.agent_id.clone();
        let system_prompt = req.system.clone();
        let user_id = req.user_id.clone();
        let origin = req.origin;
        let skip_memory = req.skip_memory_extract;
        let title_sink = self.title_sink.get().cloned();
        let user_prompt = req.prompt.clone();
        let force_skill = req.force_skill.clone();
        let skill_loader = self.skill_loader.clone();

        // Resolve fuzzy model override — prefer explicit model_override, fall back to entity preference
        let raw_model = if !req.model_override.is_empty() {
            req.model_override.clone()
        } else if let Some(ref pref) = req.model_preference {
            pref.clone()
        } else {
            String::new()
        };
        let model_override = if raw_model.is_empty() {
            String::new()
        } else {
            self.selector
                .resolve_fuzzy(&raw_model)
                .unwrap_or_else(|| raw_model.clone())
        };

        // Derive channel from session key via keyparser, fall back to explicit channel
        let channel = if !req.channel.is_empty() {
            req.channel.clone()
        } else {
            let key_info = keyparser::parse_session_key(&session_key);
            if key_info.channel.is_empty() {
                "web".to_string()
            } else {
                key_info.channel
            }
        };

        // Get model aliases for prompt injection
        let model_aliases = self.selector.get_aliases_text();

        let cancel_token = req.cancel_token.clone();
        let max_iterations = if req.max_iterations > 0 {
            req.max_iterations
        } else {
            DEFAULT_MAX_ITERATIONS
        };
        let min_iterations = req.min_iterations;
        let grant = Arc::new(crate::harness::seat::run_grant(&self.store, req.grant_request()));
        let personality_snippet = req.personality_snippet.clone();
        let run_cwd = req.cwd.clone();
        let presence_tracker = req.presence_tracker.clone();
        let proactive_inbox = req.proactive_inbox.clone();
        let prompt_mode = req.prompt_mode.clone();
        let progress = Some(progress);
        let ask_channels = self.ask_channels.clone();
        let embedding_provider = self.embedding_provider.clone();
        let hybrid_searcher = self.hybrid_searcher.clone();
        let tool_scope = req.tool_scope.clone();
        let plan_mode = req.plan_mode;
        let preactivate_tools = req.preactivate_tools.clone();
        let channel_ctx = req.channel_ctx.clone();

        let tool_credentials = self.tool_credentials.clone();

        tokio::spawn(async move {
            // Releases the session for the next turn when this task ends.
            let turn = turn_guard;
            // Sub-agent runs close their own browser tab/page when the run ends
            // (normal, error, or cancellation). Top-level runs are cleaned up by
            // their dispatcher, so gate on the subagent session key.
            let _tab_cleanup = session_key
                .starts_with("subagent:")
                .then(|| SubagentTabCleanup {
                    tools: tools.clone(),
                    session_id: session_id.clone(),
                });

            // ── Forked command execution ──────────────────────────────
            // Heavy commands (e.g. /research, /analyze) run in a sub-agent
            // context so intermediate tool calls don't consume the main
            // chat's context window.
            if should_fork_command(&user_prompt) {
                info!(
                    session_id,
                    "forking command to sub-agent: {}",
                    &user_prompt[..user_prompt.len().min(50)]
                );

                let _ = tx
                    .send(StreamEvent::text(
                        "Working on this in the background...\n\n".to_string(),
                    ))
                    .await;

                let fork_session_key = format!("fork:{}:{}", session_id, uuid::Uuid::new_v4());

                let fork_session = session_mgr.get_or_create(&fork_session_key, &user_id).ok();

                if let Some(ref fs) = fork_session {
                    let _ =
                        session_mgr.append_message(&fs.id, "user", &user_prompt, None, None, None);

                    let fork_session_id = fs.id.clone();
                    let (sub_tx, mut sub_rx) = mpsc::channel::<StreamEvent>(256);
                    let fork_taint = std::sync::Mutex::new(std::collections::BTreeSet::new());

                    let _fork_result = run_loop(
                        &session_mgr,
                        &tools,
                        &store,
                        &providers,
                        &concurrency,
                        &selector,
                        &hooks,
                        &sub_tx,
                        &fork_session_id,
                        &system_prompt,
                        &model_override,
                        &user_id,
                        &channel,
                        &model_aliases,
                        origin,
                        true, // skip_memory for forked runs
                        FORK_MAX_ITERATIONS,
                        &cancel_token,
                        &agent_registry,
                        &agent_id,
                        personality_snippet.as_deref(),
                        &grant,
                        &req.door,
                        &user_prompt,
                        &force_skill,
                        skill_loader.as_deref(),
                        run_cwd.as_deref(),
                        presence_tracker.as_ref(),
                        proactive_inbox.as_ref(),
                        0,
                        prompt::PromptMode::Minimal,
                        progress.as_ref(),
                        ask_channels.as_ref(),
                        req.handoff_depth,
                        embedding_provider.as_ref(),
                        hybrid_searcher.as_ref(),
                        tool_scope.as_deref(),
                        false, // no plan_mode for forks
                        &preactivate_tools,
                        channel_ctx.as_ref(),
                        None, // forks carry no mention context
                        None, // command forks are not review forks
                        req.tool_allowlist.as_ref(),
                        req.tool_denial_hint.clone(),
                        tool_credentials.as_ref(),
                        &fork_taint,
                        None, // forks never reply to a coworker audience
                        None, // forks are chat, never workflow mode
                        decide.as_ref(),
                    )
                    .await;

                    drop(sub_tx);

                    let mut result_text = String::new();
                    while let Some(event) = sub_rx.recv().await {
                        if event.event_type == StreamEventType::Text {
                            result_text.push_str(&event.text);
                        }
                    }

                    if result_text.len() > FORK_OUTPUT_CAP {
                        let total = result_text.len();
                        result_text.truncate(FORK_OUTPUT_CAP);
                        result_text.push_str(&format!(
                            "\n\n[Output truncated: {total} bytes, showing first {FORK_OUTPUT_CAP}; \
                             the sub-agent's full output is in its session]"
                        ));
                    }

                    let _ = session_mgr.append_message(
                        &session_id,
                        "assistant",
                        &result_text,
                        None,
                        None,
                        None,
                    );

                    let _ = tx.send(StreamEvent::text(result_text)).await;
                } else {
                    let _ = tx
                        .send(StreamEvent::error(
                            "Failed to create fork session".to_string(),
                        ))
                        .await;
                }

                let _ = tx.send(StreamEvent::done()).await;
                return;
            }

            // ── Normal (non-forked) execution ────────────────────────
            // The run's provenance accumulator (trust-boundaries design
            // 2026-08-22): seeded with the triggering input's classes, grown
            // by run_loop from the static tool→class table, stamped on Done.
            // Shared mutex (never held across .await) so the final set is
            // readable here even when run_loop exits early on error.
            let run_taint: std::sync::Mutex<
                std::collections::BTreeSet<types::provenance::ProvenanceClass>,
            > = std::sync::Mutex::new(req.seed_taint.iter().copied().collect());
            let result = run_loop(
                &session_mgr,
                &tools,
                &store,
                &providers,
                &concurrency,
                &selector,
                &hooks,
                &tx,
                &session_id,
                &system_prompt,
                &model_override,
                &user_id,
                &channel,
                &model_aliases,
                origin,
                skip_memory,
                max_iterations,
                &cancel_token,
                &agent_registry,
                &agent_id,
                personality_snippet.as_deref(),
                &grant,
                &req.door,
                &user_prompt,
                &force_skill,
                skill_loader.as_deref(),
                run_cwd.as_deref(),
                presence_tracker.as_ref(),
                proactive_inbox.as_ref(),
                min_iterations,
                prompt_mode.clone(),
                progress.as_ref(),
                ask_channels.as_ref(),
                req.handoff_depth,
                embedding_provider.as_ref(),
                hybrid_searcher.as_ref(),
                tool_scope.as_deref(),
                plan_mode,
                &preactivate_tools,
                channel_ctx.as_ref(),
                mention_context.as_deref(),
                None, // top-level runs are never review forks
                req.tool_allowlist.as_ref(),
                req.tool_denial_hint.clone(),
                tool_credentials.as_ref(),
                &run_taint,
                req.audience.as_deref(),
                req.workflow.as_ref(),
                decide.as_ref(),
            )
            .await;
            turn.close();

            if cancel_token.is_cancelled() {
                record_interrupt(&session_mgr, &session_id);
            }

            let (run_ok, loop_exit_reason) = match result {
                Ok(reason) => (true, reason),
                Err(e) => {
                    let _ = tx
                        .send(StreamEvent::error(format!("Agent error: {}", e)))
                        .await;
                    (false, String::new())
                }
            };
            let final_taint: Vec<types::provenance::ProvenanceClass> =
                run_taint.lock().unwrap().iter().copied().collect();
            let _ = tx
                .send(StreamEvent::done_with_reason(loop_exit_reason).with_provenance(final_taint))
                .await;

            if !skip_memory {
                // The ONE chat-title generator for every run path (CODE_AUDITOR Rule 8;
                // replaces the old dispatch-side copy + the RunRequest.skip_title_gen
                // flag that coordinated the two). Background paths (scheduler/mcp)
                // simply have no sink, so they title without broadcasting. The voice
                // turn loop persists turns without a Runner run and calls the same
                // generator through Runner::spawn_title_generation.
                after_turn::spawn_chat_title_generation(
                    providers.clone(),
                    store.clone(),
                    session_mgr.active_chat_id(&session_id),
                    session_id.clone(),
                    selector.get_cheapest_model(),
                    title_sink.clone(),
                );
            }

            // ── Self-improvement review fork (docs/design/SELF_IMPROVEMENT.md WS2) ──
            // After REVIEW_TURN_INTERVAL turns without a voluntary skill save,
            // fork the conversation into its own throwaway session
            // (fork:<id>:review-*), replay the history verbatim (warm prefix
            // cache), and ask "what should be learned?". Gated on the
            // employee's learning_mode = "auto"; single-flight per session.
            // The fork runs with skip_memory=true, so it can never spawn a
            // review of itself, and its harness prompt never touches the
            // user's chat (the "curator takeover" lesson).
            if !skip_memory && run_ok && !cancel_token.is_cancelled() && !agent_id.is_empty() {
                // "auto" commits directly; "staged" stages to pending_writes
                // for Inbox approval; anything else (off/NULL) = no fork.
                let learning_mode = store
                    .get_entity_config("agent", &agent_id)
                    .ok()
                    .flatten()
                    .and_then(|c| c.learning_mode)
                    .map(|m| m.to_ascii_lowercase())
                    .unwrap_or_default();
                let learning_staged = learning_mode == "staged";
                if (learning_mode == "auto" || learning_staged)
                    && crate::review_fork::should_review(&session_id)
                    && crate::review_fork::try_begin(&session_id)
                {
                    let session_mgr_rf = session_mgr.clone();
                    let tools_rf = tools.clone();
                    let store_rf = store.clone();
                    let providers_rf = providers.clone();
                    let decide_rf = decide.clone();
                    let concurrency_rf = concurrency.clone();
                    let selector_rf = selector.clone();
                    let hooks_rf = hooks.clone();
                    let session_id_rf = session_id.clone();
                    let system_prompt_rf = system_prompt.clone();
                    let model_override_rf = model_override.clone();
                    let user_id_rf = user_id.clone();
                    let channel_rf = channel.clone();
                    let model_aliases_rf = model_aliases.clone();
                    let agent_registry_rf = agent_registry.clone();
                    let agent_id_rf = agent_id.clone();
                    let personality_snippet_rf = personality_snippet.clone();
                    let grant_rf = grant.clone();
                    let door_rf = req.door.clone();
                    let skill_loader_rf = skill_loader.clone();
                    let run_cwd_rf = run_cwd.clone();
                    let embedding_provider_rf = embedding_provider.clone();
                    let hybrid_searcher_rf = hybrid_searcher.clone();
                    let tool_scope_rf = tool_scope.clone();
                    let channel_ctx_rf = channel_ctx.clone();
                    let prompt_mode_rf = prompt_mode.clone();
                    tokio::spawn(async move {
                        info!(session_id = %session_id_rf, agent_id = %agent_id_rf, "self-improvement review fork starting");
                        let fork_key =
                            format!("fork:{}:review-{}", session_id_rf, uuid::Uuid::new_v4());
                        let fork_session =
                            match session_mgr_rf.get_or_create(&fork_key, &user_id_rf) {
                                Ok(s) => s,
                                Err(e) => {
                                    warn!(error = %e, "review fork: failed to create session");
                                    crate::review_fork::finish(&session_id_rf);
                                    return;
                                }
                            };
                        // Replay the parent conversation verbatim so the fork's
                        // request shares the parent's prefix (cache reads).
                        let msgs = session_mgr_rf
                            .get_messages(&session_id_rf)
                            .unwrap_or_default();
                        for m in &msgs {
                            let _ = session_mgr_rf.append_message(
                                &fork_session.id,
                                &m.role,
                                &m.content,
                                m.tool_calls.as_deref(),
                                m.tool_results.as_deref(),
                                m.metadata.as_deref(),
                            );
                        }
                        let _ = session_mgr_rf.append_message(
                            &fork_session.id,
                            "user",
                            crate::review_fork::REVIEW_PROMPT,
                            None,
                            None,
                            None,
                        );

                        let (sub_tx, mut sub_rx) = mpsc::channel::<StreamEvent>(256);
                        // Drain concurrently — a full channel would wedge the
                        // fork and hold the single-flight slot forever.
                        let drainer = tokio::spawn(async move {
                            let mut text = String::new();
                            while let Some(ev) = sub_rx.recv().await {
                                if ev.event_type == StreamEventType::Text {
                                    text.push_str(&ev.text);
                                }
                            }
                            text
                        });

                        let fork_cancel = CancellationToken::new();
                        let rfctx =
                            crate::review_fork::ReviewForkCtx::new(agent_id_rf.clone(), learning_staged);
                        let fork_taint = std::sync::Mutex::new(std::collections::BTreeSet::new());
                        let fork_result = run_loop(
                            &session_mgr_rf,
                            &tools_rf,
                            &store_rf,
                            &providers_rf,
                            &concurrency_rf,
                            &selector_rf,
                            &hooks_rf,
                            &sub_tx,
                            &fork_session.id,
                            &system_prompt_rf,
                            &model_override_rf,
                            &user_id_rf,
                            &channel_rf,
                            &model_aliases_rf,
                            Origin::System,
                            true, // skip_memory: no extraction/title/recursion from the fork
                            crate::review_fork::REVIEW_MAX_ITERATIONS,
                            &fork_cancel,
                            &agent_registry_rf,
                            &agent_id_rf,
                            personality_snippet_rf.as_deref(),
                            &grant_rf,
                            &door_rf,
                            crate::review_fork::REVIEW_PROMPT,
                            "",
                            skill_loader_rf.as_deref(),
                            run_cwd_rf.as_deref(),
                            None,
                            None,
                            0,
                            prompt_mode_rf,
                            None,
                            None,
                            0,     // review forks never hand off
                            embedding_provider_rf.as_ref(),
                            hybrid_searcher_rf.as_ref(),
                            tool_scope_rf.as_deref(),
                            false,
                            &[],
                            channel_ctx_rf.as_ref(),
                            None,
                            Some(rfctx),
                            None, // the review fork's whitelist rides ReviewForkCtx
                            None, // review forks use the built-in denial text
                            None, // review forks never serve CLI-provider tools
                            &fork_taint,
                            None, // review forks never reply to a coworker audience
                            None, // review forks are chat, never workflow mode
                            decide_rf.as_ref(),
                        )
                        .await;
                        drop(sub_tx);

                        let summary = drainer.await.unwrap_or_default();
                        match fork_result {
                            Ok(_) => {
                                let line = summary.trim().chars().take(300).collect::<String>();
                                info!(
                                    session_id = %session_id_rf,
                                    agent_id = %agent_id_rf,
                                    summary = %line,
                                    "self-improvement review finished"
                                );
                            }
                            Err(e) => {
                                warn!(session_id = %session_id_rf, error = %e, "self-improvement review failed");
                            }
                        }
                        crate::review_fork::finish(&session_id_rf);
                    });
                }
            }
        });

        Ok(rx)
    }

    /// One-shot convenience: prompt -> response text (no tools).
    pub async fn chat(&self, trace: RequestTrace, prompt: &str) -> Result<String, ProviderError> {
        let prov_lock = self.providers.read().await;
        if prov_lock.is_empty() {
            return Err(ProviderError::Request(
                "No providers configured".to_string(),
            ));
        }

        let req = ChatRequest {
            tool_credential: None,
            tool_choice: Default::default(),
            messages: vec![Message {
                role: "user".to_string(),
                content: prompt.to_string(),
                ..Default::default()
            }],
            tools: vec![],
            max_tokens: 4096,
            temperature: 0.7,
            system: String::new(),
            static_system: String::new(),
            model: String::new(),
            enable_thinking: false,
            metadata: None,
            cache_breakpoints: vec![],
            cancel_token: None,
            trace,
        };

        let mut rx = prov_lock[0].stream(&req).await?;
        drop(prov_lock); // Release lock before consuming stream
        let mut response = String::new();

        while let Some(event) = rx.recv().await {
            if event.event_type == StreamEventType::Text {
                response.push_str(&event.text);
            }
        }

        Ok(response)
    }

    /// The tool registry — the workflow adapter executes an approved pending
    /// call through it before re-entering the loop.
    pub fn tool_registry(&self) -> Arc<Registry> {
        self.tools.clone()
    }

    pub fn sessions(&self) -> &SessionManager {
        &self.sessions
    }

    pub fn concurrency(&self) -> &Arc<ConcurrencyController> {
        &self.concurrency
    }

    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }

    /// Get the number of active providers (blocking read for sync contexts).
    pub fn provider_count(&self) -> usize {
        // Use try_read to avoid blocking; fall back to 0 if locked
        match self.providers.try_read() {
            Ok(lock) => lock.len(),
            Err(_) => 0,
        }
    }
}

/// Closes a sub-agent's browser tab/page when its run ends — on normal return,
/// error, or cancellation (the run future being dropped). Best-effort; mirrors
/// the top-level cleanup the dispatcher does for non-sub-agent runs, via the one
/// canonical `Registry::close_browser_session` pathway.
struct SubagentTabCleanup {
    tools: Arc<Registry>,
    session_id: String,
}

impl Drop for SubagentTabCleanup {
    fn drop(&mut self) {
        let tools = self.tools.clone();
        let session_id = std::mem::take(&mut self.session_id);
        tokio::spawn(async move {
            tools.close_browser_session(&session_id).await;
        });
    }
}

/// The main agentic loop, running as an async task.
#[allow(clippy::too_many_arguments)]
/// Iterations a plan may go unchecked before the runner reminds the model.
const PLAN_REMINDER_EVERY: usize = 10;

/// The ONE predicate the loop uses for the plan reminder (tested directly;
/// the live site calls this, it does not re-implement it).
fn plan_reminder_due(iteration: usize, last_touch: usize) -> bool {
    iteration.saturating_sub(last_touch) >= PLAN_REMINDER_EVERY
}

/// The done gate fires at most this many times per run: once is a nudge to
/// run the checks; a second firing would be the spiral of a model that has
/// no checks to run.
const DONE_GATE_MAX: usize = 1;

/// A shell command that IS a project check. Running one resets the edit
/// count the done gate watches, exactly as a post-tool hook verdict does.
/// Word-bounded so `rustc` is not `tsc`.
static CHECK_VERB_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(
        r"\b(?:cargo (?:test|check|clippy)|pytest|go (?:test|vet)|pnpm (?:check|test|build)|npm (?:test|run)|npx tsc|tsc|vitest|jest|ruff|make (?:test|check))\b",
    )
    .expect("CHECK_VERB_RE is a literal")
});

pub(crate) fn is_check_command(command: &str) -> bool {
    CHECK_VERB_RE.is_match(command)
}

/// Does the done gate fire at the text-response exit? Only when edits landed
/// after the last check, and only [`DONE_GATE_MAX`] times per run.
fn done_gate_due(edits_since_check: usize, fired: usize) -> bool {
    edits_since_check > 0 && fired < DONE_GATE_MAX
}

/// A reply the owner already has, word for word (whitespace aside). A
/// Simulator session on 2026-09-22 sent the same 503-character apology five
/// times, each one narrating clicks that never happened.
fn repeats_earlier_reply(reply: &str, history: &[ChatMessage]) -> bool {
    let norm = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let r = norm(reply);
    r.len() >= 80 && history.iter().any(|m| m.role == "assistant" && norm(&m.content) == r)
}

/// What the last desktop act reported, cut to what a reply must agree with:
/// its first line (what was done), the screen header, and the first lines of
/// the element list.
pub(crate) fn desktop_evidence(result: &str) -> String {
    let mut lines = result.lines().filter(|l| !l.trim().is_empty());
    let mut out: Vec<&str> = lines.by_ref().take(2).collect();
    out.extend(lines.take_while(|l| !l.starts_with("Coordinates are")).take(12));
    out.join("\n")
}

/// Add every stored tool call not yet `checked` whose tool says its result
/// may be cleared once stale (`DynTool::cleared_when_stale`) to `clearable`.
/// A call to a tool no longer registered is never cleared.
pub(crate) async fn extend_clearable(tools: &Registry, messages: &[ChatMessage], checked: &mut HashSet<String>, clearable: &mut trim::Clearable) {
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
                && tool.cleared_when_stale(&call.input)
            {
                clearable.insert(call.id);
            }
        }
    }
}

async fn run_loop(
    sessions: &SessionManager,
    tools: &Arc<Registry>,
    store: &Arc<Store>,
    providers: &Arc<RwLock<Vec<Arc<dyn Provider>>>>,
    concurrency: &Arc<ConcurrencyController>,
    selector: &ModelSelector,
    hooks: &napp::HookDispatcher,
    tx: &mpsc::Sender<StreamEvent>,
    session_id: &str,
    system_prompt: &str,
    model_override: &str,
    user_id: &str,
    channel: &str,
    model_aliases: &str,
    origin: Origin,
    mut skip_memory: bool,
    max_iterations: usize,
    cancel_token: &CancellationToken,
    agent_registry: &tools::AgentRegistry,
    agent_id: &str,
    personality_snippet: Option<&str>,
    grant: &Arc<types::permissions::Grant>,
    door: &types::permissions::Door,
    user_prompt: &str,
    force_skill: &str,
    skill_loader: Option<&tools::skills::Loader>,
    run_cwd: Option<&str>,
    presence_tracker: Option<&Arc<crate::proactive::PresenceTracker>>,
    proactive_inbox: Option<&Arc<crate::proactive::ProactiveInbox>>,
    min_iterations: usize,
    prompt_mode: prompt::PromptMode,
    progress: Option<&RunProgress>,
    ask_channels: Option<&tools::AskChannels>,
    handoff_depth: u8,
    embedding_provider: Option<&Arc<dyn ai::EmbeddingProvider>>,
    hybrid_searcher: Option<&Arc<dyn tools::HybridSearcher>>,
    tool_scope: Option<&str>,
    plan_mode: bool,
    preactivate_tools: &[String],
    channel_ctx: Option<&tools::ChannelContext>,
    mention_context: Option<&str>,
    review_fork: Option<crate::review_fork::ReviewForkCtx>,
    tool_allowlist: Option<&std::collections::HashSet<String>>,
    tool_denial_hint: Option<String>,
    tool_credentials: Option<&crate::tool_credentials::ToolCredentials>,
    run_taint: &std::sync::Mutex<std::collections::BTreeSet<types::provenance::ProvenanceClass>>,
    audience: Option<&str>,
    workflow_mode: Option<&WorkflowMode>,
    decide: Option<&Arc<ai::DecideClient>>,
) -> Result<String, String> {
    let mut state = RunState::new();
    // Stream reminders are EPHEMERAL: queued here, injected into the NEXT
    // LLM call's messages in-memory, then dropped. Never persisted to the
    // session — a reminder that lands in stored history pollutes every
    // later context window AND leaks into channel mirrors/backfills.
    let mut pending_stream_reminders: Vec<String> = Vec::new();
    // The owner's spending limit escalates once: wrap-up turn, then stop.
    let mut spend_cap_wrap_up_issued = false;
    // The runaway backstop escalates the same way: the repeated call is
    // refused and the next turn is a tool-less wrap-up ("answer with what you
    // have"); only a repeat after that ends the turn. Ending it on the first
    // trip left the user a red "Stopped:" banner and no reply (Nanna,
    // 2026-09-19). Rule 12: never a silent kill.
    let mut runaway_wrap_up: Option<String> = None;
    let mut runaway_wrap_up_issued = false;
    // The trace a side call of this run carries: its purpose and the agent.
    let side_trace = |purpose: &'static str| RequestTrace {
        agent_id: agent_id.to_string(),
        ..RequestTrace::new(purpose)
    };
    // Temporal grounding (the harness pattern): every turn's first call
    // carries WHEN the message arrived, then the marker vanishes. The model
    // resolves "today/tomorrow/in an hour" against the message, not against
    // however stale its window is.
    pending_stream_reminders.push(steering::wrap_system_reminder(&format!(
        "Message sent at {}.",
        chrono::Local::now().format("%a %Y-%m-%d %H:%M %Z")
    )));
    if let Some(ctx) = mention_context {
        pending_stream_reminders.push(steering::wrap_system_reminder(ctx));
    }
    // A restricted run with nothing enabled hears it where the message is,
    // not only at the top of a long prompt (a flash model followed the
    // static tools lesson over a closing notice, 2026-09-05).
    if let Some(notice) = seat::restricted_run_notice(
        tool_allowlist.is_some_and(|wl| wl.is_empty()),
        tool_allowlist,
        tool_denial_hint.as_deref(),
    ) {
        pending_stream_reminders.push(steering::wrap_system_reminder(&notice));
    }
    let mut call_state = model_call::CallState::default();
    // Pre-seed called_tools with preactivated tools so they pass the tool filter
    // from turn 1 (bypasses deferred-loading discovery for sub-agents).
    let mut called_tools: Vec<String> = preactivate_tools.to_vec();
    // Rolling hashes of recent tool results for stale-result detection in steering
    // (name_hash, args_hash, result_hash, was_unproductive)
    let mut recent_tool_result_hashes: Vec<(u64, u64, u64, bool)> = Vec::new();
    // Per-turn count of each exact (tool, args) call, incremented on EVERY
    // execution regardless of productivity. Deliberately not the 10-entry
    // `recent_tool_result_hashes` ring — that is sized for ping-pong detection
    // and can never show more than 10 repeats, so a turn-level budget cannot be
    // read from it. Never reset mid-turn: the reset is exactly what let the
    // spiral nudge fire forever without ever ending a run.
    // Cross-method memory of files this run has observed (read_ledger.rs).
    // Reset per run on purpose: files legitimately change between turns.
    let mut read_ledger = crate::read_ledger::ReadLedger::default();
    // Frozen tool-result renderings: one rendering per tool_use_id per run,
    // applied by the per-step trim (`trim::trim`) so the model's history
    // never mutates mid-run.
    // FROZEN DECISIONS, per chat and persisted: the rendering a compacted tool
    // result was first shown as is its rendering forever, across runs and
    // restarts (the reference's `seenIds` + `replacements`, written to the
    // transcript). Loaded here, extended after each compaction pass below.
    let chat_id_for_renderings = store.resolve_session_chat_id(session_id);
    // The stored tool calls whose results may be cleared once stale (see
    // `extend_clearable`), and every call already asked.
    let mut clearable = trim::Clearable::new();
    let mut trim_checked: HashSet<String> = HashSet::new();
    let mut frozen_renderings: std::collections::HashMap<String, String> = store
        .get_chat_renderings(&chat_id_for_renderings)
        .unwrap_or_else(|e| {
            warn!(error = %e, "could not load frozen renderings; deciding fresh this run");
            std::collections::HashMap::new()
        });
    let mut persisted_renderings: std::collections::HashSet<String> =
        frozen_renderings.keys().cloned().collect();
    let mut identical_call_budget = ai::call_budget::CallBudget::new();
    // Per-(name, args) hash of a read-only call's own last result, for the
    // no-progress check: identical read + identical answer = no new information.
    let mut readonly_result_hash_by_call: std::collections::HashMap<(u64, u64), u64> =
        std::collections::HashMap::new();
    // Parallel vec of tool names (same indexing as recent_tool_result_hashes)
    let mut recent_tool_names: Vec<String> = Vec::new();
    // Hashes of recent tool-result CONTENT (any tool, any args) for tool-agnostic
    // redundant-fetch detection: the same file read via os(read), then cat, then jq
    // returns identical bytes through different calls — catch it regardless of how it
    // was requested. Last 20 kept.
    let mut recent_result_content_hashes: Vec<u64> = Vec::new();
    // Per-target read-failure counter (defense-in-depth backstop for the #research
    // read-loop incident): repeated FAILED reads of the SAME path — even via
    // different methods/args, which the identical-args guard misses — get blocked
    // after a threshold so the agent reports instead of spiraling. NOT a substitute
    // for the file-read fix.
    // The plan this run wrote or checked last, and at which iteration. After
    // PLAN_REMINDER_EVERY iterations without a plan_check the model is told to
    // run one before reporting done (the reference nags its todo list the same
    // way; ours nags for a MEASURED check, not a self-declared tick).
    let mut plan_touch: Option<(usize, String)> = None;
    // Done gate: os write/edit results since a check last ran (a post-tool
    // hook verdict, declared or inferred, or a check verb the model ran
    // itself). At the text-response exit a non-zero count sends the model
    // back to run the project's checks, DONE_GATE_MAX times per run.
    let mut edits_since_check: usize = 0;
    let mut done_gate_fired: usize = 0;
    // Result gates at the text-response exit, each once per run: a reply that
    // repeats an earlier one, and a reply after desktop acts checked against
    // the screen those acts left.
    let mut repeat_gate_fired = false;
    let mut desktop_gate_fired = false;
    let mut last_desktop_act: Option<String> = None;
    // Context accounting for the owner (Stage 8): where this run's tokens went.
    let mut ctx_compaction_passes: usize = 0;
    let mut ctx_evictions: usize = 0;
    let mut ctx_spilled_results: usize = 0;
    let mut read_failures: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    // Spiral backstop (FRAMES Phase 2): UNPRODUCTIVE repeats of the SAME (tool,
    // action) within a turn — errored or returning already-seen content — are the
    // wander-spiral the identical-args and read-failure guards both miss (glob
    // hunting across dirs, browser page re-reads, shell retries). After the
    // configured same-action limit of such attempts, return a terminal result so
    // the run ends and the agent reports instead of looping.
    // ponytail: result-novelty keyed (see counts_toward_action_spiral) — only
    // error/redundant attempts count, so legitimate bulk work (create N distinct
    // todos, write N files) no longer false-trips. File-read errors are also
    // excluded (per-path read_failures covers them) so exploring N paths does not
    // trip os:read at 8. NOT a substitute for clear tool errors — a misleading
    // error is what STARTS the spiral.
    // Seeded with half-strength carry-over from the previous turn's hot keys —
    // the cross-turn strategy-loop breaker (see CROSS_TURN_SPIRAL).
    let mut action_call_counts: std::collections::HashMap<String, usize> =
        cross_turn_seed(session_id);
    // Loop-guardrail thresholds — Settings → Developer, loaded once per run.
    let guard_cfg = crate::guardrails::GuardrailConfig::from_json(
        &store.get_guardrails().unwrap_or_else(|_| "{}".into()),
    )
    .sanitized();
    let auto_continuations = 0usize;
    // Cycle detection: track last auto-continued response to break loops
    let prev_auto_content: Option<String> = None;
    // Cache for tool documentation (help/schema results) — survives sliding window eviction
    // via injection into the dynamic suffix. Max 5 entries, LRU-evict oldest.
    let mut tool_doc_cache: Vec<(String, String)> = Vec::new();
    let mut consecutive_error_iterations = 0usize;
    let mut post_tool_empty_nudges = 0usize;
    let mut pseudo_call_nudges: usize = 0;
    let mut no_access_nudges: usize = 0;
    // Message-stream steering: per-run cadence for <system-reminder> injection.
    let mut reminder_cadence = steering::ReminderCadence::default();
    let mut review_trigger = crate::reviewer::Trigger::default();
    // (model, iteration the window ends at). Set once per run by a reviewer
    // stop verdict when models.yaml names an escalation model.
    let mut escalation: Option<(String, usize)> = None;
    let mut escalated_once = false;
    let mut turn_exit_reason = crate::guardrails::Exit::Unknown;
    // Stage 2 guards with their escalation attached (see guardrails.rs).
    let mut spiral_escalator = crate::guardrails::Escalator::default();
    let mut error_streak = crate::guardrails::ErrorStreak::default();
    let mut final_iteration = 0usize;
    let mut last_model_name = String::new();
    // Session-scoped tool schema cache: tool schemas don't change between turns,
    // so we cache them to prevent mid-session schema churn that busts the API's
    // prompt cache.
    let mut tool_schema_cache: HashMap<String, serde_json::Value> = HashMap::new();
    // Track file paths read during this session to detect duplicate reads.
    // When the model re-reads a file, a short note is appended to the tool result.
    let mut files_read_this_session: HashSet<String> = HashSet::new();

    // Resolve agent from registry if agent_id is set
    let active_agent_entry = if !agent_id.is_empty() {
        let reg = agent_registry.read().await;
        reg.get(agent_id).cloned()
    } else {
        None
    };

    // Explicit isolation context comes from the session KEY. `session_id`
    // here is the session ROW UUID — it never matches the key grammar, so
    // the key must be resolved first.
    let session_key = sessions
        .resolve_session_key(session_id)
        .unwrap_or_default();

    // The seat: memory scope, isolation, write bar, recall-for-audience,
    // sub-agent scope inheritance and the company-Memory seal.
    let seat::Seat {
        memory: memory_scope,
        memory_topics,
        write_bar: memory_write_bar,
        audience_restricted,
        memory_matter,
        company_memory_sealed,
        inherit_scopes,
        execution_mode,
    } = seat::resolve_seat(
        store,
        &session_key,
        seat::SeatInputs {
            agent: active_agent_entry.as_ref(),
            agent_id,
            user_id,
            session_id,
            origin,
            channel,
            audience,
        },
    );
    if audience_restricted {
        pending_stream_reminders.push(steering::wrap_system_reminder(
            "You are replying to a coworker who is NOT granted access to this scope's \
             matter/project memory. It was not consulted and must not be shared — answer \
             from working knowledge, or say the information isn't shared with their role.",
        ));
    }
    // Memory writes refused (isolated with no derivable context, or a
    // sub-agent): the extraction, flush, and personality paths refuse through
    // their existing gate.
    if memory_scope.writes_disabled {
        skip_memory = true;
    }
    let memory_user_id = memory_scope.user_id;
    let memory_writes_disabled = memory_scope.writes_disabled;

    // The turn decision's question (the task-tracking nudge) rides the
    // objective call (one Jev request per real user message). Fired here,
    // before recall and the rest of setup, so the round trip (a fresh
    // connection to Janus included) overlaps that setup and the answer is
    // usually waiting when the first step asks for it; no answer in time, or
    // none at all, and the keyword nudge runs for this turn (see
    // `turn_decide::receive`). Workflow turns and review forks have no person
    // speaking and run on scratch sessions: no objective call, so no turn
    // decision rides it.
    let objective_applies = objective_detection_applies(workflow_mode, review_fork.as_ref());
    let (turn_tx, mut turn_rx) =
        if objective_applies && decide.is_some() && crate::turn_decide::enabled() {
            let (tx, rx) = tokio::sync::oneshot::channel();
            (Some(tx), Some(rx))
        } else {
            (None, None)
        };
    let turn_fired = tokio::time::Instant::now();
    let mut turn_signals: Option<crate::turn_decide::TurnSignals> = None;

    // Fire objective detection in background (non-blocking). One typed
    // decision, milliseconds; it never touches the chat provider. Workflow
    // turns and review forks have no person speaking and run on scratch
    // sessions, so an objective there is paid for and never read.
    if objective_applies {
        let decide = decide.cloned();
        let providers = providers.clone();
        let store = store.clone();
        let session_id = session_id.to_string();
        let agent_id = agent_id.to_string();
        let user_prompt = sessions
            .get_messages(&session_id)
            .ok()
            .and_then(|msgs| {
                msgs.iter()
                    .rev()
                    .find(|m| m.role == "user")
                    .map(|m| m.content.clone())
            })
            .unwrap_or_default();
        tokio::spawn(async move {
            let session_mgr = SessionManager::new(store);
            detect_objective(
                decide.as_deref(),
                &agent_id,
                &providers,
                &session_mgr,
                &session_id,
                &user_prompt,
                turn_tx,
            )
            .await;
        });
    }

    // Kick off per-message memory recall CONCURRENTLY with the rest of prompt
    // assembly: its cost is a query-embedding network round trip (~650ms
    // steady-state), while the sibling loads below (db context, configured
    // inputs, task, skill template) are local SQLite/file work that doesn't
    // depend on it. tokio::spawn rather than futures::join! because those
    // siblings are synchronous — join! polls futures on THIS task, so the
    // sync work would serialize in front of the recall instead of overlapping.
    // Joined (and deduped against the identity slice, which needs db_ctx)
    // right before the run loop starts.
    let recall_task = if !user_prompt.is_empty() {
        hybrid_searcher
            .map(|searcher| db_context::spawn_prompt_recall(searcher, &memory_user_id, user_prompt))
    } else {
        None
    };

    // Load rich DB context (agent profile, user profile, personality directive, scored memories)
    let t_run_start = std::time::Instant::now();
    let db_ctx = db_context::load_db_context(store, &memory_user_id, agent_id, &inherit_scopes);
    let t_db_ctx = t_run_start.elapsed();
    info!(
        ms = t_db_ctx.as_millis() as u64,
        session_id, "[telemetry] db_context loaded"
    );

    // Extract user-configured timezone for date/time in the dynamic suffix
    let user_timezone = db_ctx
        .user
        .as_ref()
        .and_then(|u| u.timezone.clone())
        .filter(|tz| !tz.is_empty());

    // If running as an agent (persona), use the agent name as agent_name
    let agent_name = if let Some(ref agent) = active_agent_entry {
        agent.name.clone()
    } else {
        db_ctx
            .agent
            .as_ref()
            .map(|a| a.name.clone())
            .unwrap_or_else(|| "Nebo".to_string())
    };
    let mut db_context_formatted = db_context::format_for_system_prompt(&db_ctx, &agent_name);

    // Inject agent input_values into the system prompt so the LLM knows
    // about user-configured values (API keys, target market, etc.).
    // Without this, agents and their sub-agents ignore configured inputs.
    if !agent_id.is_empty()
        && let Ok(Some(agent_rec)) = store.get_agent(agent_id)
        && let Some(inputs) = db_context::format_configured_inputs(&agent_rec.input_values)
    {
        db_context_formatted.push_str(&format!("\n\n---\n\n{inputs}"));
    }

    // Get active task (mutable: refreshed periodically to catch async detect_objective)
    let mut active_task = sessions.get_active_task(session_id).unwrap_or_default();

    // Skills follow a deferred pattern: NOT auto-loaded into system prompt.
    // The skill listing names them; the model loads one with use_skill, and
    // its content goes into message history (tool results) and unloads when
    // messages are evicted by sliding window.
    //
    // Exceptions: force_skill (explicit API activation) and agent-declared skills
    // (part of the job definition — always present for that agent).
    let active_skill_template = if let Some(loader) = skill_loader {
        if !force_skill.is_empty() {
            // Scoped to the seat this run belongs to (the ONE extractor, so a
            // sub-agent resolves through its parent seat): a forced skill may
            // be one the employee's own package ships, which no unscoped
            // lookup can see.
            let seat = keyparser::extract_agent_id(&session_key);
            let skill_scope = (!seat.is_empty()).then_some(seat.as_str());
            match loader.get(force_skill, skill_scope).await {
                Some(skill) if skill.enabled => {
                    info!(skill = %skill.name, "force-activated skill");
                    Some(loader.expand_template(&skill, Some(store)))
                }
                _ => {
                    warn!(force_skill, "forced skill not found or disabled");
                    None
                }
            }
        } else {
            None
        }
    } else {
        None
    };

    // Join the concurrent memory recall spawned before the db-context load,
    // under a hard latency budget (db_context::join_prompt_recall): the vector
    // leg is a remote embed call and must never gate prompt assembly
    // unboundedly — past budget it degrades to the synchronous FTS-only tier.
    // `wait_ms` is the residual cost recall adds to assembly, capped by the
    // budget and ~0 when the search finished under the sibling loads above.
    let mut recalled_ids: Vec<i64> = Vec::new();
    if let Some(task) = recall_task {
        let t_join = std::time::Instant::now();
        let existing_ids: std::collections::HashSet<i64> = db_ctx
            .tacit_memories
            .iter()
            .map(|sm| sm.memory.id)
            .collect();
        let (relevant, ids) = db_context::join_prompt_recall(
            task,
            store,
            &memory_user_id,
            user_prompt,
            &existing_ids,
            audience_restricted,
        )
        .await;
        recalled_ids = ids;
        if !relevant.is_empty() {
            // Recall rides the first LLM call as an EPHEMERAL stream reminder
            // (message side) instead of the system prompt: per-turn content in
            // the prompt busts the prompt-cache prefix every turn, while the
            // identity slice above stays byte-stable. Same drain as the
            // timestamp reminder — injected once, never persisted.
            pending_stream_reminders.push(steering::wrap_system_reminder(&format!(
                "Recalled from your persistent memory (not new user input — \
                 treat as authoritative reference):\n{}",
                relevant
            )));
        }
        info!(
            wait_ms = t_join.elapsed().as_millis() as u64,
            session_id, "[telemetry] hybrid memory recall"
        );
    }

    // Access accounting: memories actually injected into this turn's context —
    // the identity slice (system prompt) plus per-message recall (stream
    // reminder) — get their access_count bumped
    // so decay ranking reflects real usefulness (without this, a new correct
    // memory loses to an old touched one forever). Spawned: never blocks the
    // hot path.
    {
        let mut injected_ids: Vec<i64> = db_ctx
            .tacit_memories
            .iter()
            .map(|sm| sm.memory.id)
            .collect();
        injected_ids.extend(&recalled_ids);
        if !injected_ids.is_empty() {
            let store_bump = store.clone();
            tokio::spawn(async move {
                for id in injected_ids {
                    let _ = store_bump.increment_memory_access(id);
                }
            });
        }
    }

    // Agent-declared skills are already in the skill catalog (compact name +
    // description). The LLM discovers and loads them on-demand via the skill
    // tool — same as every other skill. No need to dump full SKILL.md bodies
    // into the system prompt (that caused 230KB+ prompt bloat).

    // Pre-activate tools declared in agent.json — these are part of the agent's job
    // definition and must be available from turn 1 (not discovered via find_tools).
    // Agent-declared tools stay active for the entire session.
    // Scope-specific plugins are merged with global requires.plugins.
    let agent_preactivated: std::collections::HashSet<String> = {
        let mut set = std::collections::HashSet::new();
        if let Some(ref agent_entry) = active_agent_entry {
            if let Some(ref cfg) = agent_entry.config {
                let mut needs_plugin = !cfg.requires.plugins.is_empty();

                // Merge scope-specific plugin requirements
                if let Some(scope_name) = tool_scope {
                    if let Some(scope) = cfg.scopes.get(scope_name) {
                        if !scope.plugins.is_empty() {
                            needs_plugin = true;
                        }
                    }
                }

                if needs_plugin {
                    set.insert("plugin".to_string());
                    info!(
                        agent = %agent_entry.name,
                        plugins = ?cfg.requires.plugins,
                        scope = ?tool_scope,
                        "pre-activating plugin tool for agent-declared dependencies"
                    );
                }
                // Tools the employee's definition names outright (`requires.tools`):
                // part of its job, present from turn 1, no keyword or discovery needed.
                for tool in &cfg.requires.tools {
                    set.insert(tool.clone());
                }
            }
        }
        set
    };

    // Build static system prompt — use modular prompt when no custom one is provided
    // STRAP docs and tool list are NOT included here — they're injected per-iteration
    // based on which tools pass the context filter (dynamic injection).
    let active_agent_body = active_agent_entry.as_ref().map(|r| crate::harness::prompt::inputs::persona_body(&r.agent_md));
    // Build focused context for agent-required plugins (descriptions + skill names).
    let agent_plugin_context = active_agent_entry
        .as_ref()
        .map(|a| crate::harness::prompt::inputs::plugin_context(a, tool_scope, skill_loader))
        .unwrap_or_default();

    // Build agent self-awareness context: workflows, skills, and capabilities.
    // The agent must know about itself from turn 1.
    let agent_self_context = active_agent_entry
        .as_ref()
        .map(crate::harness::prompt::inputs::self_context)
        .unwrap_or_default();

    // The skill listing (name + one line per enabled skill), in the text
    // the harness reminder path delivers; it rides in the system prompt until
    // that path carries it. Full bodies load on demand through use_skill.
    // Agent-scoped runs also see their own skills.
    let skill_catalog = match skill_loader {
        Some(loader) => {
            let scope = (!agent_id.is_empty()).then_some(agent_id);
            let now = loader.listing(scope).await;
            crate::harness::events::LinedDelta::between(&Default::default(), &now)
                .and_then(|d| crate::harness::events::attachment_for(&crate::harness::events::TurnEvent::SkillListing(d)))
                .map(|a| a.text)
                .unwrap_or_default()
        }
        None => String::new(),
    };

    // Build compact agent catalog from DB (installed + user agents).
    let agent_catalog = match store.list_agents(100, 0) {
        Ok(agents) => {
            let enabled: Vec<_> = agents.iter().filter(|a| a.is_enabled == 1).collect();
            if enabled.is_empty() {
                String::new()
            } else {
                let mut lines = vec![format!("## Installed Agents ({})\n", enabled.len())];
                for a in &enabled {
                    let desc = if a.description.is_empty() {
                        String::new()
                    } else {
                        format!(" — {}", a.description)
                    };
                    lines.push(format!("- **{}**{}", a.name, desc));
                }
                lines.push(String::new());
                lines.push("get_employee(name) shows one in full; list_employees lists them all.".to_string());
                lines.join("\n")
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to load agent catalog");
            String::new()
        }
    };

    // Load workspace context file (.nebo.md or NEBO.md) — walk up from CWD to git root or home.
    let context_file = crate::harness::prompt::inputs::workspace_notes();

    // Resolved model identity for the stable prompt — the run's override when
    // set (the same "provider/model" string ToolContext.model_preference
    // carries), otherwise the selector's default so the line stays byte-stable
    // for the session.
    let resolved_model = if !model_override.is_empty() {
        model_override.to_string()
    } else {
        selector.select(&[])
    };

    let static_system = if system_prompt.is_empty() {
        let pctx = prompt::PromptContext {
            mode: prompt_mode,
            execution_mode,
            // Nothing enabled on a restricted run: the prompt teaches no
            // tools, because the model imitates what it is taught.
            no_tools: tool_allowlist.is_some_and(|wl| wl.is_empty()),
            agent_name: agent_name.clone(),
            active_skill: active_skill_template,
            agent_catalog,
            skill_catalog,
            model_aliases: model_aliases.to_string(),
            resolved_model,
            channel: channel.to_string(),
            platform: std::env::consts::OS.to_string(),
            memory_context: String::new(),
            db_context: Some(db_context_formatted.clone()),
            active_agent: active_agent_body,
            agent_soul: active_agent_entry.as_ref().and_then(|r| r.soul.clone()),
            agent_rules: active_agent_entry.as_ref().and_then(|r| r.rules.clone()),
            agent_plugin_context,
            agent_self_context,
            research_prompt: None,
            context_file,
        };
        prompt::build_static(&pctx)
    } else if workflow_mode.is_some() {
        // Workflow activities own their entire prompt — the engine already
        // injects agent identity + its memory slice; appending the chat
        // memory context here would double-inject it.
        system_prompt.to_string()
    } else {
        build_system_prompt(system_prompt, &db_context_formatted)
    };

    // Prepend personality snippet if provided by entity config
    let static_system = if let Some(snippet) = personality_snippet {
        if snippet.is_empty() {
            static_system
        } else {
            format!("{}\n\n{}", snippet, static_system)
        }
    } else {
        static_system
    };

    // Record run start time for sliding window protection
    let run_start_time = chrono::Utc::now().timestamp();

    // Use the extended ceiling for the loop range; adaptive check below enforces
    // the default limit unless the agent is making genuine progress.
    let hard_ceiling = max_iterations.max(EXTENDED_MAX_ITERATIONS);

    for iteration in 1..=hard_ceiling {
        final_iteration = iteration;
        // Update progress counter for external observers (RunRegistry dashboard)
        if let Some(p) = progress {
            p.iteration_count
                .store(iteration as u32, std::sync::atomic::Ordering::Relaxed);
        }

        // A file this session saw that the owner, a formatter, or a hook changed
        // since is surfaced once with its changed lines, so the model builds on
        // the change instead of reverting it. The ledger outlives the turn, so
        // the first iteration of a later turn catches edits made in between.
        // In the same pass: diagnostics a language server published for files
        // the employee did not just touch (an edit in one file that broke
        // another), delivered once with the reference's caps.
        {
            let tools = tools.clone();
            let session_key = session_key.clone();
            let sweep = tokio::task::spawn_blocking(move || {
                let mut notes = tools.external_edit_notes(&session_key);
                notes.extend(tools.new_diagnostics_note());
                notes
            })
            .await;
            match sweep {
                Ok(notes) => {
                    for note in notes {
                        pending_stream_reminders.push(steering::wrap_system_reminder(&note));
                    }
                }
                Err(e) => warn!(error = %e, "outside-edit sweep panicked; skipped this iteration"),
            }
        }

        if cancel_token.is_cancelled() {
            info!(session_id, "run cancelled before iteration {}", iteration);
            return Ok(turn_exit_reason.label());
        }

        // Every consumer drains the event receiver until the run completes, so a
        // closed channel means the consumer task died — stop instead of burning
        // iterations and tool calls into the void (every send is `let _ =`).
        if tx.is_closed() {
            warn!(
                session_id,
                iteration, "event receiver dropped — stopping run"
            );
            return Ok(turn_exit_reason.label());
        }

        // Adaptive iteration limit: extend past default only if making genuine progress.
        if iteration > max_iterations && iteration <= hard_ceiling {
            if consecutive_error_iterations >= 2 {
                turn_exit_reason = crate::guardrails::Exit::AdaptiveLimitNoProgress;
                let last_tool = recent_tool_names.last().cloned().unwrap_or_default();
                let worst_read = read_failures
                    .iter()
                    .max_by_key(|(_, c)| **c)
                    .map(|(p, c)| format!("{} (failed {}x)", p, c))
                    .unwrap_or_default();
                warn!(
                    session_id,
                    iteration,
                    consecutive_error_iterations,
                    last_tool = %last_tool,
                    repeated_read_failures = %worst_read,
                    "agentic loop stopping at adaptive iteration limit — no progress"
                );
                break;
            }
            if iteration == max_iterations + 1 {
                info!(
                    session_id,
                    "adaptive limit: extending past {} (making progress)", max_iterations
                );
            }
        }

        // agent.should_continue filter — let apps dynamically stop the agent
        if hooks.has_subscribers("agent.should_continue") {
            let payload = serde_json::to_vec(&crate::hooks::ShouldContinuePayload {
                session_id: session_id.to_string(),
                turn: iteration,
                total_tool_calls: called_tools.clone(),
                has_active_task: !active_task.is_empty(),
            })
            .unwrap_or_default();
            let (result, _) = hooks.apply_filter("agent.should_continue", payload).await;
            if let Ok(resp) =
                serde_json::from_slice::<crate::hooks::ShouldContinueResponse>(&result)
            {
                if !resp.should_continue {
                    info!(session_id, turn = iteration, reason = ?resp.reason, "hook requested stop");
                    break;
                }
            }
        }

        let t_iter_start = std::time::Instant::now();
        info!(iteration, session_id, "agentic loop iteration");

        // Load messages from session, then sanitize ordering.
        // Matches Go's sanitizeAgentMessages: strips orphaned tool results and
        // ensures tool results immediately follow their assistant message.
        let all_messages = sanitize_message_order(
            sessions
                .get_messages(session_id)
                .map_err(|e| format!("failed to load messages: {}", e))?,
        );
        let t_msg_load = t_iter_start.elapsed();
        info!(
            ms = t_msg_load.as_millis() as u64,
            iteration,
            session_id,
            msg_count = all_messages.len(),
            "[telemetry] messages loaded"
        );

        // Refresh active_task from DB periodically to catch:
        // 1. Background detect_objective() completing after initial read
        // 2. Task updates from tool calls (bot:task:update)
        if iteration <= 5 || iteration % 10 == 0 {
            let refreshed = sessions.get_active_task(session_id).unwrap_or_default();
            if !refreshed.is_empty() && refreshed != active_task {
                info!(session_id, iteration, old = %active_task, new = %refreshed, "active_task refreshed from DB");
                active_task = refreshed;
            }
        }

        if all_messages.is_empty() {
            let chat_id = sessions
                .resolve_session_key(session_id)
                .unwrap_or_else(|_| format!("(unresolved, fallback=chat-{})", session_id));
            warn!(
                session_id,
                chat_id = %chat_id,
                "No messages in session — session_key may not have been cached"
            );
            return Err(format!(
                "No messages in session (session_id={}, chat_id={})",
                session_id, chat_id
            ));
        }

        // Compute prompt overhead on first iteration
        if iteration == 1 {
            let system_tokens = static_system.len() / 4;
            let tool_defs = tools.list().await;
            let schema_tokens: usize = tool_defs
                .iter()
                .map(|t| (t.description.len() + t.input_schema.to_string().len()) / 4)
                .sum();
            state.prompt_overhead = system_tokens + schema_tokens + 4000;
            state.system_overhead_tokens = system_tokens + schema_tokens;
        }

        // Compute context thresholds — use model's actual context window when
        // available so large-context providers (200K/128K class) aren't
        // under-utilized.  Falls back to DEFAULT_CONTEXT_TOKEN_LIMIT (80K).
        let estimate_correction = state.estimate_correction;
        let thresholds = state.thresholds.get_or_insert_with(|| {
            let model_ctx = if !model_override.is_empty() {
                selector
                    .get_model_info(model_override)
                    .map(|m| m.context_window as usize)
                    .filter(|&w| w > 0)
            } else {
                let default_model = selector.select(&[]);
                if !default_model.is_empty() {
                    selector
                        .get_model_info(&default_model)
                        .map(|m| m.context_window as usize)
                        .filter(|&w| w > 0)
                } else {
                    None
                }
            };
            let context_window = model_ctx.unwrap_or(DEFAULT_CONTEXT_TOKEN_LIMIT);
            ContextThresholds::from_context_window(context_window, state.prompt_overhead)
        });
        // Calibrate with API-reported usage from the previous iteration: the
        // chars/4 estimate undercounts (tokenizer overhead, tool-call JSON),
        // so tighten thresholds by the observed error instead of trusting it.
        let thresholds = thresholds.adjusted(estimate_correction);
        let thresholds = &thresholds;

        // Pre-compaction memory flush: extract facts from ALL messages before
        // the sliding window evicts them. Only fires when new compactions have
        // occurred and the conversation is large enough to warrant it.
        if !skip_memory {
            // Write bar: a run whose taint intersects the scope's bar must not
            // flush facts into the scope (trust-boundaries design 2026-08-22).
            let flush_taint: Vec<types::provenance::ProvenanceClass> =
                run_taint.lock().unwrap().iter().copied().collect();
            let barred = flush_taint.iter().any(|c| memory_write_bar.contains(c));
            if barred {
                info!(
                    session_id,
                    classes = %types::provenance::label_classes(&flush_taint),
                    "memory flush barred by scope write bar"
                );
            } else if crate::memory_flush::should_run_memory_flush(
                &store,
                session_id,
                thresholds.auto_compact,
            ) {
                let prov = prefer_non_gateway(&providers.read().await);
                if let Some(prov) = prov {
                    crate::memory_flush::run_memory_flush(
                        prov.as_ref(),
                        &store,
                        session_id,
                        &memory_user_id,
                        &memory_topics,
                        embedding_provider.cloned(),
                        &flush_taint,
                    )
                    .await;
                }
            }
        }

        // --- Pre-eviction progressive compaction ---
        // Stages 1-3 reduce token count BEFORE the sliding window checks.
        // The window becomes a last resort instead of the first response.

        // What each stored call's tool says about trimming it.
        extend_clearable(tools, &all_messages, &mut trim_checked, &mut clearable).await;

        // Stage 1: the per-step trim (stale results cleared, old screenshots
        // dropped; frozen renderings applied).
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let (mut working, trim_saved) = trim::trim(&all_messages, now, &clearable, &mut frozen_renderings);
        if trim_saved > 0 {
            debug!(tokens_saved = trim_saved, "Stage 1: per-step trim");
        }

        // Freeze any rendering decided this pass so the next run makes the
        // same one byte for byte.
        let newly_frozen: Vec<(String, String)> = frozen_renderings
            .iter()
            .filter(|(k, _)| !persisted_renderings.contains(*k))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        if !newly_frozen.is_empty() {
            match store.insert_chat_renderings(&chat_id_for_renderings, &newly_frozen) {
                Ok(()) => persisted_renderings.extend(newly_frozen.into_iter().map(|(k, _)| k)),
                Err(e) => warn!(error = %e, "could not persist frozen renderings"),
            }
        }

        // Stage 3: Truncate old user/assistant messages
        let (summarized, ms_saved) = pruning::message_summarize(&working, thresholds.warning, 15);
        if ms_saved > 0 {
            debug!(tokens_saved = ms_saved, "Stage 3: message summarization");
            working = summarized;
        }
        if trim_saved + ms_saved > 0 {
            ctx_compaction_passes += 1;
        }

        // --- Eviction (last resort) ---

        // Stage 4: Sliding window — only fires if still over auto_compact after stages 1-3
        let (window_messages, evicted) =
            pruning::apply_sliding_window(&working, run_start_time, thresholds.auto_compact);

        // Record the local estimate for what this request will carry; compared
        // against API-reported usage when it arrives to set estimate_correction.
        state.last_request_estimate = pruning::estimate_total_tokens(&window_messages);

        // Build rolling summary if we evicted messages.
        // Quick fallback is used immediately (no LLM call); the LLM-quality
        // summary is generated in the background and stored for next iteration.
        let summary = if !evicted.is_empty() {
            ctx_evictions += 1;
            // The pre-eviction memory flush gate reads this counter.
            if let Err(e) = store.increment_session_compaction_count(session_id) {
                warn!(error = %e, "could not record the compaction");
            }
            let existing_summary = sessions.get_summary(session_id).unwrap_or_default();

            // Immediate: quick fallback (pure string extraction, no LLM)
            let quick = pruning::build_quick_fallback_summary(&evicted, &active_task);
            let immediate_summary = if existing_summary.len() > 4000 {
                quick // Replace — LLM summary will merge properly
            } else if existing_summary.is_empty() {
                quick
            } else {
                format!("{}\n\n{}", existing_summary, quick)
            };
            let _ = sessions.update_summary(session_id, &immediate_summary);

            // Background: fire LLM summary, store when done (non-blocking).
            // Throttled by `summary_due` — the quick fallback above already
            // captured this eviction, so skipping here loses nothing.
            let cheap_model = selector.get_cheapest_model();
            let prov = prefer_non_gateway(&providers.read().await)
                .filter(|_| summary_due(session_id, evicted.len()));
            if let Some(prov) = prov {
                let sess = sessions.clone();
                let sid = session_id.to_string();
                let task = active_task.clone();
                let existing = existing_summary.clone();
                let prov = concurrency.background(prov);
                let trace = side_trace("compaction");
                let handle = tokio::spawn(async move {
                    match pruning::build_llm_summary(
                        trace,
                        prov.as_ref(),
                        &evicted,
                        &existing,
                        &task,
                        &cheap_model,
                    )
                    .await
                    {
                        Ok(s) => {
                            let _ = sess.update_summary(&sid, &s);
                        }
                        Err(e) => {
                            debug!(error = %e, "background LLM compaction failed");
                        }
                    }
                    summary_done(&sid);
                });
                crate::memory_flush::track_extraction(handle).await;
            }

            // Background: index evicted messages for cross-session semantic search.
            // Fail-closed isolation: indexing writes conversation content under
            // memory_user_id, so a run whose isolation context could not be
            // derived must not index into the shared agent scope.
            if let Some(ep) = embedding_provider.filter(|_| !memory_writes_disabled) {
                let store_c = store.clone();
                let ep_c = ep.clone();
                let sid = session_id.to_string();
                let uid = memory_user_id.clone();
                let handle = tokio::spawn(async move {
                    transcript::index_compacted_messages(&store_c, ep_c.as_ref(), &sid, &uid).await;
                });
                crate::memory_flush::track_extraction(handle).await;
            }

            immediate_summary
        } else {
            sessions.get_summary(session_id).unwrap_or_default()
        };

        // The declared set (harness::tool_surface): core ∪ always_load ∪
        // the deferred tools `find_tools` loaded, derived from the stored
        // conversation so a load survives compaction of the window.
        let t_tools_start = std::time::Instant::now();
        let deferred_names = tools.get_deferred_names().await;
        let loaded = crate::harness::tool_surface::loaded_tools(&all_messages, &deferred_names);
        let mut all_tool_defs = tools.list().await;
        let mut agent_tool_names = tools.agent_tool_names(agent_id).await;

        // Scope filtering: restrict sidecar tools to those listed in the active scope
        if let Some(scope_name) = tool_scope {
            if let Some(ref agent_entry) = active_agent_entry {
                if let Some(ref cfg) = agent_entry.config {
                    if let Some(scope) = cfg.scopes.get(scope_name) {
                        if !scope.tools.is_empty() {
                            let scope_set: HashSet<String> = scope.tools.iter().cloned().collect();
                            agent_tool_names =
                                agent_tool_names.intersection(&scope_set).cloned().collect();
                            debug!(scope = %scope_name, tools = ?agent_tool_names, "scoped agent tools");
                        }
                    }
                }
            }
        }

        // ── Ethical wall: an isolated employee gets no company Memory ──
        if company_memory_sealed {
            seat::seal_company_memory(store, tools, agent_id, &mut all_tool_defs, &mut agent_tool_names).await;
        }

        // First step only: take the turn decision if it answered in time.
        // A closed channel (no client, an error, a continuation) is an
        // immediate keyword fallback, never a wait.
        if let Some(rx) = turn_rx.take() {
            turn_signals = crate::turn_decide::receive(rx, turn_fired).await;
        }

        // Always loaded for this employee: its `requires.tools` (and the
        // plugin tool when it requires plugins), its own app tools, and the
        // tools a parent handed this helper.
        let always_load: HashSet<String> = agent_preactivated
            .iter()
            .chain(agent_tool_names.iter())
            .chain(preactivate_tools.iter())
            .cloned()
            .collect();
        // What can still be listed: withheld tools are neither sent nor listed.
        let deferred_names: HashSet<String> = all_tool_defs
            .iter()
            .filter(|d| deferred_names.contains(&d.name))
            .map(|d| d.name.clone())
            .collect();
        let mut tool_defs = crate::harness::tool_surface::declared(
            all_tool_defs,
            &deferred_names,
            &always_load,
            &loaded,
        );
        let plugin_offered = tool_defs.iter().any(|d| d.name == "plugin");

        // Restricted runs (phone callers) declare ONLY their allowlisted
        // tools — an untrusted caller must not even see the rest of the
        // roster. This deliberately diverges from the review fork's
        // declare-everything invariant: the fork shares a prompt-cache
        // lineage with its parent conversation; a phone call is a fresh
        // session with its own lineage, so there is no cache to preserve.
        // Dispatch-time denial (whitelist_allows at the runner gate AND the
        // registry choke point) remains the enforcement backstop.
        if review_fork.is_none() {
            if let Some(wl) = tool_allowlist {
                tool_defs.retain(|td| allowlist_admits(wl, &td.name));
            }
        }
        // Told, not merely fenced: a restricted run with nothing left to
        // declare hears it in the system prompt, or it narrates tool calls.
        let restricted_notice = if review_fork.is_none() {
            seat::restricted_run_notice(tool_defs.is_empty(), tool_allowlist, tool_denial_hint.as_deref())
        } else {
            None
        };

        // Workflow mode: the activity's scoped set is the declaration —
        // rebuild from the full registry (deferred included: a declared MCP
        // tool's schema must ship) and synthesize the `exit` primitive.
        if let Some(m) = workflow_mode {
            let full = tools.list().await;
            tool_defs = full
                .into_iter()
                .filter(|td| m.advertised_tools.contains(&td.name))
                .collect();
            if m.advertised_tools.contains("exit") && !tool_defs.iter().any(|t| t.name == "exit") {
                let ex = tools::ExitTool::new();
                tool_defs.push(ai::ToolDefinition {
                    name: "exit".into(),
                    description: tools::registry::DynTool::description(&ex),
                    input_schema: tools::registry::DynTool::schema(&ex),
                });
            }
        }

        // The deferred tools listed by name: those not declared, within the
        // same fences as the declaration. A workflow activity's scoped set
        // is its whole surface.
        let mut listed = crate::harness::tool_surface::listed(&deferred_names, &tool_defs);
        if workflow_mode.is_some() {
            listed.clear();
        }
        if review_fork.is_none()
            && let Some(wl) = tool_allowlist
        {
            listed.retain(|name| allowlist_admits(wl, name));
        }

        // Pattern 3: Deterministic sort for prompt cache stability.
        // Stable alphabetical ordering ensures identical tool blocks across turns,
        // maximising API-side prompt cache hits.
        tool_defs.sort_by(|a, b| a.name.cmp(&b.name));

        // Pattern 4: Session-scoped schema memoization.
        // Tool schemas are immutable within a session, so reuse cached values to
        // prevent schema churn that would bust the prompt cache.
        for td in &mut tool_defs {
            if let Some(cached) = tool_schema_cache.get(&td.name) {
                td.input_schema = cached.clone();
            } else {
                tool_schema_cache.insert(td.name.clone(), td.input_schema.clone());
            }
        }

        // Read tracking tasks from pending_tasks (session-scoped list)
        let task_items_list_id = format!("session:{}", session_id);
        let work_tasks: Vec<steering::WorkTask> = store
            .list_task_items(&task_items_list_id)
            .unwrap_or_default()
            .into_iter()
            .map(|t| steering::WorkTask {
                id: t.id.clone(),
                subject: t.description.unwrap_or(t.prompt),
                status: t.status,
                details: None,
            })
            .collect();

        // Resolve user presence for steering (live from shared tracker)
        let (user_presence, user_just_returned) = if let Some(tracker) = presence_tracker {
            let p = tracker.get("_global").await;
            let jr = tracker.just_returned("_global").await;
            (p.map(|p| p.as_str().to_string()).unwrap_or_default(), jr)
        } else {
            (String::new(), false)
        };

        // Drain proactive inbox on first iteration only
        let proactive_items = if iteration == 1 {
            if let Some(inbox) = proactive_inbox {
                inbox.drain(session_id).await
            } else {
                vec![]
            }
        } else {
            vec![]
        };

        // Build per-iteration STRAP discovery (MCP servers) based on filtered tools.
        let filtered_tool_names: Vec<String> = tool_defs.iter().map(|t| t.name.clone()).collect();
        let strap_section = prompt::build_strap_section(&filtered_tool_names);
        let declared_tools: Arc<HashSet<String>> =
            Arc::new(filtered_tool_names.iter().cloned().collect());

        // The names-only listing of the deferred tools (tools doc §4.1). It
        // rides in the system prompt, stable while the listed set is, until
        // the harness reminder path delivers it as a delta attachment.
        let deferred_listing = if listed.is_empty() {
            String::new()
        } else {
            crate::harness::tool_surface::render_listing(
                &crate::harness::tool_surface::ListingDelta::all(listed),
            )
        };
        let tools_ms = t_tools_start.elapsed().as_millis() as u64;
        info!(
            ms = tools_ms,
            iteration,
            session_id,
            tool_count = tool_defs.len(),
            "[telemetry] tools filtered + prompt sections built"
        );

        // Select model: an open escalation window wins, then the override,
        // otherwise ask the selector.
        let selected_model = match crate::reviewer::window_model(escalation.as_ref(), iteration) {
            Some(model) => model.to_string(),
            None if !model_override.is_empty() => model_override.to_string(),
            None => selector.select(&window_messages),
        };

        // Determine thinking mode
        let enable_thinking = if workflow_mode.is_some() {
            false
        } else if !selected_model.is_empty() {
            let task = selector.classify_task(&window_messages);
            task == selector::TaskType::Reasoning && selector.supports_thinking(&selected_model)
        } else {
            false
        };

        // Parse selected model to find the right provider
        let (selected_provider_id, selected_model_name) = if selected_model.is_empty() {
            ("", "")
        } else {
            selector::parse_model_id(&selected_model)
        };
        last_model_name = selected_model_name.to_string();

        // A stop is the cancel path (Esc, the stop button): the token ends the
        // stream and the tools and the record says "interrupted". It is never
        // a phrase the runner matches — "stop searching and tell me" was not
        // on the list, and the owner was ignored three times (2026-09-18).

        // Every piece of steering below joins `pending_stream_reminders`, the one
        // channel: it rides this call (and its retry) and is gone once the call
        // lands (R8). `attach_stream_reminders` puts it into the call, right
        // before the request is built — nothing steering-shaped is stored or
        // written into the system prompt.

        // Background results (proactive inbox), drained on the turn's first call.
        let proactive_context = steering::format_proactive_items(&proactive_items);
        if !proactive_context.is_empty() {
            pending_stream_reminders.push(steering::wrap_system_reminder(&format!(
                "[Background Results]\n{}",
                proactive_context.join("\n")
            )));
        }

        // Hook: steering.generate — apps inject steering, re-evaluated each iteration.
        if hooks.has_subscribers("steering.generate") {
            let payload = serde_json::to_vec(&crate::hooks::SteeringGeneratePayload {
                session_id: session_id.to_string(),
                iteration,
            })
            .unwrap_or_default();
            let (result, _) = hooks.apply_filter("steering.generate", payload).await;
            if let Ok(resp) =
                serde_json::from_slice::<crate::hooks::SteeringGenerateResponse>(&result)
            {
                for d in resp.directives {
                    pending_stream_reminders.push(steering::wrap_system_reminder(&if d.label.is_empty() {
                        d.content
                    } else {
                        format!("{}: {}", d.label, d.content)
                    }));
                }
            }
        }

        // Session wake rail (R3): payloads that arrived while this run was
        // busy are heard mid-work — they join this call's stream reminders,
        // stamped delivered at injection (same ephemerality contract).
        let wake_entries = steering::drain_wakes(&session_key);
        if !wake_entries.is_empty() {
            let ids: Vec<i64> = wake_entries.iter().filter_map(|e| e.wake_id).collect();
            {
                let mut taint = run_taint.lock().unwrap();
                for entry in &wake_entries {
                    taint.extend(entry.taint.iter().copied());
                }
            }
            pending_stream_reminders.extend(wake_entries.into_iter().map(|e| e.content));
            if !ids.is_empty() {
                if let Err(e) = store.engine_complete_events(&ids, chrono::Utc::now().timestamp()) {
                    warn!(error = %e, "wake: failed to stamp mid-run delivery");
                }
            }
        }

        // A new turn on a session with earlier tool-heavy turns: the model
        // otherwise picks up the previous job's momentum (a pile of search
        // results and its own "on it, I'll let you know") and keeps going down
        // that path instead of answering what was just asked. Claude Code has
        // no such reminder because its transcript is compacted and its model
        // strong; here the first iteration says it outright.
        if iteration == 1 {
            if let Some(text) = steering::latest_message_reminder(&all_messages) {
                info!(session_id, "steering: latest-message-is-the-task reminder injected");
                pending_stream_reminders.push(steering::wrap_system_reminder(&text));
            }
        }

        // On external channels (NeboLoop/Slack/…) a weak model sometimes opens by
        // claiming it "isn't connected" and offering to simulate — it has its full
        // toolset, it just doesn't believe it. Ground it on the first iteration with
        // a stream <system-reminder> (which weak models heed where they ignore the
        // prompt). The post-tool-round reminder registry can't cover this — it fires
        // too late to shape the first reply.
        if iteration == 1 && steering::channel_is_external(channel) {
            pending_stream_reminders.push(steering::wrap_system_reminder(&format!(
                "You are fully connected on the `{channel}` channel with your complete \
                 toolset — web, files, installed plugins (call them via the `plugin` tool), \
                 skills, and sub-agents — exactly as in any other channel. When asked to do \
                 something, actually do it: call the real tools and report what you did with \
                 concrete results. Never simulate, mock, describe hypothetically, or claim \
                 you lack access — if you're unsure what's available, discover it with \
                 `find_tools` or the `plugin` tool first."
            )));
        }

        let mut ai_messages = convert_messages(&window_messages);

        // (First-run onboarding is handled proactively + deterministically by the
        // frontend OnboardingTour — the old reactive LLM-reminder kickoff was removed so
        // there's one onboarding pathway. The `nebo-onboarding` skill remains for an
        // explicit "help me get set up" request, matched by its description.)

        // The governance record of a workflow run names the model that
        // actually ran it, written the moment routing resolves it.
        if let Some(run_id) = tools::origin::workflow_run_id(&session_key) {
            let _ = store.update_workflow_run_model(run_id, &format!("{}/{}", selected_provider_id, selected_model_name));
        }

        // Build dynamic system suffix — AFTER model selection so identity is accurate
        let dctx = prompt::DynamicContext {
            provider_name: selected_provider_id.to_string(),
            model_name: selected_model_name.to_string(),
            agent_name: agent_name.clone(),
            active_task: active_task.clone(),
            summary: summary.clone(),
            neboai_connected: channel == "neboai",
            channel: channel.to_string(),
            work_tasks: work_tasks.clone(),
            tool_doc_cache: tool_doc_cache.clone(),
            user_timezone: user_timezone.clone(),
        };
        let dynamic_suffix = prompt::build_dynamic_suffix(&dctx);

        // Each tool's full declaration (description + JSON schema) lives in the
        // provider `tools` field — the single source. We do NOT add a
        // prose tool roster ("these are your ONLY tools this turn") or re-document
        // tools here; the model reads its tools natively. The system prompt only
        // carries behavior + MCP-server discovery + deferred-tool discovery.
        let full_system = if !system_prompt.is_empty() {
            format!("{}{}", static_system, dynamic_suffix)
        } else if deferred_listing.is_empty() {
            format!("{}\n\n{}{}", static_system, strap_section, dynamic_suffix)
        } else {
            format!(
                "{}\n\n{}\n\n{}{}",
                static_system, strap_section, deferred_listing, dynamic_suffix
            )
        };
        let full_system = match &restricted_notice {
            // The dynamic suffix is assembled after build_static and carries
            // its own examples; the same filter runs over the whole thing.
            Some(notice) => format!("{}\n\n{notice}", prompt::strip_call_syntax(&full_system)),
            None => full_system,
        };

        // Log prompt component sizes for debugging token bloat
        {
            let mut tool_sizes: Vec<(String, usize, usize)> = tool_defs
                .iter()
                .map(|t| {
                    let desc_len = t.description.len();
                    let schema_len = t.input_schema.to_string().len();
                    (t.name.clone(), desc_len, schema_len)
                })
                .collect();
            tool_sizes.sort_by(|a, b| (b.1 + b.2).cmp(&(a.1 + a.2)));
            let tool_schema_chars: usize = tool_sizes.iter().map(|(_, d, s)| d + s).sum();
            for (name, desc_len, schema_len) in &tool_sizes {
                info!(
                    tool = %name,
                    desc_chars = desc_len,
                    schema_chars = schema_len,
                    total_chars = desc_len + schema_len,
                    "[telemetry] per-tool schema size"
                );
            }
            info!(
                iteration,
                static_system_chars = static_system.len(),
                strap_chars = strap_section.len(),
                deferred_listing_chars = deferred_listing.len(),
                dynamic_suffix_chars = dynamic_suffix.len(),
                full_system_chars = full_system.len(),
                tool_schema_chars,
                tool_count = tool_defs.len(),
                "prompt component sizes"
            );
        }

        // Hook: message.pre_send — let apps modify system prompt before LLM call
        let full_system = if hooks.has_subscribers("message.pre_send") {
            let payload = serde_json::to_vec(&crate::hooks::PreSendPayload {
                system_prompt: full_system.clone(),
                message_count: ai_messages.len(),
            })
            .unwrap_or_default();
            let (result, _) = hooks.apply_filter("message.pre_send", payload).await;
            match serde_json::from_slice::<crate::hooks::PreSendResponse>(&result) {
                Ok(resp) => resp
                    .system_prompt
                    .filter(|s| !s.is_empty())
                    .unwrap_or(full_system),
                Err(_) => full_system,
            }
        } else {
            full_system
        };

        // Compute cache breakpoints for providers that support prompt caching.
        // Breakpoint 1: CACHE_BOUNDARY within static_system (stable identity/behaviour — rarely changes)
        // Breakpoint 2: end of static_system (semi-dynamic: skill hints, model aliases)
        // Everything after breakpoint 2 (STRAP, tools list, dynamic suffix) is fully dynamic.
        let cache_breakpoints = if !full_system.starts_with(static_system.as_str()) {
            // A pre_send hook rewrote the prompt: the offsets below would
            // slice its text at points that mean nothing, so cache nothing.
            debug!(session_id, "pre_send hook changed the prompt prefix; cache breakpoints dropped");
            Vec::new()
        } else {
            let mut bps = Vec::new();
            if let Some(boundary) = prompt::cache_boundary_offset(&static_system) {
                bps.push(boundary);
            }
            let static_len = static_system.len();
            if static_len > 0 && (bps.is_empty() || *bps.last().unwrap() < static_len) {
                bps.push(static_len);
            }
            bps
        };

        // "A named tool call is an instruction" — enforced in code, not prose.
        // When the fresh ask IS an explicit invocation of a declared tool
        // ("use os(...)"), the first response must be a tool call: weak models
        // otherwise echo the syntax back as text. First iteration only — the
        // model needs Auto afterwards to write its final report.
        // The owner spoke while the turn ran: the step right after is a reply
        // to them, in words — never another tool call. The framed message asks
        // for that; this makes it so whatever the model's momentum (nebo-1
        // read two more files past "stop reading and tell me" in one of two
        // runs before this). If they said "keep going", the reply is one line
        // and the work resumes at the next step.
        //
        // The message can land between this step's history load and now (it
        // did, in the same second as a tool result): re-read the tail, and if
        // the owner spoke, start the step over with their words in it.
        if sessions.get_messages(session_id).is_ok_and(|fresh| mid_turn_message_landed(&fresh, &all_messages)) {
            info!(session_id, iteration, "a message landed mid-turn: restarting the step with it");
            continue;
        }
        // A parent's message carries the parent's taint into this run, the way
        // a woken payload's taint does (the wake rail above).
        run_taint.lock().unwrap().extend(parent_taint(&all_messages));
        let owner_spoke_mid_turn = unanswered_mid_turn_message(&window_messages);
        if owner_spoke_mid_turn {
            info!(session_id, iteration, "owner spoke mid-turn: this step is a reply in words");
        }
        let forced_choice = if owner_spoke_mid_turn {
            Some(ai::ToolChoice::None)
        } else if iteration == 1 && !crate::goals::is_continuation_prompt(user_prompt) {
            // A continuation has no ask of its own in the thread; the last
            // user row is the owner's earlier message, already acted on.
            ai_messages
                .iter()
                .rev()
                .find(|m| m.role == "user" && !m.content.starts_with("<system-reminder>"))
                .and_then(|m| named_tool_invocation(&m.content, &tool_defs))
        } else {
            None
        };

        // The owner's per-run spending limit, checked between turns. Rule 12:
        // a guard escalates — the first trip is a wrap-up turn with no tools
        // ("report what you have"); if the cap is still reached after it,
        // the turn ends and the engine records the run as stopped with what
        // the model reported. Never a silent kill.
        let mut wrap_up_turn = false;
        if let Some(m) = workflow_mode {
            if m.spend_cap_microcents > 0 {
                let spent = usage::run_spend_so_far(store, selector, &session_key, &last_model_name, &state);
                match usage::spend_cap_verdict(spent, m.spend_cap_microcents, spend_cap_wrap_up_issued) {
                    usage::SpendCapVerdict::Under => {}
                    usage::SpendCapVerdict::WrapUp => {
                        spend_cap_wrap_up_issued = true;
                        wrap_up_turn = true;
                        warn!(session_id, spent_microcents = spent, cap_microcents = m.spend_cap_microcents, "spend cap reached: wrap-up turn");
                        pending_stream_reminders.push(steering::wrap_system_reminder(
                            "This run has reached the owner's spending limit. This is your last turn and \
                             tools are unavailable: report what you have completed, what you found, and \
                             what remains undone, in plain words. Do not start anything new.",
                        ));
                    }
                    usage::SpendCapVerdict::Stop => {
                        turn_exit_reason = crate::guardrails::Exit::SpendCapReached;
                        break;
                    }
                }
            }
        }

        // The runaway backstop's wrap-up turn (see runaway_wrap_up): no tools,
        // one reminder, the model answers.
        if let Some(text) = runaway_wrap_up.take() {
            wrap_up_turn = true;
            pending_stream_reminders.push(steering::wrap_system_reminder(&text));
        }

        // This call's steering, attached in the one place it enters a call.
        attach_stream_reminders(&mut ai_messages, &pending_stream_reminders);

        // Build ChatRequest
        let chat_req = ChatRequest {
            tool_credential: None,
            tool_choice: forced_choice.unwrap_or_default(),
            messages: ai_messages,
            tools: if wrap_up_turn { Vec::new() } else { tool_defs },
            max_tokens: call_state.max_output_tokens(),
            temperature: if workflow_mode.is_some() { 0.0 } else { 0.7 },
            system: full_system,
            static_system: static_system.clone(),
            model: if selected_model_name.is_empty() {
                String::new()
            } else {
                selected_model_name.to_string()
            },
            enable_thinking,
            metadata: call_state.sticky_metadata.clone(),
            cache_breakpoints,
            cancel_token: Some(cancel_token.clone()),
            // Tag this chat run so Janus attributes its usage per agent (no
            // workflow_id — chat runs are excluded from per-workflow rollups by
            // design; agent_id is the rollup key for chat spend).
            trace: match workflow_mode {
                // Workflow attribution: workflow/action/step ids ride to Janus.
                Some(m) => m.trace.clone(),
                None => RequestTrace {
                    agent_id: agent_id.to_string(),
                    run_id: progress.map(|p| p.run_id.clone()).unwrap_or_default(),
                    ..RequestTrace::new("agent_turn")
                },
            },
        };

        let pre_llm_ms = t_iter_start.elapsed().as_millis() as u64;
        info!(
            ms = pre_llm_ms,
            iteration, session_id, "[telemetry] pre-LLM overhead (msg load → request built)"
        );

        // The context this run's tool calls carry — the runner's own, and a
        // CLI provider's over /agent/mcp (through the credential below).
        let tool_scope = &crate::harness::tool_round::RunToolScope {
            sessions,
            session_id,
            origin,
            memory_user_id: &memory_user_id,
            handoff_depth,
            grant,
            door,
            untrusted_input: workflow_mode.is_some_and(|m| m.tainted),
            run_cwd,
            cancel_token,
            tx,
            progress,
            ask_channels,
            channel_ctx,
            model_override,
            memory_topics: &memory_topics,
            memory_writes_disabled,
            run_taint,
            memory_write_bar: &memory_write_bar,
            audience_restricted,
            memory_matter: &memory_matter,
            review_fork: review_fork.as_ref(),
            tool_allowlist,
            tool_denial_hint: &tool_denial_hint,
            declared_tools: &declared_tools,
        };

        // A CLI provider runs its tools itself, over /agent/mcp. The call
        // below issues a credential through this when it lands on one; the
        // provider's tool calls carry it and execute as this run.
        let issue_tool_credential = tool_credentials.map(|credentials| {
            move || {
                credentials.issue(crate::tool_credentials::RunGrant {
                    ctx: tool_scope.tool_context(),
                    agent_id: agent_id.to_string(),
                })
            }
        });

        let reply = match model_call::call_model(
            model_call::ModelCall {
                request: chat_req,
                providers,
                selector,
                concurrency,
                sessions,
                cancel: cancel_token,
                tx,
                session_id,
                step: iteration,
                step_started: t_iter_start,
                selected_provider_id,
                selected_model: &selected_model,
                model_override,
                context_limit: thresholds.auto_compact,
                tool_credential: issue_tool_credential
                    .as_ref()
                    .map(|issue| issue as &(dyn Fn() -> crate::tool_credentials::CredentialGuard + Send + Sync)),
            },
            &mut call_state,
            &mut state,
            &mut pending_stream_reminders,
        )
        .await
        {
            model_call::CallOutcome::Reply(reply) => reply,
            model_call::CallOutcome::Retry(model_call::RetryWhy::StreamCut) => {
                if let Some(cut) = crate::harness::events::attachment_for(&crate::harness::events::TurnEvent::StreamCut) {
                    pending_stream_reminders.push(steering::wrap_system_reminder(&cut.text));
                }
                continue;
            }
            model_call::CallOutcome::Retry(_) => continue,
            model_call::CallOutcome::Cancelled => return Ok(turn_exit_reason.label()),
            model_call::CallOutcome::CancelledInBackoff => return Ok("cancelled".to_string()),
            model_call::CallOutcome::Exhausted => break,
            model_call::CallOutcome::Failed(e) => return Err(e),
        };
        let model_call::ModelReply {
            text: assistant_content,
            mut tool_calls,
            stop: stop_reason,
            stream_error,
            mut block_order,
            provider,
        } = reply;

        // Hook: message.post_receive — let apps modify response text before saving
        let assistant_content = if hooks.has_subscribers("message.post_receive") {
            let payload = serde_json::to_vec(&crate::hooks::PostReceivePayload {
                response_text: assistant_content.clone(),
                tool_calls_count: tool_calls.len(),
            })
            .unwrap_or_default();
            let (result, _) = hooks.apply_filter("message.post_receive", payload).await;
            match serde_json::from_slice::<crate::hooks::PostReceiveResponse>(&result) {
                Ok(resp) => resp.response_text.unwrap_or(assistant_content),
                Err(_) => assistant_content,
            }
        } else {
            assistant_content
        };

        // Early cycle detection: if this is an auto-continuation iteration and
        // the response is identical to the previous one, skip the persist entirely
        // to avoid duplicate rows in the DB.
        if auto_continuations > 0 {
            if let Some(ref prev) = prev_auto_content {
                if prev == &assistant_content {
                    info!(
                        iteration,
                        session_id,
                        auto_continuations,
                        "cycle detected before persist: identical response, skipping save"
                    );
                    break;
                }
            }
        }

        // A wrap-up turn offered no tools. A tool call that comes back anyway
        // (some providers still emit one) is dropped here, before persistence,
        // so it is neither saved nor executed. No text with it = the model
        // answered nothing; the turn ends with the exit the wrap-up was for.
        if wrap_up_turn && !tool_calls.is_empty() {
            warn!(
                session_id,
                iteration,
                dropped = tool_calls.len(),
                "wrap-up turn returned tool calls with no tools offered — dropped"
            );
            tool_calls.clear();
            if assistant_content.trim().is_empty() {
                turn_exit_reason = if runaway_wrap_up_issued {
                    crate::guardrails::Exit::RunawayToolLoop
                } else {
                    crate::guardrails::Exit::SpendCapReached
                };
                let _ = tx
                    .send(StreamEvent::control_notice(
                        "Stopped: the model kept calling tools after being asked to \
                         answer with what it has.",
                        "runaway_tool_loop",
                    ))
                    .await;
                break;
            }
        }

        // Save assistant message.
        // If there was a stream error, strip tool_calls — they won't be executed
        // so saving them would create orphans in the session history.
        let save_tool_calls = stream_error.is_none();
        if !assistant_content.is_empty() || (save_tool_calls && !tool_calls.is_empty()) {
            let tc_json = if !save_tool_calls || tool_calls.is_empty() {
                None
            } else {
                serde_json::to_string(&tool_calls).ok()
            };

            // Persist the content block order so rehydration preserves it.
            let metadata =
                if block_order.len() > 1 || block_order.first().map_or(false, |b| b.0 == "tool") {
                    let blocks: Vec<serde_json::Value> = block_order
                        .iter()
                        .map(|(kind, idx)| match (*kind, idx) {
                            ("tool", Some(i)) => {
                                serde_json::json!({"type": "tool", "toolCallIndex": i})
                            }
                            _ => serde_json::json!({"type": "text"}),
                        })
                        .collect();
                    serde_json::to_string(&serde_json::json!({"contentBlocks": blocks})).ok()
                } else {
                    None // single text block = default order, no need to persist
                };

            if let Err(e) = sessions.append_message(
                session_id,
                "assistant",
                &assistant_content,
                tc_json.as_deref(),
                None,
                metadata.as_deref(),
            ) {
                warn!(session_id = %session_id, error = %e, "failed to save assistant message to DB");
            }

            // Hook: session.message_append — notify apps that a message was saved
            if hooks.has_subscribers("session.message_append") {
                let payload = serde_json::to_vec(&crate::hooks::MessageAppendPayload {
                    session_id: session_id.to_string(),
                    role: "assistant".to_string(),
                    content: assistant_content.clone(),
                })
                .unwrap_or_default();
                hooks.do_action("session.message_append", payload).await;
            }
        }

        // Plan mode: on first iteration with tool calls, pause for user approval.
        if plan_mode && iteration == 1 && !tool_calls.is_empty() {
            if let Some(ask_chs) = ask_channels {
                let plan_text = if !assistant_content.is_empty() {
                    assistant_content.clone()
                } else {
                    format!(
                        "I'd like to execute {} tool calls: {}",
                        tool_calls.len(),
                        tool_calls
                            .iter()
                            .map(|tc| tc.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                };
                let tool_names: Vec<String> = tool_calls.iter().map(|tc| tc.name.clone()).collect();
                use crate::harness::permissions::plan::{approve_plan, PlanAnswer};
                match approve_plan(ask_chs, tx, cancel_token, session_id, &plan_text, tool_names).await {
                    PlanAnswer::Cancelled => {
                        info!(session_id, "plan approval cancelled");
                        return Ok(turn_exit_reason.label());
                    }
                    PlanAnswer::Rejected => {
                        info!(session_id, "plan rejected by user");
                        let _ = tx
                            .send(StreamEvent::text(
                                "\n\nPlan was rejected. Let me know how you'd like to proceed."
                                    .to_string(),
                            ))
                            .await;
                        let _ = sessions.append_message(
                            session_id,
                            "assistant",
                            "Plan was rejected. Let me know how you'd like to proceed.",
                            None,
                            None,
                            None,
                        );
                        break;
                    }
                    PlanAnswer::Approved => info!(session_id, "plan approved, proceeding with tool execution"),
                }
            }
        }

        if tool_calls.is_empty() && steering::looks_like_pseudo_call(&assistant_content) {
            let parsed = steering::parse_pseudo_calls(&assistant_content);
            if !parsed.is_empty() {
                // Run what it wrote: the arguments are all there, only the
                // framing was wrong. Fix the API, not the client.
                warn!(iteration, session_id, n = parsed.len(), "tool call written as text; running it");
                for (k, (name, input)) in parsed.into_iter().enumerate() {
                    let tc = ai::ToolCall {
                        id: format!("pseudo-{iteration}-{k}"),
                        name,
                        input,
                    };
                    // Announced like a streamed call, so the thread, the
                    // harness, and the run receipt all see it.
                    let _ = tx.send(StreamEvent::tool_call(tc.clone())).await;
                    tool_calls.push(tc);
                    block_order.push(("tool", Some(tool_calls.len() - 1)));
                }
            } else if pseudo_call_nudges < 1 {
                pseudo_call_nudges += 1;
                warn!(iteration, session_id, "tool call written as text; nudging");
                pending_stream_reminders.push(steering::wrap_system_reminder(
                    "You wrote a tool call as text instead of calling it. Nothing ran. \
                     Make that call now as a real tool call, with the same arguments.",
                ));
                continue;
            }
        }

        // CLI providers handle their own tool execution via MCP — skip runner tool loop
        if provider.handles_tools() && !tool_calls.is_empty() {
            info!(
                session_id,
                tool_count = tool_calls.len(),
                "CLI provider handled tools via MCP"
            );
            break;
        }

        // Execute tool calls in parallel
        if !tool_calls.is_empty() {
            let round = crate::harness::tool_round::run_tool_round(
                &crate::harness::tool_round::RoundContext {
                    scope: tool_scope,
                    tools,
                    providers,
                    concurrency,
                    hooks,
                    user_prompt,
                    iteration,
                    workflow_mode,
                    decide,
                    active_task: &active_task,
                    turn_mode: None,
                    guard_cfg: &guard_cfg,
                    side_trace: &side_trace,
                },
                crate::harness::tool_round::RoundGuards {
                    called_tools: &mut called_tools,
                    recent_tool_result_hashes: &recent_tool_result_hashes,
                    identical_call_budget: &identical_call_budget,
                    runaway_wrap_up: &mut runaway_wrap_up,
                    runaway_wrap_up_issued: &mut runaway_wrap_up_issued,
                    read_failures: &mut read_failures,
                    action_call_counts: &mut action_call_counts,
                    spiral_escalator: &mut spiral_escalator,
                    error_streak: &mut error_streak,
                    files_read_this_session: &mut files_read_this_session,
                    recent_result_content_hashes: &mut recent_result_content_hashes,
                    readonly_result_hash_by_call: &mut readonly_result_hash_by_call,
                    read_ledger: &mut read_ledger,
                    tool_doc_cache: &mut tool_doc_cache,
                    plan_touch: &mut plan_touch,
                    edits_since_check: &mut edits_since_check,
                    last_desktop_act: &mut last_desktop_act,
                    ctx_spilled_results: &mut ctx_spilled_results,
                },
                &mut tool_calls,
            )
            .await;
            let crate::harness::tool_round::RoundResults {
                all_errors_this_iteration,
                had_results,
                unproductive_this_iteration,
                iteration_rate_limited,
                summary_tool_calls,
                summary_tool_results,
            } = match round {
                crate::harness::tool_round::RoundOutcome::Ran(results) => results,
                crate::harness::tool_round::RoundOutcome::Ended(exit) => {
                    turn_exit_reason = exit;
                    break;
                }
                crate::harness::tool_round::RoundOutcome::Cancelled => {
                    return Ok(turn_exit_reason.label());
                }
            };

            // Compute tool call hashes for loop detection (OpenClaw-style).
            // Tuple: (name_hash, args_hash, result_hash) — detects same-tool-same-args
            // and stale results independently.
            for tc in &tool_calls {
                let name_hash = simple_hash(tc.name.as_bytes());
                let args_str = tc.input.to_string();
                let args_hash = simple_hash(args_str.as_bytes());
                // Hash first 2000 bytes of the most recent result for this tool
                let content_hash = sessions
                    .get_messages(session_id)
                    .ok()
                    .and_then(|msgs| msgs.iter().rev().find(|m| m.role == "tool").cloned())
                    .and_then(|m| m.tool_results)
                    .map(|tr| simple_hash(tr.as_bytes().get(..2000).unwrap_or(tr.as_bytes())))
                    .unwrap_or(0);
                let unproductive = unproductive_this_iteration
                    .get(&(name_hash, args_hash))
                    .copied()
                    .unwrap_or(false);
                recent_tool_result_hashes.push((
                    name_hash,
                    args_hash,
                    content_hash,
                    unproductive,
                ));
                // Turn-level repeat budget: counts the CALL, not the answer, so a
                // poll whose output drifts every time still accrues (see
                // IDENTICAL_CALL_ABORT).
                identical_call_budget.record(&tc.name, &tc.input);
                recent_tool_names.push(tc.name.clone());
                // Engine-stamped provenance: union this call's class into
                // the run's taint set (the tool's spec; model-invisible).
                if let Some(class) = tools.get(&tc.name).await.and_then(|t| t.taint(&tc.input)) {
                    run_taint.lock().unwrap().insert(class);
                }
                // Keep last 10 for ping-pong detection
                if recent_tool_result_hashes.len() > 10 {
                    recent_tool_result_hashes.remove(0);
                    recent_tool_names.remove(0);
                }
            }

            // Update consecutive error iteration counter
            if had_results && all_errors_this_iteration {
                consecutive_error_iterations += 1;
                warn!(
                    session_id,
                    iteration, consecutive_error_iterations, "all tool calls failed this iteration"
                );
            } else {
                consecutive_error_iterations = 0;
            }

            if let Some((at, path)) = plan_touch.as_mut() {
                if plan_reminder_due(iteration, *at) {
                    pending_stream_reminders.push(steering::wrap_system_reminder(&format!(
                        "Plan {path}: {} iterations since its last check. Run check_plan(path: \
                         \"{path}\") before reporting the task done; only a passing verify command \
                         ticks a step.",
                        iteration.saturating_sub(*at)
                    )));
                    *at = iteration;
                }
            }

            // Message-stream steering: inject at most one <system-reminder> after
            // tool results, where a weak model actually attends. Inert until the
            // reminder registry is populated in later rounds.
            {
                let msgs = sessions.get_messages(session_id).unwrap_or_default();
                let detected_mode = sessions.get_detected_mode(session_id);
                let taint_snapshot: Vec<types::provenance::ProvenanceClass> =
                    run_taint.lock().unwrap().iter().copied().collect();
                let rctx = steering::ReminderContext {
                    iteration,
                    execution_mode,
                    messages: &msgs,
                    recent_tool_names: &recent_tool_names,
                    run_taint: &taint_snapshot,
                    provider_id: selected_provider_id,
                    work_tasks: &work_tasks,
                    user_prompt,
                    multi_stage: turn_signals.as_ref().map(|t| t.multi_stage),
                    active_task: &active_task,
                    recent_tool_result_hashes: &recent_tool_result_hashes,
                    user_presence: &user_presence,
                    user_just_returned,
                    quota_warning: state.quota_warning.as_deref(),
                    consecutive_error_iterations,
                    max_iterations,
                    agent_name: &agent_name,
                    agent_soul: active_agent_entry.as_ref().and_then(|r| r.soul.as_deref()),
                    detected_mode: &detected_mode,
                    rate_limited: iteration_rate_limited,
                    channel,
                };
                let mut reviewer_stop: Option<String> = None;
                if let Some(reminder) = steering::select_reminder(&rctx, &mut reminder_cadence) {
                    info!(session_id, iteration, reminder = ?reminder_cadence.last_fired_name(), "steering reminder fired");
                    pending_stream_reminders.push(reminder);
                    // A loop-class reminder firing twice means the notes in the
                    // model's own stream are not landing: bring in the reviewer,
                    // a different reader with the goal and the last steps.
                    if let Some(name) = reminder_cadence.last_fired_name()
                        && review_trigger.note(name, iteration)
                    {
                        let prov_snapshot: Vec<Arc<dyn Provider>> = providers.read().await.clone();
                        let steps = crate::reviewer::describe_steps(&msgs);
                        let goal = if active_task.is_empty() { user_prompt } else { active_task.as_str() };
                        match crate::reviewer::review(side_trace("loop_review"), &prov_snapshot, goal, &steps, name).await {
                            Some(v) if v.stop => {
                                // A stop is the reviewer saying this model on
                                // this path cannot finish. Before ending the
                                // run, try the path on a stronger model once.
                                let spec = config::ModelsConfig::load()
                                    .defaults
                                    .map(|d| d.escalation)
                                    .unwrap_or_default();
                                match (escalated_once, crate::reviewer::escalation_model(&spec, &prov_snapshot)) {
                                    (false, Some(model)) => {
                                        escalated_once = true;
                                        let until = iteration + crate::reviewer::ESCALATION_ITERATIONS;
                                        info!(session_id, iteration, model = %model, until, advice = %v.advice, "reviewer escalated the run");
                                        pending_stream_reminders.push(steering::wrap_system_reminder(&format!(
                                            "A reviewer stopped the approach you were on: {} Your next {} steps run on a stronger model. Take a different path with them; do not repeat the last step.",
                                            v.advice,
                                            crate::reviewer::ESCALATION_ITERATIONS
                                        )));
                                        escalation = Some((model, until));
                                    }
                                    _ => reviewer_stop = Some(v.advice),
                                }
                            }
                            Some(v) => {
                                info!(session_id, iteration, advice = %v.advice, "reviewer advised");
                                pending_stream_reminders.push(steering::wrap_system_reminder(&format!(
                                    "A reviewer looked at your last {} steps. {}",
                                    steps.len(),
                                    v.advice
                                )));
                            }
                            None => {}
                        }
                    }
                }
                if let Some(reason) = reviewer_stop {
                    warn!(session_id, iteration, reason = %reason, "reviewer stopped the run");
                    turn_exit_reason = crate::guardrails::Exit::ReviewerStop;
                    let _ = tx
                        .send(StreamEvent::control_notice(
                            format!("Stopped by the reviewer: {reason}"),
                            "reviewer_stop",
                        ))
                        .await;
                    break;
                }
            }

            // Clear current tool in progress tracker
            if let Some(p) = progress {
                if let Ok(mut ct) = p.current_tool.lock() {
                    ct.clear();
                }
            }

            // agent.turn action — notify apps after tool execution
            if hooks.has_subscribers("agent.turn") {
                let turn_tool_names: Vec<String> =
                    tool_calls.iter().map(|tc| tc.name.clone()).collect();
                let payload = serde_json::to_vec(&crate::hooks::TurnPayload {
                    session_id: session_id.to_string(),
                    turn: iteration,
                    tool_calls: turn_tool_names,
                    total_tool_calls: called_tools.clone(),
                    has_active_task: !active_task.is_empty(),
                })
                .unwrap_or_default();
                hooks.do_action("agent.turn", payload).await;
            }

            // Pattern 12: skip post-run memory extraction when this iteration
            // contained an explicit memory write (`remember`). Re-extracting
            // would duplicate facts the model just wrote.
            if !skip_memory && tool_calls.iter().any(|tc| tc.name == "remember") {
                debug!(session_id, "memory write detected — skipping post-run extraction");
                skip_memory = true;
            }

            // Pattern 13: background tool summary generation via cheap model.
            after_turn::hand_off_tool_summary(
                session_id,
                providers,
                tx,
                &assistant_content,
                summary_tool_calls,
                summary_tool_results,
                side_trace("tool_summary"),
            )
            .await;

            // Reset post-tool nudge flag after successful tool execution
            // so it can fire again if the model goes empty on a later tool round.
            post_tool_empty_nudges = 0;

            // Continue loop — LLM needs to respond to tool results
            continue;
        }

        // Cut off by the output cap: retry at the escalated cap, then continue in place.
        if let Some(retry) =
            model_call::output_cutoff(&mut call_state, stop_reason.as_deref(), iteration, session_id)
        {
            if let model_call::StepRetry::Resume = retry
                && let Some(resume) = crate::harness::events::attachment_for(&crate::harness::events::TurnEvent::CutoffResume)
            {
                pending_stream_reminders.push(steering::wrap_system_reminder(&resume.text));
            }
            continue;
        }

        // Token budget continuation: if min_iterations is set and not yet reached,
        // force-continue even if the LLM wants to stop.
        if min_iterations > 0 && iteration < min_iterations && tool_calls.is_empty() {
            if cancel_token.is_cancelled() {
                info!(
                    session_id,
                    "skipping budget continuation: run was cancelled"
                );
                break;
            }
            if !assistant_content.is_empty() {
                info!(
                    iteration,
                    session_id,
                    min = min_iterations,
                    "budget continuation: forcing next iteration"
                );
                // Budget continuation as an ephemeral stream reminder (R8).
                pending_stream_reminders.push(steering::wrap_system_reminder(
                    "You stopped early but your task is not complete. \
                     Keep working — use your tools to make more progress. \
                     Do not summarize or ask to continue. Take the next action.",
                ));
                continue;
            }
        }

        // A tool call written as text ran nothing. Say so once and let the
        // model make the call; a reply of "os(resource: ..., action: ...)" is
        // not an answer the user can use.

        // "I don't have access to X" with the plugin tool on the table and no
        // discover call is an answer from memory (smoke 2026-09-05: a tweet
        // request got "no Twitter plugin" and zero calls). Once: point at
        // discover; the marketplace is where access comes from.
        if tool_calls.is_empty() && plugin_offered && no_access_nudges < 1 {
            let lower = assistant_content.to_ascii_lowercase();
            let denies = (lower.contains("don't have access") || lower.contains("do not have access")
                || lower.contains("no access to") || lower.contains("not connected") || lower.contains("isn't installed")
                || lower.contains("is not installed") || lower.contains("don't have a") || lower.contains("do not have a"))
                && (lower.contains("plugin") || lower.contains("integration") || lower.contains("connect"));
            let discovered = sessions
                .get_messages(session_id)
                .unwrap_or_default()
                .iter()
                .rev()
                .take(12)
                .any(|m| m.role == "assistant" && m.content.contains("\"discover\""));
            if denies && !discovered {
                no_access_nudges += 1;
                warn!(iteration, session_id, "access denied from memory; nudging to discover");
                pending_stream_reminders.push(steering::wrap_system_reminder(
                    "You said a service is unavailable without checking. Call \
                     plugin(action: \"discover\", query: \"<service>\") now and answer from \
                     what it returns; if it finds nothing, say that.",
                ));
                continue;
            }
        }

        // No tool calls — handle empty responses before checking auto-continuation.
        // Order: post-tool nudge → empty retries → auto-continue → break.
        if assistant_content.trim().is_empty() {
            // Post-tool empty response nudge: model returned empty after tool results.
            // Append assistant("(empty)") + user(nudge) to keep message sequence valid,
            // then continue. One-shot: only fires once per tool round.
            let prior_was_tool = sessions
                .get_messages(session_id)
                .unwrap_or_default()
                .iter()
                .rev()
                .take(5)
                .any(|m| m.role == "tool");
            if prior_was_tool && post_tool_empty_nudges < 1 {
                post_tool_empty_nudges += 1;
                warn!(
                    iteration,
                    session_id, "empty response after tool calls — nudging model to continue"
                );
                // Ephemeral nudge on the next call — nothing persisted (the
                // tool results already sit in the session; user-after-tool is
                // a valid sequence for every provider we ship).
                pending_stream_reminders.push(steering::wrap_system_reminder(
                    "You just executed tool calls but returned an empty response. \
                     Please process the tool results above and continue with the task.",
                ));
                continue;
            }

            // Empty response retry: retry up to 3 times before giving up.
            if model_call::retry_empty_reply(&mut call_state, iteration, session_id) {
                continue;
            }

            // Exhausted retries — output "(empty)" and break.
            turn_exit_reason = crate::guardrails::Exit::EmptyResponseExhausted;
            warn!(
                iteration,
                session_id,
                "empty response after {} retries — giving up",
                model_call::MAX_EMPTY_CONTENT_RETRIES
            );
            let _ = sessions.append_message(session_id, "assistant", "(empty)", None, None, None);
            let _ = tx.send(StreamEvent::text("(empty)".to_string())).await;
            break;
        }

        // Reset retry counter on successful non-empty content (read on next loop iteration)
        call_state.empty_content_retries = 0;

        // Auto-continuation: tool_use blocks are the sole continuation signal.
        // Text-only responses always exit the loop.
        // Tool-using iterations already `continue` via the tool execution path
        // at ~line 2367, so reaching this point means no tools were called.
        // Max-tokens recovery and budget continuation handle their own cases above.

        // agent.turn action — notify apps at natural break
        if hooks.has_subscribers("agent.turn") {
            let payload = serde_json::to_vec(&crate::hooks::TurnPayload {
                session_id: session_id.to_string(),
                turn: iteration,
                tool_calls: vec![],
                total_tool_calls: called_tools.clone(),
                has_active_task: !active_task.is_empty(),
            })
            .unwrap_or_default();
            hooks.do_action("agent.turn", payload).await;
        }

        // Contradictory stop: the provider says the model stopped TO CALL
        // TOOLS but none were parsed — retry the iteration instead of ending.
        if let Some(reminder) = model_call::lost_tool_calls(
            &mut call_state,
            stop_reason.as_deref(),
            &tool_calls,
            iteration,
            session_id,
        ) {
            pending_stream_reminders.push(reminder);
            continue;
        }

        // DEFERRED BACKSTOP — promise-then-stop forced continuation (do NOT enable yet).
        // We first try to fix promise-then-stop ("Now I'll create the file." then exit with
        // no tool call) via the static prompt binding (prompt.rs COMM_STYLE) + the ExecuteIntent
        // stream reminder (steering.rs). If a weak model STILL stalls in live testing, add a
        // branch HERE mirroring the lost-tool-call retry above: if the assistant text shows
        // forward-intent ("I'll…", "Now I'll…", "Let me…") with no tool call — and it is NOT a
        // question/permission-seek (don't continue past a genuine ask; the ask tool handles those)
        // — re-enter the loop with a pushed `pending_stream_reminders` reminder ("carry out exactly
        // what you just said — call the tool now") + `continue`. Gate it with a cycle guard
        // (`prev_auto_content` near-duplicate) and budget (`max_auto_continuations`, ~line 4051) to
        // avoid the old 5x-loop on "would you like me to…?". NOTE: `auto_continuations` /
        // `prev_auto_content` (~line 1092) are currently immutable — flip them back to `mut` when
        // enabling this.

        // Done gate: edits landed since a check last ran. Fires DONE_GATE_MAX
        // times per run; never in plan mode (nothing was built), never after a
        // cancel, never for a run whose edits were all followed by a check.
        if !plan_mode
            && !cancel_token.is_cancelled()
            && done_gate_due(edits_since_check, done_gate_fired)
        {
            done_gate_fired += 1;
            info!(iteration, session_id, edits = edits_since_check, "done gate fired");
            pending_stream_reminders.push(steering::wrap_system_reminder(&format!(
                "You edited {edits_since_check} file(s) since a check last ran. Run the \
                 project's checks (name them if you know them) and fix what they report \
                 before reporting done. If there are no checks that apply, say so in one \
                 sentence and finish."
            )));
            continue;
        }

        // Repeat gate: the owner already has this exact reply.
        if !repeat_gate_fired && !cancel_token.is_cancelled() && repeats_earlier_reply(&assistant_content, &all_messages) {
            repeat_gate_fired = true;
            info!(iteration, session_id, "repeat gate fired");
            pending_stream_reminders.push(steering::wrap_system_reminder(
                "You already sent the owner this exact reply earlier in this conversation. \
                 Do not send it again. Take the next concrete step with a tool, or say in \
                 one sentence what is stopping you.",
            ));
            continue;
        }

        // Desktop gate: after acting on a window, the reply is checked against
        // the screen the last act left, once. The Simulator session reported
        // "code accepted, you're all set" right after a result that read
        // `Pressed B5 "Home"` with the iPhone home screen below it.
        if !desktop_gate_fired && !cancel_token.is_cancelled() {
            if let Some(evidence) = last_desktop_act.take() {
                desktop_gate_fired = true;
                info!(iteration, session_id, "desktop gate fired");
                pending_stream_reminders.push(steering::wrap_system_reminder(&format!(
                    "Before this reply goes to the owner, check it against what your last \
                     action actually did. Its result was:\n\n{evidence}\n\nIf your reply \
                     says anything this does not show (a step done, a screen reached, a code \
                     accepted, an app opened), rewrite it to say what the screen shows and \
                     what you will do next. If it already matches, repeat it unchanged."
                )));
                continue;
            }
        }

        // A message queued into this turn while this step's call ran was not
        // in the call. Hear it before the turn ends: otherwise the owner's
        // sits in the thread unanswered and a parent's never reaches the
        // report it is waiting on.
        if !cancel_token.is_cancelled()
            && sessions.get_messages(session_id).is_ok_and(|fresh| mid_turn_message_landed(&fresh, &all_messages))
        {
            info!(iteration, session_id, "a message landed during the last step: continuing to hear it");
            continue;
        }

        // Conversation turn complete — normal exit with text response
        // The label is persisted on run_usage and read by `test runs`: a
        // plain word, never a Debug-printed Option.
        turn_exit_reason = crate::guardrails::Exit::TextResponse(stop_reason.clone().unwrap_or_else(|| "none".to_string()));
        info!(iteration, session_id, exit_reason = %turn_exit_reason, "agentic loop complete");
        break;
    }

    // Post-loop: budget exhaustion summary request.
    // If the loop exited because we hit max_iterations without a final text response,
    // make ONE more API call with tools stripped to get a summary.
    if final_iteration >= max_iterations && !turn_exit_reason.is_text_response() {
        // Only request summary if the last message is a tool result (mid-task exit)
        let last_msg_is_tool = sessions
            .get_messages(session_id)
            .unwrap_or_default()
            .last()
            .map(|m| m.role == "tool")
            .unwrap_or(false);
        if last_msg_is_tool {
            turn_exit_reason = crate::guardrails::Exit::MaxIterations { done: final_iteration, max: max_iterations };
            info!(session_id, exit_reason = %turn_exit_reason, "budget exhausted — requesting summary");

            // One toolless call asked for the summary on the one steering channel.
            pending_stream_reminders.push(steering::wrap_system_reminder(BUDGET_SUMMARY_REQUEST));

            // Pick first available provider for the summary call
            let prov_lock = providers.read().await;
            if let Some(summary_provider) = prov_lock.first() {
                let mut summary_messages =
                    convert_messages(&sessions.get_messages(session_id).unwrap_or_default());
                attach_stream_reminders(&mut summary_messages, &pending_stream_reminders);

                let summary_req = ChatRequest {
                    tool_credential: None,
                    tool_choice: Default::default(),
                    messages: summary_messages,
                    tools: vec![], // No tools — text-only response
                    max_tokens: 4096,
                    temperature: 0.7,
                    system: static_system.clone(),
                    static_system: static_system.clone(),
                    model: last_model_name.clone(),
                    enable_thinking: false,
                    metadata: call_state.sticky_metadata.clone(),
                    cache_breakpoints: vec![],
                    cancel_token: Some(cancel_token.clone()),
                    trace: side_trace("budget_summary"),
                };

                if let Ok(mut rx) = summary_provider.stream(&summary_req).await {
                    let mut summary_text = String::new();
                    while let Some(event) = rx.recv().await {
                        match event.event_type {
                            ai::StreamEventType::Text => {
                                let _ = tx.send(StreamEvent::text(event.text.clone())).await;
                                summary_text.push_str(&event.text);
                            }
                            ai::StreamEventType::Done | ai::StreamEventType::Error => break,
                            _ => {}
                        }
                    }
                    if !summary_text.is_empty() {
                        let _ = sessions.append_message(
                            session_id,
                            "assistant",
                            &summary_text,
                            None,
                            None,
                            None,
                        );
                    }
                }
            }
        }
    }

    // Turn exit diagnostic
    info!(
        session_id,
        exit_reason = %turn_exit_reason,
        iterations = final_iteration,
        max_iterations,
        "turn ended"
    );

    // Carry hot spiral keys into the next turn at half strength so a strategy
    // loop resumed across user messages still meets the backstop.
    cross_turn_save(session_id, &action_call_counts, guard_cfg.same_action_limit);

    // Memory extraction over the messages since the last one, no pre-gate.
    // The runner has no agreed goal (the objective is not one).
    after_turn::MemoryExtraction {
        sessions,
        session_id,
        providers,
        store,
        concurrency,
        embedding_provider,
        tools,
        memory_user_id: &memory_user_id,
        memory_topics: &memory_topics,
        memory_write_bar: &memory_write_bar,
        run_taint,
        goal: None,
        skip_memory,
        trace: side_trace("memory_extract"),
    }
    .schedule()
    .await;

    // Background personality synthesis (at most once per run).
    if !skip_memory {
        after_turn::spawn_personality_synthesis(store, providers, &memory_user_id, concurrency).await;
    }

    // The run becomes a record: what it cost, and (later, per role) what it
    // achieved. This is the ONE write point — chat, workflow and heartbeat
    // runs all pass through this loop, so persisting here covers every run
    // type without a second writer per caller. Best-effort like the timeline:
    // a failed insert is logged loudly, never allowed to fail a finished run.
    // (Runs that end in an error return earlier and are not yet recorded —
    // their cost is real, and wiring the error exits is deliberate follow-up
    // rather than a silent partial number today.)
    // By the session KEY, not its UUID: the key names the run
    // (`agent:<id>:workflow:<run>:…`); the UUID classified every workflow
    // turn as a chat with no run id, so no run ever had a cost to sum.
    usage::record_run_usage(
        store,
        selector,
        agent_id,
        &session_key,
        &last_model_name,
        &state,
        &turn_exit_reason.label(),
    );

    // Context accounting for the owner: one event per turn, rendered as a
    // quiet line under the reply (Stage 8), never as reply text.
    usage::send_context_stats(
        tx,
        read_ledger.stats(),
        ctx_compaction_passes,
        ctx_evictions,
        ctx_spilled_results,
        &state,
    )
    .await;
    Ok(turn_exit_reason.label())
}

/// Truncate a string to at most `max_bytes` bytes without splitting a multi-byte
/// UTF-8 character. Returns a `&str` that is always valid UTF-8.
pub(crate) fn truncate_str(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    // Walk backwards from max_bytes to find a char boundary
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Compute max auto-continuations based on incomplete work tasks.
/// Scales with remaining work so batch tasks get more runway.
#[allow(dead_code)] // reserved for auto-continuation logic
fn max_auto_continuations(work_tasks: &[steering::WorkTask]) -> usize {
    let incomplete = work_tasks
        .iter()
        .filter(|t| t.status != "completed")
        .count();
    if incomplete > 0 {
        (incomplete * 2).clamp(10, MAX_AUTO_CONTINUATIONS_CEILING)
    } else {
        MAX_AUTO_CONTINUATIONS_DEFAULT
    }
}

/// Convert database ChatMessages to ai::Messages for the provider.
/// Detect a prompt that IS an explicit invocation of a declared tool —
/// "use os(resource: ...)", "call use_skill(...)", or the bare "read_file(...)" — and
/// return the ToolChoice that forces that tool. Conservative on purpose: the
/// whole trimmed prompt must be the invocation (optional leading verb, known
/// tool name, parenthesized args to the end), so prose that merely mentions a
/// call is never hijacked.
fn named_tool_invocation(
    prompt: &str,
    tools: &[ai::ToolDefinition],
) -> Option<ai::ToolChoice> {
    let t = prompt.trim();
    if !t.ends_with(')') {
        return None;
    }
    let lower = t.to_lowercase();
    let rest = ["use ", "call ", "run ", "invoke "]
        .iter()
        .find_map(|v| lower.starts_with(*v).then(|| t[v.len()..].trim_start()))
        .unwrap_or(t);
    let name: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    if name.is_empty() || !rest[name.len()..].trim_start().starts_with('(') {
        return None;
    }
    tools
        .iter()
        .any(|d| d.name == name)
        .then(|| ai::ToolChoice::Tool(name))
}

/// What the post-loop summary call asks for when the iteration budget ran
/// out mid-task. Steering: it rides that one call. Older builds stored it as
/// a user row, which history load drops by this exact text.
pub(crate) const BUDGET_SUMMARY_REQUEST: &str = "You've reached the maximum number of tool-calling \
iterations allowed. Please provide a final response summarizing what you've found and accomplished \
so far, without calling any more tools.";

/// The one place steering enters a model call. `reminders` is the run's
/// `pending_stream_reminders` — every reminder, nudge and briefing is queued
/// there and nowhere else. Each rides as a user-role `<system-reminder>`
/// message, once (a hook re-evaluated on a retried call queues the same text
/// again), INSERTED BEFORE a fresh user ask rather than after it: when the
/// transcript's tail is the user's just-sent message, anything placed after
/// it becomes the last thing the model reads — and weak models answer the
/// tail (a 39-char ask followed by 1.3k of recalled memory got the ASK echoed
/// back as text instead of executed). Mid-run (tail = tool results), appending
/// at the end is correct — a correction should be the freshest signal.
fn attach_stream_reminders(messages: &mut Vec<Message>, reminders: &[String]) {
    let mut seen = HashSet::new();
    let batch: Vec<Message> = reminders
        .iter()
        .filter(|r| seen.insert(r.as_str()))
        .map(|content| Message {
            role: "user".to_string(),
            content: content.clone(),
            ..Default::default()
        })
        .collect();
    let at = if messages.last().is_some_and(|m| m.role == "user") {
        messages.len() - 1
    } else {
        messages.len()
    };
    messages.splice(at..at, batch);
}

/// Objective classifier call ceiling; on timeout the objective is left as is.
/// A decision answers in milliseconds; this only bounds a stalled connection.
const OBJECTIVE_TIMEOUT_SECS: u64 = 5;
/// A `keep` below this confidence while no objective is set is treated as
/// `set`: an agent with no objective for work the person just asked for is
/// worse than an objective they did not mean to start.
const OBJECTIVE_KEEP_FLOOR: f64 = 0.6;
/// Char-boundary-safe cap on the objective sentence and on each recent
/// message the classifier sees.
const OBJECTIVE_MESSAGE_CAP: usize = 200;
/// Instruction for the cheap model that writes the objective whenever the
/// latest message cannot stand as it is (see [`objective_text`]). It reads
/// the current objective, the recent conversation and the latest message.
const OBJECTIVE_INSTRUCTION: &str = "Write the person's working objective as ONE sentence of at \
     most 25 words. Read the latest message against the current objective and the recent \
     conversation: a message that refines the current objective changes it, it does not replace \
     it. The sentence must make sense to someone who has not read the conversation: name what \
     every 'it', 'that', 'another' or bare name refers to, and keep what the conversation has \
     already settled. Use the person's own words where they fit. Output ONLY the sentence.";
/// How much of the latest message the cheap model reads.
const OBJECTIVE_WRITER_INPUT_CAP: usize = 2_000;
/// UNTUNED. On a `set`, a plain message is stored as it is only when
/// `self_contained` is at or above this; below it, or with no answer, the
/// objective is written. Set high because the costs are lopsided: a
/// fragment stored as the objective loses its referent and the employee
/// asks "what's 'it'?" at the next turn, while writing a message that could
/// have stood costs one cheap background call and a paraphrase. Set by
/// hand from one thread (cloud bot, 2026-09-24), where every message after
/// the first was stored raw:
///
///   "can you find everything you can about <company>"   self-contained
///   "should it change its name? if so what would you call it?"   not: "it"
///   "<company>"                                          not: bare name
///   "sorry I meant <domain>"                             not: a correction
///   "no we need another"                                 not: "another"
///
/// Shadow data from the `objective` site's `self_contained` log field sets
/// it properly.
const SELF_CONTAINED_FLOOR: f64 = 0.8;
/// How many recent messages the classifier sees.
const OBJECTIVE_RECENT_MESSAGES: usize = 6;

/// Whether a run classifies the person's objective. A workflow turn is the
/// engine's step on a scratch session deleted at run end, and a review fork
/// is the runner's own self-review prompt on a scratch fork session: neither
/// is the person speaking, and nothing reads an objective set there. A
/// command fork does classify: it is the only run its message gets.
fn objective_detection_applies(
    workflow_mode: Option<&WorkflowMode>,
    review_fork: Option<&crate::review_fork::ReviewForkCtx>,
) -> bool {
    workflow_mode.is_none() && review_fork.is_none()
}

/// What the objective classifier decided to do with the session's objective.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ObjectiveDecision {
    /// A new task: the objective is the latest message when it stands on
    /// its own, written otherwise (see [`objective_text`]); `mode` applies.
    Set { mode: String },
    /// A refinement: the objective is rewritten from the current one and
    /// the latest message, never replaced by it; `mode` applies only when
    /// the classifier named one.
    Update { mode: String },
    /// The task is done: drop the objective and the mode.
    Clear,
    /// No change.
    Keep,
}

/// Map Jev's `action` choice (with its confidence) and `mode` choice to the
/// decision applied to the session. The priority rule lives here as a
/// threshold, not in the prompt: a `keep` under [`OBJECTIVE_KEEP_FLOOR`]
/// while no objective is set is a `set`. An unrecognised action is `Keep`,
/// the no-op.
pub(crate) fn objective_decision(
    action: &str,
    confidence: f64,
    mode: &str,
    objective_is_none: bool,
) -> ObjectiveDecision {
    match action {
        "set" => ObjectiveDecision::Set {
            mode: mode.to_string(),
        },
        "update" => ObjectiveDecision::Update {
            mode: mode.to_string(),
        },
        "clear" => ObjectiveDecision::Clear,
        "keep" if objective_is_none && confidence < OBJECTIVE_KEEP_FLOOR => ObjectiveDecision::Set {
            mode: mode.to_string(),
        },
        _ => ObjectiveDecision::Keep,
    }
}

/// What the objective writer reads besides its instruction: the objective
/// the classifier saw, its recent conversation, and the latest message.
pub(crate) struct ObjectiveContext<'a> {
    pub current_objective: &'a str,
    pub recent_conversation: &'a [String],
    pub message: &'a str,
}

/// Where the stored objective comes from on a set or an update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ObjectiveText {
    /// The latest message as it is.
    Verbatim(String),
    /// One line from the cheap model, given this input.
    Written(String),
}

/// Decide where the objective comes from. An update refines the current
/// objective, so it is always written from it: the message alone ("no we
/// need another") would discard what it refines. A set keeps the message
/// only when it is plain and Jev judged it self-contained at or above
/// [`SELF_CONTAINED_FLOOR`]; no answer is not a yes, so it is written.
pub(crate) fn objective_text(
    refines: bool,
    self_contained: Option<f64>,
    ctx: &ObjectiveContext<'_>,
) -> ObjectiveText {
    let message = ctx.message.trim();
    if !refines
        && objective_is_plain(message)
        && self_contained.is_some_and(|p| p >= SELF_CONTAINED_FLOOR)
    {
        return ObjectiveText::Verbatim(message.to_string());
    }
    let current = if ctx.current_objective.is_empty() {
        "none"
    } else {
        ctx.current_objective
    };
    let recent = if ctx.recent_conversation.is_empty() {
        "none".to_string()
    } else {
        ctx.recent_conversation.join("\n")
    };
    ObjectiveText::Written(format!(
        "Current objective: {current}\n\nRecent conversation:\n{recent}\n\nLatest message:\n{}",
        truncate_str(message, OBJECTIVE_WRITER_INPUT_CAP)
    ))
}

/// A short message with no framing can stand as the objective. A long one,
/// or one that opens with a bracketed frame (a coworker note, a background
/// event, a case event, a hire prompt), cannot: its first line would become
/// the objective, and "[Coworker message from Nebo]" is no objective.
pub(crate) fn objective_is_plain(text: &str) -> bool {
    let text = text.trim();
    !text.is_empty() && text.len() <= OBJECTIVE_MESSAGE_CAP && !text.starts_with('[')
}

/// The sentence stored as the objective. Jev decides and does not write, so
/// a written objective gets one line from the cheap model; `None` when it
/// could not write one.
async fn objective_sentence(
    agent_id: &str,
    providers: &Arc<RwLock<Vec<Arc<dyn Provider>>>>,
    text: ObjectiveText,
) -> Option<String> {
    match text {
        ObjectiveText::Verbatim(message) => Some(message),
        ObjectiveText::Written(input) => {
            crate::summarizer::one_line(
                RequestTrace {
                    agent_id: agent_id.to_string(),
                    ..RequestTrace::new("objective_sentence")
                },
                providers,
                "",
                OBJECTIVE_INSTRUCTION,
                &input,
                60,
            )
            .await
        }
    }
}

/// Apply the classifier's decision to the session. On a set or an update
/// with no sentence written, the objective is left as it is: a fragment is
/// never stored in its place.
async fn apply_objective_decision(
    decision: ObjectiveDecision,
    self_contained: Option<f64>,
    ctx: &ObjectiveContext<'_>,
    agent_id: &str,
    providers: &Arc<RwLock<Vec<Arc<dyn Provider>>>>,
    sessions: &SessionManager,
    session_id: &str,
) {
    match decision {
        ObjectiveDecision::Set { mode } => {
            let text = objective_text(false, self_contained, ctx);
            let Some(objective) = objective_sentence(agent_id, providers, text).await else {
                debug!("objective set: no sentence could be written; leaving objective as is");
                return;
            };
            info!(objective = %objective, mode = %mode, "objective set");
            let _ = sessions.set_active_task(session_id, &objective);
            sessions.set_detected_mode(session_id, &mode);
        }
        ObjectiveDecision::Update { mode } => {
            let text = objective_text(true, self_contained, ctx);
            let Some(objective) = objective_sentence(agent_id, providers, text).await else {
                debug!("objective update: no sentence could be written; leaving objective as is");
                return;
            };
            info!(objective = %objective, mode = %mode, "objective updated");
            let _ = sessions.set_active_task(session_id, &objective);
            if !mode.is_empty() {
                sessions.set_detected_mode(session_id, &mode);
            }
        }
        ObjectiveDecision::Clear => {
            info!("objective cleared");
            let _ = sessions.clear_active_task(session_id);
            sessions.set_detected_mode(session_id, "");
        }
        ObjectiveDecision::Keep => {
            // No change
        }
    }
}

/// Detect the person's working objective from their latest message.
/// Runs as a background task (fire-and-forget) before the main loop: one
/// typed decision (Jev through Janus, [`ai::DecideClient`]) answers whether
/// the message starts, refines, finishes or continues the current objective,
/// whether the work is research or normal, and whether the message makes
/// sense without the conversation. The objective sentence comes from
/// [`objective_text`], only when the decision is set or update. A
/// continuation nudge is never classified: it is not the person speaking.
/// No client, any error or a timeout leaves the objective untouched.
///
/// `turn` carries the turn decision's context groups and the channel its
/// answer goes back on: those questions ride this same request (see
/// [`crate::turn_decide`]). Any early return drops the sender, which the
/// runner reads as "no decision" and falls back to keywords at once.
async fn detect_objective(
    decide: Option<&ai::DecideClient>,
    agent_id: &str,
    providers: &Arc<RwLock<Vec<Arc<dyn Provider>>>>,
    sessions: &SessionManager,
    session_id: &str,
    user_prompt: &str,
    turn: Option<tokio::sync::oneshot::Sender<crate::turn_decide::TurnSignals>>,
) {
    if user_prompt.trim().is_empty() || crate::goals::is_continuation_prompt(user_prompt) {
        return;
    }
    let Some(client) = decide else {
        debug!("objective detection: no decide client (Janus absent); leaving objective as is");
        return;
    };

    let current_objective = sessions.get_active_task(session_id).unwrap_or_default();
    let objective_is_none = current_objective.is_empty();

    // Recent conversation (last 6 spoken messages, each capped) for
    // context. Tool rows and tool-call-only assistant rows are skipped
    // before counting: after a research turn they would fill the window and
    // leave the latest message with nothing to refer back to.
    let recent_conversation: Vec<String> = sessions
        .get_messages(session_id)
        .ok()
        .map(|msgs| {
            msgs.iter()
                .rev()
                .filter(|m| {
                    (m.role == "user" || m.role == "assistant") && !m.content.trim().is_empty()
                })
                .take(OBJECTIVE_RECENT_MESSAGES)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .map(|m| {
                    let content = if m.content.len() > OBJECTIVE_MESSAGE_CAP {
                        format!("{}...", truncate_str(&m.content, OBJECTIVE_MESSAGE_CAP))
                    } else {
                        m.content.clone()
                    };
                    format!("[{}]: {}", m.role, content)
                })
                .collect()
        })
        .unwrap_or_default();

    let state = serde_json::json!({
        "current_objective": if objective_is_none { "none" } else { current_objective.as_str() },
        "recent_conversation": recent_conversation,
        "latest_user_message": ai::decide::clip(user_prompt, crate::turn_decide::LATEST_USER_MESSAGE_CAP),
    });
    let mut questions = BTreeMap::from([
        (
            "action",
            Question::choice(
                "Read `latest_user_message` against `current_objective`, with `recent_conversation` for context, and pick what happens to the objective.",
                &[
                    (
                        "set",
                        "`latest_user_message` starts a new task, or talks about a subject, system or goal unrelated to `current_objective`; or `current_objective` is `none` and the message asks for anything to be done.",
                    ),
                    (
                        "update",
                        "`latest_user_message` refines the task in `current_objective`: it adds scope, adds a requirement, or corrects what was asked, in the same area of work.",
                    ),
                    (
                        "clear",
                        "`latest_user_message` says the task is done and asks for nothing new: thanks, looks good, perfect, that's it, never mind, done.",
                    ),
                    (
                        "keep",
                        "`latest_user_message` stays on the task in `current_objective` without changing it: a greeting, a question about the current work, or feedback on it; or `current_objective` is `none` and the message is a greeting or a question with no task in it.",
                    ),
                ],
            ),
        ),
        (
            "mode",
            Question::choice(
                "Pick how the work asked for in `latest_user_message` should be carried out.",
                &[
                    (
                        "research",
                        "The work is a multi-source investigation: comparing options, finding deals, evaluating alternatives, or gathering information from several websites.",
                    ),
                    (
                        "normal",
                        "Everything else: a direct action, a conversation, a single lookup, a creative task.",
                    ),
                ],
            ),
        ),
        (
            "self_contained",
            Question::noul(
                "`latest_user_message` states a complete task that makes sense to someone who has not read `recent_conversation`: it has no 'it', 'that', 'another' or 'the one' pointing back, and it is not a bare name, a correction or a fragment.",
            ),
        ),
    ]);
    let turn_questions = if turn.is_some() {
        crate::turn_decide::questions()
    } else {
        Vec::new()
    };
    questions.extend(turn_questions.iter().map(|(k, q)| (k.as_str(), q.clone())));

    let trace = RequestTrace {
        agent_id: agent_id.to_string(),
        ..RequestTrace::new("objective")
    };
    let t_call = std::time::Instant::now();
    let call = client.decide(&trace, &state, &questions);
    let decision =
        match tokio::time::timeout(Duration::from_secs(OBJECTIVE_TIMEOUT_SECS), call).await {
            Ok(Ok(decision)) => decision,
            Ok(Err(e)) => {
                debug!(error = %e, "objective detection failed; leaving objective as is");
                return;
            }
            Err(_) => {
                debug!("objective detection timed out; leaving objective as is");
                return;
            }
        };
    let action = decision.answer("action");
    let picked = action.map(Answer::picked).unwrap_or("");
    let confidence = action.and_then(|a| a.confidence).unwrap_or(1.0);
    let mode = decision.answer("mode").map(Answer::picked).unwrap_or("");
    let self_contained = decision.answer("self_contained").and_then(|a| a.noul);
    debug!(
        site = "objective",
        model = %decision.model,
        action = picked,
        confidence,
        mode,
        self_contained = ?self_contained,
        questions = questions.len(),
        call_ms = t_call.elapsed().as_millis() as u64,
        input_tokens = decision.usage.input_tokens,
        output_tokens = decision.usage.output_tokens,
        cost_micro = decision.usage.cost_micro,
        "objective classifier decided"
    );
    if let Some(tx) = turn {
        let signals = crate::turn_decide::signals_from(&decision);
        debug!(multi_stage = signals.multi_stage, "turn decision");
        // The runner stops listening once its wait trips; a late answer
        // has nowhere to go and the keyword path already ran.
        let _ = tx.send(signals);
    }

    let ctx = ObjectiveContext {
        current_objective: &current_objective,
        recent_conversation: &recent_conversation,
        message: user_prompt,
    };
    apply_objective_decision(
        objective_decision(picked, confidence, mode, objective_is_none),
        self_contained,
        &ctx,
        agent_id,
        providers,
        sessions,
        session_id,
    )
    .await;
}

/// Build the static system prompt.
fn build_system_prompt(custom_system: &str, memory_context: &str) -> String {
    let mut prompt = if custom_system.is_empty() {
        "You are Nebo, a personal AI assistant. You are helpful, accurate, and proactive. \
         You have access to tools for file operations, shell commands, web browsing, and memory.\n\
         \n\
         Guidelines:\n\
         - Use tools to accomplish tasks rather than just describing how to do them\n\
         - Be concise but thorough in your responses\n\
         - When asked to do something, do it — don't just explain how\n\
         - Store important information about the user using memory tools\n\
         - If a task requires multiple steps, work through them systematically\n"
            .to_string()
    } else {
        custom_system.to_string()
    };

    if !memory_context.is_empty() {
        prompt.push_str("\n\n# Memory context\n");
        prompt.push_str(memory_context);
    }

    prompt
}

/// Extract the plain skill name from a qualified ref.
/// "@nebo/skills/gws-gmail@^1.0.0" → "gws-gmail"
/// "SKIL-ABCD-1234" → "SKIL-ABCD-1234" (passed through)
/// "gws-gmail" → "gws-gmail" (plain names pass through)
/// Simple FNV-1a hash for stale-result detection. Not cryptographic.
pub(crate) fn simple_hash(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &byte in data {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[cfg(test)]
mod plan_reminder_tests {
    use super::*;

    #[test]
    fn plan_reminder_fires_after_ten_iterations_without_a_check() {
        assert!(!plan_reminder_due(9, 0), "absent at 9");
        assert!(!plan_reminder_due(11, 2), "absent at 9 since the touch");
        assert!(plan_reminder_due(10, 0), "present at 10");
        assert!(plan_reminder_due(21, 11), "and again 10 after the reminder re-touched it");
    }
}

#[cfg(test)]
mod done_gate_tests {
    use super::*;

    #[test]
    fn a_repeated_reply_is_caught_and_a_short_or_new_one_is_not() {
        let msg = |role: &str, content: &str| ChatMessage {
            id: String::new(),
            chat_id: String::new(),
            role: role.into(),
            content: content.into(),
            metadata: None,
            created_at: 0,
            day_marker: None,
            tool_calls: None,
            tool_results: None,
            token_estimate: None,
            html: None,
        };
        let apology = "I'm sorry. I'll stop taking screenshots and interact directly with the Simulator: let me click Continue.";
        let history = vec![msg("user", "you suck"), msg("assistant", apology)];
        assert!(repeats_earlier_reply(&format!("  {apology}\n"), &history), "same words, other whitespace");
        assert!(!repeats_earlier_reply("I'm sorry.", &[msg("assistant", "I'm sorry.")]), "short replies repeat naturally");
        assert!(!repeats_earlier_reply(apology, &[msg("user", apology)]), "only the assistant's own replies count");
        assert!(!repeats_earlier_reply(&format!("{apology} Then type the code."), &history));
    }

    #[test]
    fn desktop_evidence_keeps_what_was_done_and_what_is_on_screen() {
        let result = "Pressed B5 \"Home\" via accessibility. 24 elements now (29 before).\n\nSimulator — window at 1,2 size 3×4 pt; via ax; snapshot s\nB5  AXButton  \"Fitness\"\nB6  AXButton  \"Watch\"\nCoordinates are pixels of the image below.\nnever";
        let e = desktop_evidence(result);
        assert!(e.starts_with("Pressed B5 \"Home\""), "{e}");
        assert!(e.contains("Simulator — window") && e.contains("\"Watch\""), "{e}");
        assert!(!e.contains("Coordinates") && !e.contains("never"), "{e}");
    }

    #[test]
    fn done_gate_fires_once_and_only_after_an_unchecked_edit() {
        assert!(!done_gate_due(0, 0), "a run that made no edits is never gated");
        assert!(done_gate_due(1, 0), "one unchecked edit is enough");
        assert!(done_gate_due(7, 0));
        assert!(!done_gate_due(7, DONE_GATE_MAX), "at most DONE_GATE_MAX per run");
        assert_eq!(DONE_GATE_MAX, 1, "the gate is a single nudge, not a loop");
    }

    #[test]
    fn check_verb_regex_matches_the_named_runners_only() {
        for cmd in [
            "cargo test -p nebo-agent",
            "CARGO_TARGET_DIR=x cargo check -q 2>&1 | tail -n 40",
            "cargo clippy --all-targets",
            "pytest tests/",
            "go test ./...",
            "go vet ./...",
            "pnpm check",
            "pnpm test",
            "cd app && pnpm build",
            "npm test",
            "npm run lint",
            "npx tsc --noEmit",
            "node_modules/.bin/tsc -p .",
            "vitest run",
            "jest --ci",
            "ruff check .",
            "make test",
            "make check",
        ] {
            assert!(is_check_command(cmd), "{cmd}");
        }
        for cmd in [
            "cargo build --release",
            "cargo run",
            "git status",
            "rustc --version",
            "npm install",
            "pnpm dev",
            "pnpm install",
            "make build",
            "go build ./...",
            "ls -la",
            "python -m http.server",
        ] {
            assert!(!is_check_command(cmd), "{cmd}");
        }
    }

}

#[cfg(test)]
mod named_invocation_tests {
    use super::named_tool_invocation;
    use ai::{ToolChoice, ToolDefinition};

    fn defs(names: &[&str]) -> Vec<ToolDefinition> {
        names
            .iter()
            .map(|n| ToolDefinition {
                name: n.to_string(),
                description: String::new(),
                input_schema: serde_json::json!({}),
            })
            .collect()
    }

    #[test]
    fn explicit_invocations_force_the_tool() {
        let tools = defs(&["os", "use_skill", "mcp__nebo_kb__memory_recall"]);
        for p in [
            r#"use os(resource: "app", action: "list")"#,
            r#"os(resource: "shell", action: "exec", command: "ls")"#,
            r#"call use_skill(name: "invoicing")"#,
            r#"Use os(resource: "mail", action: "unread")"#,
        ] {
            match named_tool_invocation(p, &tools) {
                Some(ToolChoice::Tool(name)) => assert!(!name.is_empty(), "{p}"),
                other => panic!("{p} → {other:?}"),
            }
        }
    }

    #[test]
    fn prose_and_unknown_tools_stay_auto() {
        let tools = defs(&["os", "use_skill"]);
        for p in [
            r#"how do I use os(resource: "app") safely?"#, // prose prefix
            r#"use frobnicate(action: "x")"#,              // undeclared tool
            r#"use os(resource: "app") and then summarize the results for me"#, // trailing prose
            "what apps are open?",
        ] {
            assert!(named_tool_invocation(p, &tools).is_none(), "{p}");
        }
    }
}


#[cfg(test)]
mod objective_decision_tests {
    use std::sync::{Arc, Mutex};

    use ai::{ChatRequest, EventReceiver, Provider, ProviderError};
    use tokio::sync::RwLock;

    use super::{
        OBJECTIVE_INSTRUCTION, OBJECTIVE_KEEP_FLOOR, ObjectiveContext, ObjectiveDecision,
        ObjectiveText, SELF_CONTAINED_FLOOR, WorkflowMode, apply_objective_decision,
        objective_decision, objective_detection_applies, objective_is_plain, objective_text,
    };
    use crate::session::SessionManager;

    /// Workflow turns and review forks are scratch runs with no person
    /// speaking; only a chat run (a command fork included) classifies.
    #[test]
    fn workflow_turns_and_review_forks_skip_the_objective_call() {
        assert!(objective_detection_applies(None, None));
        let workflow = WorkflowMode {
            trace: ai::RequestTrace::new("workflow_activity"),
            objective: String::new(),
            instruction: String::new(),
            advertised_tools: Default::default(),
            tainted: false,
            spend_cap_microcents: 0,
            park: None,
        };
        assert!(!objective_detection_applies(Some(&workflow), None));
        let review = crate::review_fork::ReviewForkCtx::new("agent-1".into(), false);
        assert!(!objective_detection_applies(None, Some(&review)));
    }

    /// A pasted document is capped at both ends: the ask at the close of a
    /// long message still reaches the classifier.
    #[test]
    fn the_latest_message_is_capped_at_both_ends() {
        let pasted = format!(
            "Here is the contract. {} Please summarize the termination clause.",
            "Clause text. ".repeat(5_000)
        );
        let sent = ai::decide::clip(&pasted, crate::turn_decide::LATEST_USER_MESSAGE_CAP);
        assert!(
            sent.len() < crate::turn_decide::LATEST_USER_MESSAGE_CAP + 64,
            "{}",
            sent.len()
        );
        assert!(sent.starts_with("Here is the contract."));
        assert!(sent.ends_with("Please summarize the termination clause."));
    }

    #[test]
    fn a_plain_short_ask_is_its_own_objective_and_a_framed_one_is_not() {
        assert!(objective_is_plain("Ask the chief-of-staff agent to draft my weekly report."));
        assert!(!objective_is_plain("[Coworker message from Nebo]\n\nDraft a weekly report."));
        assert!(!objective_is_plain("[Background event — not an owner message]\nA task finished"));
        assert!(!objective_is_plain(&"You have just been hired, and ".repeat(20)));
        assert!(!objective_is_plain("   "));
    }

    fn set(mode: &str) -> ObjectiveDecision {
        ObjectiveDecision::Set {
            mode: mode.to_string(),
        }
    }

    #[test]
    fn choices_map_straight_through() {
        assert_eq!(objective_decision("set", 1.0, "research", false), set("research"));
        assert_eq!(
            objective_decision("update", 1.0, "normal", false),
            ObjectiveDecision::Update {
                mode: "normal".to_string()
            }
        );
        assert_eq!(objective_decision("clear", 1.0, "normal", false), ObjectiveDecision::Clear);
        assert_eq!(objective_decision("keep", 1.0, "normal", false), ObjectiveDecision::Keep);
        // A missing mode answer rides through as the empty mode, as before.
        assert_eq!(objective_decision("set", 1.0, "", true), set(""));
    }

    #[test]
    fn a_doubtful_keep_with_no_objective_is_a_set() {
        // Below the floor and nothing to keep: prefer set.
        assert_eq!(objective_decision("keep", 0.3, "normal", true), set("normal"));
        assert_eq!(
            objective_decision("keep", OBJECTIVE_KEEP_FLOOR - 0.01, "research", true),
            set("research")
        );
        // At the floor the keep stands.
        assert_eq!(
            objective_decision("keep", OBJECTIVE_KEEP_FLOOR, "normal", true),
            ObjectiveDecision::Keep
        );
        // With an objective in place a doubtful keep never resets it.
        assert_eq!(objective_decision("keep", 0.3, "normal", false), ObjectiveDecision::Keep);
    }

    #[test]
    fn an_unrecognised_action_is_a_no_op() {
        assert_eq!(objective_decision("", 0.0, "", true), ObjectiveDecision::Keep);
        assert_eq!(objective_decision("other", 1.0, "normal", false), ObjectiveDecision::Keep);
    }

    // The thread these tests replay, with the names made generic: the first
    // message stood on its own, and every follow-up was stored raw.
    const FIRST_ASK: &str = "can you find everything you can about Example Co";
    const CURRENT: &str = "Find a new, globally pronounceable name for Example Co.";

    fn recent() -> Vec<String> {
        vec![
            "[user]: sorry I meant example.ai".to_string(),
            "[assistant]: example.ai is taken; it has been registered since 2019.".to_string(),
            "[user]: no we need another".to_string(),
        ]
    }

    fn ctx<'a>(current: &'a str, recent: &'a [String], message: &'a str) -> ObjectiveContext<'a> {
        ObjectiveContext {
            current_objective: current,
            recent_conversation: recent,
            message,
        }
    }

    fn written(text: ObjectiveText) -> String {
        match text {
            ObjectiveText::Written(input) => input,
            ObjectiveText::Verbatim(raw) => panic!("stored raw: {raw}"),
        }
    }

    /// An update refines the current objective: the writer always gets the
    /// current objective, the recent turns and the fragment, and the
    /// fragment is never the objective, however self-contained Jev says it is.
    #[test]
    fn an_update_is_always_written_from_the_current_objective() {
        let recent = recent();
        for p in [None, Some(0.0), Some(1.0)] {
            let input = written(objective_text(true, p, &ctx(CURRENT, &recent, "no we need another")));
            assert!(input.contains(&format!("Current objective: {CURRENT}")), "{input}");
            assert!(input.contains("example.ai is taken"), "{input}");
            assert!(input.ends_with("Latest message:\nno we need another"), "{input}");
        }
    }

    /// A set keeps the message as it is only when it is plain and judged
    /// self-contained at the floor or above.
    #[test]
    fn a_self_contained_set_stands_as_it_is() {
        let none: Vec<String> = Vec::new();
        assert_eq!(
            objective_text(false, Some(SELF_CONTAINED_FLOOR), &ctx("", &none, &format!("  {FIRST_ASK} "))),
            ObjectiveText::Verbatim(FIRST_ASK.to_string())
        );
    }

    /// A set that leans on the conversation is written, with the recent
    /// turns; a missing answer is not a yes; a framed message is written
    /// whatever Jev says.
    #[test]
    fn a_set_that_leans_on_the_conversation_is_written() {
        let recent = recent();
        let below = SELF_CONTAINED_FLOOR - 0.01;
        let input = written(objective_text(false, Some(below), &ctx("", &recent, "no we need another")));
        assert!(input.starts_with("Current objective: none"), "{input}");
        assert!(input.contains("example.ai is taken"), "{input}");
        written(objective_text(false, None, &ctx("", &recent, "Example Co")));
        written(objective_text(
            false,
            Some(1.0),
            &ctx("", &recent, "[Coworker message from Nebo]\n\nDraft a weekly report."),
        ));
    }

    /// Answers every stream with `reply` (or fails when `None`) and keeps
    /// the system and user text of each request it saw.
    struct Writer {
        reply: Option<&'static str>,
        seen: Mutex<Vec<(String, String)>>,
    }

    #[async_trait::async_trait]
    impl Provider for Writer {
        fn id(&self) -> &str {
            "writer"
        }
        async fn stream(&self, req: &ChatRequest) -> Result<EventReceiver, ProviderError> {
            let user = req.messages.first().map(|m| m.content.clone()).unwrap_or_default();
            self.seen.lock().unwrap().push((req.system.clone(), user));
            let Some(reply) = self.reply else {
                return Err(ProviderError::Request("writer down".into()));
            };
            let (tx, rx) = tokio::sync::mpsc::channel(4);
            let _ = tx.send(ai::StreamEvent::text(reply)).await;
            let _ = tx.send(ai::StreamEvent::done()).await;
            Ok(rx)
        }
    }

    fn session_with(objective: &str) -> (SessionManager, String) {
        let path = std::env::temp_dir().join(format!("nebo-objective-test-{}.db", uuid::Uuid::new_v4()));
        let store = Arc::new(db::Store::new(path.to_str().unwrap()).expect("test store"));
        let sessions = SessionManager::new(store);
        let id = sessions.get_or_create("agent:a1:web", "").expect("session").id;
        if !objective.is_empty() {
            sessions.set_active_task(&id, objective).expect("objective");
        }
        (sessions, id)
    }

    fn providers(writer: &Arc<Writer>) -> Arc<RwLock<Vec<Arc<dyn Provider>>>> {
        Arc::new(RwLock::new(vec![writer.clone() as Arc<dyn Provider>]))
    }

    fn update() -> ObjectiveDecision {
        ObjectiveDecision::Update {
            mode: "research".to_string(),
        }
    }

    /// The logged failure: "no we need another" arrived as an update. The
    /// writer is asked with the current objective and its instruction, and
    /// its sentence is stored; the fragment never is.
    #[tokio::test]
    async fn an_update_stores_the_written_sentence_never_the_fragment() {
        let sentence = "Find another globally pronounceable name for Example Co; example.ai is taken.";
        let writer = Arc::new(Writer {
            reply: Some(sentence),
            seen: Mutex::new(Vec::new()),
        });
        let (sessions, id) = session_with(CURRENT);
        let recent = recent();
        let c = ctx(CURRENT, &recent, "no we need another");
        apply_objective_decision(update(), Some(1.0), &c, "a1", &providers(&writer), &sessions, &id).await;

        assert_eq!(sessions.get_active_task(&id).unwrap(), sentence);
        let seen = writer.seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "one writer call");
        assert_eq!(seen[0].0, OBJECTIVE_INSTRUCTION);
        assert!(seen[0].1.contains(&format!("Current objective: {CURRENT}")), "{}", seen[0].1);
    }

    /// No sentence written on an update: the current objective stays.
    #[tokio::test]
    async fn a_failed_writer_on_an_update_keeps_the_current_objective() {
        let writer = Arc::new(Writer {
            reply: None,
            seen: Mutex::new(Vec::new()),
        });
        let (sessions, id) = session_with(CURRENT);
        let recent = recent();
        let c = ctx(CURRENT, &recent, "no we need another");
        apply_objective_decision(update(), Some(1.0), &c, "a1", &providers(&writer), &sessions, &id).await;

        assert_eq!(writer.seen.lock().unwrap().len(), 1, "the writer was asked");
        assert_eq!(sessions.get_active_task(&id).unwrap(), CURRENT);
    }

    /// A self-contained set is stored as it is, with no writer call.
    #[tokio::test]
    async fn a_self_contained_set_needs_no_writer() {
        let writer = Arc::new(Writer {
            reply: None,
            seen: Mutex::new(Vec::new()),
        });
        let (sessions, id) = session_with("");
        let none: Vec<String> = Vec::new();
        let c = ctx("", &none, FIRST_ASK);
        let set = ObjectiveDecision::Set {
            mode: "research".to_string(),
        };
        apply_objective_decision(set, Some(0.95), &c, "a1", &providers(&writer), &sessions, &id).await;

        assert_eq!(sessions.get_active_task(&id).unwrap(), FIRST_ASK);
        assert!(writer.seen.lock().unwrap().is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_system_prompt() {
        let prompt = build_system_prompt("", "- favorite color: blue");
        assert!(prompt.contains("Nebo"));
        assert!(prompt.contains("favorite color: blue"));
    }

    #[test]
    fn test_build_system_prompt_custom() {
        let prompt = build_system_prompt("You are a coding assistant.", "");
        assert!(prompt.contains("coding assistant"));
        assert!(!prompt.contains("Memory context"));
    }

}

#[cfg(test)]
mod runaway_backstop_tests {
    use super::*;

    /// The compaction gate: an eviction on every iteration must NOT produce an
    /// LLM summary on every iteration. Reproduces the 2026-08-27 ratio (0.97
    /// summaries per turn) and asserts the throttled behaviour.
    #[test]
    fn summary_is_throttled_not_per_eviction() {
        let sid = "throttle-test-session";
        SUMMARY_EVICTED_SINCE
            .lock()
            .unwrap()
            .remove(sid);
        SUMMARY_INFLIGHT.lock().unwrap().remove(sid);

        // 30 iterations that each evict 2 messages — the shape of any session
        // past MAX_MESSAGE_COUNT. Ungated this fired 30 LLM summaries.
        let mut spawned = 0;
        for _ in 0..30 {
            if summary_due(sid, 2) {
                spawned += 1;
                summary_done(sid); // simulate the task finishing immediately
            }
        }
        // 60 evicted / 20 per summary = 3, never 30.
        assert_eq!(spawned, 3, "one summary per {SUMMARY_MIN_EVICTED} evicted messages");
    }

    /// While a summary is in flight, no second one is spawned for that session —
    /// the fire-and-forget spawn otherwise ran N concurrently, each taking an
    /// LLM permit and racing on update_summary.
    #[test]
    fn summary_never_runs_concurrently_for_one_session() {
        let sid = "inflight-test-session";
        SUMMARY_EVICTED_SINCE.lock().unwrap().remove(sid);
        SUMMARY_INFLIGHT.lock().unwrap().remove(sid);

        assert!(summary_due(sid, SUMMARY_MIN_EVICTED), "first crosses the bar");
        // Still running: further evictions accumulate but must not spawn.
        for _ in 0..10 {
            assert!(
                !summary_due(sid, SUMMARY_MIN_EVICTED),
                "no second summary while one is in flight"
            );
        }
        summary_done(sid);
        assert!(summary_due(sid, 1), "spawns again once the first finished");
    }

    /// Sessions are throttled independently — one busy conversation must not
    /// starve another's compaction.
    #[test]
    fn throttle_is_per_session() {
        for sid in ["sess-a", "sess-b"] {
            SUMMARY_EVICTED_SINCE.lock().unwrap().remove(sid);
            SUMMARY_INFLIGHT.lock().unwrap().remove(sid);
        }
        assert!(summary_due("sess-a", SUMMARY_MIN_EVICTED));
        assert!(summary_due("sess-b", SUMMARY_MIN_EVICTED));
    }
}

#[cfg(test)]
mod cross_turn_lru_tests {
    use super::*;

    #[test]
    fn cross_turn_spiral_evicts_lru_per_session() {
        let mut m = CrossTurnSpiral::default();
        let hot = |k: &str| std::collections::HashMap::from([(k.to_string(), 4usize)]);
        for i in 0..CROSS_TURN_SPIRAL_SESSIONS {
            m.save(&format!("s{i}"), hot("os:glob"));
        }
        // Re-saving the oldest makes it the newest.
        m.save("s0", hot("os:glob"));
        m.save("extra", hot("web:fetch"));
        assert_eq!(m.hot.len(), CROSS_TURN_SPIRAL_SESSIONS, "one in, one out");
        assert!(m.hot.contains_key("s0"), "the re-saved session survives");
        assert!(!m.hot.contains_key("s1"), "the least recently saved is the one evicted");
        assert!(m.hot.contains_key("extra"));
        // A session that cooled off is dropped, not kept as an empty entry.
        m.save("extra", std::collections::HashMap::new());
        assert!(!m.hot.contains_key("extra"));
    }
}

#[cfg(test)]
mod cross_turn_spiral_tests {
    use super::*;

    // A strategy loop resumed across turns must trip the backstop faster each
    // time: hot keys carry over at half strength, success clears them.
    #[test]
    fn hot_keys_carry_over_and_clear() {
        let sid = format!("test-xturn-{}", uuid::Uuid::new_v4());
        let mut counts = std::collections::HashMap::new();
        counts.insert("agent:spawn_parallel".to_string(), 8usize);
        counts.insert("os:read".to_string(), 1usize); // cold — must not carry
        cross_turn_save(&sid, &counts, 8);

        let seeded = cross_turn_seed(&sid);
        assert_eq!(seeded.get("agent:spawn_parallel"), Some(&4));
        assert!(!seeded.contains_key("os:read"));

        // Next turn ends calm — the carry-over clears.
        cross_turn_save(&sid, &std::collections::HashMap::new(), 8);
        assert!(cross_turn_seed(&sid).is_empty());
    }
}
