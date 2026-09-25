use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::ToolContext;

/// The limits a sub-agent runs under: its parent run's, copied from the
/// parent's `ToolContext` by [`SpawnRequest::child_of`]. Nothing the model
/// sends reaches these fields, so a child is limited exactly like its parent
/// (or further, by isolation) and never less. A child must never start from
/// `Default`.
#[derive(Debug, Clone, Default)]
pub struct ChildSeat {
    /// The parent run's grant: the child's ceiling and the mode it runs in.
    /// `None` only when the parent ran without one; the child then runs
    /// under its employee's own grant.
    pub grant: Option<Arc<types::permissions::Grant>>,
    /// A hard fence for the child: file writes and shell stay inside these.
    /// The parent's fence, or an isolated child's own copy.
    pub fence: Option<Vec<std::path::PathBuf>>,
    /// Restricted-run allowlist (outside callers, review fork).
    pub tool_allowlist: Option<HashSet<String>>,
    /// The denial text that goes with `tool_allowlist`.
    pub tool_denial_hint: Option<String>,
    /// Default working directory (an isolated parent's copy).
    pub cwd: Option<String>,
    /// Classes of untrusted content the parent had touched when it spawned.
    pub taint: Vec<types::provenance::ProvenanceClass>,
}

impl ChildSeat {
    /// Fence an isolated child to its own copy of `workspace`. A fenced
    /// parent may only isolate a project inside its folders: the copy is
    /// merged back into `workspace`, so isolating anything else would write
    /// where the parent cannot.
    pub fn isolate_to(&mut self, workspace: &str, copy: &str) -> Result<(), String> {
        let strings = |v: &[std::path::PathBuf]| -> Vec<String> {
            v.iter().map(|p| p.to_string_lossy().into_owned()).collect()
        };
        let folders = self.grant.as_ref().map(|g| g.folders()).unwrap_or_default();
        let target = [workspace.to_string()];
        if let Some(blocked) = crate::safeguard::outside_allowed("isolate", &target, &strings(&folders))
            .or_else(|| {
                self.fence
                    .as_ref()
                    .and_then(|f| crate::safeguard::outside_allowed("isolate", &target, &strings(f)))
            })
        {
            return Err(blocked);
        }
        self.fence = Some(vec![std::path::PathBuf::from(copy)]);
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
    /// Parent's stream sender — forwarded to sub-agents so that `AskRequest`
    /// events reach the user's WebSocket (permission forwarding).
    pub parent_stream_tx: Option<mpsc::Sender<ai::StreamEvent>>,
    /// Parent run's agent-to-agent hop count — inherited so a sub-agent cannot
    /// restart the coworker chain cap at zero.
    pub handoff_depth: u8,
    /// A batch (`spawn_parallel`) only: "worktree" gives each child its own copy of the
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
                grant: ctx.grant.clone(),
                fence: ctx.grant.as_ref().and_then(|g| g.fence.clone()),
                tool_allowlist: ctx.tool_whitelist.clone(),
                tool_denial_hint: ctx.whitelist_denial_hint.clone(),
                cwd: ctx.cwd.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn folder_grant(folders: &[&str]) -> Arc<types::permissions::Grant> {
        use types::permissions::*;
        let mut g = Grant::new("emp", Mode::Automatic);
        for f in folders {
            g.rules.push(Rule {
                id: format!("r-{f}"),
                scope: Scope::Employee("emp".into()),
                key: RuleKey::Capability("file".into()),
                field: Some(RuleField::Folder((*f).into())),
                effect: Effect::Allow,
                money: None,
                source: RuleSource::Owner,
                locked: false,
                created_at: 0,
            });
        }
        Arc::new(g)
    }

    /// Isolation narrows the fence to the child's own copy. A fenced parent
    /// can only isolate a project inside its folders; an unfenced one
    /// anything.
    #[test]
    fn isolation_narrows_and_never_widens_the_fence() {
        let mut fenced = ChildSeat { grant: Some(folder_grant(&["/work/a"])), ..Default::default() };
        fenced.isolate_to("/work/a/app", "/tmp/copy-1").expect("inside the fence");
        assert_eq!(fenced.fence, Some(vec!["/tmp/copy-1".into()]));
        assert_eq!(fenced.cwd.as_deref(), Some("/tmp/copy-1"));

        let mut outside = ChildSeat { grant: Some(folder_grant(&["/work/a"])), ..Default::default() };
        let refused = outside.isolate_to("/work/b", "/tmp/copy-2").unwrap_err();
        assert!(refused.starts_with("BLOCKED"), "{refused}");
        assert_eq!(outside.fence, None, "a refusal leaves the fence as it was");

        let mut nested = ChildSeat { fence: Some(vec!["/tmp/copy-1".into()]), ..Default::default() };
        assert!(nested.isolate_to("/work/a", "/tmp/copy-4").is_err(), "an isolated parent isolates only inside its copy");

        let mut open = ChildSeat::default();
        open.isolate_to("/anywhere", "/tmp/copy-3").unwrap();
        assert_eq!(open.fence, Some(vec!["/tmp/copy-3".into()]));
    }

    /// `child_of` reads every limit from the parent's context.
    #[test]
    fn child_of_copies_the_parents_limits() {
        let mut grant = (*folder_grant(&["/work/a"])).clone();
        grant.fence = Some(vec!["/tmp/copy".into()]);
        let ctx = ToolContext {
            grant: Some(Arc::new(grant)),
            run_taint: vec![types::provenance::ProvenanceClass::Phone],
            tool_whitelist: Some(["os".to_string()].into_iter().collect()),
            ..Default::default()
        };
        let seat = SpawnRequest::child_of(&ctx).seat;
        assert_eq!(seat.grant, ctx.grant);
        assert_eq!(seat.fence, Some(vec!["/tmp/copy".into()]));
        assert_eq!(seat.taint, ctx.run_taint);
        assert_eq!(seat.tool_allowlist, ctx.tool_whitelist);
    }
}
