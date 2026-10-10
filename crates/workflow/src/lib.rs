pub mod cases;
pub mod engine;
pub mod loop_contract;
pub mod events;
pub mod expert;
mod graph;
pub mod loader;
pub mod parser;
pub mod triggers;

pub use engine::{RUNTIME_TOOLS, WorkflowProgress, enforced_tools, execute_activity, execute_workflow};
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
    /// One step used every model turn it had without finishing. `step` is
    /// (1-based step, steps in the activity) for an activity with steps.
    #[error("{}", step_cap_message(activity_id, *step, *turns))]
    MaxIterations {
        activity_id: String,
        step: Option<(usize, usize)>,
        turns: u32,
    },
    /// The whole run used every model turn a run has
    /// (`engine::RUN_MAX_ITERATIONS`) without finishing.
    #[error(
        "Stopped: this run used its {0} model turns in total without finishing. A run that needs more is doing \
         too much at once, or going round in circles: split the work into smaller steps, or into separate workflows."
    )]
    RunMaxIterations(u32),
    /// The owner's per-run spending limit was reached. The activity was given
    /// one last turn to report; `partial` is what it said.
    #[error("Stopped at your limit: this run reached ${spent_cents_display} of the ${cap_cents_display} you set for {activity_id}", spent_cents_display = format_args!("{:.2}", *.spent_cents as f64 / 100.0), cap_cents_display = format_args!("{:.2}", *.cap_cents as f64 / 100.0))]
    SpendCapReached {
        activity_id: String,
        spent_cents: i64,
        cap_cents: i64,
        partial: String,
    },
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
    /// or improvising won't help. Like `Exited`, it is a standing outcome, not
    /// a failure: every run hits the same wall until the owner changes
    /// something, so the run ends with this as its reason (see
    /// [`WorkflowError::standing_outcome`]).
    /// The second field is what only the owner can supply, when the tool
    /// that refused named it.
    #[error("blocked: {0}")]
    Blocked(String, Option<types::OwnerNeed>),
    #[error("workflow cancelled")]
    Cancelled,
    /// The run reached a gated operation whose per-employee policy says
    /// "Needs approval": it SUSPENDED at the checkpoint (state persisted in
    /// the run's engine wait, run status `awaiting_approval`) and waits for
    /// the owner's decision. Not a failure — the manager notifies the owner
    /// and the run resumes (or aborts) via the approval endpoint.
    #[error("awaiting owner approval for operation: {operation}")]
    AwaitingApproval { operation: String, display: String },
    /// The run reached `expert` steps whose experts have not answered yet.
    /// Not a failure: the run parks on one engine wait (`expert_reply` on
    /// `expert:<run>`, deadline the earliest timeout) and re-enters under its
    /// own id when a reply or a deadline wakes it.
    /// The count is of the requests still out across the whole run.
    #[error("waiting on {0} expert(s)")]
    AwaitingExpert(usize),
    #[error("circuit breaker tripped: {0}")]
    CircuitBreak(String),
    #[error("{0}")]
    Other(String),
}

/// What a step that used every model turn it had says to the owner.
fn step_cap_message(activity_id: &str, step: Option<(usize, usize)>, turns: u32) -> String {
    let which = match step {
        Some((n, of)) => format!("Step {n}/{of} of \"{activity_id}\""),
        None => format!("\"{activity_id}\""),
    };
    format!(
        "Stopped: {which} used its {turns} model turns without finishing. Split the work into smaller steps, \
         each one thing to do with its result."
    )
}

impl WorkflowError {
    /// The reason a run ended with a standing outcome — a condition that holds
    /// until something outside the run changes — or None for every other end.
    /// `Exited`: the step evaluator or the employee said there is nothing to
    /// do. `Blocked`: a tool refused terminally, and the next run would be
    /// refused the same way. Provider errors, timeouts and tool exceptions are
    /// failures and have none.
    pub fn standing_outcome(&self) -> Option<String> {
        match self {
            WorkflowError::Exited(reason) => Some(reason.clone()),
            WorkflowError::Blocked(..) => Some(self.to_string()),
            _ => None,
        }
    }

    /// What only the owner can supply before the run can do its job, when
    /// the tool that blocked it named it.
    pub fn owner_need(&self) -> Option<&types::OwnerNeed> {
        match self {
            WorkflowError::Blocked(_, need) => need.as_ref(),
            _ => None,
        }
    }
}

impl From<types::NeboError> for WorkflowError {
    fn from(e: types::NeboError) -> Self {
        WorkflowError::Database(e.to_string())
    }
}
