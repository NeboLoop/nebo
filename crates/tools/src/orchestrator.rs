use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};

use crate::ToolContext;

/// The limits a sub-agent runs under: its parent run's, copied from the
/// parent's `ToolContext` by [`SpawnRequest::child_of`]. Nothing the model
/// sends reaches these fields, so a child is limited exactly like its parent
/// (or further, by isolation) and never less. A child must never start from
/// `Default`.
#[derive(Debug, Clone, Default)]
pub struct ChildSeat {
    /// Who the parent run serves: an owner's run starts its children as the
    /// system; any other origin (an outside caller, a coworker, an MCP
    /// client) stays what it was, with its limits.
    pub origin: crate::Origin,
    /// The parent run's grant: the child's ceiling and the mode it runs in.
    /// `None` only when the parent ran without one; the child then runs
    /// under its employee's own grant.
    pub grant: Option<Arc<types::permissions::Grant>>,
    /// Restricted-run allowlist (outside callers, review fork).
    pub tool_allowlist: Option<HashSet<String>>,
    /// The denial text that goes with `tool_allowlist`.
    pub tool_denial_hint: Option<String>,
    /// Default working directory (an isolated parent's copy).
    pub cwd: Option<String>,
    /// Classes of untrusted content the parent had touched when it spawned.
    pub taint: Vec<types::provenance::ProvenanceClass>,
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
    /// Maximum agentic loop iterations for this sub-agent (0 = default 100).
    pub max_iterations: usize,
    /// Skill names to pre-load into the sub-agent's context. Full SKILL.md
    /// content is injected so the sub-agent has instructions without needing
    /// to discover/load them itself. Keeps the parent context lean.
    pub skills: Vec<String>,
    /// Parent run's agent-to-agent hop count — inherited so a sub-agent cannot
    /// restart the coworker chain cap at zero.
    pub handoff_depth: u8,
    /// "worktree" gives the helper its own copy of the
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
            skills,
            handoff_depth: ctx.handoff_depth,
            seat: ChildSeat {
                origin: ctx.origin,
                grant: ctx.grant.clone(),
                tool_allowlist: ctx.tool_whitelist.clone(),
                tool_denial_hint: ctx.whitelist_denial_hint.clone(),
                cwd: ctx.cwd.clone(),
                taint: ctx.run_taint.clone(),
            },
            ..Default::default()
        }
    }
}

/// Result from a helper.
#[derive(Debug, Clone)]
pub struct SpawnResult {
    pub task_id: String,
    pub success: bool,
    /// The harness's own words for what happened, returned to the model as
    /// they are: the launch receipt, or the finished helper's report.
    pub output: String,
    pub error: Option<String>,
    /// The untrusted content a finished helper read: its report carries it.
    pub taint: Vec<types::provenance::ProvenanceClass>,
}

/// Background work that is not a model turn (the deep-research pipeline),
/// run as one of the caller's helpers: it gets the helper's stop token and a
/// progress channel whose events become the helper's activity on the
/// owner's screen, and returns its report.
pub type Work = Box<
    dyn FnOnce(
            tokio_util::sync::CancellationToken,
            tokio::sync::mpsc::Sender<ai::StreamEvent>,
        ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send>>
        + Send,
>;

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

/// The helper tools' door onto the harness's helper registry
/// (`agent::harness::delegation`). Every call names the conversation it
/// comes from: a caller sees, stops and messages only the helpers it
/// started. Uses `Pin<Box<dyn Future>>` for object safety.
pub trait SubAgentOrchestrator: Send + Sync {
    /// Start one helper for the run `req` was built from
    /// ([`SpawnRequest::child_of`]).
    fn spawn(
        &self,
        req: SpawnRequest,
    ) -> Pin<Box<dyn Future<Output = Result<SpawnResult, String>> + Send + '_>>;

    /// Start `work` in the background as a helper of the run `req` was built
    /// from: the one helper lifecycle (row, stop, progress, one notification
    /// when it ends). Returns the launch receipt.
    fn start_work(
        &self,
        req: SpawnRequest,
        work: Work,
    ) -> Pin<Box<dyn Future<Output = Result<SpawnResult, String>> + Send + '_>>;

    /// Stop one of the helpers the conversation `caller` started.
    fn cancel(
        &self,
        task_id: &str,
        caller: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>>;

    /// The status and output of one of the helpers `caller` started.
    fn status(
        &self,
        task_id: &str,
        caller: &str,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + '_>>;

    /// Send one of the helpers `parent` started a message. A running one
    /// hears it at its next step (`FollowUp::Delivered`); a finished one
    /// keeps its session (everything it read and did) and runs again with
    /// the message as its next input, reporting the way it was started
    /// (`FollowUp::Continued`). `parent` is [`SpawnRequest::child_of`] the
    /// sender's run: a continued helper runs under the sender's limits.
    fn send(
        &self,
        task_id: &str,
        message: &str,
        parent: SpawnRequest,
    ) -> Pin<Box<dyn Future<Output = Result<FollowUp, String>> + Send + '_>>;

    /// The helpers `caller` started that are still in hand:
    /// (task_id, description, status).
    fn list_active(
        &self,
        caller: &str,
    ) -> Pin<Box<dyn Future<Output = Vec<(String, String, String)>> + Send + '_>>;

    /// Settle helpers a restart interrupted.
    fn recover(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

/// Late-binding handle for the helper door: created empty with the tool
/// registry, filled once the harness exists.
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

    /// `child_of` reads every limit from the parent's context.
    #[test]
    fn child_of_copies_the_parents_limits() {
        let mut grant = (*folder_grant(&["/work/a"])).clone();
        grant.fence = Some(vec!["/tmp/copy".into()]);
        let ctx = ToolContext {
            origin: crate::Origin::Caller,
            grant: Some(Arc::new(grant)),
            run_taint: vec![types::provenance::ProvenanceClass::Phone],
            tool_whitelist: Some(["os".to_string()].into_iter().collect()),
            ..Default::default()
        };
        let seat = SpawnRequest::child_of(&ctx).seat;
        assert_eq!(seat.origin, crate::Origin::Caller);
        assert_eq!(seat.grant, ctx.grant);
        assert_eq!(seat.taint, ctx.run_taint);
        assert_eq!(seat.tool_allowlist, ctx.tool_whitelist);
    }
}
