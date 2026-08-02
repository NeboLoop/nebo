use std::future::Future;
use std::pin::Pin;

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
    /// List workflows visible to an agent: its own `agent_workflows` bindings
    /// (what the Settings → Workflows panel shows) plus any standalone
    /// marketplace-installed workflows.
    fn list<'a>(
        &'a self,
        agent_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Vec<WorkflowInfo>> + Send + 'a>>;

    /// Install a workflow from a marketplace code (WORK-XXXX-XXXX).
    fn install<'a>(
        &'a self,
        code: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<WorkflowInfo, String>> + Send + 'a>>;

    /// Uninstall a workflow by ID.
    fn uninstall<'a>(
        &'a self,
        id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;

    /// Resolve a workflow name or ID to full info. Matches the calling agent's
    /// own `agent_workflows` bindings first (returned with a binding-scoped id
    /// of the form `agent:{agent_id}:{binding_name}`), then standalone
    /// workflows by ID or name.
    fn resolve<'a>(
        &'a self,
        agent_id: &'a str,
        name_or_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<WorkflowInfo, String>> + Send + 'a>>;

    /// Resolve an agent reference (id, exact name, or slug) to the agent's id.
    /// Backs the work tool's `agent` input: the session key only identifies the
    /// CALLER, so without this an assistant asked to change another employee's
    /// duties could only self-scope — which is how weekend workflows silently
    /// landed on the assistant instead of the Content Creator (2026-08-01).
    fn resolve_agent<'a>(
        &'a self,
        agent_ref: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>>;

    /// Run a workflow. Returns run_id immediately; execution happens in a spawned task.
    /// Accepts a standalone workflow id, or a binding-scoped id
    /// (`agent:{agent_id}:{binding_name}`) as returned by `resolve` for agent
    /// bindings — those fire through `run_inline`.
    fn run<'a>(
        &'a self,
        id: &'a str,
        inputs: serde_json::Value,
        trigger_type: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>>;

    /// Get the status of a workflow run.
    fn run_status<'a>(
        &'a self,
        run_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<WorkflowRunInfo, String>> + Send + 'a>>;

    /// List recent runs for a workflow.
    fn list_runs<'a>(
        &'a self,
        workflow_id: &'a str,
        limit: i64,
    ) -> Pin<Box<dyn Future<Output = Vec<WorkflowRunInfo>> + Send + 'a>>;

    /// Toggle a workflow's enabled state. Returns new is_enabled.
    fn toggle<'a>(
        &'a self,
        id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<bool, String>> + Send + 'a>>;

    /// Create a workflow the calling agent owns, as an `agent_workflows`
    /// binding — the canonical store the UI panel and the AgentWorker trigger
    /// system both read. This is the ONLY way an agent gives itself a workflow;
    /// it never writes the standalone `workflows` table (that path produced
    /// orphans invisible to the panel and never fired by the engine).
    fn create<'a>(
        &'a self,
        agent_id: &'a str,
        name: &'a str,
        definition: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<WorkflowInfo, String>> + Send + 'a>>;

    /// Full-replacement edit of an existing binding the calling agent owns.
    /// Same definition shape as create; errors when the binding doesn't exist
    /// (typo-safe, mirrors delete). Run history is keyed by binding name and
    /// stays attached across updates — no uninstall/recreate cycle.
    fn update<'a>(
        &'a self,
        agent_id: &'a str,
        name: &'a str,
        definition: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<WorkflowInfo, String>> + Send + 'a>>;

    /// Periodic workflow tuning sweep (self-optimization). Default no-op so
    /// lightweight implementations aren't forced to care; the server's
    /// manager overrides it with the real evidence-gated pass.
    fn tuning_sweep<'a>(&'a self) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async {})
    }

    /// Delete a workflow binding the calling agent owns: frontmatter,
    /// tracking row, cron trigger, and on-disk agent.json. Mirrors the REST
    /// delete so the tool pathway has parity with the UI panel.
    fn delete<'a>(
        &'a self,
        agent_id: &'a str,
        binding_name: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;

    /// Run an inline workflow from a JSON definition (no DB/filesystem lookup).
    /// Used by agent workers for inline workflow bindings defined in agent.json.
    /// `emit_source` — if set, the last activity will be instructed to emit its output.
    fn run_inline<'a>(
        &'a self,
        definition_json: String,
        inputs: serde_json::Value,
        trigger_type: &'a str,
        trigger_detail: Option<String>,
        agent_id: &'a str,
        emit_source: Option<String>,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>>;

    /// Cancel a running workflow by run_id.
    fn cancel<'a>(
        &'a self,
        run_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;

    /// Cancel all running workflows for a given agent. Default no-op.
    fn cancel_runs_for_agent<'a>(
        &'a self,
        _agent_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async {})
    }
}
