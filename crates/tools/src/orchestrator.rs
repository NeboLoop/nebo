use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

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

/// What `send` did with a message.
#[derive(Debug, Clone)]
pub enum FollowUp {
    /// The sub-agent was running: the message is in its thread and it hears
    /// it at its next step. Its result arrives the way it was spawned to
    /// report (a wake for a background spawn).
    Delivered { task_id: String },
    /// The sub-agent had finished: it ran again with the message as its next
    /// turn.
    Continued(SpawnResult),
}

/// Trait implemented by agent::Orchestrator, consumed by tools::AgentTool.
/// Uses Pin<Box<dyn Future>> for object safety (async_trait alternative).
pub trait SubAgentOrchestrator: Send + Sync {
    /// Spawn a single sub-agent.
    fn spawn(
        &self,
        req: SpawnRequest,
    ) -> Pin<Box<dyn Future<Output = Result<SpawnResult, String>> + Send + '_>>;

    /// Decompose a complex task into a DAG and execute it.
    ///
    /// `model_override` is the resolved "provider/model" of the run that asked
    /// for the decomposition — the same string `SpawnRequest.model_override`
    /// carries, so a DAG's children run at the conversation's model instead of
    /// each falling back to the global default. Empty = let the selector pick.
    fn execute_dag(
        &self,
        prompt: &str,
        user_id: &str,
        parent_session_id: &str,
        model_override: &str,
        parent_cancel: Option<CancellationToken>,
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

    /// Send a sub-agent a message. A running one hears it at its next step
    /// and reports the way it was spawned to (`FollowUp::Delivered`); a
    /// finished one keeps its session (everything it read and did), runs again
    /// the way it was spawned with the message as its next turn, and answers
    /// the same way (`FollowUp::Continued`). `from_session_key` and `taint`
    /// are the sender's: a running child records where the message came from
    /// and takes on its taint. Fails once a finished child's context has been
    /// released.
    fn send(
        &self,
        task_id: &str,
        message: &str,
        from_session_key: &str,
        taint: Vec<types::provenance::ProvenanceClass>,
        parent_cancel: Option<CancellationToken>,
        parent_stream_tx: Option<mpsc::Sender<ai::StreamEvent>>,
    ) -> Pin<Box<dyn Future<Output = Result<FollowUp, String>> + Send + '_>>;

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
