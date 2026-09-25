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
    /// Made for one piece of work: it runs once and is deleted after its
    /// outcome reaches the owner.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub temporary: bool,
    /// The run a save started (a temporary workflow run by hand starts as
    /// it is made).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
}

/// How long a workflow lives (owner, 09-25). One create path, with this as
/// its option.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lifetime {
    /// Kept until it is deleted.
    Saved,
    /// Made for one piece of work: it runs once, and when that run has
    /// ended and its outcome has reached the owner it is deleted. Its run
    /// history, receipts and cost stay. `report_to` is the session woken
    /// with the outcome.
    Temporary { report_to: String },
}

/// What a create or an update carries besides the definition.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SaveOptions {
    /// `None` keeps an existing workflow's lifetime; a new one is saved.
    pub lifetime: Option<Lifetime>,
    /// Start from the definition a past run ran with: the same piece of
    /// work, kept (a temporary workflow that already finished, saved to run
    /// again). The definition given with it adds to or replaces its fields
    /// (a schedule trigger, say).
    pub from_run: Option<String>,
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
    /// Backs the workflow tools' `employee` input: the session key only identifies the
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

    /// Human-readable receipt for one run — the run narrator's projection
    /// (input summary + per-activity line/facts), for rich card rendering in
    /// chat. `None` when the run doesn't exist. ONE narrator: the server impl
    /// delegates to the same builder the run-detail endpoint uses. Default
    /// `None` so test doubles don't have to care.
    fn run_receipt<'a>(
        &'a self,
        _run_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Option<serde_json::Value>> + Send + 'a>> {
        Box::pin(async { None })
    }

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
        options: SaveOptions,
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
        options: SaveOptions,
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
    /// `emit_sources` — the events the last activity is instructed to announce its output as.
    fn run_inline<'a>(
        &'a self,
        definition_json: String,
        inputs: serde_json::Value,
        trigger_type: &'a str,
        trigger_detail: Option<String>,
        agent_id: &'a str,
        emit_sources: Vec<String>,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>>;

    /// Cancel a running workflow by run_id.
    fn cancel<'a>(
        &'a self,
        run_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;

    /// A binding cannot run until the owner supplies what `need` names (a
    /// plugin its watch trigger needs; its record already says so). The
    /// server's manager tells the owner once per need. Default no-op so test
    /// doubles don't have to care.
    fn announce_binding_need(&self, _agent_id: &str, _binding_name: &str, _need: &str) {}

    /// Cancel all running workflows for a given agent. Default no-op.
    fn cancel_runs_for_agent<'a>(
        &'a self,
        _agent_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async {})
    }
}
