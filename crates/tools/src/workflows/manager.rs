use std::pin::Pin;
use std::future::Future;

use serde::{Deserialize, Serialize};

/// Info about an installed workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub is_enabled: bool,
    pub trigger_count: usize,
    pub activity_count: usize,
}

/// Info about a workflow run.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRunInfo {
    pub id: String,
    pub workflow_id: String,
    pub status: String,
    pub trigger_type: String,
    pub total_tokens_used: Option<i64>,
    pub error: Option<String>,
    pub started_at: i64,
    pub completed_at: Option<i64>,
}

/// Trait for managing workflows and dispatching runs.
///
/// Defined in tools crate, implemented in server crate.
pub trait WorkflowManager: Send + Sync {
    /// List all installed workflows.
    fn list(&self) -> Pin<Box<dyn Future<Output = Vec<WorkflowInfo>> + Send + '_>>;

    /// Install a workflow from a marketplace code (WORK-XXXX-XXXX).
    fn install<'a>(&'a self, code: &'a str) -> Pin<Box<dyn Future<Output = Result<WorkflowInfo, String>> + Send + 'a>>;

    /// Uninstall a workflow by ID.
    fn uninstall<'a>(&'a self, id: &'a str) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;

    /// Resolve a workflow name or ID to full info.
    fn resolve<'a>(&'a self, name_or_id: &'a str) -> Pin<Box<dyn Future<Output = Result<WorkflowInfo, String>> + Send + 'a>>;

    /// Run a workflow. Returns run_id immediately; execution happens in a spawned task.
    fn run<'a>(
        &'a self,
        id: &'a str,
        inputs: serde_json::Value,
        trigger_type: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>>;

    /// Get the status of a workflow run.
    fn run_status<'a>(&'a self, run_id: &'a str) -> Pin<Box<dyn Future<Output = Result<WorkflowRunInfo, String>> + Send + 'a>>;

    /// List recent runs for a workflow.
    fn list_runs<'a>(&'a self, workflow_id: &'a str, limit: i64) -> Pin<Box<dyn Future<Output = Vec<WorkflowRunInfo>> + Send + 'a>>;

    /// Toggle a workflow's enabled state. Returns new is_enabled.
    fn toggle<'a>(&'a self, id: &'a str) -> Pin<Box<dyn Future<Output = Result<bool, String>> + Send + 'a>>;

    /// Create a new workflow from a name and JSON definition.
    fn create<'a>(
        &'a self,
        name: &'a str,
        definition: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<WorkflowInfo, String>> + Send + 'a>>;

    /// Run an inline workflow from a JSON definition (no DB/filesystem lookup).
    /// Used by role workers for inline workflow bindings defined in role.json.
    /// `emit_source` — if set, the last activity will be instructed to emit its output.
    fn run_inline<'a>(
        &'a self,
        definition_json: String,
        inputs: serde_json::Value,
        trigger_type: &'a str,
        role_id: &'a str,
        emit_source: Option<String>,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>>;

    /// Cancel a running workflow by run_id.
    fn cancel<'a>(&'a self, run_id: &'a str) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;

    /// Cancel all running workflows for a given role. Default no-op.
    fn cancel_runs_for_role<'a>(&'a self, _role_id: &'a str) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async {})
    }
}
