pub mod events;
pub mod loader;
pub mod parser;
pub mod engine;
pub mod triggers;

pub use parser::{WorkflowDef, Activity};
pub use engine::{execute_workflow, execute_activity};

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
    #[error("workflow cancelled")]
    Cancelled,
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
