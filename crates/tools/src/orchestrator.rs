use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};

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

    /// Decompose a complex task into a DAG and execute it.
    fn execute_dag(
        &self,
        prompt: &str,
        user_id: &str,
        parent_session_id: &str,
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

    /// List all active sub-agents: (task_id, description, status).
    fn list_active(
        &self,
    ) -> Pin<Box<dyn Future<Output = Vec<(String, String, String)>> + Send + '_>>;

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
