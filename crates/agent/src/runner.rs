use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use futures::stream::{FuturesUnordered, StreamExt};
use tokio::sync::{RwLock, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use ai::{
    ChatRequest, Message, Provider, ProviderError, RequestTrace, StreamEvent, StreamEventType,
};
use db::Store;
use db::models::ChatMessage;
use tools::{Origin, Registry, ToolContext, ToolResult};

use crate::concurrency::ConcurrencyController;
use crate::db_context;
use crate::dedupe::{self, DedupeCache};
use types::keyparser;
use crate::memory;
use crate::prompt;
use crate::pruning::{self, ContextThresholds};
use crate::selector::{self, ModelSelector};
use crate::session::SessionManager;
use crate::steering;
use crate::tool_filter;
use crate::transcript;

/// Default maximum agentic loop iterations per run.
const DEFAULT_MAX_ITERATIONS: usize = 100;
/// Extended ceiling when agent is making genuine progress (successful tool calls, no loops).
const EXTENDED_MAX_ITERATIONS: usize = 200;
/// Default context token limit for models that don't report one.
const DEFAULT_CONTEXT_TOKEN_LIMIT: usize = 80_000;
/// Max transient error retries before giving up.
const MAX_TRANSIENT_RETRIES: usize = 10;
/// Max retryable (provider/rate_limit/billing) retries before giving up.
const MAX_RETRYABLE_RETRIES: usize = 5;
/// Max reactive-compaction attempts when the provider rejects for context
/// overflow despite the local estimate saying we fit (single-shot reactive
/// compact + give-up, a guard against an auto-compaction death-spiral).
const MAX_OVERFLOW_RETRIES: usize = 2;
/// Consecutive overloaded (529) errors before falling back to a cheaper model.
#[allow(dead_code)] // reserved for overload fallback logic
const MAX_OVERLOADS_BEFORE_FALLBACK: usize = 3;
/// Timeout for individual tool execution.
const TOOL_EXECUTION_TIMEOUT: Duration = Duration::from_secs(300);
/// Max gap between stream events before the stream is declared wedged
/// (connection open, no tokens). A 90s idle watchdog, classified transient so
/// the normal retry/failover path re-issues the request.
// Generous for the same reason as the HTTP client's read_timeout: buffered
// tool-call arguments make healthy streams go silent for minutes. TCP
// keepalive surfaces dead sockets as read errors long before this fires.
const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// Retry backoff: exponential 500ms × 2^(n−1) capped at 32s, plus 0–25% jitter.
/// An explicit provider Retry-After wins outright.
fn retry_backoff(attempt: usize, retry_after_secs: Option<u64>) -> Duration {
    if let Some(secs) = retry_after_secs {
        return Duration::from_secs(secs);
    }
    let exp = attempt.saturating_sub(1).min(6) as u32; // 500ms × 2^6 = 32s cap
    let base_ms = 500u64 << exp;
    // ponytail: clock-nanos jitter instead of a rand dependency
    let jitter_ms = base_ms
        * (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as u64 % 250)
            .unwrap_or(0))
        / 1000;
    Duration::from_millis(base_ms + jitter_ms)
}
/// Default max auto-continuations when agent stops mid-task (no work tasks).
#[allow(dead_code)] // used by max_auto_continuations, reserved for auto-continuation logic
const MAX_AUTO_CONTINUATIONS_DEFAULT: usize = 5;
/// Ceiling for auto-continuations even with many work tasks.
#[allow(dead_code)] // used by max_auto_continuations, reserved for auto-continuation logic
const MAX_AUTO_CONTINUATIONS_CEILING: usize = 50;
/// Max recovery attempts when output is truncated by token limit.
const MAX_OUTPUT_RECOVERY_ATTEMPTS: usize = 3;
/// Default output token cap for LLM requests.
const DEFAULT_MAX_OUTPUT_TOKENS: i32 = 16_384;
/// Escalated output token cap after a max_tokens truncation.
const ESCALATED_MAX_OUTPUT_TOKENS: i32 = 65_536;
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

/// Extract file path from an os(resource: "file", action: "read") tool call.
/// Returns None if the call is not a file read.
/// "Approve Always" on the ApprovalModal → grant the capability category for
/// next time (PERMISSIONS_SME §14). Flips the global `user_profiles.tool_permissions`
/// entry ON, the same store the Settings → Permissions toggles write.
/// The shell command a tool call would execute, if it's an `os` shell exec —
/// used by the per-command allowlist. `None` for any non-shell tool call.
fn shell_command_of(tc: &ai::ToolCall) -> Option<String> {
    if tc.name != "os" {
        return None;
    }
    let resource = tc
        .input
        .get("resource")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let action = tc
        .input
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if resource == "shell" || action == "exec" {
        tc.input
            .get("command")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    } else {
        None
    }
}

fn persist_capability_grant(store: &Store, category: &str) -> Result<(), String> {
    let raw = store
        .get_user_profile()
        .map_err(|e| e.to_string())?
        .and_then(|p| p.tool_permissions)
        .unwrap_or_else(|| "{}".to_string());
    let mut map: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&raw).unwrap_or_default();
    map.insert(category.to_string(), serde_json::Value::Bool(true));
    let json = serde_json::to_string(&map).map_err(|e| e.to_string())?;
    store
        .update_tool_permissions(&json)
        .map_err(|e| e.to_string())
}

/// "Approve Always" on an MCP tool call → pin an explicit Always-allow override
/// in the server's tool-permission map — the SAME store Settings → MCP → Tool
/// permissions edits (tools::policy::McpServerPermissions on the integration row).
fn persist_mcp_tool_allow(store: &Store, integration_id: &str, tool: &str) -> Result<(), String> {
    let mut perms = tools::policy::McpServerPermissions::from_json(
        store
            .get_mcp_tool_permissions(integration_id)
            .map_err(|e| e.to_string())?
            .as_deref(),
    );
    perms
        .tools
        .insert(tool.to_string(), tools::policy::McpToolAccess::Allow);
    store
        .set_mcp_tool_permissions(integration_id, &perms.to_json())
        .map_err(|e| e.to_string())
}

/// One tool-approval round-trip (PERMISSIONS_SME §11): register a oneshot keyed
/// by the tool_call id, emit `approval_request`, and await the ApprovalModal
/// decision ("deny" / "once" / "always"). Run cancellation resolves to "deny".
/// The ONE ask pathway shared by the capability gate and the MCP tri-state gate.
async fn ask_tool_approval(
    channels: &tools::ApprovalChannels,
    tx: &mpsc::Sender<StreamEvent>,
    cancel_token: &CancellationToken,
    tool_call: &ai::ToolCall,
    session_id: &str,
    gate: &str,
) -> String {
    let request_id = tool_call.id.clone();
    let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
    channels.lock().await.insert(request_id.clone(), resp_tx);
    let _ = tx
        .send(StreamEvent::approval_request(tool_call.clone()))
        .await;
    info!(
        session_id,
        request_id = %request_id,
        gate,
        tool = %tool_call.name,
        "tool approval: waiting for user decision"
    );
    tokio::select! {
        _ = cancel_token.cancelled() => {
            channels.lock().await.remove(&request_id);
            "deny".to_string()
        }
        result = resp_rx => result.unwrap_or_else(|_| "deny".to_string()),
    }
}

/// Stable per-turn identity for spiral detection: tool name + action (e.g.
/// "os:glob", "os:read", "web:navigate"). Resource is omitted — the action alone
/// distinguishes glob/read/exec/navigate, and the os tool infers resource from
/// action anyway, so this is stable whether or not `resource` was passed.
fn action_key(call: &ai::ToolCall) -> String {
    let action = call
        .input
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if !action.is_empty() {
        return format!("{}:{}", call.name, action);
    }
    // No `action` field — the plugin tool's verb lives in `command` ("gmail
    // +send …", "drive +upload …"). Without this every plugin call in a turn
    // collapsed into the single key "plugin:", so eight DIFFERENT commands
    // tripped the backstop as if they were one retried call.
    let verb = call
        .input
        .get("command")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ");
    format!("{}:{}", call.name, verb)
}

/// Cap on unproductive repeats of the same (tool, action) before the spiral
/// backstop ends the turn. Default only — the live value comes from
/// Settings → Developer via `guardrails::GuardrailConfig`.
const SAME_ACTION_LIMIT: usize = crate::guardrails::DEFAULT_SAME_ACTION_LIMIT;

/// Whether an unproductive tool attempt should feed the coarse (tool, action)
/// spiral counter (`SAME_ACTION_LIMIT`).
///
/// File-read errors are excluded: per-path `read_failures` already caps retries
/// on the same target. Counting every failed read across different paths toward
/// the action-wide limit false-trips legitimate exploration (8 wrong paths →
/// turn ends with "os:read was called 8 times without progress" even when the
/// model is about to read a file it just discovered). Redundant content still
/// counts — re-fetching bytes the model already has is the wander the spiral
/// is meant to catch for reads.
fn counts_toward_action_spiral(call: &ai::ToolCall, is_error: bool, flagged_redundant: bool) -> bool {
    if flagged_redundant {
        return true;
    }
    if is_error && extract_file_read_path(call).is_none() {
        return true;
    }
    false
}

/// Apply one spiral-counter update for a tool result. Mirrors the runner loop
/// so unit tests can assert the turn-level budget without driving a full run.
fn record_action_spiral(
    counts: &mut std::collections::HashMap<String, usize>,
    call: &ai::ToolCall,
    is_error: bool,
    flagged_redundant: bool,
) {
    if counts_toward_action_spiral(call, is_error, flagged_redundant) {
        *counts.entry(action_key(call)).or_insert(0) += 1;
    }
}

/// Cross-turn spiral memory. The per-turn counters reset every run, so a model
/// that resumed the same doomed strategy after each user message ("Let me read
/// the frames using sub-agents" x7, across turns, until the user gave up) never
/// tripped the backstop. Keys that ended a turn hot are carried into the next
/// turn at half strength: a resumed loop trips the nudge in half the calls, and
/// a third resumption almost immediately. Success on a key clears it.
static CROSS_TURN_SPIRAL: std::sync::Mutex<
    Option<std::collections::HashMap<String, std::collections::HashMap<String, usize>>>,
> = std::sync::Mutex::new(None);

fn cross_turn_seed(session_id: &str) -> std::collections::HashMap<String, usize> {
    let mut guard = CROSS_TURN_SPIRAL.lock().unwrap_or_else(|p| p.into_inner());
    guard
        .get_or_insert_with(Default::default)
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
    let map = guard.get_or_insert_with(Default::default);
    // ponytail: unbounded-growth backstop — a rare full clear beats an LRU.
    if map.len() > 512 {
        map.clear();
    }
    let hot: std::collections::HashMap<String, usize> = counts
        .iter()
        .filter(|(_, c)| **c * 2 >= limit)
        .map(|(k, c)| (k.clone(), c / 2))
        .collect();
    if hot.is_empty() {
        map.remove(session_id);
    } else {
        map.insert(session_id.to_string(), hot);
    }
}

fn extract_file_read_path(call: &ai::ToolCall) -> Option<String> {
    if call.name != "os" {
        return None;
    }
    let action = call.input.get("action").and_then(|v| v.as_str())?;
    // Resource is frequently omitted — the os tool infers it from the action
    // (read→file, exec→shell). Mirror that inference here so dedup tracking
    // works for the no-resource call shape the model actually produces.
    let resource = call
        .input
        .get("resource")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| match action {
            "read" | "write" | "edit" | "glob" | "grep" => "file".into(),
            "exec" | "shell" | "poll" | "log" => "shell".into(),
            _ => String::new(),
        });

    // Direct file read.
    if resource == "file" && action == "read" {
        return call
            .input
            .get("path")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
    }

    // Shell read: cat/head/tail/jq/python json.tool etc. re-reading a file the
    // model already has. These bypass file-read dedup entirely otherwise.
    if resource == "shell" {
        let command = call.input.get("command").and_then(|v| v.as_str())?;
        return extract_shell_read_path(command);
    }

    None
}

/// Detect a shell command whose sole purpose is dumping a file's contents and
/// return the target path, so the duplicate-read note can fire for shell reads.
/// Only matches read-only file-dump commands — not commands with side effects.
fn extract_shell_read_path(command: &str) -> Option<String> {
    let trimmed = command.trim();
    // Bail on anything that pipes, redirects, or chains — too ambiguous to
    // attribute to a single file read.
    if trimmed.contains('|') || trimmed.contains('>') || trimmed.contains("&&") {
        return None;
    }
    let tokens: Vec<&str> = trimmed.split_whitespace().collect();
    let cmd = *tokens.first()?;
    let base = cmd.rsplit('/').next().unwrap_or(cmd);
    let is_dump = matches!(
        base,
        "cat" | "head" | "tail" | "less" | "more" | "bat" | "nl" | "jq"
    );
    if !is_dump {
        return None;
    }
    // Take the last token that is not a flag or a jq filter expression.
    let path = tokens
        .iter()
        .skip(1)
        .rev()
        .find(|t| !t.starts_with('-') && **t != "." && !t.starts_with('\''))?;
    let cleaned = path.trim_matches(|c| c == '"' || c == '\'');
    if cleaned.is_empty() {
        return None;
    }
    Some(cleaned.to_string())
}

/// Pick a non-gateway provider when available.  Falls back to first provider
/// (which may be Janus) only when no other option exists.  This prevents
/// background operations (memory extraction, compaction, summarisation) from
/// burning Janus credits when a CLI or direct-API provider is loaded.
pub(crate) fn prefer_non_gateway(providers: &[Arc<dyn Provider>]) -> Option<Arc<dyn Provider>> {
    providers
        .iter()
        .find(|p| p.id() != "janus")
        .cloned()
        .or_else(|| providers.first().cloned())
}

/// Resolve the (provider, model) for auxiliary side-tasks — chat title
/// generation, memory extraction, tool-batch summaries, and the auto-continue
/// judge.  Honors `task_routing.aux` ("provider/model") from models.yaml when
/// set AND that provider is loaded; returns `None` otherwise so each caller
/// silently keeps its existing selection.  Aux routing must never make a
/// side-task fail that would have succeeded before.
pub(crate) fn resolve_aux(
    cfg: &config::ModelsConfig,
    providers: &[Arc<dyn Provider>],
) -> Option<(Arc<dyn Provider>, String)> {
    let spec = cfg.task_routing.as_ref().map(|tr| tr.aux.as_str())?;
    if spec.is_empty() {
        return None;
    }
    let (provider_id, model) = spec.split_once('/')?;
    let provider = providers.iter().find(|p| p.id() == provider_id)?.clone();
    Some((provider, model.to_string()))
}

/// JSON shape for tool results stored in the DB. Includes optional image_url
/// so vision-capable providers can receive screenshots in tool result content.
#[derive(serde::Serialize)]
struct ToolResultRow {
    tool_call_id: String,
    content: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    is_error: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    image_url: Option<String>,
    /// Structured rendering payload (ToolResult::payload) so reloaded history
    /// renders the same rich cards as the live stream.
    #[serde(skip_serializing_if = "Option::is_none")]
    payload: Option<serde_json::Value>,
}

/// Input parameters for a run.
#[derive(Debug, Clone, Default)]
pub struct RunRequest {
    pub session_key: String,
    pub prompt: String,
    pub system: String,
    pub model_override: String,
    pub user_id: String,
    pub skip_memory_extract: bool,
    pub origin: Origin,
    /// Allow an autonomous (comm-origin) run to REQUEST tool approval instead
    /// of refusing gated tools — set only when the dispatching context can
    /// relay the approval to the owner (personal-loop conversations). The
    /// desktop's Interactive runs don't need this; it exists so a run whose
    /// approvals reach the owner over comm isn't treated as unattended.
    pub approval_relay: bool,
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
    /// Per-entity permission overrides (tool category → allowed).
    pub permissions: Option<HashMap<String, bool>>,
    /// Per-employee three-state approval policy over gated interface operations
    /// (Always / Approval / Blocked). None = inherit seat defaults.
    pub operation_policy: Option<tools::policy::OperationPolicy>,
    /// Per-entity resource grant overrides (resource → "allow"|"deny"|"inherit").
    pub resource_grants: Option<HashMap<String, String>>,
    /// Per-entity model preference (fuzzy-resolved before provider selection).
    pub model_preference: Option<String>,
    /// Per-entity personality snippet prepended to system prompt.
    pub personality_snippet: Option<String>,
    /// Images attached to the user's message (base64-encoded).
    pub images: Vec<ai::ImageContent>,
    /// Allowed filesystem paths — restricts file writes and shell commands to these directories.
    /// Empty = unrestricted.
    pub allowed_paths: Vec<String>,
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
    /// are bare tool names ("skill") or `tool:resource` compounds
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
    /// Plugin install codes to include in the sub-agent's system prompt.
    /// The plugin inventory and usage docs are injected so the sub-agent
    /// knows how to use these plugins from turn 1.
    pub preload_plugins: Vec<String>,
    /// STRAP domain tool names to include in the sub-agent's system prompt.
    /// The tool's STRAP doc (resources, actions, examples) is injected so the
    /// sub-agent knows how to use these tools without discovery.
    pub preload_tools: Vec<String>,
    /// Tool names to pre-activate (bypass deferred-loading discovery).
    /// Populated automatically from preload_plugins/preload_tools to ensure
    /// sub-agents have the tools available from turn 1.
    pub preactivate_tools: Vec<String>,
    /// When true, agent presents a plan before executing any tool calls.
    /// The plan is sent via a PlanApproval event for user approval.
    pub plan_mode: bool,
    /// Channel context (Slack/Discord/etc.) when this run was triggered by an
    /// inbound channel message. Surfaces on `ToolContext.channel` so the
    /// plugin tool can inject `NEBO_CHANNEL_*` env vars into plugin processes
    /// (e.g. for `slack upload`). See `docs/publishers-guide/channel-plugins.md`.
    pub channel_ctx: Option<tools::ChannelContext>,
    /// Master "Full Access" flag (settings.full_access). When true, the runner's
    /// per-tool approval gate is bypassed entirely — the agent executes without
    /// asking. When false, an OFF capability prompts via the Approval Modal.
    pub full_access: bool,
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
}

/// Shared atomic counters for live run progress reporting.
/// Created by the server's RunRegistry and threaded into the runner.
#[derive(Clone, Debug)]
pub struct RunProgress {
    pub run_id: String,
    pub iteration_count: Arc<std::sync::atomic::AtomicU32>,
    pub tool_call_count: Arc<std::sync::atomic::AtomicU32>,
    pub current_tool: Arc<std::sync::Mutex<String>>,
}

/// Per-run mutable state (prevents data races across concurrent runs).
struct RunState {
    prompt_overhead: usize,
    /// System prompt + tool-schema tokens (display estimate, no threshold fudge).
    system_overhead_tokens: usize,
    last_input_tokens: usize,
    /// Local estimate (chars/4) of the message tokens sent in the last request.
    /// Compared against API-reported usage to calibrate compaction thresholds.
    last_request_estimate: usize,
    /// Observed undercount of the local estimate vs API-reported usage
    /// (hybrid counting, expressed as a threshold adjustment:
    /// actual_prev + est(tail) > threshold  ⇔  est(prev) + est(tail) > threshold − undercount).
    estimate_correction: usize,
    /// Cumulative input tokens across all iterations in this run.
    total_input_tokens: i32,
    /// Cumulative output tokens across all iterations in this run.
    total_output_tokens: i32,
    /// Cumulative cache tokens. Read for calibration since forever but never
    /// kept — and cache reads are most of a long conversation's bill.
    total_cache_read_tokens: i32,
    total_cache_creation_tokens: i32,
    thresholds: Option<ContextThresholds>,
    /// Janus quota warning string, populated when session or weekly usage exceeds 80%.
    quota_warning: Option<String>,
    /// Whether a quota warning WS event has already been sent this run (fire once).
    quota_warning_sent: bool,
}

