pub mod engine;
pub mod loop_contract;
pub mod events;
mod graph;
pub mod loader;
pub mod parser;
pub mod triggers;

pub use engine::{WorkflowProgress, execute_activity, execute_workflow};
pub use loop_contract::{ActivityLoop, LoopOutcome, LoopTurn};
pub use parser::{Activity, WorkflowDef};

#[derive(Debug, thiserror::Error)]
pub enum WorkflowError {
    #[error("parse error: {0}")]
    Parse(String),
    #[error("validation error: {0}")]
    Validation(String),
    #[error("missing dependency: {0}")]
    MissingDependency(String),
    #[error("unresolved interface: {0}")]
    UnresolvedInterface(String),
    #[error("activity {0} exceeded max iterations")]
    MaxIterations(String),
    #[error("activity {activity_id} exceeded token budget ({used}/{limit})")]
    BudgetExceeded {
        activity_id: String,
        used: u32,
        limit: u32,
    },
    #[error("activity {0} failed: {1}")]
    ActivityFailed(String, String),
    #[error("workflow not found: {0}")]
    NotFound(String),
    #[error("database error: {0}")]
    Database(String),
    #[error("provider error: {0}")]
    Provider(String),
    /// Workflow exited early by agent decision — not a failure.
    #[error("workflow exited: {0}")]
    Exited(String),
    /// A tool returned a terminal error (auth expired, account not connected,
    /// permission off — see FRAMES.md): the run cannot do its job and retrying
    /// or improvising won't help. Unlike `Exited`, this IS a failure — it must
    /// surface to the owner, not read as a clean stop.
    #[error("blocked: {0}")]
    Blocked(String),
    #[error("workflow cancelled")]
    Cancelled,
    #[error("runaway call loop: {0}")]
    RunawayLoop(String),
    /// The run reached a gated operation whose per-employee policy says
    /// "Needs approval": it SUSPENDED at the checkpoint (state persisted in
    /// workflow_run_suspensions, run status `awaiting_approval`) and waits for
    /// the owner's decision. Not a failure — the manager notifies the owner
    /// and the run resumes (or aborts) via the approval endpoint.
    #[error("awaiting owner approval for operation: {operation}")]
    AwaitingApproval { operation: String, display: String },
    #[error("circuit breaker tripped: {0}")]
    CircuitBreak(String),
    #[error("{0}")]
    Other(String),
}

impl From<types::NeboError> for WorkflowError {
    fn from(e: types::NeboError) -> Self {
        WorkflowError::Database(e.to_string())
    }
}
