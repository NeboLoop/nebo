use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// The authority a spawn inherits from the run that asked for it.
///
/// A sub-agent runs at its parent's authority and never above it. Before this
/// was carried, a spawn was the most privileged context in the process: every
/// child got `Origin::System` (which `is_trusted()`) and no policy, so a
/// comm- or visitor-driven parent whose gated operations were floored to
/// `Approval` produced a child with no floor at all, and a restricted parent
/// produced a child with an unrestricted toolset.
///
/// Build it with [`SpawnAuthority::of`] rather than field by field — the whole
/// point is that one call carries every part of it, so a new spawn pathway
/// cannot inherit half.
#[derive(Debug, Clone)]
pub struct SpawnAuthority {
    /// The origin the child decides gated operations against. This is the
    /// parent's GATE origin, not necessarily the origin it arrived on.
    pub origin: crate::Origin,
    /// The spawning employee's per-operation policy. `None` only when the
    /// parent itself had none.
    pub operation_policy: Option<crate::policy::OperationPolicy>,
    /// The spawning run's restricted-run allowlist: a spawn must not be the way
    /// a restricted run gets an unrestricted toolset. `None` for a normal run.
    pub tool_allowlist: Option<std::collections::HashSet<String>>,
    /// The denial text that goes with that allowlist — it teaches the recovery
    /// for the run's actual situation, and the child is in the same situation.
    pub tool_denial_hint: Option<String>,
}

impl SpawnAuthority {
    /// Read the authority off the tool context that asked for the spawn.
    ///
    /// The origin is the GATE origin (`policy::gate_origin`): a run carrying
    /// tainted inputs hands its child the `Comm` floor rather than the
    /// nominally-trusted origin it arrived on, so the child of a tainted
    /// workflow cannot decide a gated `Always` the parent could not.
    pub fn of(ctx: &crate::ToolContext) -> Self {
        Self {
            origin: crate::policy::gate_origin(ctx.origin, ctx.tainted),
            operation_policy: ctx.operation_policy.clone(),
            tool_allowlist: ctx.tool_whitelist.clone(),
            tool_denial_hint: ctx.whitelist_denial_hint.clone(),
        }
    }
}

/// Request to spawn a single sub-agent or execute a DAG.
#[derive(Debug, Clone)]
pub struct SpawnRequest {
    pub prompt: String,
    pub description: String,
    pub agent_type: String,
    pub model_override: String,
    pub parent_session_id: String,
    pub parent_session_key: String,
    pub user_id: String,
    pub wait: bool,
    /// Parent's cancellation token — sub-agents derive a child token from this
    /// so that cancelling the parent cascades to all children.
    pub parent_cancel: Option<CancellationToken>,
    /// Maximum agentic loop iterations for this sub-agent (0 = default 100).
    pub max_iterations: usize,
    /// Skill names to pre-load into the sub-agent's context. Full SKILL.md
    /// content is injected so the sub-agent has instructions without needing
    /// to discover/load them itself. Keeps the parent context lean.
    pub skills: Vec<String>,
    /// Plugin install codes the sub-agent should have access to. Plugin docs
    /// and capabilities are injected into the sub-agent's system prompt so it
    /// knows how to use them from turn 1.
    pub plugins: Vec<String>,
    /// STRAP domain tool names the sub-agent needs (e.g. "web", "loop", "message").
    /// The corresponding STRAP doc is injected so the sub-agent knows the tool's
    /// resources, actions, and usage patterns.
    pub tools: Vec<String>,
    /// Parent's stream sender — forwarded to sub-agents so that `AskRequest`
    /// events reach the user's WebSocket (permission forwarding).
    pub parent_stream_tx: Option<mpsc::Sender<ai::StreamEvent>>,
    /// Parent run's agent-to-agent hop count — inherited so a sub-agent cannot
    /// restart the coworker chain cap at zero.
    pub handoff_depth: u8,
    /// The authority this spawn inherits — see [`SpawnAuthority`].
    pub authority: SpawnAuthority,
    /// spawn_parallel only: "worktree" gives each child its own copy of the
    /// project (a git worktree when `workspace` is a repo, a scratch copy
    /// otherwise) and merges the results back. Empty = share the tree.
    pub isolate: String,
    /// The project folder to isolate (empty = the process cwd).
    pub workspace: String,
}

/// Result from a sub-agent or DAG execution.
#[derive(Debug, Clone)]
pub struct SpawnResult {
    pub task_id: String,
    pub success: bool,
    pub output: String,
    pub error: Option<String>,
}

/// Trait implemented by agent::Orchestrator, consumed by tools::AgentTool.
/// Uses Pin<Box<dyn Future>> for object safety (async_trait alternative).
pub trait SubAgentOrchestrator: Send + Sync {
    /// Spawn a single sub-agent.
    fn spawn(
        &self,
        req: SpawnRequest,
    ) -> Pin<Box<dyn Future<Output = Result<SpawnResult, String>> + Send + '_>>;

    /// Decompose a complex task into a DAG and execute it. `authority` is the
    /// spawning run's — every task the DAG produces inherits it, so a
    /// decomposed task is no more privileged than the run that asked for it.
    fn execute_dag(
        &self,
        prompt: &str,
        user_id: &str,
        parent_session_id: &str,
        parent_cancel: Option<CancellationToken>,
        authority: SpawnAuthority,
    ) -> Pin<Box<dyn Future<Output = Result<SpawnResult, String>> + Send + '_>>;

    /// Cancel a running sub-agent or DAG task.
    fn cancel(
        &self,
        task_id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>>;

    /// Get the status of a sub-agent task.
    fn status(
        &self,
        task_id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + '_>>;

    /// Continue a finished sub-agent with a follow-up. It keeps its session
    /// (everything it read and did), runs again the way it was spawned, and
    /// answers the same way. Fails while it is still running, or once its
    /// context has been released.
    fn send(
        &self,
        task_id: &str,
        message: &str,
        parent_cancel: Option<CancellationToken>,
        parent_stream_tx: Option<mpsc::Sender<ai::StreamEvent>>,
    ) -> Pin<Box<dyn Future<Output = Result<SpawnResult, String>> + Send + '_>>;

    /// List all active sub-agents: (task_id, description, status).
    fn list_active(
        &self,
    ) -> Pin<Box<dyn Future<Output = Vec<(String, String, String)>> + Send + '_>>;

    /// Spawn multiple sub-agents in parallel and wait for all to complete.
    /// Progress updates are sent via the progress_tx channel.
    fn spawn_parallel(
        &self,
        requests: Vec<SpawnRequest>,
        progress_tx: mpsc::Sender<ai::StreamEvent>,
    ) -> Pin<Box<dyn Future<Output = Result<SpawnResult, String>> + Send + '_>>;

    /// Recover incomplete tasks from a previous crash.
    fn recover(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

/// Late-binding handle for the orchestrator.
/// Created empty before Runner exists, filled after Runner is built.
pub type OrchestratorHandle = Arc<OnceLock<Box<dyn SubAgentOrchestrator>>>;

/// Create a new empty orchestrator handle.
pub fn new_handle() -> OrchestratorHandle {
    Arc::new(OnceLock::new())
}
