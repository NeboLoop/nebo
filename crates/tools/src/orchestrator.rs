use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::ToolContext;

/// The limits a sub-agent runs under: its parent run's, copied from the
/// parent's `ToolContext` by [`SpawnRequest::child_of`]. Nothing the model
/// sends reaches these fields, so a child is limited exactly like its parent
/// (or further, by isolation) and never less. `None`/empty means "no limit"
/// to the runner's gates, which is why a child must never start from
/// `Default`.
#[derive(Debug, Clone, Default)]
pub struct ChildSeat {
    /// Capability toggles (category → allowed).
    pub permissions: Option<HashMap<String, bool>>,
    /// Per-employee approval policy over gated operations.
    pub operation_policy: Option<crate::policy::OperationPolicy>,
    /// Resource grants (resource → "allow" | "deny" | "inherit").
    pub resource_grants: Option<HashMap<String, String>>,
    /// Restricted-run allowlist (outside callers, review fork).
    pub tool_allowlist: Option<HashSet<String>>,
    /// The denial text that goes with `tool_allowlist`.
    pub tool_denial_hint: Option<String>,
    /// Path fence: file writes and shell stay inside these. Empty = no fence.
    pub allowed_paths: Vec<String>,
    /// Default working directory (an isolated parent's copy).
    pub cwd: Option<String>,
    /// The owner's Full Access switch as the parent ran with it.
    pub full_access: bool,
    /// Classes of untrusted content the parent had touched when it spawned.
    pub taint: Vec<types::provenance::ProvenanceClass>,
}

impl ChildSeat {
    /// Fence an isolated child to its own copy of `workspace`. A fenced
    /// parent may only isolate a project inside its fence: the copy is merged
    /// back into `workspace`, so isolating anything else would write where
    /// the parent cannot.
    pub fn isolate_to(&mut self, workspace: &str, copy: &str) -> Result<(), String> {
        if let Some(blocked) =
            crate::safeguard::outside_allowed("isolate", &[workspace.to_string()], &self.allowed_paths)
        {
            return Err(blocked);
        }
        self.allowed_paths = vec![copy.to_string()];
        self.cwd = Some(copy.to_string());
        Ok(())
    }
}

/// Request to spawn a single sub-agent or execute a DAG.
///
/// Built only by [`SpawnRequest::child_of`] (the parent half) with the task
/// filled on top, so every spawn path carries the same inheritance.
#[derive(Debug, Clone, Default)]
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
    /// The project folder to isolate (empty = the parent's cwd, else the
    /// process cwd).
    pub workspace: String,
    /// The parent run's limits. See [`ChildSeat`].
    pub seat: ChildSeat,
}

impl SpawnRequest {
    /// The ONE place a child request starts: everything a sub-agent inherits
    /// from the run that spawned it — where it sits, its model, its cancel
    /// and stream, its hop depth, the skills it loaded, and its limits. The
    /// caller fills in the task (prompt, description, type, …) on top.
    pub fn child_of(ctx: &ToolContext) -> Self {
        // A delegate does not see its parent's context, so skills the parent
        // loaded this run — the instructions the delegated work is supposed
        // to follow — vanish at the handoff unless they travel with it.
        // Observed live: a parent loaded the deck design system, spawned the
        // deck build, and the sub-agent worked without it. An explicit skills
        // list on the call still wins.
        let mut skills: Vec<String> = ctx
            .skills_read
            .lock()
            .map(|read| read.iter().cloned().collect())
            .unwrap_or_default();
        skills.sort();
        SpawnRequest {
            agent_type: "general".to_string(),
            // The conversation's model: without it a child falls to the
            // global default, which can be another provider.
            model_override: ctx.model_preference.clone().unwrap_or_default(),
            parent_session_id: ctx.session_id.clone(),
            parent_session_key: ctx.session_key.clone(),
            user_id: ctx.user_id.clone(),
            wait: true,
            parent_cancel: Some(ctx.cancel_token.clone()),
            skills,
            parent_stream_tx: ctx.stream_tx.clone(),
            handoff_depth: ctx.handoff_depth,
            seat: ChildSeat {
                permissions: ctx.entity_permissions.clone(),
                operation_policy: ctx.operation_policy.clone(),
                resource_grants: ctx.resource_grants.clone(),
                tool_allowlist: ctx.tool_whitelist.clone(),
                tool_denial_hint: ctx.whitelist_denial_hint.clone(),
                allowed_paths: ctx.allowed_paths.clone(),
                cwd: ctx.cwd.clone(),
                full_access: ctx.full_access,
                taint: ctx.run_taint.clone(),
            },
            ..Default::default()
        }
    }
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
    ///
    /// `parent` is [`SpawnRequest::child_of`] the run that asked: every node
    /// is built from it, so the DAG's children sit, run at the model, and are
    /// limited exactly as a single spawn's would be. A node's own model, when
    /// the decomposition names one, replaces the parent's.
    fn execute_dag(
        &self,
        prompt: &str,
        parent: SpawnRequest,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Isolation narrows the fence to the child's own copy. A fenced parent
    /// can only isolate a project inside its fence; an unfenced one anything.
    #[test]
    fn isolation_narrows_and_never_widens_the_fence() {
        let mut fenced = ChildSeat { allowed_paths: vec!["/work/a".into()], ..Default::default() };
        fenced.isolate_to("/work/a/app", "/tmp/copy-1").expect("inside the fence");
        assert_eq!(fenced.allowed_paths, vec!["/tmp/copy-1".to_string()]);
        assert_eq!(fenced.cwd.as_deref(), Some("/tmp/copy-1"));

        let mut outside = ChildSeat { allowed_paths: vec!["/work/a".into()], ..Default::default() };
        let refused = outside.isolate_to("/work/b", "/tmp/copy-2").unwrap_err();
        assert!(refused.starts_with("BLOCKED"), "{refused}");
        assert_eq!(outside.allowed_paths, vec!["/work/a".to_string()], "a refusal leaves the fence as it was");

        let mut open = ChildSeat::default();
        open.isolate_to("/anywhere", "/tmp/copy-3").unwrap();
        assert_eq!(open.allowed_paths, vec!["/tmp/copy-3".to_string()]);
    }

    /// `child_of` reads every limit from the parent's context.
    #[test]
    fn child_of_copies_the_parents_limits() {
        let ctx = ToolContext {
            entity_permissions: Some([("browser".to_string(), false)].into_iter().collect()),
            allowed_paths: vec!["/work/a".into()],
            full_access: true,
            run_taint: vec![types::provenance::ProvenanceClass::Phone],
            tool_whitelist: Some(["os".to_string()].into_iter().collect()),
            ..Default::default()
        };
        let seat = SpawnRequest::child_of(&ctx).seat;
        assert_eq!(seat.permissions, ctx.entity_permissions);
        assert_eq!(seat.allowed_paths, ctx.allowed_paths);
        assert!(seat.full_access);
        assert_eq!(seat.taint, ctx.run_taint);
        assert_eq!(seat.tool_allowlist, ctx.tool_whitelist);
    }
}
