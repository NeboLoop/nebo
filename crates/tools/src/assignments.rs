//! Assignments (R5), the tool side. The tools crate cannot depend on the
//! workflow crate (the workflow crate depends on tools), so the opener that
//! turns a request into a case is installed at boot, the way the
//! orchestrator handle is, and the `agent` tool calls through it.

use std::sync::{Arc, OnceLock};

/// What the assigner's tool call carries.
#[derive(Debug, Clone)]
pub struct AssignmentRequest {
    pub assigner_agent_id: String,
    pub assigner_name: String,
    pub assigner_session_key: String,
    pub parent_run_id: Option<String>,
    pub assignee_agent_id: String,
    pub subject: String,
    pub done_means: String,
    pub due: Option<String>,
}

/// Opens an assignment as the assignee's own work and returns its id.
pub trait AssignmentOpener: Send + Sync {
    fn open(&self, req: &AssignmentRequest) -> Result<String, String>;
}

static OPENER: OnceLock<Arc<dyn AssignmentOpener>> = OnceLock::new();

/// Installed once at boot by the server (the workflow crate's case opener).
pub fn install_assignment_opener(opener: Arc<dyn AssignmentOpener>) {
    let _ = OPENER.set(opener);
}

pub fn assignment_opener() -> Option<Arc<dyn AssignmentOpener>> {
    OPENER.get().cloned()
}