impl RunState {
    fn new() -> Self {
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
/// Sink for a freshly auto-generated chat title. The runner writes the title to
/// the store itself; the server installs a sink (`set_title_sink`) that
/// broadcasts the change to connected clients and propagates it to the loop —
/// concerns the agent crate can't reach. ONE sink, set once at startup, used by
/// every run path (replaces the per-path title generators + the skip_title_gen
/// flag). Implementations must not block (spawn for async work).
pub trait ChatTitleSink: Send + Sync {
    fn on_title(&self, session_key: String, chat_id: String, title: String);
}

/// The ONE chat-title generator body (CODE_AUDITOR Rule 8). Names the chat on
/// its first user turn and refines once at the third — language-independent
/// (message count, not a default-title string) — and never clobbers a title
/// the user set. Entered from the run loop after each turn and from
/// Runner::spawn_title_generation for chats whose turns are persisted outside
/// a run (voice).
fn spawn_chat_title_generation(
    providers: Arc<RwLock<Vec<Arc<dyn Provider>>>>,
    store: Arc<Store>,
    chat_id: String,
    session_id: String,
    cheap_model: String,
    title_sink: Option<Arc<dyn ChatTitleSink>>,
) {
    tokio::spawn(async move {
        let chat = match store.get_chat(&chat_id) {
            Ok(Some(c)) => c,
            _ => return,
        };
        // Never clobber a title the user explicitly set.
        if chat.title_custom {
            return;
        }
        // Gate on user turns across the WHOLE chat, not the recent window: a
        // windowed count kept re-hitting 1 or 3 as the conversation grew,
        // re-titling the chat from whatever the user said most recently.
        let user_turns = match store.count_chat_user_messages(&chat_id) {
            Ok(n) => n as usize,
            _ => return,
        };
        if user_turns != 1 && user_turns != 3 {
            return; // name once, refine once — at most twice
        }
        let messages = match store.get_recent_chat_messages(&chat_id, 8) {
            Ok(m) => m,
            _ => return,
        };
        if messages.len() < 2 {
            return; // need a user+assistant exchange to name from
        }
        // Use more of the conversation on the count-3 refinement.
        let take_n = if user_turns >= 3 { 8 } else { 4 };
        let transcript: String = messages
            .iter()
            .take(take_n)
            .map(|m| {
                let snippet: String = m.content.chars().take(200).collect();
                format!("{}: {}", m.role, snippet)
            })
            .collect::<Vec<_>>()
            .join("\n");
        if let Some(title) =
            crate::summarizer::generate_session_title(&providers, &transcript, &cheap_model).await
        {
            let _ = store.update_chat_title(&chat_id, &title, false);
            info!(chat_id = %chat_id, title = %title, "auto-generated chat title");
            if let Some(sink) = title_sink {
                sink.on_title(session_id, chat_id, title);
            }
        }
    });
}

pub struct Runner {
    sessions: SessionManager,
    providers: Arc<RwLock<Vec<Arc<dyn Provider>>>>,
    tools: Arc<Registry>,
    store: Arc<Store>,
    selector: Arc<ModelSelector>,
    concurrency: Arc<ConcurrencyController>,
    hooks: Arc<napp::HookDispatcher>,
    mcp_context: Option<Arc<tokio::sync::Mutex<ToolContext>>>,
    agent_registry: tools::AgentRegistry,
    skill_loader: Option<Arc<tools::skills::Loader>>,
    ask_channels: Option<tools::AskChannels>,
    /// Tool-approval channels (PERMISSIONS_SME §11). The runner inserts a
    /// oneshot per tool_call_id, emits `approval_request`, and awaits the user's
    /// ApprovalModal decision. Shares the map with the WS `approval_response`
    /// handler. Set via `set_approval_channels`.
    approval_channels: Option<tools::ApprovalChannels>,
    embedding_provider: Option<Arc<dyn ai::EmbeddingProvider>>,
    /// The SAME hybrid-search adapter instance the memory tool uses (shared
    /// TurboVec index cache) — powers per-message prompt recall.
    hybrid_searcher: Option<Arc<dyn tools::HybridSearcher>>,
    /// Optional broadcast/loop-push sink for auto-generated chat titles.
    title_sink: std::sync::OnceLock<Arc<dyn ChatTitleSink>>,
}

impl Runner {
    pub fn new(
        store: Arc<Store>,
        tools: Arc<Registry>,
        providers: Vec<Arc<dyn Provider>>,
        selector: ModelSelector,
        concurrency: Arc<ConcurrencyController>,
        hooks: Arc<napp::HookDispatcher>,
        mcp_context: Option<Arc<tokio::sync::Mutex<ToolContext>>>,
        agent_registry: tools::AgentRegistry,
        skill_loader: Option<Arc<tools::skills::Loader>>,
    ) -> Self {
        Self {
            sessions: SessionManager::new(store.clone()),
            providers: Arc::new(RwLock::new(providers)),
            tools,
            store,
            ask_channels: None,
            approval_channels: None,
            selector: Arc::new(selector),
            concurrency,
            hooks,
            mcp_context,
            agent_registry,
            skill_loader,
            embedding_provider: None,
            hybrid_searcher: None,
            title_sink: std::sync::OnceLock::new(),
        }
    }

    /// Install the chat-title sink (broadcast + loop propagation). Set once at
    /// startup after AppState exists; no-op if already set.
    pub fn set_title_sink(&self, sink: Arc<dyn ChatTitleSink>) {
        let _ = self.title_sink.set(sink);
    }

    /// Run the ONE chat-title generator for a chat that gained turns outside a
    /// Runner run (the voice loop persists turns directly). Same gates, same
    /// summarizer, same sink as the run-path call.
    pub fn spawn_title_generation(&self, session_id: &str, chat_id: &str) {
        spawn_chat_title_generation(
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
    pub fn set_ask_channels(mut self, channels: tools::AskChannels) -> Self {
        self.ask_channels = Some(channels);
        self
    }

    /// Set the shared tool-approval channels (PERMISSIONS_SME §11) so the runner
    /// can emit `approval_request` and await the user's ApprovalModal decision.
    pub fn set_approval_channels(mut self, channels: tools::ApprovalChannels) -> Self {
        self.approval_channels = Some(channels);
        self
    }

    /// Set the embedding provider for transcript indexing and memory embedding.
    pub fn set_embedding_provider(mut self, provider: Arc<dyn ai::EmbeddingProvider>) -> Self {
        self.embedding_provider = Some(provider);
        self
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
    pub async fn run(&self, req: RunRequest) -> Result<mpsc::Receiver<StreamEvent>, ProviderError> {
        let t_run_entry = std::time::Instant::now();
        info!(
            session_key = %req.session_key,
            channel = %req.channel,
            full_access = req.full_access,
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

        // Pre-load skills into the sub-agent's conversation.
        // Each skill becomes a user message with isMeta metadata so the UI doesn't
        // render it as real user input. Injected BEFORE the task prompt so the
        // sub-agent has instructions in its context from turn 1.
        if !req.preload_skills.is_empty() {
            if let Some(ref loader) = self.skill_loader {
                for skill_name in &req.preload_skills {
                    if let Some(skill) = loader.get(skill_name, None).await {
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

        // Pre-load plugin docs into the sub-agent's conversation.
        // Plugin context (description, skills, usage) is injected as a user message
        // so the sub-agent knows how to use these plugins from turn 1.
        if !req.preload_plugins.is_empty() {
            if let Some(ref loader) = self.skill_loader {
                let plugin_context = loader.agent_plugin_context(&req.preload_plugins);
                if !plugin_context.is_empty() {
                    let meta = serde_json::json!({
                        "isMeta": true,
                        "pluginPreload": true,
                    })
                    .to_string();
                    let _ = self.sessions.append_message(
                        &session_id,
                        "user",
                        &format!("[Loading plugin context]\n\n{}", plugin_context),
                        None,
                        None,
                        Some(&meta),
                    );
                    info!(
                        plugins = ?req.preload_plugins,
                        len = plugin_context.len(),
                        "pre-loaded plugin context into sub-agent"
                    );
                }
            }
        }

        // Pre-load STRAP tool docs into the sub-agent's conversation.
        // Each tool's full documentation (resources, actions, examples) is injected
        // so the sub-agent knows exactly how to call these tools.
        if !req.preload_tools.is_empty() {
            let mut tool_docs = String::new();
            for tool_name in &req.preload_tools {
                // Try core tool doc first, then OS sub-context doc
                let doc = prompt::strap_tool_doc(tool_name)
                    .or_else(|| prompt::strap_context_doc(tool_name));
                if let Some(d) = doc {
                    if !tool_docs.is_empty() {
                        tool_docs.push_str("\n\n---\n\n");
                    }
                    tool_docs.push_str(d);
                }
            }
            if !tool_docs.is_empty() {
                let meta = serde_json::json!({
                    "isMeta": true,
                    "toolPreload": true,
                })
                .to_string();
                let _ = self.sessions.append_message(
                    &session_id,
                    "user",
                    &format!(
                        "[Loading tool documentation for: {}]\n\n{}",
                        req.preload_tools.join(", "),
                        tool_docs,
                    ),
                    None,
                    None,
                    Some(&meta),
                );
                info!(
                    tools = ?req.preload_tools,
                    len = tool_docs.len(),
                    "pre-loaded STRAP tool docs into sub-agent"
                );
            }
        }

        // Append user message — large inputs are offloaded to a temp file and
        // replaced with an LLM-generated summary so the full document never
        // enters the main chat context.
        if !req.prompt.is_empty() {
            let (effective_content, metadata) = if crate::large_input::is_large(&req.prompt) {
                info!(
                    session_id = %session_id,
                    prompt_len = req.prompt.len(),
                    "large input detected — saving to file and summarising"
                );

                let msg_id = uuid::Uuid::new_v4().to_string();

                // 1. Save full content to disk
                let file_path = crate::large_input::save_to_file(&req.prompt, &msg_id)
                    .map_err(|e| ProviderError::Request(format!("large input save: {e}")))?;
                let file_path_str = file_path.to_string_lossy().to_string();

                // 2. Detect content type for prompt tuning
                let content_type = crate::large_input::detect_content_type(&req.prompt);

                // 3. Summarise in an ISOLATED context (sidecar pattern).
                //    Acquire provider, drop lock, then call — the full text
                //    never touches the session or DB.
                let cheap_model = self.selector.get_cheapest_model();
                let summary = {
                    let prov = prefer_non_gateway(&self.providers.read().await);
                    match prov {
                        Some(p) => crate::large_input::summarize(
                            p.as_ref(),
                            &req.prompt,
                            content_type,
                            &cheap_model,
                        )
                        .await
                        .unwrap_or_else(|e| {
                            warn!(error = %e, "large input summarisation failed, using fallback");
                            crate::large_input::fallback_summary(&req.prompt)
                        }),
                        None => crate::large_input::fallback_summary(&req.prompt),
                    }
                };

                // 4. Build replacement content + metadata
                let result = crate::large_input::build_replacement(
                    &req.prompt,
                    &summary,
                    &file_path_str,
                    content_type,
                );

                // Merge with image metadata when both are present
                let mut meta_value: serde_json::Value =
                    serde_json::from_str(&result.metadata_json).unwrap_or_default();
                if !req.images.is_empty() {
                    meta_value["images"] = serde_json::json!(req.images);
                }

                info!(
                    session_id = %session_id,
                    summary_len = result.content.len(),
                    file = %file_path_str,
                    "large input replaced with summary"
                );

                (result.content, Some(meta_value.to_string()))
            } else {
                // Normal-sized prompt — pass through as-is
                let metadata = if !req.images.is_empty() {
                    Some(serde_json::json!({"images": req.images}).to_string())
                } else {
                    None
                };
                (req.prompt.clone(), metadata)
            };

            // An auto-continuation is the house nudging the employee, not the
            // owner speaking. It stays in the model's history (that is the
            // whole point) and out of the owner's transcript — `isMeta` is what
            // the read path filters on.
            let metadata = if crate::goals::is_continuation_prompt(&effective_content) || req.hidden_prompt {
                let mut value: serde_json::Value = metadata
                    .as_deref()
                    .and_then(|m| serde_json::from_str(m).ok())
                    .unwrap_or_else(|| serde_json::json!({}));
                value["isMeta"] = serde_json::json!(true);
                if req.hidden_prompt {
                    value["hiddenPrompt"] = serde_json::json!(true);
                } else {
                    value["autoContinue"] = serde_json::json!(true);
                }
                Some(value.to_string())
            } else {
                metadata
            };

            let t_msg_save = std::time::Instant::now();
            info!(session_id = %session_id, prompt_len = effective_content.len(), "appending user message");
            self.sessions
                .append_message(
                    &session_id,
                    "user",
                    &effective_content,
                    None,
                    None,
                    metadata.as_deref(),
                )
                .map_err(|e| {
                    warn!(session_id = %session_id, error = %e, "failed to append user message");
                    ProviderError::Request(format!("failed to store message: {}", e))
                })?;

            info!(ms = t_msg_save.elapsed().as_millis() as u64, session_id = %session_id, "[telemetry] user message saved");

            // @mention routing context rides the FIRST LLM call as an
            // ephemeral <system-reminder> (seeded into run_loop's pending
            // reminders) — never persisted to the session.
        }
        let mention_context = req.mention_context.clone();

        // Create result channel
        let (tx, rx) = mpsc::channel(100);

        // Clone refs for the spawned task (SessionManager shares cache via Arc)
        let session_mgr = self.sessions.clone();
        let store = self.store.clone();
        let tools = self.tools.clone();
        let providers = self.providers.clone();
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
        let entity_permissions = req.permissions.clone();
        let operation_policy = req.operation_policy.clone();
        let entity_resource_grants = req.resource_grants.clone();
        let personality_snippet = req.personality_snippet.clone();
        let allowed_paths = req.allowed_paths.clone();
        let presence_tracker = req.presence_tracker.clone();
        let proactive_inbox = req.proactive_inbox.clone();
        let prompt_mode = req.prompt_mode.clone();
        let progress = req.progress.clone();
        let ask_channels = self.ask_channels.clone();
        let approval_channels = self.approval_channels.clone();
        let full_access = req.full_access;
        let embedding_provider = self.embedding_provider.clone();
        let hybrid_searcher = self.hybrid_searcher.clone();
        let tool_scope = req.tool_scope.clone();
        let plan_mode = req.plan_mode;
        let preactivate_tools = req.preactivate_tools.clone();
        let channel_ctx = req.channel_ctx.clone();

        // Set MCP context so CLI providers can access tools with the right session info.
        // user_id is NOT stamped here: the raw request user_id must never reach
        // memory (see the memory-owner derivation in run_loop). run_loop stamps
        // the RESOLVED memory scope once it exists; until then the context is
        // fail-closed so an external /mcp call between runs can't write memory
        // under a stale or caller-chosen scope.
        if let Some(ref mcp_ctx) = self.mcp_context {
            let mut ctx = mcp_ctx.lock().await;
            ctx.session_key = session_key.clone();
            ctx.session_id = session_id.clone();
            ctx.origin = req.origin;
            ctx.user_id = String::new();
            ctx.memory_writes_disabled = true;
            // Sub-agents spawned from this run inherit its model unless
            // explicitly overridden.
            ctx.model_preference = (!model_override.is_empty()).then(|| model_override.clone());
        }
        let mcp_context = self.mcp_context.clone();

        tokio::spawn(async move {
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
                        entity_permissions.as_ref(),
                        operation_policy.as_ref(),
                        entity_resource_grants.as_ref(),
                        &user_prompt,
                        &force_skill,
                        skill_loader.as_deref(),
                        &allowed_paths,
                        presence_tracker.as_ref(),
                        proactive_inbox.as_ref(),
                        0,
                        prompt::PromptMode::Minimal,
                        progress.as_ref(),
                        ask_channels.as_ref(),
                        approval_channels.as_ref(),
                        full_access,
                        req.approval_relay,
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
                        mcp_context.as_ref(),
                        &fork_taint,
                        None, // forks never reply to a coworker audience
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
                        result_text.truncate(FORK_OUTPUT_CAP);
                        result_text.push_str("\n\n[Output truncated]");
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
                entity_permissions.as_ref(),
                operation_policy.as_ref(),
                entity_resource_grants.as_ref(),
                &user_prompt,
                &force_skill,
                skill_loader.as_deref(),
                &allowed_paths,
                presence_tracker.as_ref(),
                proactive_inbox.as_ref(),
                min_iterations,
                prompt_mode.clone(),
                progress.as_ref(),
                ask_channels.as_ref(),
                approval_channels.as_ref(),
                full_access,
                req.approval_relay,
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
                mcp_context.as_ref(),
                &run_taint,
                req.audience.as_deref(),
            )
            .await;

            let run_ok = result.is_ok();
            if let Err(e) = result {
                let _ = tx
                    .send(StreamEvent::error(format!("Agent error: {}", e)))
                    .await;
            }
            let final_taint: Vec<types::provenance::ProvenanceClass> =
                run_taint.lock().unwrap().iter().copied().collect();
            let _ = tx
                .send(StreamEvent::done().with_provenance(final_taint))
                .await;

            if !skip_memory {
                // The ONE chat-title generator for every run path (CODE_AUDITOR Rule 8;
                // replaces the old dispatch-side copy + the RunRequest.skip_title_gen
                // flag that coordinated the two). Background paths (scheduler/mcp)
                // simply have no sink, so they title without broadcasting. The voice
                // turn loop persists turns without a Runner run and calls the same
                // generator through Runner::spawn_title_generation.
                spawn_chat_title_generation(
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
                    let entity_permissions_rf = entity_permissions.clone();
                    let operation_policy_rf = operation_policy.clone();
                    let entity_resource_grants_rf = entity_resource_grants.clone();
                    let skill_loader_rf = skill_loader.clone();
                    let allowed_paths_rf = allowed_paths.clone();
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
                            entity_permissions_rf.as_ref(),
                            operation_policy_rf.as_ref(),
                            entity_resource_grants_rf.as_ref(),
                            crate::review_fork::REVIEW_PROMPT,
                            "",
                            skill_loader_rf.as_deref(),
                            &allowed_paths_rf,
                            None,
                            None,
                            0,
                            prompt_mode_rf,
                            None,
                            None,
                            None,
                            false, // no full_access — the whitelist blocks shell anyway
                            false, // review forks never relay approvals
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
                        )
                        .await;
                        drop(sub_tx);

                        let summary = drainer.await.unwrap_or_default();
                        match fork_result {
                            Ok(()) => {
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
    pub async fn chat(&self, prompt: &str) -> Result<String, ProviderError> {
        let prov_lock = self.providers.read().await;
        if prov_lock.is_empty() {
            return Err(ProviderError::Request(
                "No providers configured".to_string(),
            ));
        }

        let req = ChatRequest {
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
            trace: None,
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

    pub fn sessions(&self) -> &SessionManager {
        &self.sessions
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
    entity_permissions: Option<&HashMap<String, bool>>,
    operation_policy: Option<&tools::policy::OperationPolicy>,
    entity_resource_grants: Option<&HashMap<String, String>>,
    user_prompt: &str,
    force_skill: &str,
    skill_loader: Option<&tools::skills::Loader>,
    allowed_paths: &[String],
    presence_tracker: Option<&Arc<crate::proactive::PresenceTracker>>,
    proactive_inbox: Option<&Arc<crate::proactive::ProactiveInbox>>,
    min_iterations: usize,
    prompt_mode: prompt::PromptMode,
    progress: Option<&RunProgress>,
    ask_channels: Option<&tools::AskChannels>,
    approval_channels: Option<&tools::ApprovalChannels>,
    full_access: bool,
    approval_relay: bool,
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
    mcp_context: Option<&Arc<tokio::sync::Mutex<ToolContext>>>,
    run_taint: &std::sync::Mutex<std::collections::BTreeSet<types::provenance::ProvenanceClass>>,
    audience: Option<&str>,
) -> Result<(), String> {
    let mut state = RunState::new();
    // Stream reminders are EPHEMERAL: queued here, injected into the NEXT
    // LLM call's messages in-memory, then dropped. Never persisted to the
    // session — a reminder that lands in stored history pollutes every
    // later context window AND leaks into channel mirrors/backfills.
    let mut pending_stream_reminders: Vec<String> = Vec::new();
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
    // External messaging channels (NeboLoop/Slack/etc.) get the full Interactive treatment —
    // narrating comm-style, progress + action-confirm reminders, smaller streamed chunks — even
    // though the run itself is Autonomous. The person on the other end is waiting on a reply and
    // only sees messages, so they should get the same live experience as the local app.
    let execution_mode = if steering::channel_is_external(channel) {
        tools::ExecutionMode::Interactive
    } else {
        origin.into()
    };
    let mut transient_retries = 0usize;
    let mut retryable_retries = 0usize;
    let mut overflow_retries = 0usize;
    // Pre-seed called_tools with preactivated tools so they pass the tool filter
    // from turn 1 (bypasses deferred-loading discovery for sub-agents).
    let mut called_tools: Vec<String> = preactivate_tools.to_vec();
    // Rolling hashes of recent tool results for stale-result detection in steering
    // (name_hash, args_hash, result_hash, was_unproductive)
    let mut recent_tool_result_hashes: Vec<(u64, u64, u64, bool)> = Vec::new();
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
    let mut read_failures: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    const READ_FAILURE_LIMIT: usize = 3;
    // Spiral backstop (FRAMES Phase 2): UNPRODUCTIVE repeats of the SAME (tool,
    // action) within a turn — errored or returning already-seen content — are the
    // wander-spiral the identical-args and read-failure guards both miss (glob
    // hunting across dirs, browser page re-reads, shell retries). After
    // SAME_ACTION_LIMIT such attempts, return a terminal result so the run ends and
    // the agent reports instead of looping.
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
    let mut provider_idx: usize = 0;
    // Janus provider metadata for tool stickiness — echoed back in subsequent requests
    let mut sticky_metadata: Option<std::collections::HashMap<String, String>> = None;
    let auto_continuations = 0usize;
    // Cycle detection: track last auto-continued response to break loops
    let prev_auto_content: Option<String> = None;
    // Sticky flag: once user demands action ("don't stop", "do them all"), stays true for the run
    let mut user_demanded_action_sticky =
        user_demanded_action(&sessions.get_messages(session_id).unwrap_or_default());
    // Cache for tool documentation (help/schema results) — survives sliding window eviction
    // via injection into the dynamic suffix. Max 5 entries, LRU-evict oldest.
    let mut tool_doc_cache: Vec<(String, String)> = Vec::new();
    const MAX_TOOL_DOC_ENTRIES: usize = 5;
    const MAX_TOOL_DOC_CONTENT: usize = 4_000;
    let mut output_recovery_attempts = 0usize;
    let mut output_escalated = false;
    // Provider said the model stopped to call tools but the stream carried no
    // parsed tool calls (payload lost between proxy and parser). Retried, not
    // trusted — ending the turn silently strands the user mid-task.
    let mut lost_toolcall_retries = 0usize;
    let mut consecutive_error_iterations = 0usize;
    let mut post_tool_empty_nudges = 0usize;
    let mut empty_content_retries = 0usize;
    const MAX_EMPTY_CONTENT_RETRIES: usize = 3;
    // Message-stream steering: per-run cadence for <system-reminder> injection.
    let mut reminder_cadence = steering::ReminderCadence::default();
    let mut turn_exit_reason = "unknown".to_string();
    let mut final_iteration = 0usize;
    let mut last_model_name = String::new();
    // Session-scoped tool schema cache: tool schemas don't change between turns,
    // so we cache them to prevent mid-session schema churn that busts the API's
    // prompt cache.
    let mut tool_schema_cache: HashMap<String, serde_json::Value> = HashMap::new();
    // Track file paths read during this session to detect duplicate reads.
    // When the model re-reads a file, a short note is appended to the tool result.
    let mut files_read_this_session: HashSet<String> = HashSet::new();
    // Deferred tool discovery: each turn, `extract_discovered_deferred_tools`
    // scans window_messages for tool_search results and direct calls to deferred
    // tools. The results accumulate into this per-run set so a tool stays loaded
    // for the rest of the run even after compaction evicts its discovery message.
    // (Snapshotting the discovered set onto the compact boundary would solve the
    // same eviction race — a monotonic per-run set is equivalent and simpler.)
    let mut discovered_deferred: HashSet<String> = HashSet::new();

    // Resolve agent from registry if agent_id is set
    let active_agent_entry = if !agent_id.is_empty() {
        let reg = agent_registry.read().await;
        reg.get(agent_id).cloned()
    } else {
        None
    };

    // Resolve memory config from agent entry. Registry entries can carry
    // `config: None` (agent duplication, a frontmatter parse failure at
    // activation) — that must NOT default to "not isolated", or a copied
    // isolated employee silently runs unisolated (isolation audit 2026-08-22,
    // fail-open class). Fail closed: re-read the store row; empty frontmatter
    // is the legitimate default, unparseable frontmatter counts as isolated.
    let memory_config = active_agent_entry
        .as_ref()
        .and_then(|e| e.config.as_ref())
        .map(|c| c.memory.clone())
        .unwrap_or_else(|| {
            if agent_id.is_empty() {
                return Default::default();
            }
            match store.get_agent(agent_id) {
                Ok(Some(a)) if a.frontmatter.is_empty() => Default::default(),
                Ok(Some(a)) => match napp::agent::parse_agent_config(&a.frontmatter) {
                    Ok(c) => c.memory,
                    Err(e) => {
                        warn!(
                            agent_id,
                            error = %e,
                            "agent config unparseable — treating as context_isolated (fail closed)"
                        );
                        napp::agent::MemoryConfig {
                            context_isolated: true,
                            ..Default::default()
                        }
                    }
                },
                // No row (deleted agent): nothing to isolate. Read error:
                // fail closed like an unparseable config.
                Ok(None) => Default::default(),
                Err(_) => napp::agent::MemoryConfig {
                    context_isolated: true,
                    ..Default::default()
                },
            }
        });

    // Declared memory topics for this scope (agent.json memory.topics) —
    // threaded into extraction, the flush, and the memory tool's layer map.
    let memory_topics = memory_config.topics.clone();

    // Effective provenance write bar for this scope (trust-boundaries design
    // 2026-08-22): agent config `memory.write_bar` (kebab-case class names)
    // when declared — explicit [] is a deliberate opt-out — else the engine
    // default: context-isolated scopes refuse channel/phone content (untrusted
    // interlocutors never write case files); non-isolated scopes have no bar.
    let memory_write_bar: Vec<types::provenance::ProvenanceClass> = match &memory_config.write_bar
    {
        Some(names) => names
            .iter()
            .filter_map(|n| {
                serde_json::from_value(serde_json::Value::String(n.clone()))
                    .map_err(|_| {
                        warn!(agent_id, class = %n, "unknown provenance class in memory.write_bar — ignored");
                    })
                    .ok()
            })
            .collect(),
        None if memory_config.context_isolated => vec![
            types::provenance::ProvenanceClass::Channel,
            types::provenance::ProvenanceClass::Phone,
        ],
        None => Vec::new(),
    };

    // Recall-for-audience (trust-boundaries design 2026-08-22): replying to a
    // coworker not granted by `memory.share_with` restricts recall to
    // `tacit/` — matter/project facts never surface. Owner-set policy,
    // default deny; never per-conversation model judgment.
    let audience_restricted = audience
        .map(|aud| !memory_config.share_with.iter().any(|g| g == aud || g == "*"))
        .unwrap_or(false);
    if audience_restricted {
        info!(
            session_id,
            agent_id,
            audience = audience.unwrap_or(""),
            "recall restricted to tacit/ — audience not granted by memory.share_with"
        );
        pending_stream_reminders.push(steering::wrap_system_reminder(
            "You are replying to a coworker who is NOT granted access to this scope's \
             matter/project memory. It was not consulted and must not be shared — answer \
             from working knowledge, or say the information isn't shared with their role.",
        ));
    }

    // Explicit isolation context from the session KEY, if the channel set one
    // ("agent:{agent_id}:{channel}:{context_id}"). `session_id` here is the
    // session ROW UUID — it never matches the key grammar, so the key must be
    // resolved first or the explicit-ctx design is dead code and every run
    // falls through to the chat derivation below.
    let session_key = sessions
        .resolve_session_key(session_id)
        .unwrap_or_default();
    let explicit_ctx = crate::memory::session_key_context(&session_key);

    // Context-isolated agents whose session key carries NO explicit segment
    // (desktop chat threads) derive the context from the session's ACTIVE
    // CHAT id — thread = matter — via the canonical session→chat resolution.
    // Precedence: an explicit channel segment always wins over the chat
    // derivation (see memory::resolve_memory_scope).
    let chat_ctx = if memory_config.context_isolated
        && !agent_id.is_empty()
        && explicit_ctx.is_none()
    {
        store.session_chat_id(session_id)
    } else {
        None
    };
    let has_context = explicit_ctx.is_some() || chat_ctx.is_some();

    // Canonical memory owner: the on-device local user id, NOT the loosely-passed
    // (often empty) request user_id. ALL memory scoping derives from this so the
    // bot tool, extraction, injection, and the per-agent UI agree on one owner
    // base — otherwise the same memory could land under different scopes between
    // sessions depending on what the caller passed.
    let memory_owner = store
        .ensure_local_user_id()
        .unwrap_or_else(|_| user_id.to_string());

    // Scope memory by agent: each agent gets its own memory namespace to prevent
    // cross-contamination. Main bot uses the raw owner; agents use
    // "owner:agent:agent_id"; with context_isolated, further scoped to
    // "owner:agent:agent_id:ctx:context_id". The ONE derivation — every read
    // and write path below inherits it.
    let memory_scope = crate::memory::resolve_memory_scope(
        &memory_owner,
        agent_id,
        memory_config.context_isolated,
        explicit_ctx.as_deref(),
        chat_ctx.as_deref(),
    );
    // Fail-closed: context_isolated with no derivable context must NEVER write
    // to the shared agent scope (readable from every isolation context — the
    // exact leak the flag exists to prevent). Setting skip_memory refuses the
    // extraction, flush, and personality paths through their existing gate;
    // transcript indexing and the memory tool's mutations check
    // memory_writes_disabled directly. Reads still serve the base agent scope
    // + owner identity chain.
    let memory_writes_disabled = memory_scope.writes_disabled;
    if memory_writes_disabled {
        warn!(
            session_id,
            agent_id, "context_isolated: no context derivable — memory writes disabled for this run"
        );
        skip_memory = true;
    }
    let memory_user_id = memory_scope.user_id;

    // ── Sub-agent scope inheritance ────────────────────────────────────
    // Sub-agent runs (anonymous task spawns and persona delegations) execute
    // inside the CALLER's task: they read under the parent run's already-
    // resolved scope — the orchestrator forwards it as the request user_id —
    // and NEVER write. Without this, a spawn carries an empty agent_id, the
    // derivation above short-circuits to the raw owner scope with writes
    // enabled, and one task-spawn exfiltrates an isolated matter's data into
    // the scope every agent inherits (isolation audit 2026-08-22, leak #3).
    let (memory_user_id, memory_writes_disabled) = if session_key.starts_with("subagent:") {
        skip_memory = true;
        let parent_scope = if user_id.is_empty() {
            memory_user_id
        } else {
            user_id.to_string()
        };
        (parent_scope, true)
    } else {
        (memory_user_id, memory_writes_disabled)
    };

    // CLI providers execute tools out-of-band through the shared MCP context;
    // stamp it with the RESOLVED scope (never the raw request user_id) so
    // memory reads/writes on that path land exactly where this run's do.
    if let Some(mcp_ctx) = mcp_context {
        let mut ctx = mcp_ctx.lock().await;
        ctx.user_id = memory_user_id.clone();
        ctx.memory_writes_disabled = memory_writes_disabled;
    }

    // Company Memory's confidentiality scope for this run. An isolated
    // employee is sealed to ONE matter — the same context its own memory is
    // scoped by — so it can remember its client without ever reaching another.
    // The value is the platform's; it travels as a header the model can't set.
    //
    // Sub-agents inherit it. A spawn carries an empty agent_id, so the
    // context_isolated check below sees a default config and would hand the
    // child UNSCOPED Memory — the company-Memory twin of isolation-audit
    // leak #3. The parent's resolved scope arrives as the request user_id and
    // ends in ":ctx:<id>" when the parent was sealed, so read the matter back
    // out of it rather than trusting the child's own (absent) config.
    let memory_matter: Option<String> = if session_key.starts_with("subagent:") {
        user_id
            .rsplit_once(":ctx:")
            .map(|(_, ctx)| format!("matter/{ctx}"))
    } else if memory_config.context_isolated {
        explicit_ctx
            .as_deref()
            .or(chat_ctx.as_deref())
            .map(|c| format!("matter/{c}"))
    } else {
        None
    };
    // A sealed parent's child is sealed too, even though its own config says
    // nothing: no matter derivable means no company Memory at all.
    let inherits_isolation = session_key.starts_with("subagent:") && user_id.contains(":ctx:");

    // Build the inheritance chain for READ access: agent tacit/ (context-
    // isolated runs only) + owner identity prefixes. Sibling ctx scopes are
    // never in the chain.
    let inherit_scopes = crate::memory::build_inherit_scopes(
        &memory_owner,
        agent_id,
        memory_config.context_isolated,
        has_context,
    );

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
        hybrid_searcher.map(|searcher| {
            let searcher = searcher.clone();
            let recall_user_id = memory_user_id.clone();
            let recall_prompt = user_prompt.to_string();
            let t_start = std::time::Instant::now();
            tokio::spawn(async move {
                let results = searcher
                    .search(
                        &recall_prompt,
                        &recall_user_id,
                        db_context::PROMPT_MEMORY_CANDIDATES,
                        // Relevance floor: with single-leg renormalization
                        // and the corrected BM25 orientation, both installs
                        // score real matches well above this — and a prompt
                        // with NO relevant memories now injects NOTHING
                        // instead of the best of the irrelevant (which was
                        // 1.2k of noise on every turn, and what weak models
                        // answered instead of the ask).
                        Some(db_context::PROMPT_RECALL_MIN_SCORE),
                    )
                    .await;
                (results, t_start.elapsed())
            })
        })
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
    if !agent_id.is_empty() {
        if let Ok(Some(agent_rec)) = store.get_agent(agent_id) {
            if let Ok(vals) = serde_json::from_str::<serde_json::Value>(&agent_rec.input_values) {
                if let Some(obj) = vals.as_object() {
                    if !obj.is_empty() {
                        let lines: Vec<String> = obj
                            .iter()
                            .filter_map(|(key, val)| {
                                let display = match val {
                                    serde_json::Value::String(s) if !s.is_empty() => s.clone(),
                                    serde_json::Value::String(_) => return None,
                                    other => other.to_string(),
                                };
                                Some(format!("- **{}**: {}", key, display))
                            })
                            .collect();
                        if !lines.is_empty() {
                            db_context_formatted.push_str(&format!(
                                "\n\n---\n\n# Configured Inputs\nThe user has configured the following inputs for this agent. \
                                Use these values — do NOT ask the user for information that is already provided here.\n{}",
                                lines.join("\n")
                            ));
                        }
                    }
                }
            }
        }
    }

    // Get active task (mutable: refreshed periodically to catch async detect_objective)
    let mut active_task = sessions.get_active_task(session_id).unwrap_or_default();

    // Skills follow a deferred pattern: NOT auto-loaded into system prompt.
    // Model uses skill(action: "discover") to find skills and skill(action: "load") to
    // activate them. Loaded skill content goes into message history (tool results) and
    // unloads when messages are evicted by sliding window.
    //
    // Exceptions: force_skill (explicit API activation) and agent-declared skills
    // (part of the job definition — always present for that agent).
    let active_skill_template = if let Some(loader) = skill_loader {
        if !force_skill.is_empty() {
            match loader.get(force_skill, None).await {
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
    // definition and must be available from turn 1 (not discovered via tool_search).
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
            }
        }
        set
    };

    // Build static system prompt — use modular prompt when no custom one is provided
    // STRAP docs and tool list are NOT included here — they're injected per-iteration
    // based on which tools pass the context filter (dynamic injection).
    let active_agent_body = active_agent_entry.as_ref().map(|r| {
        // Strip YAML frontmatter from AGENT.md — inject only the prose body.
        // Frontmatter is machine metadata (name, triggers, etc.), not persona instructions.
        match napp::agent::split_frontmatter(&r.agent_md) {
            Ok((yaml_str, body)) => {
                if yaml_str.is_empty() {
                    body
                } else {
                    // Include a compact identity header from frontmatter properties
                    let mut result = String::new();
                    if let Ok(mapping) = serde_yaml::from_str::<serde_yaml::Mapping>(&yaml_str) {
                        let mut identity_parts = Vec::new();
                        for (k, v) in &mapping {
                            if let (serde_yaml::Value::String(key), val) = (k, v) {
                                match key.as_str() {
                                    "name" | "description" | "triggers" => {
                                        let val_str = match val {
                                            serde_yaml::Value::String(s) => s.clone(),
                                            serde_yaml::Value::Sequence(seq) => seq
                                                .iter()
                                                .filter_map(|i| match i {
                                                    serde_yaml::Value::String(s) => {
                                                        Some(s.as_str())
                                                    }
                                                    _ => None,
                                                })
                                                .collect::<Vec<_>>()
                                                .join(", "),
                                            _ => continue,
                                        };
                                        identity_parts.push(format!("- **{}**: {}", key, val_str));
                                    }
                                    _ => {}
                                }
                            }
                        }
                        if !identity_parts.is_empty() {
                            result.push_str(&identity_parts.join("\n"));
                            result.push_str("\n\n");
                        }
                    }
                    result.push_str(&body);
                    result
                }
            }
            Err(_) => r.agent_md.clone(),
        }
    });
    // Build focused context for agent-required plugins (descriptions + skill names).
    let agent_plugin_context = if let Some(ref agent_entry) = active_agent_entry {
        if let Some(ref cfg) = agent_entry.config {
            let mut required = cfg.requires.plugins.clone();
            // Merge scope-specific plugins
            if let Some(scope_name) = tool_scope {
                if let Some(scope) = cfg.scopes.get(scope_name) {
                    for p in &scope.plugins {
                        if !required.contains(p) {
                            required.push(p.clone());
                        }
                    }
                }
            }
            skill_loader
                .as_ref()
                .map(|l| l.agent_plugin_context(&required))
                .unwrap_or_default()
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    // Build agent self-awareness context: workflows, skills, and capabilities.
    // The agent must know about itself from turn 1.
    let agent_self_context = if let Some(ref agent_entry) = active_agent_entry {
        if let Some(ref cfg) = agent_entry.config {
            let mut parts = Vec::new();

            // Workflows
            if !cfg.workflows.is_empty() {
                let mut wf_lines = vec![format!("## Your Workflows ({})\n", cfg.workflows.len())];
                let mut sorted: Vec<_> = cfg.workflows.iter().collect();
                sorted.sort_by_key(|(name, _)| name.as_str());
                for (name, binding) in &sorted {
                    let trigger_desc = match &binding.trigger {
                        napp::agent::AgentTrigger::Schedule { schedule, cron, .. } => {
                            if let Some(s) = schedule {
                                format!("schedule: {}", s)
                            } else {
                                format!("schedule: {}", cron)
                            }
                        }
                        napp::agent::AgentTrigger::Heartbeat { interval, window } => {
                            if let Some(w) = window {
                                format!("heartbeat: every {} within {}", interval, w)
                            } else {
                                format!("heartbeat: every {}", interval)
                            }
                        }
                        napp::agent::AgentTrigger::Event { sources } => {
                            format!("event: {}", sources.join(", "))
                        }
                        napp::agent::AgentTrigger::Watch { plugin, event, .. } => {
                            if let Some(ev) = event {
                                format!("watch: {}.{}", plugin, ev)
                            } else {
                                format!("watch: {}", plugin)
                            }
                        }
                        napp::agent::AgentTrigger::Folder { path, .. } => {
                            format!("folder: {}", path)
                        }
                        napp::agent::AgentTrigger::Manual => "manual".to_string(),
                        napp::agent::AgentTrigger::Call { line } => {
                            format!(
                                "call tree for the {} phone line",
                                if line.is_empty() { "every" } else { line }
                            )
                        }
                    };
                    let desc = if binding.description.is_empty() {
                        String::new()
                    } else {
                        format!(" — {}", binding.description)
                    };
                    let activity_count = binding.activities.len();
                    wf_lines.push(format!(
                        "- **{}**{} [{}] ({} activities)",
                        name, desc, trigger_desc, activity_count
                    ));
                }
                wf_lines.push(String::new());
                wf_lines.push(
                    "Use work(resource: \"<name>\", action: \"run\") to trigger a workflow manually. \
                     Use work(resource: \"<name>\", action: \"status\") to check its last run."
                        .to_string(),
                );
                parts.push(wf_lines.join("\n"));
            }

            // Skills declared by this agent
            if !cfg.skills.is_empty() {
                let mut sk_lines = vec![format!("## Your Skills ({})\n", cfg.skills.len())];
                for skill_ref in &cfg.skills {
                    sk_lines.push(format!("- {}", skill_ref));
                }
                sk_lines.push(String::new());
                sk_lines.push(
                    "These skills are part of your configuration. Use skill(action: \"discover\", query: \"...\") or plugin(action: \"skills\") to explore their capabilities."
                        .to_string(),
                );
                parts.push(sk_lines.join("\n"));
            }

            // Sidecar tools (custom HTTP endpoint tools defined by this agent)
            if !cfg.tools.is_empty() {
                let mut tool_lines = vec![format!("## Your Custom Tools ({})\n", cfg.tools.len())];
                for tool_def in &cfg.tools {
                    tool_lines.push(format!(
                        "- **{}** — {}",
                        tool_def.name, tool_def.description
                    ));
                }
                parts.push(tool_lines.join("\n"));
            }

            parts.join("\n\n")
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    // Compact skill listing (name + capped description per enabled skill).
    // Discovery metadata only — full bodies load on demand via skill(action: "load").
    // Agent-scoped runs also see their own Learned skills in the index.
    let skill_catalog = match skill_loader {
        Some(loader) => {
            let scope = (!agent_id.is_empty()).then_some(agent_id);
            loader.compact_catalog(scope).await
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
                lines.push("Use agents(action: \"list\") for full details. Use agents(action: \"activate\", name: \"...\") to switch.".to_string());
                lines.join("\n")
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to load agent catalog");
            String::new()
        }
    };

    // Load workspace context file (.nebo.md or NEBO.md) — walk up from CWD to git root or home.
    let context_file = load_context_file();

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

    // Fire objective detection in background (non-blocking).
    // Acquires an LLM permit so it doesn't steal provider capacity from the main request.
    {
        let providers = providers.clone();
        let store = store.clone();
        let conc = concurrency.clone();
        let session_id = session_id.to_string();
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
            let _permit = conc.acquire_llm_permit().await;
            let session_mgr = SessionManager::new(store);
            detect_objective(&providers, &session_mgr, &session_id, &user_prompt).await;
        });
    }

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

        if cancel_token.is_cancelled() {
            info!(session_id, "run cancelled before iteration {}", iteration);
            return Ok(());
        }

        // Every consumer drains the event receiver until the run completes, so a
        // closed channel means the consumer task died — stop instead of burning
        // iterations and tool calls into the void (every send is `let _ =`).
        if tx.is_closed() {
            warn!(
                session_id,
                iteration, "event receiver dropped — stopping run"
            );
            return Ok(());
        }

        // Adaptive iteration limit: extend past default only if making genuine progress.
        if iteration > max_iterations && iteration <= hard_ceiling {
            if consecutive_error_iterations >= 2
                || steering::should_force_break(
                    &sessions.get_messages(session_id).unwrap_or_default(),
                    iteration,
                )
                .is_some()
            {
                turn_exit_reason = "adaptive_limit_no_progress".to_string();
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

        // Refresh sticky demand flag — once set, stays true for the entire run
        if !user_demanded_action_sticky {
            user_demanded_action_sticky = user_demanded_action(&all_messages);
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

        // Stage 1: Clear stale tool results (cache-cold session)
        let (mut working, tb_saved) = pruning::time_based_micro_compact(
            &all_messages,
            pruning::TIME_BASED_KEEP_RECENT,
            pruning::TIME_BASED_GAP_THRESHOLD_SECS,
            thresholds.warning,
        );
        if tb_saved > 0 {
            debug!(tokens_saved = tb_saved, "Stage 1: time-based micro-compact");
        }

        // Stage 2: Compress tool results with informative summaries
        let (compacted, mc_saved) = pruning::micro_compact(&working, thresholds.warning);
        if mc_saved > 0 {
            debug!(
                tokens_saved = mc_saved,
                "Stage 2: micro-compact tool results"
            );
            working = compacted;
        }

        // Stage 3: Truncate old user/assistant messages
        let (summarized, ms_saved) = pruning::message_summarize(&working, thresholds.warning, 15);
        if ms_saved > 0 {
            debug!(tokens_saved = ms_saved, "Stage 3: message summarization");
            working = summarized;
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

            // Background: fire LLM summary, store when done (non-blocking)
            let cheap_model = selector.get_cheapest_model();
            let prov = prefer_non_gateway(&providers.read().await);
            if let Some(prov) = prov {
                let sess = sessions.clone();
                let sid = session_id.to_string();
                let task = active_task.clone();
                let existing = existing_summary.clone();
                let conc = concurrency.clone();
                let handle = tokio::spawn(async move {
                    let _permit = conc.acquire_llm_permit().await;
                    match pruning::build_llm_summary(
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

        // Discover which deferred tools are active by scanning the message window:
        // tools load when tool_search results or direct calls appear in messages,
        // and unload when those messages are evicted by sliding window compaction.
        let t_tools_start = std::time::Instant::now();
        let deferred_names = tools.get_deferred_names().await;
        discovered_deferred.extend(tool_filter::extract_discovered_deferred_tools(
            &window_messages,
            &deferred_names,
        ));
        let mut active_deferred = discovered_deferred.clone();

        // Merge agent-declared dependencies — these stay active for the entire session
        // regardless of message window state (they're part of the job definition).
        active_deferred.extend(agent_preactivated.iter().cloned());

        if !active_deferred.is_empty() {
            debug!(tools = ?active_deferred, "deferred tools active (discovered + agent-declared)");
        }

        // Get tool definitions: active (non-deferred + active deferred) tools get full schemas
        let mut all_tool_defs = tools.list_active(&active_deferred).await;
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
        // memory.context_isolated is per EMPLOYEE, while an MCP integration is
        // bot-wide — one Nebo can host an isolated legal assistant alongside a
        // receptionist that should see everything. So the wall lives here, in
        // this run's toolset, not in whether the server is installed.
        //
        // Company Memory is currently single-principal: any caller sees the
        // whole graph, unprojected (DESIGN §11's domain ∩ sensitivity
        // projection is designed, not built). Handing that to an employee whose
        // own memory is sealed per matter would break the promise its setting
        // makes — one case, client, or matter never bleeding into another.
        // Until Memory is matter-scoped, isolated employees simply don't get it.
        // An isolated employee with a derivable matter gets MATTER-SCOPED
        // Memory (the header on every call confines it server-side). Only when
        // no matter can be derived does the blunt wall apply: unscoped access
        // to a single-principal graph is exactly what isolation forbids.
        let isolated_employee = inherits_isolation
            || active_agent_entry
                .as_ref()
                .and_then(|e| e.config.as_ref())
                .map(|c| c.memory.context_isolated)
                .unwrap_or(false);
        if isolated_employee && memory_matter.is_none() {
            // Exact URL match, not a substring guess: a customer's own KB at
            // some other host is their business and must not be withheld.
            let memory_url = config::memory_url();
            let memory_integration_ids: HashSet<String> = if memory_url.is_empty() {
                HashSet::new()
            } else {
                store
                    .list_mcp_integrations()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|i| i.server_url.as_deref() == Some(memory_url.as_str()))
                    .map(|i| i.id)
                    .collect()
            };
            if !memory_integration_ids.is_empty() {
                let mut memory_tool_names: HashSet<String> = HashSet::new();
                for def in &all_tool_defs {
                    if let Some((integration_id, _)) = tools.mcp_proxy_info(&def.name).await {
                        if memory_integration_ids.contains(&integration_id) {
                            memory_tool_names.insert(def.name.clone());
                        }
                    }
                }
                let (kept, withheld) = tool_filter::withhold_memory_tools(
                    all_tool_defs,
                    &mut agent_tool_names,
                    &memory_tool_names,
                );
                all_tool_defs = kept;
                if withheld > 0 {
                    debug!(
                        agent = %agent_id,
                        withheld,
                        "context_isolated employee: company Memory withheld (no matter scoping yet)"
                    );
                }
            }
        }

        let (mut tool_defs, active_contexts) = tool_filter::filter_tools_with_context(
            &all_tool_defs,
            &window_messages,
            &called_tools,
            &agent_tool_names,
        );

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
                tool_defs.retain(|td| {
                    wl.contains(&td.name)
                        || wl.iter().any(|e| {
                            e.split_once(':').is_some_and(|(tool, _)| tool == td.name)
                                || e.strip_suffix('*').is_some_and(|prefix| {
                                    !prefix.is_empty() && td.name.starts_with(prefix)
                                })
                        })
                });
            }
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
        let strap_section =
            prompt::build_strap_section(&filtered_tool_names, &active_contexts, &called_tools);

        // Build compact listing of deferred (not yet discovered) tools
        let deferred_stubs = tools.list_deferred_stubs(&active_deferred).await;
        let deferred_listing = prompt::build_deferred_listing(&deferred_stubs);
        let tools_ms = t_tools_start.elapsed().as_millis() as u64;
        info!(
            ms = tools_ms,
            iteration,
            session_id,
            tool_count = tool_defs.len(),
            "[telemetry] tools filtered + prompt sections built"
        );

        // Select model: use override if set, otherwise ask the selector
        let selected_model = if !model_override.is_empty() {
            model_override.to_string()
        } else {
            selector.select(&window_messages)
        };

        // Determine thinking mode
        let enable_thinking = if !selected_model.is_empty() {
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

        // Circuit-breaker: hard-stop only on an explicit user stop command (budget handles
        // the rest). The behavioral steering moved to the message-stream reminder channel.
        if let Some(reason) = steering::should_force_break(&window_messages, iteration) {
            warn!(session_id, iteration, reason = %reason, "circuit breaker triggered");
            // Typed termination: a ControlNotice status event, never assistant
            // prose (the old apology text leaked verbatim into channel replies).
            turn_exit_reason = "user_requested_stop".to_string();
            let _ = tx
                .send(StreamEvent::control_notice(
                    "Stopped at your request.",
                    "user_requested_stop",
                ))
                .await;
            break;
        }

        // Background-results context (the only survivor of the old steering pipeline).
        let proactive_context = steering::format_proactive_items(&proactive_items);

        // Hook: steering.generate — apps inject additional steering. Delivered as
        // ephemeral <system-reminder> messages in this turn's stream (R8), re-evaluated
        // each iteration like the old suffix injection (never persisted to the session).
        let mut hook_reminders: Vec<String> = Vec::new();
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
                    hook_reminders.push(if d.label.is_empty() {
                        d.content
                    } else {
                        format!("{}: {}", d.label, d.content)
                    });
                }
            }
        }

        // Continuation steering, plugin affinity, and the research-mode nudge all moved
        // to the message-stream reminder channel (R8).

        // Convert ChatMessage to ai::Message, then append any app-injected steering as
        // ephemeral <system-reminder> turns for this iteration only (R8).
        let mut ai_messages = convert_messages(&window_messages);
        // Reminders are collected first, then INSERTED BEFORE a fresh user ask
        // rather than appended after it. When the transcript's tail is the
        // user's just-sent message, anything placed after it becomes the last
        // thing the model reads — and weak models answer the tail: a 39-char
        // ask followed by 1.3k of recalled memory got the ASK echoed back as
        // text instead of executed. Mid-run (tail = tool results), appending
        // at the end is correct — a correction should be the freshest signal.
        let mut reminder_msgs: Vec<Message> = Vec::new();
        for text in hook_reminders {
            reminder_msgs.push(Message {
                role: "user".to_string(),
                content: steering::wrap_system_reminder(&text),
                ..Default::default()
            });
        }
        // Queued stream reminders ride THIS call only, then vanish (R8:
        // reminders are ephemeral — never persisted, never re-sent).
        for content in pending_stream_reminders.drain(..) {
            reminder_msgs.push(Message {
                role: "user".to_string(),
                content,
                ..Default::default()
            });
        }

        // On external channels (NeboLoop/Slack/…) a weak model sometimes opens by
        // claiming it "isn't connected" and offering to simulate — it has its full
        // toolset, it just doesn't believe it. Ground it on the first iteration with
        // a stream <system-reminder> (which weak models heed where they ignore the
        // prompt). Ephemeral: this iteration only, never persisted. The post-tool-round
        // reminder registry can't cover this — it fires too late to shape the first reply.
        if iteration == 1 && steering::channel_is_external(channel) {
            reminder_msgs.push(Message {
                role: "user".to_string(),
                content: steering::wrap_system_reminder(&format!(
                    "You are fully connected on the `{channel}` channel with your complete \
                     toolset — web, files, installed plugins (call them via the `plugin` tool), \
                     skills, and sub-agents — exactly as in any other channel. When asked to do \
                     something, actually do it: call the real tools and report what you did with \
                     concrete results. Never simulate, mock, describe hypothetically, or claim \
                     you lack access — if you're unsure what's available, discover it with \
                     `tool_search` or the `plugin` tool first."
                )),
                ..Default::default()
            });
        }

        // The splice: before the fresh ask when it is the tail, else at the end.
        if !reminder_msgs.is_empty() {
            let insert_at = if ai_messages.last().map(|m| m.role == "user").unwrap_or(false) {
                ai_messages.len() - 1
            } else {
                ai_messages.len()
            };
            for (i, m) in reminder_msgs.into_iter().enumerate() {
                ai_messages.insert(insert_at + i, m);
            }
        }

        // (First-run onboarding is handled proactively + deterministically by the
        // frontend OnboardingTour — the old reactive LLM-reminder kickoff was removed so
        // there's one onboarding pathway. The `nebo-onboarding` skill remains for an
        // explicit "help me get set up" request, matched by its description.)

        let proactive_text = if proactive_context.is_empty() {
            String::new()
        } else {
            proactive_context.join("\n")
        };

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
            proactive_context: proactive_text,
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
        let cache_breakpoints = {
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
        let forced_choice = if iteration == 1 {
            ai_messages
                .iter()
                .rev()
                .find(|m| m.role == "user" && !m.content.starts_with("<system-reminder>"))
                .and_then(|m| named_tool_invocation(&m.content, &tool_defs))
        } else {
            None
        };

        // Build ChatRequest
        let chat_req = ChatRequest {
            tool_choice: forced_choice.unwrap_or_default(),
            messages: ai_messages,
            tools: tool_defs,
            max_tokens: if output_escalated {
                ESCALATED_MAX_OUTPUT_TOKENS
            } else {
                DEFAULT_MAX_OUTPUT_TOKENS
            },
            temperature: 0.7,
            system: full_system,
            static_system: static_system.clone(),
            model: if selected_model_name.is_empty() {
                String::new()
            } else {
                selected_model_name.to_string()
            },
            enable_thinking,
            metadata: sticky_metadata.clone(),
            cache_breakpoints,
            cancel_token: Some(cancel_token.clone()),
            // Tag this chat run so Janus attributes its usage per agent (no
            // workflow_id — chat runs are excluded from per-workflow rollups by
            // design; agent_id is the rollup key for chat spend).
            trace: Some(RequestTrace {
                agent_id: agent_id.to_string(),
                run_id: progress.map(|p| p.run_id.clone()).unwrap_or_default(),
                ..Default::default()
            }),
        };

        let pre_llm_ms = t_iter_start.elapsed().as_millis() as u64;
        info!(
            ms = pre_llm_ms,
            iteration, session_id, "[telemetry] pre-LLM overhead (msg load → request built)"
        );

        // Acquire LLM permit before provider call (blocks if at capacity)
        let t_permit_start = std::time::Instant::now();
        let _llm_permit = tokio::select! {
            _ = cancel_token.cancelled() => {
                info!(session_id, "run cancelled waiting for LLM permit");
                return Ok(());
            }
            permit = concurrency.acquire_llm_permit() => permit,
        };
        let permit_wait_ms = t_permit_start.elapsed().as_millis() as u64;
        if permit_wait_ms > 5 {
            info!(
                ms = permit_wait_ms,
                iteration, session_id, "[telemetry] LLM permit wait"
            );
        }

        // Snapshot provider from lock, then release before I/O
        let provider = {
            let prov_lock = providers.read().await;
            if prov_lock.is_empty() {
                return Err("No AI providers available".to_string());
            }

            // Find provider: use model-based lookup on first attempt,
            // but after retries (provider_idx > 0) use round-robin so we
            // actually fall through to the next provider (e.g. CLI agent).
            let idx = if provider_idx > 0 {
                provider_idx % prov_lock.len()
            } else if !selected_provider_id.is_empty() {
                prov_lock
                    .iter()
                    .position(|p| p.id() == selected_provider_id)
                    .unwrap_or(0)
            } else {
                0
            };

            info!(
                iteration,
                session_id,
                provider_idx = idx,
                provider_id = prov_lock[idx].id(),
                selected_provider_id,
                selected_model = %selected_model,
                provider_count = prov_lock.len(),
                message_count = chat_req.messages.len(),
                tool_count = chat_req.tools.len(),
                enable_thinking,
                "sending request to provider"
            );
            prov_lock[idx].clone()
        };

        // If we fell through to a different provider (e.g. CLI after Janus rate limit),
        // clear the model so the fallback provider uses its own default.
        let mut chat_req = chat_req;
        if provider.id() != selected_provider_id {
            chat_req.model = String::new();
        }

        // Providers that never put images on the wire get them as text instead.
        // Decided here, after the fallback above, because a run that started on a
        // vision provider can land on a blind one mid-retry.
        if !provider.supports_vision() && chat_req.messages.iter().any(|m| m.images.is_some()) {
            crate::sidecar::describe_attached_images(provider.as_ref(), &mut chat_req).await;
        }

        let t_stream_start = std::time::Instant::now();
        let stream_result = tokio::select! {
            _ = cancel_token.cancelled() => {
                info!(session_id, "run cancelled during provider.stream() call");
                return Ok(());
            }
            result = provider.stream(&chat_req) => result,
        };
        let stream_connect_ms = t_stream_start.elapsed().as_millis() as u64;

        let mut rx = match stream_result {
            Ok(rx) => {
                info!(
                    iteration,
                    session_id,
                    connect_ms = stream_connect_ms,
                    "[telemetry] provider stream connected"
                );
                rx
            }
            Err(e) => {
                // Deduplicate repeated errors to avoid log spam
                let err_str = format!("{}", e);
                let fingerprint = dedupe::fingerprint_error(&err_str);
                static ERROR_DEDUP: std::sync::OnceLock<DedupeCache> = std::sync::OnceLock::new();
                let dedup = ERROR_DEDUP.get_or_init(DedupeCache::default);
                if dedup.check(&fingerprint) {
                    debug!(iteration, session_id, "deduplicated provider error");
                } else {
                    warn!(iteration, session_id, error = %e, "provider error");
                }

                if ai::is_context_overflow(&e) {
                    overflow_retries += 1;
                    if overflow_retries > MAX_OVERFLOW_RETRIES {
                        return Err(format!(
                            "Context overflow persisted after {} compaction attempts: {}",
                            MAX_OVERFLOW_RETRIES, e
                        ));
                    }
                    // The provider disagreed with our local estimate. Make the
                    // retry state-changing: tighten thresholds by 20% of the
                    // compaction budget so next iteration's stages actually
                    // evict, instead of re-sending the identical request.
                    let bump = state
                        .thresholds
                        .as_ref()
                        .map(|t| t.auto_compact / 5)
                        .unwrap_or(20_000);
                    state.estimate_correction += bump;
                    warn!(
                        overflow_retries,
                        correction = state.estimate_correction,
                        "context overflow: forcing reactive compaction"
                    );
                    continue;
                }

                if ai::is_transient_error(&e) {
                    transient_retries += 1;
                    selector.mark_failed(&selected_model);
                    if transient_retries > MAX_TRANSIENT_RETRIES {
                        return Err(format!("Too many transient errors: {}", e));
                    }
                    // Try next provider on transient error — but never
                    // silently fall from CLI to Janus (burns Nebo credits).
                    let prov_lock = providers.read().await;
                    let prov_count = prov_lock.len();
                    if prov_count > 1 {
                        let next_idx = (provider_idx + 1) % prov_count;
                        if prov_lock[next_idx].id() == "janus" {
                            drop(prov_lock);
                            return Err(format!("Provider error (no fallback to Janus): {}", e));
                        }
                        drop(prov_lock);
                        provider_idx += 1;
                    } else {
                        drop(prov_lock);
                    }
                    tokio::select! {
                        _ = cancel_token.cancelled() => return Ok(()),
                        _ = tokio::time::sleep(retry_backoff(transient_retries, None)) => {}
                    }
                    continue;
                }

                if e.is_retryable() {
                    retryable_retries += 1;
                    selector.mark_failed(&selected_model);
                    if retryable_retries > MAX_RETRYABLE_RETRIES {
                        return Err(format!(
                            "Service temporarily unavailable after {} retries: {}",
                            MAX_RETRYABLE_RETRIES, e
                        ));
                    }
                    let prov_lock = providers.read().await;
                    let prov_count = prov_lock.len();
                    if prov_count > 1 {
                        let next_idx = (provider_idx + 1) % prov_count;
                        if prov_lock[next_idx].id() == "janus" {
                            drop(prov_lock);
                            return Err(format!("Provider error (no fallback to Janus): {}", e));
                        }
                        drop(prov_lock);
                        provider_idx += 1;
                    } else {
                        drop(prov_lock);
                    }
                    tokio::select! {
                        _ = cancel_token.cancelled() => return Ok(()),
                        _ = tokio::time::sleep(retry_backoff(retryable_retries, None)) => {}
                    }
                    continue;
                }

                return Err(format!("Provider error: {}", e));
            }
        };

        // Process stream (retry counters are reset after stream produces content)
        let mut assistant_content = String::new();
        let mut tool_calls: Vec<ai::ToolCall> = Vec::new();
        let mut stream_error: Option<String> = None;
        let mut last_retry_after: Option<u64> = None;
        let mut stop_reason: Option<String> = None;
        let mut t_first_token: Option<std::time::Instant> = None;
        // Track the order of content blocks (text vs tool) for correct rehydration.
        // Each entry is either "text" (coalesced) or a tool index.
        let mut block_order: Vec<(&str, Option<usize>)> = Vec::new();
        // CLI providers run multi-turn tool loops — save each turn incrementally.
        let cli_incremental = provider.handles_tools();

        loop {
            let mut event = tokio::select! {
                _ = cancel_token.cancelled() => {
                    info!(session_id, "run cancelled during LLM stream");
                    // Best-effort: save whatever content we accumulated before cancellation
                    if !assistant_content.is_empty() || !tool_calls.is_empty() {
                        let tc_json = if !tool_calls.is_empty() {
                            serde_json::to_string(&tool_calls).ok()
                        } else {
                            None
                        };
                        if let Err(e) = sessions.append_message(
                            session_id, "assistant", &assistant_content,
                            tc_json.as_deref(), None, None,
                        ) {
                            warn!(session_id = %session_id, error = %e, "failed to save partial assistant message on cancel");
                        } else {
                            info!(session_id, content_len = assistant_content.len(), tool_count = tool_calls.len(), "saved partial assistant message before cancel");
                        }
                    }
                    return Ok(());
                }
                ev = tokio::time::timeout(STREAM_IDLE_TIMEOUT, rx.recv()) => match ev {
                    Ok(Some(e)) => e,
                    Ok(None) => break,
                    Err(_) => {
                        warn!(
                            iteration,
                            session_id,
                            "stream idle timeout: no events for {}s",
                            STREAM_IDLE_TIMEOUT.as_secs()
                        );
                        stream_error = Some(format!(
                            "stream idle timeout: no events for {}s",
                            STREAM_IDLE_TIMEOUT.as_secs()
                        ));
                        break;
                    }
                }
            };
            if t_first_token.is_none() {
                t_first_token = Some(std::time::Instant::now());
                let ttft = t_stream_start.elapsed().as_millis() as u64;
                let iter_elapsed = t_iter_start.elapsed().as_millis() as u64;
                info!(
                    ttft_ms = ttft,
                    iter_total_ms = iter_elapsed,
                    iteration,
                    session_id,
                    provider = %provider.id(),
                    model = %chat_req.model,
                    "[telemetry] first token received"
                );
            }
            match event.event_type {
                StreamEventType::Text => {
                    // CLI incremental save: text after tool calls = new turn.
                    // Flush the previous turn's content + tool calls to DB.
                    if cli_incremental && !tool_calls.is_empty() {
                        let tc_json = serde_json::to_string(&tool_calls).ok();
                        if let Err(e) = sessions.append_message(
                            session_id,
                            "assistant",
                            &assistant_content,
                            tc_json.as_deref(),
                            None,
                            None,
                        ) {
                            warn!(session_id = %session_id, error = %e, "failed to save CLI turn to DB");
                        } else {
                            debug!(
                                session_id,
                                content_len = assistant_content.len(),
                                tool_count = tool_calls.len(),
                                "saved CLI turn incrementally"
                            );
                        }
                        assistant_content.clear();
                        tool_calls.clear();
                        block_order.clear();
                    }
                    assistant_content.push_str(&event.text);
                    // Coalesce consecutive text events into one block
                    if block_order.last().map_or(true, |b| b.0 != "text") {
                        block_order.push(("text", None));
                    }
                    let _ = tx.send(event).await;
                }
                StreamEventType::Thinking => {
                    info!(session_id, "received thinking block");
                    let _ = tx.send(event).await;
                }
                StreamEventType::ToolCall => {
                    if let Some(ref tc) = event.tool_call {
                        info!(session_id, tool = %tc.name, tool_id = %tc.id, "tool call received");
                        tool_calls.push(tc.clone());
                        block_order.push(("tool", Some(tool_calls.len() - 1)));
                    }
                    let _ = tx.send(event).await;
                }
                StreamEventType::Error => {
                    warn!(session_id, error = ?event.error, "stream error event");
                    stream_error = event.error.clone();
                    // Don't forward to user yet — classify after stream ends
                }
                StreamEventType::Usage => {
                    if let Some(ref mut usage) = event.usage {
                        state.last_input_tokens = usage.input_tokens as usize;
                        state.total_input_tokens += usage.input_tokens;
                        state.total_output_tokens += usage.output_tokens;
                        state.total_cache_read_tokens += usage.cache_read_input_tokens;
                        state.total_cache_creation_tokens += usage.cache_creation_input_tokens;
                        usage.overhead_tokens = state.system_overhead_tokens as i32;

                        // Calibrate the local token estimate against ground truth.
                        // Context actually sent = input + cache tokens (with prompt
                        // caching, input_tokens alone excludes the cached bulk).
                        let context_actual = (usage.input_tokens
                            + usage.cache_creation_input_tokens
                            + usage.cache_read_input_tokens)
                            as usize;
                        if context_actual > 0 && state.last_request_estimate > 0 {
                            let conversation_actual =
                                context_actual.saturating_sub(state.system_overhead_tokens);
                            state.estimate_correction =
                                conversation_actual.saturating_sub(state.last_request_estimate);
                        }
                    }
                    let _ = tx.send(event).await;
                }
                StreamEventType::RateLimit => {
                    if let Some(ref meta) = event.rate_limit {
                        last_retry_after = meta.retry_after_secs;
                        concurrency.report_success(Some(meta));

                        // Check Janus session/weekly usage and generate quota warning at >80%
                        let mut warnings = Vec::new();
                        if let (Some(limit), Some(remaining)) =
                            (meta.session_limit_credits, meta.session_remaining_credits)
                        {
                            if limit > 0 {
                                let used_pct = ((limit.saturating_sub(remaining)) as f64
                                    / limit as f64)
                                    * 100.0;
                                if used_pct >= 80.0 {
                                    warnings.push(format!(
                                        "Session usage at {:.0}% (resets at {})",
                                        used_pct,
                                        meta.session_reset_at.as_deref().unwrap_or("unknown"),
                                    ));
                                }
                            }
                        }
                        if let (Some(limit), Some(remaining)) =
                            (meta.weekly_limit_credits, meta.weekly_remaining_credits)
                        {
                            if limit > 0 {
                                let used_pct = ((limit.saturating_sub(remaining)) as f64
                                    / limit as f64)
                                    * 100.0;
                                if used_pct >= 80.0 {
                                    warnings.push(format!(
                                        "Weekly usage at {:.0}% (resets at {})",
                                        used_pct,
                                        meta.weekly_reset_at.as_deref().unwrap_or("unknown"),
                                    ));
                                }
                            }
                        }
                        if !warnings.is_empty() {
                            let warning_text = warnings.join(". ");
                            state.quota_warning = Some(warning_text.clone());

                            // Forward the rate limit event with warning text once per run
                            // so chat_dispatch can broadcast a quota_warning WS event.
                            if !state.quota_warning_sent {
                                state.quota_warning_sent = true;
                                let _ = tx
                                    .send(StreamEvent { payload: None,
                                        provenance: None,
                                        event_type: StreamEventType::RateLimit,
                                        text: warning_text,
                                        tool_call: None,
                                        error: None,
                                        usage: None,
                                        rate_limit: event.rate_limit.clone(),
                                        widgets: None,
                                        provider_metadata: None,
                                        stop_reason: None,
                                        image_url: None,
                                    })
                                    .await;
                            }
                        }
                    }
                }
                StreamEventType::Done => {
                    // Capture stop reason for max output recovery
                    if event.stop_reason.is_some() {
                        stop_reason = event.stop_reason.clone();
                    }
                    // Capture provider metadata for Janus tool stickiness
                    if let Some(meta) = event.provider_metadata {
                        sticky_metadata = Some(meta);
                    }
                }
                StreamEventType::ToolResult => {
                    // CLI providers (handles_tools) execute tools themselves via
                    // MCP and stream the results back; relay so chat_dispatch can
                    // broadcast tool_result. API providers never emit this event —
                    // the runner synthesizes it after executing tools itself.
                    let _ = tx.send(event).await;
                }
                StreamEventType::ApprovalRequest
                | StreamEventType::AskRequest
                | StreamEventType::PlanApproval
                | StreamEventType::ControlNotice => {
                    // Approval/Ask/Plan/ControlNotice: only sent by runner, not
                    // received from provider.
                }
                StreamEventType::ToolSummary => {
                    // Tool execution summary — relay to parent for display.
                    let _ = tx.send(event).await;
                }
                StreamEventType::SubagentStart
                | StreamEventType::SubagentProgress
                | StreamEventType::SubagentComplete => {
                    // Forwarded from sub-agent orchestrator via stream_tx; relay to parent.
                    let _ = tx.send(event).await;
                }
            }
        }

        // Drop LLM permit now that stream is complete
        drop(_llm_permit);

        // Reset retry counters only when stream actually produced content
        if stream_error.is_none() && (!assistant_content.is_empty() || !tool_calls.is_empty()) {
            transient_retries = 0;
            retryable_retries = 0;
            // Note: estimate_correction is NOT reset — the compaction that
            // recovered from overflow must stay in effect for the rest of the run.
            overflow_retries = 0;
        }

        // Report success or rate limit to concurrency controller
        if stream_error.is_none() {
            concurrency.report_success(None);
        }

        // Handle stream errors — classify and retry (matches Go runner logic)
        if let Some(ref err_msg) = stream_error {
            warn!("stream error: {}", err_msg);
            let err = ProviderError::Stream(err_msg.clone());
            let reason = ai::classify_error_reason(&err);

            // Mid-stream cutoff continuation: partial text the user already
            // watched stream would die with the retry `continue` below (which
            // skips the normal end-of-iteration save), so the retried call
            // would regenerate — repeating or restarting what was already
            // delivered. Persist the partial turn (same append pathway as the
            // cancel save in the stream loop) and steer the retry to resume in
            // place. Partial tool calls are NOT saved — an assistant tool_use
            // with no tool result is an invalid sequence for every provider.
            // Called only on the branches that actually retry; the
            // non-retryable fall-through persists via the normal save.
            // The user watched this text stream and then freeze mid-sentence.
            // Without a status line the dead bubble reads as the model giving
            // up; with one, the retry reads as what it is — a reconnect.
            let had_partial = !assistant_content.is_empty();
            let reconnect_notice = || {
                tx.send(StreamEvent::control_notice(
                    "Connection dropped mid-response — reconnecting to resume where it left off.",
                    "stream_reconnecting",
                ))
            };
            let mut queue_cutoff_continuation = || {
                if assistant_content.is_empty() && tool_calls.is_empty() {
                    return;
                }
                if !assistant_content.is_empty() {
                    if let Err(e) = sessions.append_message(
                        session_id,
                        "assistant",
                        &assistant_content,
                        None,
                        None,
                        None,
                    ) {
                        warn!(session_id = %session_id, error = %e, "failed to save partial assistant message before stream retry");
                    }
                }
                // A stream that dies while a tool call is in flight is almost
                // always killed by the call itself — one enormous streamed
                // argument (a whole document inline). A generic "continue"
                // makes the model re-emit the same giant call and die the same
                // way; name the cause and steer it to chunk the work instead.
                let reminder = if tool_calls.is_empty() {
                    "Your previous response was cut off mid-stream by a \
                     connection error. Continue EXACTLY where you left off — \
                     do not repeat or restart."
                        .to_string()
                } else {
                    let names: Vec<&str> =
                        tool_calls.iter().map(|tc| tc.name.as_str()).collect();
                    format!(
                        "Your previous response was cut off mid-stream while \
                         emitting a tool call ({}) — the call was NOT delivered. \
                         Oversized tool arguments are the usual cause. Do NOT \
                         retry one giant call: break the work into several \
                         smaller tool calls (write large files in pieces, edit \
                         one section at a time), then continue from where you \
                         stopped.",
                        names.join(", ")
                    )
                };
                pending_stream_reminders.push(steering::wrap_system_reminder(&reminder));
            };

            // Layer 1: Transient errors (connection reset, timeout, EOF)
            if ai::is_transient_error(&err) {
                transient_retries += 1;
                if transient_retries <= MAX_TRANSIENT_RETRIES {
                    queue_cutoff_continuation();
                    if had_partial {
                        let _ = reconnect_notice().await;
                    }
                    let prov_count = providers.read().await.len();
                    if prov_count > 1 {
                        provider_idx += 1;
                    }
                    tokio::select! {
                        _ = cancel_token.cancelled() => return Ok(()),
                        _ = tokio::time::sleep(retry_backoff(transient_retries, None)) => {}
                    }
                    continue;
                }
            }

            // Report rate limit to concurrency controller
            if reason == "rate_limit" {
                concurrency.report_rate_limit(last_retry_after);
            }

            // Layer 2: Retryable errors (rate_limit, billing, provider errors)
            let is_retryable = err.is_retryable()
                || reason == "rate_limit"
                || reason == "billing"
                || reason == "provider"
                || reason == "timeout";
            if is_retryable {
                retryable_retries += 1;
                if retryable_retries > MAX_RETRYABLE_RETRIES {
                    let _ = tx
                        .send(StreamEvent::error(format!(
                            "Service temporarily unavailable after {} retries: {}",
                            MAX_RETRYABLE_RETRIES, err_msg
                        )))
                        .await;
                    break;
                }
                warn!(
                    reason,
                    retryable_retries, "retryable stream error, trying next provider"
                );
                queue_cutoff_continuation();
                if had_partial {
                    let _ = reconnect_notice().await;
                }
                let prov_count = providers.read().await.len();
                if prov_count > 1 {
                    provider_idx += 1;
                }
                tokio::select! {
                    _ = cancel_token.cancelled() => return Ok(()),
                    _ = tokio::time::sleep(retry_backoff(retryable_retries, last_retry_after)) => {}
                }
                continue;
            }

            // Layer 3: Non-retryable — send error to user
            let _ = tx.send(StreamEvent::error(err_msg.clone())).await;
        }

        let stream_total_ms = t_stream_start.elapsed().as_millis() as u64;
        let iter_total_ms = t_iter_start.elapsed().as_millis() as u64;
        info!(
            session_id,
            iteration,
            content_len = assistant_content.len(),
            tool_call_count = tool_calls.len(),
            has_error = stream_error.is_some(),
            stream_ms = stream_total_ms,
            iter_ms = iter_total_ms,
            "[telemetry] stream complete"
        );

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

                let request_id = uuid::Uuid::new_v4().to_string();
                let tool_names: Vec<String> = tool_calls.iter().map(|tc| tc.name.clone()).collect();

                let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
                ask_chs.lock().await.insert(request_id.clone(), resp_tx);

                let _ = tx
                    .send(StreamEvent::plan_approval_request(
                        &request_id,
                        &plan_text,
                        tool_names,
                    ))
                    .await;

                info!(session_id, request_id = %request_id, "plan mode: waiting for user approval");

                let approved = tokio::select! {
                    _ = cancel_token.cancelled() => {
                        info!(session_id, "plan approval cancelled");
                        ask_chs.lock().await.remove(&request_id);
                        return Ok(());
                    }
                    result = resp_rx => {
                        match result {
                            Ok(value) => {
                                let v = value.to_lowercase();
                                v == "approve" || v == "approved" || v == "yes" || v == "true"
                            }
                            Err(_) => false,
                        }
                    }
                };

                if !approved {
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

                info!(session_id, "plan approved, proceeding with tool execution");
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
            let resolved_key = sessions
                .resolve_session_key(session_id)
                .unwrap_or_else(|_| session_id.to_string());
            let mut ctx = ToolContext {
                origin,
                session_key: resolved_key,
                session_id: session_id.to_string(),
                user_id: memory_user_id.clone(),
                trusted_plugin_env: false,
                handoff_depth,
                entity_permissions: entity_permissions.cloned(),
                operation_policy: operation_policy.cloned(),
                resource_grants: entity_resource_grants.cloned(),
                allowed_paths: allowed_paths.to_vec(),
                cancel_token: cancel_token.clone(),
                stream_tx: Some(tx.clone()),
                run_id: progress.map(|p| p.run_id.clone()),
                ask_channels: ask_channels.cloned(),
                channel: channel_ctx.cloned(),
                model_preference: (!model_override.is_empty()).then(|| model_override.to_string()),
                memory_topics: memory_topics.iter().map(|t| t.slug.clone()).collect(),
                memory_writes_disabled,
                run_taint: run_taint.lock().unwrap().iter().copied().collect(),
                memory_write_bar: memory_write_bar.clone(),
                audience_restricted,
                memory_matter: memory_matter.clone(),
                // Populated by the approval gate below, before tool execution.
                approved_categories: std::collections::HashSet::new(),
                // Restricted-run allowlist: the review fork's whitelist, or
                // the request's explicit allowlist (phone callers). None for
                // every normal run.
                tool_whitelist: review_fork
                    .as_ref()
                    .map(|r| r.whitelist.clone())
                    .or_else(|| tool_allowlist.cloned()),
                whitelist_denial_hint: tool_denial_hint.clone(),
                learned_write_agent: review_fork.as_ref().map(|r| r.owner_agent_id.clone()),
                learned_write_staged: review_fork.as_ref().map(|r| r.staged).unwrap_or(false),
                skills_read: review_fork
                    .as_ref()
                    .map(|r| r.skills_read.clone())
                    .unwrap_or_default(),
            };

            // Track tool names for context filtering
            for tc in &tool_calls {
                called_tools.push(tc.name.clone());
            }

            // Launch all tool calls concurrently via FuturesUnordered
            let mut futures = FuturesUnordered::new();
            // Update progress: count tools and set current tool name
            if let Some(p) = progress {
                p.tool_call_count.fetch_add(
                    tool_calls.len() as u32,
                    std::sync::atomic::Ordering::Relaxed,
                );
                if let Ok(mut ct) = p.current_tool.lock() {
                    ct.clear();
                    if tool_calls.len() == 1 {
                        ct.push_str(&tool_calls[0].name);
                    } else {
                        ct.push_str(&format!("{} tools", tool_calls.len()));
                    }
                }
            }
            // Apply tool.pre_execute filter hooks — may block individual tools.
            let mut blocked_results: Vec<Option<(ai::ToolCall, ToolResult)>> =
                vec![None; tool_calls.len()];
            let has_pre_hook = hooks.has_subscribers("tool.pre_execute");
            if has_pre_hook {
                for idx in 0..tool_calls.len() {
                    let payload = serde_json::to_vec(&crate::hooks::ToolPreExecutePayload {
                        tool_name: tool_calls[idx].name.clone(),
                        input: tool_calls[idx].input.clone(),
                        session_id: session_id.to_string(),
                    })
                    .unwrap_or_default();
                    let (result, _handled) = hooks.apply_filter("tool.pre_execute", payload).await;
                    if let Ok(resp) =
                        serde_json::from_slice::<crate::hooks::ToolPreExecuteResponse>(&result)
                    {
                        if resp.blocked {
                            let msg = resp
                                .blocked_message
                                .unwrap_or_else(|| "Blocked by plugin hook".into());
                            blocked_results[idx] =
                                Some((tool_calls[idx].clone(), ToolResult::error(msg)));
                        } else if let Some(mutated_input) = resp.input {
                            tool_calls[idx].input = mutated_input;
                        }
                    }
                }
            }

            // Hard guard: block tool calls that keep repeating identical args WITHOUT
            // making progress.
            //
            // Only UNPRODUCTIVE repeats count — a call that errored, or returned content
            // the model already had. This guard used to count every repeat including
            // successes, and "the result will not change" is simply false for a tool that
            // changes the world: re-running `pptx create` after editing its input spec
            // produces a different file every time. Blocking it stranded a model
            // mid-iteration and, because the block says to "use different parameters",
            // pushed it into renaming its own output to get past the guard — which is how
            // one deck became `deck.pptx`, `deck-v2.pptx`, `deck-final.pptx`.
            //
            // This is the same correction already applied to the spiral backstop (see
            // `counts_toward_action_spiral`): failures accrue, successes reset, and a tool
            // that mutates state is never held to a no-progress rule.
            for (idx, tc) in tool_calls.iter().enumerate() {
                if blocked_results[idx].is_some() {
                    continue;
                }
                let name_hash = simple_hash(tc.name.as_bytes());
                let args_hash = simple_hash(tc.input.to_string().as_bytes());
                let unproductive_repeats = recent_tool_result_hashes
                    .iter()
                    .filter(|&&(nh, ah, _, unproductive)| {
                        nh == name_hash && ah == args_hash && unproductive
                    })
                    .count();
                if unproductive_repeats >= guard_cfg.identical_args_block_after {
                    blocked_results[idx] = Some((
                        tc.clone(),
                        ToolResult::error(format!(
                            "Blocked: {} has been called {} times with identical arguments \
                             and has not made progress — it errored or returned what you \
                             already had. Retrying it unchanged will not help: read the \
                             error, change the arguments, or tell the user what is blocking \
                             you. Do NOT work around this by renaming an output file.",
                            tc.name,
                            unproductive_repeats + 1
                        )),
                    ));
                }
            }

            // Defense-in-depth: block repeated reads of the SAME target that keep
            // FAILING via different methods/args (which the identical-args guard above
            // misses — the #research read-loop). After READ_FAILURE_LIMIT failures of a
            // path, force the model to report instead of retrying. NOT a substitute for
            // the file-read fix.
            for (idx, tc) in tool_calls.iter().enumerate() {
                if blocked_results[idx].is_some() {
                    continue;
                }
                if let Some(p) = extract_file_read_path(tc) {
                    if read_failures.get(&p).copied().unwrap_or(0) >= READ_FAILURE_LIMIT {
                        warn!(session_id, path = %p, "blocking read after repeated failures");
                        blocked_results[idx] = Some((
                            tc.clone(),
                            ToolResult::error(format!(
                                "Blocked: reading {} has failed {} times via different methods. \
                                 Stop retrying — tell the user the file could not be read and ask \
                                 how they'd like to proceed.",
                                p, READ_FAILURE_LIMIT
                            )),
                        ));
                    }
                }
            }

            // Spiral backstop (see action_call_counts): once one (tool, action) has
            // racked up SAME_ACTION_LIMIT UNPRODUCTIVE attempts this turn (errored or
            // returning content the model already had — glob-wander / browser re-read /
            // shell-retry), nudge the model off that action. Productive calls that
            // return novel results don't count, so legitimate bulk work (create N
            // todos, write N files) never trips this. (FRAMES Phase 2.)
            //
            // This is a NUDGE, not a stop. It used to end the run with a terminal
            // result, which turned every false positive into a dead turn the user saw
            // as a red "Stopped: … called 8 times without progress" banner — the model
            // had more to do and no way to say so. The offending call is refused with a
            // corrective error; every other tool, and the turn, carries on.
            //
            // The budget resets when it fires, so a model that changes approach isn't
            // locked out of the action for the rest of the turn — a genuine loop simply
            // earns another nudge in another SAME_ACTION_LIMIT unproductive calls.
            let mut spiral_hard_stop: Option<String> = None;
            for (idx, tc) in tool_calls.iter().enumerate() {
                if blocked_results[idx].is_some() {
                    continue;
                }
                let key = action_key(tc);
                if action_call_counts.get(&key).copied().unwrap_or(0)
                    >= guard_cfg.same_action_limit
                {
                    warn!(
                        session_id,
                        action = %key,
                        limit = guard_cfg.same_action_limit,
                        hard_stop = guard_cfg.hard_stop,
                        "spiral backstop: nudging model off repeated action"
                    );
                    // Hard-stop opt-in (Settings → Developer): end the turn with
                    // a typed ControlNotice instead of nudging. Off by default —
                    // interactive sessions keep the gentle correction.
                    if guard_cfg.hard_stop {
                        spiral_hard_stop = Some(key);
                        break;
                    }
                    action_call_counts.insert(key.clone(), 0);
                    blocked_results[idx] = Some((
                        tc.clone(),
                        ToolResult::error(format!(
                            "'{}' has been called {} times this turn without resolving. \
                             Do not repeat it unchanged — change the arguments, use a \
                             different tool, or tell the user what you have so far and \
                             what is blocking you.",
                            key, guard_cfg.same_action_limit
                        )),
                    ));
                }
            }
            if let Some(key) = spiral_hard_stop {
                turn_exit_reason = "repeated_tool_calls".to_string();
                let _ = tx
                    .send(StreamEvent::control_notice(
                        format!(
                            "Stopped: '{}' was repeated {} times without progress \
                             (hard stop is enabled in Settings → Developer).",
                            key, guard_cfg.same_action_limit
                        ),
                        "repeated_tool_calls",
                    ))
                    .await;
                break;
            }

            // ── Restricted-run allowlist ─────────────────────────────────────
            // Review fork (docs/design/SELF_IMPROVEMENT.md WS2) and phone-
            // caller runs: only allowlisted tools may EXECUTE; everything
            // else is denied here with a corrective error. Matching is
            // `tool:resource`-aware and shared with the registry choke point
            // (ToolContext::whitelist_allows) so the two fences can't drift.
            if ctx.tool_whitelist.is_some() {
                for (idx, tc) in tool_calls.iter().enumerate() {
                    if blocked_results[idx].is_none() && !ctx.whitelist_allows(&tc.name, &tc.input)
                    {
                        let msg = if review_fork.is_some() {
                            format!(
                                "Background review denied non-whitelisted tool: {}. \
                                 Only the skill tool is available in this review pass — \
                                 save the learning with it or reply 'Nothing to save.'",
                                tc.name
                            )
                        } else if let Some(ref hint) = ctx.whitelist_denial_hint {
                            format!("'{}' is not available in this run. {}", tc.name, hint)
                        } else {
                            format!(
                                "'{}' is not available in this call. Use the tools you \
                                 were given, or tell the caller plainly that you can't \
                                 do that and offer to take a message.",
                                tc.name
                            )
                        };
                        blocked_results[idx] = Some((tc.clone(), ToolResult::error(msg)));
                    }
                }
            }

            // ── Per-tool approval gate (PERMISSIONS_SME §11) ──────────────────
            // A capability that's OFF means ASK the user, not hard-fail. We wire
            // the previously-dangling producer: emit `approval_request` and await
            // the ApprovalModal decision via the shared `approval_channels`
            // round-trip (the SAME pathway plan-mode uses). Autonomous mode and
            // pre-granted (ON) categories proceed without asking; Deny returns a
            // clean declined result; "Always" flips the capability ON for next
            // time. Categories cleared here are recorded on the ToolContext so the
            // registry permission gate (Phase 1c) treats them as allowed.
            let mut approved_cats: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            // Per-command allowlist: prefixes the user chose "Approve Always" for.
            // Loaded once; appended on an "always" decision for a shell command.
            let mut approved_cmds: Vec<String> = store.get_approved_commands().unwrap_or_default();
            for idx in 0..tool_calls.len() {
                if blocked_results[idx].is_some() {
                    continue;
                }
                // ── MCP tri-state gate (Settings → MCP → Tool permissions) ────
                // Per-server default + per-tool override, decided by
                // tools::policy::McpServerPermissions: Allow auto-approves,
                // Ask runs the SAME ApprovalGate round-trip as capabilities,
                // Deny refuses with an error naming the setting. This gate is
                // the ONE enforcement site for MCP tool permissions; MCP
                // proxies carry no ambient capability, so the loop `continue`s
                // here and never reaches the capability gate below.
                if tool_calls[idx].name.starts_with("mcp__") {
                    if let Some((integration_id, original)) =
                        tools.mcp_proxy_info(&tool_calls[idx].name).await
                    {
                        let perms = tools::policy::McpServerPermissions::from_json(
                            store
                                .get_mcp_tool_permissions(&integration_id)
                                .unwrap_or_default()
                                .as_deref(),
                        );
                        // mcp__<server>__<tool> — the server slug, for messages.
                        let server = tool_calls[idx]
                            .name
                            .split("__")
                            .nth(1)
                            .unwrap_or("server")
                            .to_string();
                        match perms.decide(&original) {
                            tools::policy::McpToolAccess::Allow => {}
                            tools::policy::McpToolAccess::Deny => {
                                blocked_results[idx] = Some((
                                    tool_calls[idx].clone(),
                                    ToolResult::error(format!(
                                        "Blocked: the MCP tool '{original}' on server \
                                         '{server}' is set to Blocked in Settings → MCP → \
                                         Tool permissions. Tell the user this tool is \
                                         blocked by that setting and stop — do not retry \
                                         or work around it."
                                    )),
                                ));
                            }
                            tools::policy::McpToolAccess::Ask if full_access => {
                                // Full Access bypasses the ask, same as the
                                // capability gate. Blocked above still blocks.
                            }
                            tools::policy::McpToolAccess::Ask => {
                                match approval_channels {
                                    Some(chs)
                                        if tools::ExecutionMode::from(origin)
                                            == tools::ExecutionMode::Interactive
                                            || approval_relay =>
                                    {
                                        let decision = ask_tool_approval(
                                            chs,
                                            tx,
                                            cancel_token,
                                            &tool_calls[idx],
                                            session_id,
                                            "mcp",
                                        )
                                        .await;
                                        match decision.as_str() {
                                            "always" => {
                                                if let Err(e) = persist_mcp_tool_allow(
                                                    store,
                                                    &integration_id,
                                                    &original,
                                                ) {
                                                    warn!(session_id, tool = %original, error = %e, "failed to persist MCP tool grant");
                                                }
                                            }
                                            "once" | "approve" | "approved" | "yes" | "true" => {}
                                            _ => {
                                                blocked_results[idx] = Some((
                                                    tool_calls[idx].clone(),
                                                    ToolResult::error(format!(
                                                        "The user declined to allow the MCP \
                                                         tool '{original}' on server \
                                                         '{server}'. Tell the user it needs \
                                                         their approval and stop — do not \
                                                         retry or work around it."
                                                    )),
                                                ));
                                            }
                                        }
                                    }
                                    // Unattended (cron/workflow/comm/subagent) or no
                                    // channel: nobody can answer — refuse instead of
                                    // hanging on a prompt nobody sees. Unlike the
                                    // capability gate there is no registry backstop
                                    // for MCP tools, so the refusal happens here.
                                    _ => {
                                        blocked_results[idx] = Some((
                                            tool_calls[idx].clone(),
                                            ToolResult::error(format!(
                                                "The MCP tool '{original}' on server \
                                                 '{server}' needs the user's approval \
                                                 (Settings → MCP → Tool permissions) and no \
                                                 one is available to approve it in this \
                                                 run. Report this and stop."
                                            )),
                                        ));
                                    }
                                }
                            }
                        }
                    }
                    continue;
                }
                // ── Per-operation approval gate (per-employee three-state policy) ──
                // A gated interface operation (the `plugin` tool with a typed
                // `operation`) is decided by the employee's OperationPolicy: Always
                // runs, Approval asks the owner (interactive) / refuses when
                // unattended, Blocked is refused (the toolset also omits it — this
                // is the hard backstop). Only enforced when a policy is set; with
                // none, plugin ops stay ungated (installation is the grant). The
                // workflow path enforces the same decision at its checkpoint.
                if tool_calls[idx].name == "plugin" {
                    if let (Some(policy), Some(op)) = (
                        operation_policy,
                        tool_calls[idx]
                            .input
                            .get("operation")
                            .and_then(|v| v.as_str())
                            .filter(|s| !s.is_empty()),
                    ) {
                        match policy.decide(op) {
                            tools::policy::OperationAccess::Always => {}
                            tools::policy::OperationAccess::Blocked => {
                                blocked_results[idx] = Some((
                                    tool_calls[idx].clone(),
                                    ToolResult::error(format!(
                                        "The operation '{op}' is turned OFF (Blocked) for this AI \
                                         employee in its Controls. Tell the user it's blocked and \
                                         stop — do not retry or work around it."
                                    )),
                                ));
                            }
                            tools::policy::OperationAccess::Approval => {
                                // NOTE: deliberately NO full_access bypass here. The
                                // per-employee operation policy is an explicit setting;
                                // the whole point is that a global convenience (Full
                                // Access) never overrides a per-employee gate on money/
                                // outbound/irreversible operations. decide() rules.
                                //
                                // The approval prompt must be comprehensible to a
                                // non-technical owner: require the `display` sentence
                                // (real names + formatted amounts, not ids/cents).
                                // Missing → corrective retry, never a raw-JSON prompt.
                                let display = tool_calls[idx]
                                    .input
                                    .get("display")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .trim()
                                    .to_string();
                                if display.is_empty() {
                                    blocked_results[idx] = Some((
                                        tool_calls[idx].clone(),
                                        ToolResult::error(format!(
                                            "The operation '{op}' needs the owner's approval, and \
                                             the approval prompt requires a `display` sentence. \
                                             Retry the SAME call adding display: one plain-language \
                                             sentence a non-technical person understands — real \
                                             names and formatted amounts (e.g. \"Pay Acme Supplies \
                                             $2,500.00 for bill #1042\"), never raw ids or cents."
                                        )),
                                    ));
                                } else if tools::ExecutionMode::from(origin)
                                    == tools::ExecutionMode::Interactive
                                    || approval_relay
                                {
                                    match approval_channels {
                                        Some(chs) => {
                                            let decision = ask_tool_approval(
                                                chs,
                                                tx,
                                                cancel_token,
                                                &tool_calls[idx],
                                                session_id,
                                                "operation",
                                            )
                                            .await;
                                            match decision.as_str() {
                                                "always" => {
                                                    // Approve Always → persist this op as
                                                    // Always in the employee's policy so the
                                                    // button does what it says.
                                                    if !agent_id.is_empty() {
                                                        let mut policy = operation_policy
                                                            .cloned()
                                                            .unwrap_or_default();
                                                        policy.operations.insert(
                                                            tools::plugin_tool::port_suffix(op),
                                                            tools::policy::OperationAccess::Always,
                                                        );
                                                        let patch = serde_json::json!({
                                                            "operationPolicy": policy.to_json()
                                                        });
                                                        if let Err(e) = store
                                                            .upsert_entity_config(
                                                                "agent", agent_id, &patch,
                                                            )
                                                        {
                                                            warn!(session_id, op, error = %e, "failed to persist operation Always grant");
                                                        }
                                                    }
                                                }
                                                "once" | "approve" | "approved" | "yes"
                                                | "true" => {}
                                                _ => {
                                                    blocked_results[idx] = Some((
                                                        tool_calls[idx].clone(),
                                                        ToolResult::error(format!(
                                                            "The user declined to approve the \
                                                             operation '{op}'. Tell the user it \
                                                             needs their approval and stop — do \
                                                             not retry or work around it."
                                                        )),
                                                    ));
                                                }
                                            }
                                        }
                                        None => {
                                            blocked_results[idx] = Some((
                                                tool_calls[idx].clone(),
                                                ToolResult::error(format!(
                                                    "The operation '{op}' needs approval and no \
                                                     one is available to approve it in this run. \
                                                     Report this and stop."
                                                )),
                                            ));
                                        }
                                    }
                                } else {
                                    // Unattended chat origin (cron/comm/subagent): the chat
                                    // gate can't pause. The workflow path handles suspend/
                                    // resume at its checkpoint; other unattended origins refuse.
                                    blocked_results[idx] = Some((
                                        tool_calls[idx].clone(),
                                        ToolResult::error(format!(
                                            "The operation '{op}' needs your approval and this is \
                                             an unattended run. It was not performed."
                                        )),
                                    ));
                                }
                            }
                        }
                    }
                    // `plugin` calls without a typed operation (list/discover/exec-by-
                    // slug) fall through ungated — the capability gate returns None
                    // for `plugin`.
                    continue;
                }
                let category = match tools::capabilities::gating_capability(
                    &tool_calls[idx].name,
                    &tool_calls[idx].input,
                ) {
                    Some(c) => c,
                    None => continue, // ungated (installed extension / non-ambient tool)
                };
                let cap_off = entity_permissions
                    .map(|p| p.get(category) == Some(&false))
                    .unwrap_or(false);
                // The shell command this call would run, if any (for the per-command
                // allowlist). None for non-shell tools.
                let shell_cmd = shell_command_of(&tool_calls[idx]);
                if !cap_off || full_access {
                    // Pre-granted (capability ON), no permission map, or Full Access
                    // → proceed without asking.
                    approved_cats.insert(category.to_string());
                    continue;
                }
                // Capability OFF, but this exact shell command was "approved always"
                // (matched by prefix; compound/interpreter commands never match) →
                // run without asking. Hard safeguards still apply unconditionally.
                if let Some(ref c) = shell_cmd {
                    if tools::policy::command_matches(&approved_cmds, c) {
                        approved_cats.insert(category.to_string());
                        continue;
                    }
                }
                // Capability OFF + not Full Access + not pre-approved → ask, but ONLY
                // when a human is present. Unattended runs (cron/heartbeat/workflow/
                // comm/subagent) have no one to answer, so it's denied (left ungranted
                // → Phase 1c blocks) rather than hanging on a prompt nobody sees.
                if tools::ExecutionMode::from(origin) != tools::ExecutionMode::Interactive
                    && !approval_relay
                {
                    continue;
                }
                let chs = match approval_channels {
                    Some(c) => c,
                    // No channel to ask through: leave the category ungranted so
                    // registry Phase 1c hard-blocks (safe).
                    None => continue,
                };
                let decision = ask_tool_approval(
                    chs,
                    tx,
                    cancel_token,
                    &tool_calls[idx],
                    session_id,
                    category,
                )
                .await;
                match decision.as_str() {
                    "always" => {
                        approved_cats.insert(category.to_string());
                        match &shell_cmd {
                            // Shell command → remember just this command's PREFIX
                            // (not all of Shell). Interpreters/compound commands
                            // yield None → no durable grant (approved once only).
                            Some(c) => {
                                if let Some(prefix) = tools::policy::command_prefix(c) {
                                    if !approved_cmds.iter().any(|p| p == &prefix) {
                                        approved_cmds.push(prefix.clone());
                                        if let Err(e) = store.set_approved_commands(&approved_cmds)
                                        {
                                            warn!(session_id, error = %e, "failed to persist approved command");
                                        }
                                    }
                                }
                            }
                            // Non-shell capability → grant the whole capability for
                            // next time (per-item grants aren't meaningful there).
                            None => {
                                if let Err(e) = persist_capability_grant(store, category) {
                                    warn!(session_id, category, error = %e, "failed to persist capability grant");
                                }
                            }
                        }
                    }
                    "once" | "approve" | "approved" | "yes" | "true" => {
                        approved_cats.insert(category.to_string());
                    }
                    _ => {
                        // Deny → skip execution with a clean, non-spiraling result.
                        blocked_results[idx] = Some((
                            tool_calls[idx].clone(),
                            ToolResult::error(format!(
                                "The user declined to allow this action (the \"{}\" capability \
                                 is off). Tell the user it needs their approval and stop — do \
                                 not retry or work around it.",
                                tools::capabilities::capability_label(category)
                            )),
                        ));
                    }
                }
            }
            ctx.approved_categories = approved_cats;

            // Partition tool calls into concurrent-safe and sequential phases.
            // Concurrent tools run in parallel via FuturesUnordered, then
            // sequential tools run one at a time to prevent state conflicts.
            // Beyond read-only calls, path-disjoint os file mutations are also
            // admitted to the parallel phase — see partition_tool_calls for
            // the admission rules.
            let mut live_indices = Vec::new();
            let mut partition_inputs: Vec<(&str, &serde_json::Value, bool)> = Vec::new();
            for (idx, tc) in tool_calls.iter().enumerate() {
                if blocked_results[idx].is_some() {
                    continue;
                }
                let safe = tools.is_concurrent_safe(&tc.name, &tc.input).await;
                live_indices.push(idx);
                partition_inputs.push((tc.name.as_str(), &tc.input, safe));
            }
            let (concurrent, sequential) = partition_tool_calls(&partition_inputs);
            let concurrent_indices: Vec<usize> =
                concurrent.into_iter().map(|i| live_indices[i]).collect();
            let sequential_indices: Vec<usize> =
                sequential.into_iter().map(|i| live_indices[i]).collect();

            // Phase 1: Execute concurrent-safe tools in parallel
            for &idx in &concurrent_indices {
                let tools = tools.clone();
                let ctx = ctx.clone();
                let tc = tool_calls[idx].clone();
                let concurrency = concurrency.clone();
                futures.push(async move {
                    let _permit = concurrency.acquire_tool_permit().await;
                    let input_str = tc.input.to_string();
                    let input_log = truncate_str(&input_str, 500);
                    info!(tool = %tc.name, id = %tc.id, input = %input_log, "executing tool (concurrent)");
                    let budget = tools
                        .execution_timeout(&tc.name, &tc.input)
                        .await
                        .unwrap_or(TOOL_EXECUTION_TIMEOUT);
                    let result = tokio::time::timeout(
                        budget,
                        tools.execute(&ctx, &tc.name, tc.input.clone()),
                    )
                    .await;
                    let result = match result {
                        Ok(r) => r,
                        Err(_) => ToolResult::error(format!(
                            "Tool '{}' timed out after {}s",
                            tc.name,
                            budget.as_secs()
                        )),
                    };
                    let result_log = truncate_str(&result.content, 300);
                    info!(tool = %tc.name, id = %tc.id, is_error = result.is_error, result = %result_log, "tool result");
                    (idx, tc, result)
                });
            }

            // Collect results as they complete, send events immediately
            let mut results: Vec<Option<(ai::ToolCall, ToolResult)>> = vec![None; tool_calls.len()];
            loop {
                let item = tokio::select! {
                    _ = cancel_token.cancelled() => {
                        info!(session_id, "run cancelled during tool execution");
                        return Ok(());
                    }
                    next = futures.next() => match next {
                        Some(v) => v,
                        None => break,
                    }
                };
                let (idx, tc, result) = item;
                // Send tool result event immediately as each completes
                let _ = tx
                    .send(StreamEvent { payload: result.payload.clone(),
                        provenance: None,
                        event_type: StreamEventType::ToolResult,
                        text: result.content.clone(),
                        tool_call: Some(ai::ToolCall {
                            id: tc.id.clone(),
                            name: tc.name.clone(),
                            // Carry the call's input so downstream consumers
                            // (loop tool-activity labels) can read the STRAP
                            // resource/action signature.
                            input: tc.input.clone(),
                        }),
                        error: if result.is_error {
                            Some(result.content.clone())
                        } else {
                            None
                        },
                        usage: None,
                        rate_limit: None,
                        widgets: None,
                        provider_metadata: None,
                        stop_reason: None,
                        image_url: result.image_url.clone(),
                    })
                    .await;
                results[idx] = Some((tc, result));
            }

            // Phase 2: Execute sequential (write) tools one at a time
            for &idx in &sequential_indices {
                if cancel_token.is_cancelled() {
                    info!(session_id, "run cancelled during sequential tool execution");
                    return Ok(());
                }
                let tc = tool_calls[idx].clone();
                let _permit = concurrency.acquire_tool_permit().await;
                let input_str = tc.input.to_string();
                let input_log = truncate_str(&input_str, 500);
                info!(tool = %tc.name, id = %tc.id, input = %input_log, "executing tool (sequential)");
                let budget = tools
                    .execution_timeout(&tc.name, &tc.input)
                    .await
                    .unwrap_or(TOOL_EXECUTION_TIMEOUT);
                let result = tokio::time::timeout(
                    budget,
                    tools.execute(&ctx, &tc.name, tc.input.clone()),
                )
                .await;
                let result = match result {
                    Ok(r) => r,
                    Err(_) => ToolResult::error(format!(
                        "Tool '{}' timed out after {}s",
                        tc.name,
                        budget.as_secs()
                    )),
                };
                let result_log = truncate_str(&result.content, 300);
                info!(tool = %tc.name, id = %tc.id, is_error = result.is_error, result = %result_log, "tool result");
                let _ = tx
                    .send(StreamEvent { payload: result.payload.clone(),
                        provenance: None,
                        event_type: StreamEventType::ToolResult,
                        text: result.content.clone(),
                        tool_call: Some(ai::ToolCall {
                            id: tc.id.clone(),
                            name: tc.name.clone(),
                            // Carry the call's input so downstream consumers
                            // (loop tool-activity labels) can read the STRAP
                            // resource/action signature.
                            input: tc.input.clone(),
                        }),
                        error: if result.is_error {
                            Some(result.content.clone())
                        } else {
                            None
                        },
                        usage: None,
                        rate_limit: None,
                        widgets: None,
                        provider_metadata: None,
                        stop_reason: None,
                        image_url: result.image_url.clone(),
                    })
                    .await;
                results[idx] = Some((tc, result));
            }

            // Inject blocked tool results (from pre_execute hooks).
            for (idx, blocked) in blocked_results.into_iter().enumerate() {
                if let Some((tc, result)) = blocked {
                    let _ = tx
                        .send(StreamEvent { payload: None,
                            provenance: None,
                            event_type: StreamEventType::ToolResult,
                            text: result.content.clone(),
                            tool_call: Some(ai::ToolCall {
                                id: tc.id.clone(),
                                name: tc.name.clone(),
                                input: tc.input.clone(),
                            }),
                            error: Some(result.content.clone()),
                            usage: None,
                            rate_limit: None,
                            widgets: None,
                            provider_metadata: None,
                            stop_reason: None,
                            image_url: None,
                        })
                        .await;
                    results[idx] = Some((tc, result));
                }
            }

            // Fire tool.post_execute action hooks for completed tools.
            if hooks.has_subscribers("tool.post_execute") {
                for entry in &results {
                    if let Some((tc, result)) = entry {
                        let payload = serde_json::to_vec(&crate::hooks::ToolPostExecutePayload {
                            tool_name: tc.name.clone(),
                            result: result.content.clone(),
                            is_error: result.is_error,
                            session_id: session_id.to_string(),
                        })
                        .unwrap_or_default();
                        hooks.do_action("tool.post_execute", payload).await;
                    }
                }
            }

            // Sidecar vision verification — only for providers that can't include
            // images directly in tool results. Vision-capable providers (Anthropic,
            // Gemini) get the raw image passed through instead.
            let mut had_image: Vec<usize> = Vec::new();
            {
                let main_supports_images = {
                    let prov_lock = providers.read().await;
                    prov_lock
                        .first()
                        .map_or(false, |p| p.supports_tool_result_images())
                };

                if !main_supports_images {
                    let sidecar_provider = {
                        let prov_lock = providers.read().await;
                        prov_lock.first().cloned()
                    };
                    if let Some(provider) = sidecar_provider {
                        let mut sidecar_futures = FuturesUnordered::new();

                        for (idx, entry) in results.iter().enumerate() {
                            if let Some((tc, result)) = entry {
                                if let Some(ref image_url) = result.image_url {
                                    had_image.push(idx);
                                    let image_url = image_url.clone();
                                    let action_ctx = format!("{} — {}", tc.name, result.content);
                                    let prov = provider.clone();
                                    sidecar_futures.push(async move {
                                        let verification = crate::sidecar::verify_screenshot(
                                            prov.as_ref(),
                                            &image_url,
                                            &action_ctx,
                                        )
                                        .await;
                                        (idx, verification)
                                    });
                                }
                            }
                        }

                        while let Some((idx, verification)) = tokio::select! {
                            _ = cancel_token.cancelled() => {
                                info!(session_id, "run cancelled during sidecar verification");
                                return Ok(());
                            }
                            next = sidecar_futures.next() => next
                        } {
                            if let Some((_, ref mut result)) = results[idx] {
                                match verification {
                                    Some(text) => result
                                        .content
                                        .push_str(&format!("\n\n[Screen Visual]\n{}", text)),
                                    // Sidecar couldn't describe it — still tell the model an
                                    // image exists, so it never claims "no image was returned."
                                    None => result.content.push_str(
                                        "\n\n[Screen Visual] A screenshot was captured (saved and \
                                         available to the user), but automatic description was \
                                         unavailable. Acknowledge the capture — do NOT say the tool \
                                         returned no image.",
                                    ),
                                }
                                // The main model is non-vision, so the raw image is useless to it;
                                // always drop it (otherwise the provider silently strips it and the
                                // model is left blind with no signal).
                                result.image_url = None;
                            }
                        }
                    }
                }
            }

            // For comm-origin runs, tell the model that screenshots will be
            // delivered as attachments — otherwise it has no way to know.
            if origin == tools::Origin::Comm && !had_image.is_empty() {
                for idx in &had_image {
                    if let Some((_, ref mut result)) = results[*idx] {
                        result.content.push_str(
                            "\n\n✓ Screenshot captured and will be delivered as an attachment in your reply to the user."
                        );
                    }
                }
            }

            // Duplicate file read detection: if the model re-reads a file it
            // already read this session, append a note so it knows.
            for entry in results.iter_mut().flatten() {
                if let Some(path) = extract_file_read_path(&entry.0) {
                    if !files_read_this_session.insert(path.clone()) {
                        entry.1.content.push_str(
                            "\n\n(Note: this file was already read earlier in this session)",
                        );
                    }
                }
            }

            // Save all tool results to session in deterministic order
            // and track whether ALL results in this iteration were errors.
            //
            // Context protection:
            // - Success results: 30K cap. Oversized → persist to file, return preview + path.
            // - Error results:   10K cap. Oversized → first 5K + last 5K with truncation marker.
            // - Universal:      100K hard ceiling as final safety net.
            const RESULT_CAP: usize = 30_000;
            const ERROR_CAP: usize = 10_000;
            const ERROR_HALF: usize = 5_000;
            const UNIVERSAL_TOOL_RESULT_CAP: usize = 100_000;
            let mut all_errors_this_iteration = true;
            // Per-call productivity, indexed alongside the hash push below, so the
            // identical-args guard can count only the repeats that made no progress.
            let mut unproductive_this_iteration: std::collections::HashMap<(u64, u64), bool> =
                std::collections::HashMap::new();
            let mut had_results = false;
            // Terminal tool error (auth/permission/connection) → end the turn after
            // this batch and surface to the user, instead of feeding it back for the
            // model to retry/improvise (the death-spiral fix; FRAMES.md Phase 1).
            let mut terminal_error: Option<String> = None;
            // Highest-signal rate-limit status seen this iteration (429/403) — feeds the
            // RateLimit reminder so the model backs off instead of hammer-retrying a host.
            let mut iteration_rate_limited: Option<u16> = None;
            // Lightweight snapshots for the background tool summary generator.
            let mut summary_tool_calls: Vec<ai::ToolCall> = Vec::new();
            let mut summary_tool_results: Vec<ToolResult> = Vec::new();
            for entry in results.into_iter().flatten() {
                let (tc, mut result) = entry;
                had_results = true;
                // Terminal error (auth/permission/connection) — narrow, set only by
                // ToolResult::terminal(). End the run after this batch instead of
                // letting the model retry/improvise. Critical for autonomous
                // workflows: there's no human to ask or to hit stop, so a dead
                // account must fail the run cleanly, not spiral. (FRAMES Phase 1.)
                if result.terminal && terminal_error.is_none() {
                    terminal_error = Some(result.content.clone());
                }
                if matches!(result.http_status, Some(429) | Some(403)) {
                    iteration_rate_limited = result.http_status;
                }
                // Capture pre-truncation snapshots for the summarizer (only name + short content)
                summary_tool_calls.push(tc.clone());
                summary_tool_results.push(ToolResult { payload: None,
                    content: crate::runner::truncate_str(&result.content, 300).to_string(),
                    is_error: result.is_error,
                    image_url: None,
                    http_status: None,
                    terminal: result.terminal,
                });
                if !result.is_error {
                    all_errors_this_iteration = false;
                    // A successful read clears the failure count for that target.
                    if let Some(p) = extract_file_read_path(&tc) {
                        read_failures.remove(&p);
                    }
                } else if let Some(p) = extract_file_read_path(&tc) {
                    // A failed read of a path bumps its counter — even when interleaved
                    // with successful discovery calls (which is why the all-errors
                    // counter alone misses this).
                    *read_failures.entry(p).or_insert(0) += 1;
                }

                // Empty result guard: prevent models from
                // interpreting empty tool_result as end-of-output.
                if result.content.is_empty() && !result.is_error {
                    result.content = format!("({} completed with no output)", tc.name);
                }

                // Tool-agnostic redundant-result dedup: if this result's content is
                // identical to one returned earlier this session — by ANY tool or args
                // (e.g. the same file read via os(read), then cat, then jq) — tell the
                // model it already has this instead of letting it re-fetch in a loop.
                // Hash is taken pre-truncation so it reflects the full original content.
                let mut flagged_redundant = false;
                if !result.is_error && result.content.len() > 200 {
                    let content_hash = simple_hash(result.content.as_bytes());
                    if recent_result_content_hashes.contains(&content_hash) {
                        result.content.push_str(
                            "\n\n(Note: this is identical to a result you already received earlier in this session. You already have this content — use it instead of fetching it again.)",
                        );
                        flagged_redundant = true;
                    } else {
                        recent_result_content_hashes.push(content_hash);
                        if recent_result_content_hashes.len() > 20 {
                            recent_result_content_hashes.remove(0);
                        }
                    }
                }

                // Idempotent-mutation dedup: os(write) carries its ENTIRE effect
                // (path + content) in its arguments, so a successful re-write with
                // byte-identical args leaves the file exactly as it was — no
                // progress by construction. Mutating tools are rightly exempt from
                // no-progress rules in general (re-running a build after editing
                // its input is legitimate), but that exemption let the
                // promise → write-same-JSON → rebuild-same-deck spiral run for an
                // hour: every cycle "succeeded", every counter stayed at zero.
                // Flagging it as redundant feeds the same downstream guards as a
                // redundant read (spiral counter, no-progress ledger, 3-strike
                // identical-args block).
                if !flagged_redundant
                    && !result.is_error
                    && tc.name == "os"
                    && tc.input.get("action").and_then(|v| v.as_str()) == Some("write")
                {
                    let nh = simple_hash(tc.name.as_bytes());
                    let ah = simple_hash(tc.input.to_string().as_bytes());
                    if recent_tool_result_hashes
                        .iter()
                        .any(|&(n, a, _, unproductive)| n == nh && a == ah && !unproductive)
                    {
                        result.content.push_str(
                            "\n\n(Note: this wrote byte-identical content to the same path as an earlier write this turn — the file is unchanged. If you meant to expand or modify it, actually change the content before writing again.)",
                        );
                        flagged_redundant = true;
                    }
                }

                // Spiral backstop counter: only UNPRODUCTIVE attempts count. A call
                // that errored or returned content the model already had is a
                // wander-loop step (glob-wander / browser re-read / shell-retry); a
                // call that succeeded with a NOVEL result made progress. Counting
                // successes cut legitimate bulk work off at 8 (e.g. creating N
                // distinct todos, writing N files) — the false-trip this guard's own
                // comment warned about. File-read errors are excluded here — the
                // per-path read_failures map already stops same-target retry spirals;
                // counting cross-path failures toward os:read false-tripped codebase
                // exploration after 8 misses.
                record_action_spiral(
                    &mut action_call_counts,
                    &tc,
                    result.is_error,
                    flagged_redundant,
                );

                // Remember whether THIS call made progress, keyed the same way the
                // identical-args guard looks calls up. A call that succeeded with a
                // novel result is progress and must never count toward a block.
                //
                // For a READ-ONLY call, "novel" is checked against its own previous
                // result under the same arguments: a browse that answers the same
                // "no resources found" for the twentieth time is a loop even though
                // every response was a success. (The general content-dedup above
                // ignores results under 200 chars, which is exactly the size of
                // such answers.) A mutating call is judged only by errors —
                // re-running a build after editing its input legitimately repeats
                // the same args AND the same "Created: <path>" result.
                let call_key = (
                    simple_hash(tc.name.as_bytes()),
                    simple_hash(tc.input.to_string().as_bytes()),
                );
                let mut no_progress = result.is_error || flagged_redundant;
                // Read-only per the registry's own classifier — the same verdict
                // the concurrency scheduler trusts, so there is exactly one
                // definition of "this call has no side effects".
                if !no_progress && tools.is_concurrent_safe(&tc.name, &tc.input).await {
                    let own_hash = simple_hash(result.content.as_bytes());
                    if readonly_result_hash_by_call.get(&call_key) == Some(&own_hash) {
                        no_progress = true;
                    }
                    readonly_result_hash_by_call.insert(call_key, own_hash);
                }
                unproductive_this_iteration.insert(call_key, no_progress);

                // Arg-identity dedup (complements the content check above, which only
                // fires on byte-identical output): the model repeated a call it already
                // made this turn — same tool, identical arguments. Results that drift
                // slightly (mtime ordering, timestamps) slip past the content hash, so
                // flag the repeated CALL itself. The 3+ hard guard still blocks loops;
                // this annotates the second call so it never gets that far.
                if !flagged_redundant {
                    let nh = simple_hash(tc.name.as_bytes());
                    let ah = simple_hash(tc.input.to_string().as_bytes());
                    if recent_tool_result_hashes
                        .iter()
                        .any(|&(n, a, _, _)| n == nh && a == ah)
                    {
                        result.content.push_str(
                            "\n\n(Note: you already made this exact call — same tool, same arguments — earlier this turn. Reuse results you already have instead of repeating calls.)",
                        );
                    }
                }

                // Error truncation: first 5K + last 5K with marker
                if result.is_error && result.content.len() > ERROR_CAP {
                    let total_len = result.content.len();
                    let first = truncate_str(&result.content, ERROR_HALF).to_string();
                    let last_start = result.content.len().saturating_sub(ERROR_HALF);
                    // Find char boundary for the tail
                    let mut tail_start = last_start;
                    while tail_start < result.content.len()
                        && !result.content.is_char_boundary(tail_start)
                    {
                        tail_start += 1;
                    }
                    let last = &result.content[tail_start..];
                    result.content = format!(
                        "{}\n\n[{} characters truncated]\n\n{}",
                        first,
                        total_len - first.len() - last.len(),
                        last
                    );
                }

                // Success result truncation: persist to file, return preview + path
                if !result.is_error && result.content.len() > RESULT_CAP {
                    let total_len = result.content.len();
                    // Persist full result to temp file so agent can Read it if needed
                    let result_id = uuid::Uuid::new_v4().to_string();
                    #[cfg(not(windows))]
                    let result_dir = std::path::PathBuf::from("/tmp/nebo-tool-results");
                    // Windows: no /tmp — use the real temp dir (matches the
                    // pathres /tmp mapping the read-back path goes through).
                    #[cfg(windows)]
                    let result_dir = std::env::temp_dir().join("nebo-tool-results");
                    let result_dir = result_dir.as_path();
                    let _ = std::fs::create_dir_all(result_dir);
                    let result_path = result_dir.join(format!("{}.txt", result_id));
                    if let Err(e) = std::fs::write(&result_path, &result.content) {
                        warn!(error = %e, "failed to persist large tool result");
                    }
                    let preview = truncate_str(&result.content, 4_000);
                    // Guidance matters: models follow it literally. Telling them to
                    // "read the file" re-inhales the whole payload into context —
                    // observed live with a 99KB tool list read straight back in.
                    // Point at targeted search, with full reads as the exception.
                    result.content = format!(
                        "{}\n\n[Output too large ({} chars); preview above. Full output saved to: {}. Search it with os(resource: \"file\", action: \"grep\", path: \"{}\", pattern: \"...\") — extract only what you need; avoid reading the whole file into context. For broad exploration of it, delegate to a subagent and keep only the conclusions.]",
                        preview,
                        total_len,
                        result_path.display(),
                        result_path.display()
                    );
                }

                // Universal hard ceiling as final safety net
                if result.content.len() > UNIVERSAL_TOOL_RESULT_CAP {
                    let total_len = result.content.len();
                    let preview = truncate_str(&result.content, 4_000);
                    result.content = format!(
                        "{}\n\n[Result truncated: {} chars total, showing first 4000. Re-run the tool with narrower parameters.]",
                        preview, total_len
                    );
                }
                // Log tool_search discoveries (activation happens via message-window
                // scanning on the next iteration — no persistent set needed)
                if tc.name == "tool_search" && !result.is_error {
                    if let Ok(search) = serde_json::from_str::<serde_json::Value>(&result.content) {
                        if let Some(matches) = search.get("matches").and_then(|v| v.as_array()) {
                            let names: Vec<&str> =
                                matches.iter().filter_map(|m| m.as_str()).collect();
                            if !names.is_empty() {
                                debug!(tools = ?names, "tool_search discovered tools (active next turn)");
                            }
                        }
                    }
                }

                // Voluntary skill save: the model updated its library on its
                // own — push the self-improvement review backstop out (the
                // review only fires when organic learning has stalled).
                if !result.is_error && tc.name == "skill" {
                    let action = tc.input.get("action").and_then(|v| v.as_str()).unwrap_or("");
                    if matches!(action, "create" | "update") {
                        crate::review_fork::note_voluntary_save(session_id);
                    }
                }

                // Cache tool documentation results so they survive sliding window eviction.
                // Detect help/schema actions on skill and plugin tools.
                if !result.is_error && result.content.len() > 100 {
                    if let Some(cache_key) = detect_tool_doc_call(&tc.name, &tc.input) {
                        let content = if result.content.len() > MAX_TOOL_DOC_CONTENT {
                            truncate_str(&result.content, MAX_TOOL_DOC_CONTENT).to_string()
                        } else {
                            result.content.clone()
                        };
                        // Remove existing entry with same key (LRU refresh)
                        tool_doc_cache.retain(|(k, _)| k != &cache_key);
                        // Evict oldest if at capacity
                        if tool_doc_cache.len() >= MAX_TOOL_DOC_ENTRIES {
                            tool_doc_cache.remove(0);
                        }
                        tool_doc_cache.push((cache_key.clone(), content));
                        debug!(key = %cache_key, "cached tool documentation");
                    }
                }

                let row = ToolResultRow {
                    tool_call_id: tc.id.clone(),
                    content: result.content,
                    is_error: result.is_error,
                    image_url: result.image_url,
                    payload: result.payload,
                };
                let tr_json = serde_json::json!([row]).to_string();

                if let Err(e) =
                    sessions.append_message(session_id, "tool", "", None, Some(&tr_json), None)
                {
                    warn!(session_id = %session_id, error = %e, "failed to save tool message to DB");
                }
            }

            // Terminal tool error → end the run now (FRAMES Phase 1). The failure is
            // unrecoverable (auth/permission/connection) — surface it and stop rather
            // than feed it back for the model to retry/improvise. This is the
            // death-spiral fix: in an autonomous workflow there is no human to ask or
            // to interrupt, so a dead account must stop the run cleanly. (Narrow: only
            // ToolResult::terminal sets this — healthy long-running tasks never trip it.)
            //
            // Typed termination: emitted as a ControlNotice status event, never Text —
            // the old emit-text-then-break made the notice indistinguishable from
            // assistant prose and it leaked verbatim into channel replies.
            if let Some(msg) = terminal_error {
                warn!(session_id, iteration, "terminal tool error — ending run");
                turn_exit_reason = "terminal_tool_error".to_string();
                let _ = tx
                    .send(StreamEvent::control_notice(msg, "terminal_tool_error"))
                    .await;
                break;
            }

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
                recent_tool_names.push(tc.name.clone());
                // Engine-stamped provenance: union this call's classes into
                // the run's taint set (static table; model-invisible).
                {
                    let mut taint = run_taint.lock().unwrap();
                    for class in crate::provenance::classify_tool(&tc.name, &tc.input) {
                        taint.insert(class);
                    }
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
                if let Some(reminder) = steering::select_reminder(&rctx, &mut reminder_cadence) {
                    pending_stream_reminders.push(reminder);
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
            // contained an explicit memory write (agent resource:"memory" action:"store").
            // Re-extracting would duplicate facts the model just wrote.
            if !skip_memory {
                for tc in &tool_calls {
                    if tc.name == "agent" {
                        let resource = tc
                            .input
                            .get("resource")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let action = tc
                            .input
                            .get("action")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        if resource == "memory" && action == "store" {
                            debug!(
                                session_id,
                                "memory write detected — skipping post-run extraction"
                            );
                            skip_memory = true;
                            break;
                        }
                    }
                }
            }

            // Pattern 13: background tool summary generation via cheap model.
            // Spawns a fire-and-forget task that calls the cheapest provider to
            // generate a one-line label for the UX showing what the agent did.
            {
                let prov_lock = providers.read().await;
                let prov_snapshot: Vec<Arc<dyn Provider>> = prov_lock.clone();
                drop(prov_lock);
                let summary_tx = tx.clone();
                let summary_assistant = assistant_content.clone();
                let summary_tcs = summary_tool_calls;
                let summary_trs = summary_tool_results;
                tokio::spawn(async move {
                    if let Some(summary) = crate::summarizer::summarize_tool_batch(
                        &prov_snapshot,
                        &summary_tcs,
                        &summary_trs,
                        &summary_assistant,
                    )
                    .await
                    {
                        let _ = summary_tx.send(StreamEvent::tool_summary(summary)).await;
                    }
                });
            }

            // Reset post-tool nudge flag after successful tool execution
            // so it can fire again if the model goes empty on a later tool round.
            post_tool_empty_nudges = 0;

            // Continue loop — LLM needs to respond to tool results
            continue;
        }

        // Output token escalation: on first truncation, retry with a higher cap
        // before falling through to the multi-attempt continuation recovery.
        if (stop_reason.as_deref() == Some("length")
            || stop_reason.as_deref() == Some("max_tokens"))
            && !output_escalated
        {
            info!(
                iteration,
                session_id,
                "output truncated at {}K tokens, retrying with {}K",
                DEFAULT_MAX_OUTPUT_TOKENS / 1024,
                ESCALATED_MAX_OUTPUT_TOKENS / 1024,
            );
            output_escalated = true;
            continue;
        }

        // Max output tokens recovery: if response was truncated, force continuation
        if stop_reason.as_deref() == Some("length") || stop_reason.as_deref() == Some("max_tokens")
        {
            if output_recovery_attempts < MAX_OUTPUT_RECOVERY_ATTEMPTS {
                output_recovery_attempts += 1;
                info!(
                    iteration,
                    session_id,
                    attempt = output_recovery_attempts,
                    "max output tokens recovery"
                );
                // Continuation rides the next call as an ephemeral reminder
                // after the (already persisted, line ~2740) truncated turn.
                pending_stream_reminders.push(steering::wrap_system_reminder(
                    "Your previous response was cut off by the output token limit. \
                     Resume directly from where you stopped — no recap, no apology. \
                     If you had pending tool calls, make them now.",
                ));
                continue;
            }
        }
        // Reset recovery counter and escalation flag on successful non-truncated completion
        if stop_reason.as_deref() != Some("length") && stop_reason.as_deref() != Some("max_tokens")
        {
            output_recovery_attempts = 0;
            output_escalated = false;
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
            if empty_content_retries < MAX_EMPTY_CONTENT_RETRIES {
                empty_content_retries += 1;
                warn!(
                    iteration,
                    session_id,
                    retry = empty_content_retries,
                    "empty response — retrying"
                );
                continue;
            }

            // Exhausted retries — output "(empty)" and break.
            turn_exit_reason = "empty_response_exhausted".to_string();
            warn!(
                iteration,
                session_id,
                "empty response after {} retries — giving up",
                MAX_EMPTY_CONTENT_RETRIES
            );
            let _ = sessions.append_message(session_id, "assistant", "(empty)", None, None, None);
            let _ = tx.send(StreamEvent::text("(empty)".to_string())).await;
            break;
        }

        // Reset retry counter on successful non-empty content (read on next loop iteration)
        #[allow(unused_assignments)]
        {
            empty_content_retries = 0;
        }

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

        // Contradictory stop: the provider says the model stopped TO CALL TOOLS,
        // but no tool calls were parsed from the stream — the payload was lost
        // in transit (observed live with Janus: stop_reason="tool_calls",
        // tool_call_count=0). Ending the turn here strands the user with only
        // the preamble text; retry the iteration instead.
        let stop_says_tools = matches!(stop_reason.as_deref(), Some("tool_calls" | "tool_use"));
        if stop_says_tools && tool_calls.is_empty() && lost_toolcall_retries < 2 {
            lost_toolcall_retries += 1;
            warn!(
                iteration,
                session_id,
                attempt = lost_toolcall_retries,
                "stop_reason says tool_calls but none were parsed — retrying iteration"
            );
            pending_stream_reminders.push(steering::wrap_system_reminder(
                "Your previous response ended as if calling tools, but no tool \
                 calls arrived. Make the tool calls now — do not re-introduce \
                 the task.",
            ));
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

        // Conversation turn complete — normal exit with text response
        turn_exit_reason = format!("text_response(stop_reason={:?})", stop_reason);
        info!(iteration, session_id, exit_reason = %turn_exit_reason, "agentic loop complete");
        break;
    }

    // Post-loop: budget exhaustion summary request.
    // If the loop exited because we hit max_iterations without a final text response,
    // make ONE more API call with tools stripped to get a summary.
    if final_iteration >= max_iterations && !turn_exit_reason.starts_with("text_response") {
        // Only request summary if the last message is a tool result (mid-task exit)
        let last_msg_is_tool = sessions
            .get_messages(session_id)
            .unwrap_or_default()
            .last()
            .map(|m| m.role == "tool")
            .unwrap_or(false);
        if last_msg_is_tool {
            turn_exit_reason = format!(
                "max_iterations_reached({}/{})",
                final_iteration, max_iterations
            );
            info!(session_id, exit_reason = %turn_exit_reason, "budget exhausted — requesting summary");

            // Append a user message requesting summary, then make one toolless API call
            let _ = sessions.append_message(
                session_id, "user",
                "You've reached the maximum number of tool-calling iterations allowed. \
                 Please provide a final response summarizing what you've found and accomplished so far, \
                 without calling any more tools.",
                None, None, None,
            );

            // Pick first available provider for the summary call
            let prov_lock = providers.read().await;
            if let Some(summary_provider) = prov_lock.first() {
                let summary_messages =
                    convert_messages(&sessions.get_messages(session_id).unwrap_or_default());

                let summary_req = ChatRequest {
                    tool_choice: Default::default(),
                    messages: summary_messages,
                    tools: vec![], // No tools — text-only response
                    max_tokens: 4096,
                    temperature: 0.7,
                    system: static_system.clone(),
                    static_system: static_system.clone(),
                    model: last_model_name.clone(),
                    enable_thinking: false,
                    metadata: sticky_metadata.clone(),
                    cache_breakpoints: vec![],
                    cancel_token: Some(cancel_token.clone()),
                    trace: None,
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

    // Debounced memory extraction: only runs after 5s idle per session.
    // Extract from last exchange only (last user msg + assistant response + tool
    // calls) to avoid re-extracting facts from old messages and creating duplicates.
    let has_providers = !providers.read().await.is_empty();
    let final_taint: Vec<types::provenance::ProvenanceClass> =
        run_taint.lock().unwrap().iter().copied().collect();
    let extraction_barred = final_taint.iter().any(|c| memory_write_bar.contains(c));
    if extraction_barred {
        info!(
            session_id,
            classes = %types::provenance::label_classes(&final_taint),
            "memory extraction barred by scope write bar"
        );
    }
    if !skip_memory && has_providers && !extraction_barred {
        let all_msgs = sessions.get_messages(session_id).unwrap_or_default();
        // Find the last user message and take everything from there onward.
        let last_exchange: Vec<_> = {
            let last_user_idx = all_msgs.iter().rposition(|m| m.role == "user");
            match last_user_idx {
                Some(idx) => all_msgs[idx..].to_vec(),
                None => vec![],
            }
        };
        if last_exchange.len() >= 2 {
            use crate::memory_debounce::MemoryDebouncer;
            use std::sync::OnceLock;
            static DEBOUNCER: OnceLock<MemoryDebouncer> = OnceLock::new();
            let debouncer = DEBOUNCER.get_or_init(MemoryDebouncer::default);

            let providers = providers.clone();
            let store = store.clone();
            let mem_uid = memory_user_id.clone();
            let session_id_owned = session_id.to_string();
            let embed_prov = embedding_provider.cloned();
            let topics = memory_topics.clone();
            let taint = final_taint.clone();

            debouncer
                .schedule(session_id, move || async move {
                    let resolved = {
                        let prov_lock = providers.read().await;
                        resolve_aux(&config::ModelsConfig::load(), &prov_lock)
                            .or_else(|| prefer_non_gateway(&prov_lock).map(|p| (p, String::new())))
                    };
                    if let Some((provider, aux_model)) = resolved {
                        if let Some(facts) = memory::extract_facts(
                            provider.as_ref(),
                            &last_exchange,
                            Some(&store),
                            Some(&mem_uid),
                            &topics,
                            &aux_model,
                        )
                        .await
                        {
                            memory::store_facts(
                                &store, &facts, &mem_uid, embed_prov, &topics, &taint,
                            );
                            debug!(
                                session_id = session_id_owned,
                                "extracted and stored memory facts"
                            );
                        }
                    }
                })
                .await;
        }
    }

    // Background personality synthesis: if enough style observations exist,
    // synthesize a personality directive. Runs at most once per run (spawned
    // as a background task so it doesn't block the response).
    if !skip_memory {
        let store_clone = store.clone();
        let providers_clone = providers.clone();
        let uid = memory_user_id.clone();
        let conc = concurrency.clone();
        let handle = tokio::spawn(async move {
            let _permit = conc.acquire_llm_permit().await;
            let prov = prefer_non_gateway(&providers_clone.read().await);
            if let Some(prov) = prov {
                crate::personality::synthesize_directive(&store_clone, prov.as_ref(), &uid).await;
            }
        });
        crate::memory_flush::track_extraction(handle).await;
    }

    // The run becomes a record: what it cost, and (later, per role) what it
    // achieved. This is the ONE write point — chat, workflow and heartbeat
    // runs all pass through this loop, so persisting here covers every run
    // type without a second writer per caller. Best-effort like the timeline:
    // a failed insert is logged loudly, never allowed to fail a finished run.
    // (Runs that end in an error return earlier and are not yet recorded —
    // their cost is real, and wiring the error exits is deliberate follow-up
    // rather than a silent partial number today.)
    record_run_usage(
        store,
        selector,
        agent_id,
        session_id,
        &last_model_name,
        &state,
    );

    Ok(())
}

/// Persists the finished run's usage. Cost is computed from models.yaml
/// pricing at write time; a model with no pricing records zero rather than a
/// wrong number — a silently invented figure is worse than a visibly missing
/// one, because this number ends up on an invoice.
fn record_run_usage(
    store: &Arc<Store>,
    selector: &ModelSelector,
    agent_id: &str,
    session_id: &str,
    model_name: &str,
    state: &RunState,
) {
    if state.total_input_tokens == 0 && state.total_output_tokens == 0 {
        // Nothing was spent — a run that never reached a provider (immediate
        // cancellation, empty prompt) has no cost to record.
        return;
    }

    let (run_type, run_id) = classify_run(session_id);

    let mut cost: i64 = 0;
    if let Some(info) = selector.get_model_info(model_name) {
        cost = db::cost_microcents(
            state.total_input_tokens as i64,
            state.total_output_tokens as i64,
            state.total_cache_read_tokens as i64,
            state.total_cache_creation_tokens as i64,
            info.input_price,
            info.output_price,
            info.cached_input_price,
        );
    }

    let entry = db::models::RunUsageEntry {
        agent_id: agent_id.to_string(),
        session_key: Some(session_id.to_string()),
        run_id,
        run_type: run_type.to_string(),
        model_id: model_name.to_string(),
        input_tokens: state.total_input_tokens as i64,
        output_tokens: state.total_output_tokens as i64,
        cache_read_tokens: state.total_cache_read_tokens as i64,
        cache_creation_tokens: state.total_cache_creation_tokens as i64,
        cost_microcents: cost,
        outcome: None,
    };
    if let Err(e) = store.record_run_usage(&entry) {
        // Loudly: this row is money. But the work is already done, and
        // failing a finished run over its receipt would be worse.
        tracing::error!(session_id, error = %e, "failed to record run usage");
    }
}

/// classify_run derives what kind of run a session key names, and for the
/// canonical workflow form, which workflow run it was — the join that lets
/// "what did this workflow cost" be answered at all.
fn classify_run(session_id: &str) -> (&'static str, Option<String>) {
    if session_id.starts_with("heartbeat-") {
        return ("heartbeat", None);
    }
    if let Some(idx) = session_id.find(":workflow:") {
        let run_id = &session_id[idx + ":workflow:".len()..];
        if !run_id.is_empty() {
            return ("workflow", Some(run_id.to_string()));
        }
        return ("workflow", None);
    }
    // The engine's legacy key ("workflow-{def}-{run}") is ambiguous — both
    // segments may contain hyphens — so it classifies without a join rather
    // than guessing a wrong id into a money table.
    if session_id.starts_with("workflow-") {
        return ("workflow", None);
    }
    ("chat", None)
}

/// Load workspace context from `.nebo.md` or `NEBO.md`.
/// Walks up from CWD to git root (or home dir), returns the first match.
fn load_context_file() -> Option<String> {
    let cwd = std::env::current_dir().ok()?;
    let mut dir = cwd.as_path();

    loop {
        for name in &[".nebo.md", "NEBO.md"] {
            let path = dir.join(name);
            if path.is_file() {
                match std::fs::read_to_string(&path) {
                    Ok(content) => {
                        let sanitized = crate::sanitize::sanitize_for_prompt(&content);
                        debug!(path = %path.display(), "loaded workspace context file");
                        return Some(sanitized);
                    }
                    Err(e) => {
                        warn!(path = %path.display(), error = %e, "failed to read context file");
                    }
                }
            }
        }

        // Stop at git root
        if dir.join(".git").exists() {
            break;
        }

        // Walk up
        match dir.parent() {
            Some(parent) if parent != dir => dir = parent,
            _ => break,
        }
    }

    None
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

/// Detect if a tool call is requesting documentation (help/schema).
/// Returns a cache key like "skill:gws-sheets" or "plugin:sheets:help" if so.
fn detect_tool_doc_call(tool_name: &str, input: &serde_json::Value) -> Option<String> {
    let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");
    let resource = input.get("resource").and_then(|v| v.as_str()).unwrap_or("");

    match tool_name {
        "skill" => {
            if action == "help" || action == "list" || action == "docs" {
                let skill_name = input
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                Some(format!("skill:{}", skill_name))
            } else {
                None
            }
        }
        "plugin" => {
            if action == "help" || action == "schema" || action == "services" {
                let name = if !resource.is_empty() {
                    resource
                } else {
                    input
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                };
                Some(format!("plugin:{}:{}", name, action))
            } else {
                None
            }
        }
        // MCP tool documentation
        "mcp" => {
            if action == "help" || action == "list" || action == "schema" {
                let server = input
                    .get("server")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                Some(format!("mcp:{}:{}", server, action))
            } else {
                None
            }
        }
        _ => None,
    }
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

/// Check if recent user messages contain imperative language demanding action.
/// Serves as an implicit active-task signal for auto-continuation when objective
/// detection hasn't run yet.
fn user_demanded_action(messages: &[ChatMessage]) -> bool {
    let imperative_patterns = [
        "do it",
        "just do it",
        "get it done",
        "finish it",
        "keep going",
        "don't stop",
        "dont stop",
        "do not stop",
        "handle it",
        "do them all",
        "go ahead",
        "get them done",
        "do them",
        "finish them",
        "just go",
        "proceed",
        "continue",
        "keep at it",
        "do the rest",
        "all of them",
        "finish all",
        "complete all",
        "process all",
        "handle all",
        "work through all",
        "why did you stop",
        "why are you stopping",
    ];
    // Check last 5 user messages (was 2) to catch demands that scroll off
    let recent_user: Vec<&ChatMessage> = messages
        .iter()
        .rev()
        .filter(|m| {
            m.role == "user"
                && !m.content.starts_with("<system>")
                && !m.content.starts_with("<steering")
        })
        .take(5)
        .collect();

    for msg in &recent_user {
        let lower = msg.content.to_lowercase();
        // Match short-to-medium imperative messages (raised from 120 to 200)
        if lower.len() < 200 {
            for p in &imperative_patterns {
                if lower.contains(p) {
                    return true;
                }
            }
        }
    }
    false
}

/// Convert database ChatMessages to ai::Messages for the provider.
/// Detect a prompt that IS an explicit invocation of a declared tool —
/// "use os(resource: ...)", "call web(...)", or the bare "skill(...)" — and
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

fn convert_messages(messages: &[ChatMessage]) -> Vec<Message> {
    messages
        .iter()
        .filter_map(|msg| {
            // Skip empty messages
            if msg.content.is_empty()
                && msg.tool_calls.as_ref().map_or(true, |tc| tc.is_empty())
                && msg.tool_results.as_ref().map_or(true, |tr| tr.is_empty())
            {
                return None;
            }

            let tool_calls = msg.tool_calls.as_ref().and_then(|tc| {
                if tc.is_empty() || tc == "[]" || tc == "null" {
                    None
                } else {
                    serde_json::from_str::<serde_json::Value>(tc).ok()
                }
            });

            let tool_results = msg.tool_results.as_ref().and_then(|tr| {
                if tr.is_empty() || tr == "[]" || tr == "null" {
                    None
                } else {
                    serde_json::from_str::<serde_json::Value>(tr).ok()
                }
            });

            let images = msg
                .metadata
                .as_ref()
                .and_then(|m| serde_json::from_str::<serde_json::Value>(m).ok())
                .and_then(|v| v.get("images").cloned())
                .and_then(|v| serde_json::from_value::<Vec<ai::ImageContent>>(v).ok());

            Some(Message {
                role: msg.role.clone(),
                content: msg.content.clone(),
                tool_calls,
                tool_results,
                images,
            })
        })
        .collect()
}

/// Sanitize message ordering: ensure tool results immediately follow their
/// corresponding assistant message. Self-heals corrupted session data
/// (back-to-back assistants, out-of-order tool results) that strict providers
/// like GPT-5-mini reject. Also strips orphaned tool results that reference
/// tool_call_ids not found in any preceding assistant message (matches Go's
/// sanitizeAgentMessages).
fn sanitize_message_order(messages: Vec<ChatMessage>) -> Vec<ChatMessage> {
    if messages.is_empty() {
        return messages;
    }

    // Phase 1: Collect all tool_call_ids issued by assistant messages
    let mut issued_call_ids = HashSet::new();
    for msg in &messages {
        if msg.role == "assistant" {
            if let Some(ref tc_json) = msg.tool_calls {
                if let Ok(calls) = serde_json::from_str::<Vec<serde_json::Value>>(tc_json) {
                    for call in &calls {
                        if let Some(id) = call.get("id").and_then(|v| v.as_str()) {
                            issued_call_ids.insert(id.to_string());
                        }
                    }
                }
            }
        }
    }

    // Phase 2: Map tool_call_id → tool result message for reordering.
    // Each tool message in DB has a single-element tool_results array.
    // Track which message indices are tool messages to skip in output.
    let mut tool_result_map: HashMap<String, ChatMessage> = HashMap::new();
    let mut tool_msg_indices = HashSet::new();
    let mut orphaned = 0u32;

    for (i, msg) in messages.iter().enumerate() {
        if msg.role != "tool" {
            continue;
        }
        if let Some(ref tr_json) = msg.tool_results {
            if let Ok(results) = serde_json::from_str::<Vec<serde_json::Value>>(tr_json) {
                let mut valid_results = Vec::new();
                for r in &results {
                    let tcid = r.get("tool_call_id").and_then(|v| v.as_str()).unwrap_or("");
                    if tcid.is_empty() || !issued_call_ids.contains(tcid) {
                        orphaned += 1;
                        continue;
                    }
                    valid_results.push((tcid.to_string(), r.clone()));
                }

                if !valid_results.is_empty() {
                    tool_msg_indices.insert(i);
                    for (tcid, result_val) in valid_results {
                        let single_tr = serde_json::json!([result_val]).to_string();
                        tool_result_map.insert(
                            tcid,
                            ChatMessage {
                                id: msg.id.clone(),
                                chat_id: msg.chat_id.clone(),
                                role: "tool".to_string(),
                                content: msg.content.clone(),
                                metadata: msg.metadata.clone(),
                                created_at: msg.created_at,
                                day_marker: msg.day_marker.clone(),
                                tool_calls: None,
                                tool_results: Some(single_tr),
                                token_estimate: msg.token_estimate,
                                html: None,
                            },
                        );
                    }
                } else if orphaned > 0 {
                    // All results in this message were orphaned — skip entire message
                    tool_msg_indices.insert(i);
                }
            }
        }
    }

    // Phase 3: Rebuild with tool results injected after their assistant
    let mut result = Vec::with_capacity(messages.len());
    let mut reordered = 0u32;
    let mut orphaned_uses = 0u32;

    for (i, msg) in messages.into_iter().enumerate() {
        if tool_msg_indices.contains(&i) {
            continue;
        }
        let has_tool_calls = msg.role == "assistant" && msg.tool_calls.is_some();
        let tc_json = msg.tool_calls.clone();
        result.push(msg);

        if has_tool_calls {
            if let Some(ref tc) = tc_json {
                if let Ok(calls) = serde_json::from_str::<Vec<serde_json::Value>>(tc) {
                    for call in &calls {
                        if let Some(id) = call.get("id").and_then(|v| v.as_str()) {
                            if let Some(tool_msg) = tool_result_map.remove(id) {
                                reordered += 1;
                                result.push(tool_msg);
                            } else {
                                // Orphaned tool_use: no matching tool_result exists.
                                // Inject a synthetic result so strict providers
                                // (Anthropic, GPT) don't reject the conversation.
                                orphaned_uses += 1;
                                let synthetic = serde_json::json!([{
                                    "tool_call_id": id,
                                    "content": "[Tool result unavailable]"
                                }]);
                                result.push(ChatMessage {
                                    id: String::new(),
                                    chat_id: String::new(),
                                    role: "tool".to_string(),
                                    content: "[Tool result unavailable]".to_string(),
                                    metadata: None,
                                    created_at: chrono::Utc::now().timestamp(),
                                    day_marker: None,
                                    tool_calls: None,
                                    tool_results: Some(synthetic.to_string()),
                                    token_estimate: Some(0),
                                    html: None,
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    if reordered > 0 {
        debug!(
            reordered,
            "reordered tool results for correct message ordering"
        );
    }
    if orphaned > 0 {
        debug!(orphaned, "stripped orphaned tool results");
    }
    if orphaned_uses > 0 {
        debug!(
            orphaned_uses,
            "injected synthetic results for orphaned tool_use blocks"
        );
    }
    // Drop any remaining unmatched results — they're double orphans
    let unmatched = tool_result_map.len();
    if unmatched > 0 {
        debug!(unmatched, "dropped unmatched tool results");
    }

    result
}

/// Detect user's working objective from latest message.
/// Runs as a background task (fire-and-forget) before the main loop.
async fn detect_objective(
    providers: &Arc<RwLock<Vec<Arc<dyn Provider>>>>,
    sessions: &SessionManager,
    session_id: &str,
    user_prompt: &str,
) {
    if user_prompt.is_empty() {
        return;
    }

    let provider = {
        let prov_lock = providers.read().await;
        prefer_non_gateway(&prov_lock)
    };
    let provider = match provider {
        Some(p) => p,
        None => return,
    };

    let current_objective = sessions.get_active_task(session_id).unwrap_or_default();
    let obj_display = if current_objective.is_empty() {
        "none".to_string()
    } else {
        current_objective.clone()
    };

    // Gather recent conversation context (last 6 messages) for better classification
    let recent_context = sessions
        .get_messages(session_id)
        .ok()
        .map(|msgs| {
            let recent: Vec<String> = msgs
                .iter()
                .rev()
                .take(6)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .filter(|m| m.role == "user" || m.role == "assistant")
                .map(|m| {
                    let content = if m.content.len() > 200 {
                        format!("{}...", truncate_str(&m.content, 200))
                    } else {
                        m.content.clone()
                    };
                    format!("[{}]: {}", m.role, content)
                })
                .collect();
            recent.join("\n")
        })
        .unwrap_or_default();

    let classify_prompt = format!(
        r#"You are classifying whether a user has started a NEW task or is continuing their current one.

Current objective: {obj}
Recent conversation:
{context}
Latest user message: {msg}

Respond with ONLY one JSON line, no markdown fences:
{{"action": "set", "objective": "concise 1-sentence objective", "mode": "normal"}}
OR {{"action": "update", "objective": "refined objective incorporating the addition", "mode": "normal"}}
OR {{"action": "clear"}}
OR {{"action": "keep"}}

The "mode" field (required for "set" and "update") classifies HOW the agent should work:
- "research" — the user wants multi-source investigation: comparing options, finding deals, evaluating alternatives, gathering information from multiple websites. The agent should use parallel sub-agents for coverage.
- "normal" — everything else: direct actions, conversations, single lookups, creative tasks.

## Decision rules (in priority order):

1. **TOPIC CHANGE → "set"**: If the user's message introduces a DIFFERENT subject, domain, or goal than the current objective, this is a new task. People don't announce "I'm starting a new task" — they just start talking about something else.
   - Current: "fix the login bug" → User: "can you help me write tests for the API" → **set** (different area)
   - Current: "build the dashboard" → User: "let's work on the deployment pipeline" → **set** (different system)
   - Current: "optimize the search query" → User: "I need to update the README" → **set** (different task entirely)

2. **CONTINUATION / REFINEMENT → "update"**: The user is adding scope or adjusting the SAME objective.
   - Current: "build user auth" → User: "also add password reset" → **update** (same domain, added scope)
   - Current: "fix the API" → User: "and add rate limiting while you're at it" → **update** (same area, added requirement)

3. **COMPLETION SIGNALS → "clear"**: The user signals the task is done with no new goal.
   - "thanks", "looks good", "perfect", "that's it", "never mind", "done"

4. **SAME TASK INTERACTION → "keep"**: Questions, feedback, or corrections that are clearly ABOUT the current objective.
   - Current: "fix the login bug" → User: "what's causing the null pointer?" → **keep** (investigating same bug)
   - Current: "build the dashboard" → User: "use a bar chart instead" → **keep** (feedback on same work)

5. **When objective is "none"**: Any message with an action or request → **"set"**. Pure greetings or questions with no task → **"keep"**.

## Key principle: When in doubt between "set" and "keep", prefer "set". A stale objective that doesn't match what the user actually wants is MORE harmful than resetting. The user can always continue the old task, but they can't unstick an agent that's persisting on a finished objective."#,
        obj = obj_display,
        context = if recent_context.is_empty() {
            "(no prior messages)".to_string()
        } else {
            recent_context
        },
        msg = user_prompt
    );

    let req = ChatRequest {
        tool_choice: Default::default(),
        messages: vec![Message {
            role: "user".to_string(),
            content: classify_prompt,
            ..Default::default()
        }],
        tools: vec![],
        max_tokens: 256,
        temperature: 0.0,
        system: String::new(),
        static_system: String::new(),
        model: String::new(),
        enable_thinking: false,
        metadata: None,
        cache_breakpoints: vec![],
        cancel_token: None,
        trace: None,
    };

    let stream_result = provider.stream(&req).await;

    let mut rx = match stream_result {
        Ok(rx) => rx,
        Err(e) => {
            debug!(error = %e, "objective detection failed");
            return;
        }
    };

    let mut resp = String::new();
    while let Some(event) = rx.recv().await {
        if event.event_type == StreamEventType::Text {
            resp.push_str(&event.text);
        }
        if event.event_type == StreamEventType::Error {
            return;
        }
    }

    // Strip markdown fences and parse
    let resp = resp.trim();
    let resp = resp.trim_start_matches("```json").trim_start_matches("```");
    let resp = resp.trim_end_matches("```").trim();

    #[derive(serde::Deserialize)]
    struct ObjectiveResult {
        action: String,
        #[serde(default)]
        objective: String,
        #[serde(default)]
        mode: String,
    }

    let result: ObjectiveResult = match serde_json::from_str(resp) {
        Ok(r) => r,
        Err(e) => {
            debug!(error = %e, response = resp, "objective parse failed");
            return;
        }
    };

    match result.action.as_str() {
        "set" if !result.objective.is_empty() => {
            info!(objective = %result.objective, mode = %result.mode, "objective set");
            let _ = sessions.set_active_task(session_id, &result.objective);
            sessions.set_detected_mode(session_id, &result.mode);
        }
        "update" if !result.objective.is_empty() => {
            info!(objective = %result.objective, mode = %result.mode, "objective updated");
            let _ = sessions.set_active_task(session_id, &result.objective);
            if !result.mode.is_empty() {
                sessions.set_detected_mode(session_id, &result.mode);
            }
        }
        "clear" => {
            info!("objective cleared");
            let _ = sessions.clear_active_task(session_id);
            sessions.set_detected_mode(session_id, "");
        }
        "keep" | _ => {
            // No change
        }
    }
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
fn simple_hash(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &byte in data {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Ceiling on path-admitted file mutations in one parallel batch. Read-only
/// calls don't count against it — their parallelism is unchanged from the
/// pre-path-scoping behavior and total simultaneous tool execution is already
/// throttled by the shared tool-permit semaphore (`ConcurrencyControl`,
/// min 8). This cap only bounds the newly-admitted parallel writes; the
/// excess spills to the sequential phase.
const MAX_CONCURRENT_FILE_MUTATIONS: usize = 8;

/// Decide which of a turn's tool calls run in the parallel phase.
///
/// `calls` holds one `(tool_name, input, concurrent_safe)` tuple per call,
/// where `concurrent_safe` is the registry's `is_concurrent_safe` verdict.
/// Returns `(concurrent, sequential)` index lists into `calls`, each
/// preserving the original call order.
///
/// Admission rules — strictly additive over the plain safe/unsafe split:
/// - `concurrent_safe` calls are admitted exactly as before.
/// - an os file mutation (write/edit/delete/move/copy — see
///   [`tools::registry::file_mutation_paths`]) is admitted iff all of its
///   canonical target paths (source, plus destination for move/copy) are
///   disjoint — not equal to, not an ancestor of, not a descendant of — every
///   path reserved by a mutation already admitted in this batch, and fewer
///   than [`MAX_CONCURRENT_FILE_MUTATIONS`] mutations have been admitted.
/// - everything else (overlapping or unparseable paths, non-file mutations)
///   stays sequential, preserving original relative order.
fn partition_tool_calls(calls: &[(&str, &serde_json::Value, bool)]) -> (Vec<usize>, Vec<usize>) {
    let mut concurrent = Vec::new();
    let mut sequential = Vec::new();
    // Canonical paths reserved by mutations admitted so far in this batch.
    let mut reserved: Vec<std::path::PathBuf> = Vec::new();
    let mut admitted_mutations = 0usize;
    for (i, (name, input, safe)) in calls.iter().enumerate() {
        if *safe {
            // Read-only per the registry — runs in parallel exactly as before.
            concurrent.push(i);
            continue;
        }
        // Path-scoped admission: a file mutation may join the parallel phase
        // when its target paths don't overlap anything already reserved.
        if admitted_mutations < MAX_CONCURRENT_FILE_MUTATIONS {
            if let Some(paths) = tools::registry::file_mutation_paths(name, input) {
                let disjoint = paths
                    .iter()
                    .all(|p| reserved.iter().all(|r| !paths_overlap(p, r)));
                if disjoint {
                    reserved.extend(paths);
                    admitted_mutations += 1;
                    concurrent.push(i);
                    continue;
                }
            }
        }
        sequential.push(i);
    }
    (concurrent, sequential)
}

/// Two canonical paths overlap when they are equal or one contains the other
/// (ancestor/descendant). Mutations to overlapping paths must not race.
fn paths_overlap(a: &std::path::Path, b: &std::path::Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
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
        let tools = defs(&["os", "skill", "mcp__nebo_kb__memory_recall"]);
        for p in [
            r#"use os(resource: "app", action: "list")"#,
            r#"os(resource: "shell", action: "exec", command: "ls")"#,
            r#"call skill(action: "catalog")"#,
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
        let tools = defs(&["os", "skill"]);
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
mod tests {
    use super::*;


    /// Minimal provider stub for resolve_aux tests (only id() matters).
    struct StubProvider(&'static str);

    #[async_trait::async_trait]
    impl Provider for StubProvider {
        fn id(&self) -> &str {
            self.0
        }
        async fn stream(
            &self,
            _req: &ChatRequest,
        ) -> Result<ai::EventReceiver, ProviderError> {
            Err(ProviderError::Request("stub".into()))
        }
    }

    fn aux_config(aux: &str) -> config::ModelsConfig {
        config::ModelsConfig {
            version: "1.0".into(),
            defaults: None,
            task_routing: Some(config::models::TaskRouting {
                vision: String::new(),
                audio: String::new(),
                reasoning: String::new(),
                code: String::new(),
                general: String::new(),
                aux: aux.to_string(),
                fallbacks: std::collections::HashMap::new(),
            }),
            lane_routing: None,
            aliases: vec![],
            providers: std::collections::HashMap::new(),
            cli_providers: vec![],
        }
    }

    #[test]
    fn test_resolve_aux_unset_returns_none() {
        let providers: Vec<Arc<dyn Provider>> = vec![Arc::new(StubProvider("anthropic"))];
        // Empty aux field → fallback.
        assert!(resolve_aux(&aux_config(""), &providers).is_none());
        // No task_routing at all → fallback.
        let mut cfg = aux_config("");
        cfg.task_routing = None;
        assert!(resolve_aux(&cfg, &providers).is_none());
    }

    #[test]
    fn test_resolve_aux_provider_missing_returns_none() {
        let providers: Vec<Arc<dyn Provider>> = vec![Arc::new(StubProvider("anthropic"))];
        let cfg = aux_config("openai/gpt-4o-mini");
        assert!(resolve_aux(&cfg, &providers).is_none());
    }

    #[test]
    fn test_resolve_aux_malformed_spec_returns_none() {
        // Bare model id without a provider prefix cannot be routed → fallback.
        let providers: Vec<Arc<dyn Provider>> = vec![Arc::new(StubProvider("anthropic"))];
        assert!(resolve_aux(&aux_config("gpt-4o-mini"), &providers).is_none());
    }

    #[test]
    fn test_resolve_aux_set_and_available_routes() {
        let providers: Vec<Arc<dyn Provider>> = vec![
            Arc::new(StubProvider("anthropic")),
            Arc::new(StubProvider("openai")),
        ];
        let cfg = aux_config("openai/gpt-4o-mini");
        let (provider, model) = resolve_aux(&cfg, &providers).expect("aux route should resolve");
        assert_eq!(provider.id(), "openai");
        assert_eq!(model, "gpt-4o-mini");
    }

    #[test]
    fn test_convert_messages() {
        let messages = vec![
            ChatMessage {
                id: "1".into(),
                chat_id: "c".into(),
                role: "user".into(),
                content: "hello".into(),
                metadata: None,
                created_at: 0,
                day_marker: None,
                tool_calls: None,
                tool_results: None,
                token_estimate: None,
                html: None,
            },
            ChatMessage {
                id: "2".into(),
                chat_id: "c".into(),
                role: "assistant".into(),
                content: "hi there".into(),
                metadata: None,
                created_at: 0,
                day_marker: None,
                tool_calls: None,
                tool_results: None,
                token_estimate: None,
                html: None,
            },
        ];

        let result = convert_messages(&messages);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].role, "user");
        assert_eq!(result[1].role, "assistant");
    }

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

    #[test]
    fn test_partition_disjoint_writes_run_concurrently() {
        let w1 = serde_json::json!({"resource": "file", "action": "write", "path": "/a/b.txt"});
        let w2 = serde_json::json!({"resource": "file", "action": "write", "path": "/a/c.txt"});
        let calls = vec![("os", &w1, false), ("os", &w2, false)];
        let (concurrent, sequential) = partition_tool_calls(&calls);
        assert_eq!(concurrent, vec![0, 1], "disjoint writes both join the parallel phase");
        assert!(sequential.is_empty());
    }

    #[test]
    fn test_partition_same_path_write_stays_sequential() {
        let w1 = serde_json::json!({"resource": "file", "action": "write", "path": "/a/b.txt"});
        let w2 = serde_json::json!({"resource": "file", "action": "write", "path": "/a/b.txt"});
        let calls = vec![("os", &w1, false), ("os", &w2, false)];
        let (concurrent, sequential) = partition_tool_calls(&calls);
        assert_eq!(concurrent, vec![0]);
        assert_eq!(sequential, vec![1], "second write to the same path is sequential");
    }

    #[test]
    fn test_partition_ancestor_overlap_stays_sequential() {
        let w1 = serde_json::json!({"resource": "file", "action": "write", "path": "/a"});
        let w2 = serde_json::json!({"resource": "file", "action": "write", "path": "/a/c.txt"});
        let calls = vec![("os", &w1, false), ("os", &w2, false)];
        let (concurrent, sequential) = partition_tool_calls(&calls);
        assert_eq!(concurrent, vec![0]);
        assert_eq!(sequential, vec![1], "write under an already-reserved dir is sequential");
    }

    #[test]
    fn test_partition_non_file_mutation_stays_sequential() {
        let shell = serde_json::json!({"resource": "shell", "action": "exec", "command": "ls"});
        let calls = vec![("os", &shell, false)];
        let (concurrent, sequential) = partition_tool_calls(&calls);
        assert!(concurrent.is_empty(), "non-file mutations never join the parallel phase");
        assert_eq!(sequential, vec![0]);
    }

    #[test]
    fn test_partition_reads_unchanged_and_order_preserved() {
        // Reads (concurrent_safe=true) are admitted as before, even when a
        // conflicting write is forced sequential; both phases keep original order.
        let r1 = serde_json::json!({"resource": "file", "action": "read", "path": "/a/b.txt"});
        let w1 = serde_json::json!({"resource": "file", "action": "write", "path": "/x/y.txt"});
        let w2 = serde_json::json!({"resource": "file", "action": "write", "path": "/x/y.txt"});
        let r2 = serde_json::json!({"action": "search", "query": "q"});
        let calls = vec![
            ("os", &r1, true),
            ("os", &w1, false),
            ("os", &w2, false),
            ("web", &r2, true),
        ];
        let (concurrent, sequential) = partition_tool_calls(&calls);
        assert_eq!(concurrent, vec![0, 1, 3]);
        assert_eq!(sequential, vec![2]);
    }

    #[test]
    fn test_partition_move_reserves_destination() {
        let mv = serde_json::json!({
            "resource": "file", "action": "move", "path": "/src/a.txt", "destination": "/dst/a.txt"
        });
        let w = serde_json::json!({"resource": "file", "action": "write", "path": "/dst/a.txt"});
        let calls = vec![("os", &mv, false), ("os", &w, false)];
        let (concurrent, sequential) = partition_tool_calls(&calls);
        assert_eq!(concurrent, vec![0]);
        assert_eq!(sequential, vec![1], "write to a move's destination is sequential");
    }

    #[test]
    fn test_partition_mutation_cap_spills_to_sequential() {
        let inputs: Vec<serde_json::Value> = (0..10)
            .map(|i| {
                serde_json::json!({"resource": "file", "action": "write", "path": format!("/a/f{i}.txt")})
            })
            .collect();
        let calls: Vec<(&str, &serde_json::Value, bool)> =
            inputs.iter().map(|input| ("os", input, false)).collect();
        let (concurrent, sequential) = partition_tool_calls(&calls);
        assert_eq!(concurrent.len(), MAX_CONCURRENT_FILE_MUTATIONS);
        assert_eq!(sequential, vec![8, 9], "excess past the cap spills to sequential");
    }

    fn make_msg(id: &str, role: &str, content: &str) -> ChatMessage {
        ChatMessage {
            id: id.into(),
            chat_id: "c".into(),
            role: role.into(),
            content: content.into(),
            metadata: None,
            created_at: 0,
            day_marker: None,
            tool_calls: None,
            tool_results: None,
            token_estimate: None,
            html: None,
        }
    }

    fn os_read(path: &str) -> ai::ToolCall {
        ai::ToolCall {
            id: "c1".into(),
            name: "os".into(),
            input: serde_json::json!({"action": "read", "path": path}),
        }
    }

    fn os_glob(dir: &str) -> ai::ToolCall {
        ai::ToolCall {
            id: "c1".into(),
            name: "os".into(),
            input: serde_json::json!({"action": "glob", "path": dir, "pattern": "*"}),
        }
    }

    fn os_exec(command: &str) -> ai::ToolCall {
        ai::ToolCall {
            id: "c1".into(),
            name: "os".into(),
            input: serde_json::json!({"action": "exec", "command": command}),
        }
    }

    fn web_search(query: &str) -> ai::ToolCall {
        ai::ToolCall {
            id: "c1".into(),
            name: "web".into(),
            input: serde_json::json!({"action": "search", "query": query}),
        }
    }

    #[test]
    fn spiral_exploration_read_errors_never_trip_limit() {
        // Regression: 8+ failed os:reads across different paths used to end the
        // turn with "os:read was called 8 times without progress" and block the
        // next real read. Per-path read_failures owns that case; the coarse
        // spiral must stay at 0 so exploration can continue.
        let mut counts = std::collections::HashMap::new();
        for i in 0..(SAME_ACTION_LIMIT + 4) {
            record_action_spiral(&mut counts, &os_read(&format!("/tmp/miss-{i}.rs")), true, false);
        }
        assert_eq!(counts.get("os:read").copied().unwrap_or(0), 0);
        assert!(
            counts.get("os:read").copied().unwrap_or(0) < SAME_ACTION_LIMIT,
            "exploration read errors must not trip the spiral backstop"
        );
    }

    #[test]
    fn spiral_redundant_reads_still_trip_limit() {
        let mut counts = std::collections::HashMap::new();
        let call = os_read("/tmp/same.rs");
        for _ in 0..SAME_ACTION_LIMIT {
            record_action_spiral(&mut counts, &call, false, true);
        }
        assert_eq!(counts["os:read"], SAME_ACTION_LIMIT);
    }

    #[test]
    fn spiral_skips_file_read_errors_across_paths() {
        // Exploring many missing paths must not feed the coarse os:read counter —
        // read_failures owns per-path caps.
        let a = os_read("/tmp/a.rs");
        let b = os_read("/tmp/b.rs");
        assert!(!counts_toward_action_spiral(&a, true, false));
        assert!(!counts_toward_action_spiral(&b, true, false));
        // Redundant content still counts (true wander).
        assert!(counts_toward_action_spiral(&a, false, true));
        // Successful novel read never counts.
        assert!(!counts_toward_action_spiral(&a, false, false));
        // Error + redundant: redundant still counts (wander via re-fetch).
        assert!(counts_toward_action_spiral(&a, true, true));
    }

    #[test]
    fn spiral_skips_shell_dump_read_errors() {
        // cat/head/tail dump failures are file-reads for dedup / read_failures;
        // they must not feed the coarse os:exec spiral either.
        let cat = os_exec("cat /tmp/missing.rs");
        assert!(extract_file_read_path(&cat).is_some());
        assert!(!counts_toward_action_spiral(&cat, true, false));
    }

    #[test]
    fn spiral_still_counts_non_read_errors() {
        let g = os_glob("/tmp");
        assert!(counts_toward_action_spiral(&g, true, false));
        assert!(!counts_toward_action_spiral(&g, false, false));

        let mut counts = std::collections::HashMap::new();
        for i in 0..SAME_ACTION_LIMIT {
            record_action_spiral(&mut counts, &os_glob(&format!("/tmp/dir-{i}")), true, false);
        }
        assert_eq!(counts["os:glob"], SAME_ACTION_LIMIT);

        // Non-dump shell failures still count (true exec retry spiral).
        let ls = os_exec("ls /nope");
        assert!(extract_file_read_path(&ls).is_none());
        assert!(counts_toward_action_spiral(&ls, true, false));

        // Other tools' errors still count.
        assert!(counts_toward_action_spiral(&web_search("nebo"), true, false));
    }

    /// The repeated-action backstop is a nudge: it refuses the offending call and
    /// lets the turn continue. Making it terminal again would resurrect the dead
    /// turns users saw as "Stopped: … called 8 times without progress".
    #[test]
    fn spiral_backstop_is_a_nudge_not_a_stop() {
        let src = include_str!("runner.rs");
        let block = src
            .split("racked up SAME_ACTION_LIMIT UNPRODUCTIVE attempts")
            .nth(1)
            .expect("spiral backstop block");
        let block = &block[..block.find("\n            // ──").unwrap_or(block.len())];
        assert!(
            !block.contains("ToolResult::terminal"),
            "spiral backstop must not end the run — use ToolResult::error"
        );
        assert!(
            block.contains("action_call_counts.insert(key.clone(), 0)"),
            "the budget must reset when the nudge fires, or the action is locked out for the turn"
        );
    }

    /// The plugin tool carries no `action` — its verb is the head of `command`.
    /// Distinct commands must land in distinct buckets, or a turn that ran eight
    /// different plugin commands trips the backstop as one retried call.
    #[test]
    fn action_key_separates_plugin_commands() {
        let call = |cmd: &str| ai::ToolCall {
            id: String::new(),
            name: "plugin".into(),
            input: serde_json::json!({"resource": "gws", "command": cmd}),
        };
        assert_eq!(action_key(&call("drive +upload --path /a/b")), "plugin:drive +upload");
        assert_ne!(
            action_key(&call("drive +upload --path /a/b")),
            action_key(&call("gmail +send --to a@b.c"))
        );
        // Tools that do carry an action are unchanged.
        assert_eq!(action_key(&os_glob("/tmp")), "os:glob");
    }

    #[test]
    fn spiral_os_read_without_path_still_counts_as_error() {
        // Malformed read (no path) is not a tracked file-read target — keep it
        // on the coarse spiral so a broken call shape cannot loop forever.
        let bare = ai::ToolCall {
            id: "c1".into(),
            name: "os".into(),
            input: serde_json::json!({"action": "read"}),
        };
        assert!(extract_file_read_path(&bare).is_none());
        assert!(counts_toward_action_spiral(&bare, true, false));
    }

    #[test]
    fn test_sanitize_preserves_correct_order() {
        // Already correct: assistant → tool → assistant → tool
        let msg1 = make_msg("1", "user", "hello");
        let mut msg2 = make_msg("2", "assistant", "let me help");
        msg2.tool_calls = Some(r#"[{"id":"call_1","name":"web","input":{}}]"#.into());
        let mut msg3 = make_msg("3", "tool", "");
        msg3.tool_results =
            Some(r#"[{"tool_call_id":"call_1","content":"result","is_error":false}]"#.into());
        let msg4 = make_msg("4", "assistant", "done");

        let result = sanitize_message_order(vec![msg1, msg2, msg3, msg4]);
        assert_eq!(result.len(), 4);
        assert_eq!(result[0].role, "user");
        assert_eq!(result[1].role, "assistant");
        assert_eq!(result[2].role, "tool");
        assert_eq!(result[3].role, "assistant");
    }

    #[test]
    fn test_sanitize_reorders_back_to_back_assistants() {
        // Broken: assistant, assistant, tool(for #2), tool(for #1)
        let msg1 = make_msg("1", "user", "hello");
        let mut msg2 = make_msg("2", "assistant", "calling web");
        msg2.tool_calls = Some(r#"[{"id":"call_A","name":"web","input":{}}]"#.into());
        let mut msg3 = make_msg("3", "assistant", "calling system");
        msg3.tool_calls = Some(r#"[{"id":"call_B","name":"system","input":{}}]"#.into());
        let mut msg4 = make_msg("4", "tool", "");
        msg4.tool_results =
            Some(r#"[{"tool_call_id":"call_B","content":"sys result","is_error":false}]"#.into());
        let mut msg5 = make_msg("5", "tool", "");
        msg5.tool_results =
            Some(r#"[{"tool_call_id":"call_A","content":"web result","is_error":false}]"#.into());

        let result = sanitize_message_order(vec![msg1, msg2, msg3, msg4, msg5]);

        // Expected: user, assistant(A), tool(A), assistant(B), tool(B)
        assert_eq!(result.len(), 5);
        assert_eq!(result[0].role, "user");
        assert_eq!(result[1].role, "assistant"); // call_A
        assert_eq!(result[2].role, "tool"); // result for call_A
        assert!(result[2].tool_results.as_ref().unwrap().contains("call_A"));
        assert_eq!(result[3].role, "assistant"); // call_B
        assert_eq!(result[4].role, "tool"); // result for call_B
        assert!(result[4].tool_results.as_ref().unwrap().contains("call_B"));
    }

    #[test]
    fn test_sanitize_strips_orphaned_tool_results() {
        // Tool result references a call_id that no assistant ever issued
        let msg1 = make_msg("1", "user", "hello");
        let mut msg2 = make_msg("2", "tool", "");
        msg2.tool_results = Some(
            r#"[{"tool_call_id":"call_ORPHAN","content":"orphaned","is_error":false}]"#.into(),
        );
        let msg3 = make_msg("3", "assistant", "hi");

        let result = sanitize_message_order(vec![msg1, msg2, msg3]);
        // Orphaned tool message should be stripped
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].role, "user");
        assert_eq!(result[1].role, "assistant");
    }

    // classify_run feeds a money table: a wrong run_id joins someone's cost
    // to the wrong workflow, so the parse gets a check rather than a comment.
    #[test]
    fn classify_run_reads_every_session_key_shape() {
        // Canonical workflow key carries the run id for the cost join.
        assert_eq!(
            classify_run("agent:abc:workflow:run-123-xyz"),
            ("workflow", Some("run-123-xyz".to_string()))
        );
        // The legacy engine key is ambiguous (both segments may contain
        // hyphens) — classified, but never a guessed id in a money table.
        assert_eq!(classify_run("workflow-def-1-run-2"), ("workflow", None));
        assert_eq!(classify_run("heartbeat-agent-42"), ("heartbeat", None));
        assert_eq!(classify_run("agent:abc:desktop"), ("chat", None));
        assert_eq!(classify_run("subagent:parent:child"), ("chat", None));
        // A truncated workflow key must not record an empty-string id.
        assert_eq!(classify_run("agent:abc:workflow:"), ("workflow", None));
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
