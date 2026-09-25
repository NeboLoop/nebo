//! The helper tools' door onto the registry: `tools::SubAgentOrchestrator`
//! over [`Helpers`]. A tool call arrives as a `SpawnRequest` built from the
//! calling run (`SpawnRequest::child_of`); the door turns it into the
//! parent's side of [`child::child_request`] and hands it to the registry,
//! which starts, collects and notifies every helper the one way.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use tools::orchestrator::{FollowUp, SpawnRequest, SpawnResult, SubAgentOrchestrator};
use tracing::{info, warn};

use super::{Completion, CompletionStatus, HelperKind, HelperSpec, Helpers, Isolation, Launch, depth_of, launch_result, notify};
use crate::harness::{Delivery, Harness, SeatRequest, TurnInput, TurnMode, TurnRequest};

type Fut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The door the helper tools reach the registry through.
pub struct HelperDoor {
    helpers: Arc<Helpers>,
    harness: Harness,
}

impl HelperDoor {
    pub fn new(helpers: Arc<Helpers>, harness: Harness) -> Self {
        Self { helpers, harness }
    }

    /// The skills the parent loaded, with their content, for the helper's
    /// thread. A skill that is gone or disabled is left out.
    async fn skills(&self, names: &[String], agent_id: &str) -> Vec<(String, String)> {
        let Some(loader) = self.harness.skill_loader.as_ref() else {
            return Vec::new();
        };
        let scope = (!agent_id.is_empty()).then_some(agent_id);
        let mut out = Vec::new();
        for name in names {
            match loader.get(name, scope).await {
                Some(skill) if skill.enabled => {
                    let content = loader.expand_template(&skill, Some(&self.harness.store));
                    if !content.is_empty() {
                        out.push((name.clone(), content));
                    }
                }
                _ => warn!(skill = %name, "a skill the parent loaded is not available to its helper"),
            }
        }
        out
    }

    async fn spec(&self, req: &SpawnRequest, turn: &TurnRequest, background: bool) -> Result<HelperSpec, String> {
        Ok(HelperSpec {
            description: req.description.clone(),
            prompt: req.prompt.clone(),
            kind: HelperKind::parse(&req.agent_type).unwrap_or(HelperKind::General),
            background,
            isolation: (req.isolate == "worktree").then_some(Isolation::Worktree),
            skills: self.skills(&req.skills, &turn.seat.agent_id).await,
            speed: self.speed(&req.speed)?,
        })
    }

    /// The model a `speed` names, resolved the way every turn resolves the
    /// model it was given; `None` when the call named none. A name this bot
    /// doesn't know starts nothing.
    fn speed(&self, named: &str) -> Result<Option<String>, String> {
        if named.is_empty() {
            return Ok(None);
        }
        match self.harness.selector.resolve_fuzzy(named) {
            Some(model) => Ok(Some(model)),
            None => Err(format!(
                "'{named}' isn't a speed on this Nebo. Leave speed out to run the helper at your own, or name one of:\n{}",
                self.harness.selector.get_aliases_text()
            )),
        }
    }

    async fn spawn_one(&self, req: SpawnRequest, background: bool) -> Result<SpawnResult, String> {
        let turn = parent_turn(&req);
        let spec = self.spec(&req, &turn, background).await?;
        let launched = self.helpers.launch(&turn, req.seat.grant.as_deref(), &req.seat.taint, spec).await?;
        Ok(match launched {
            Launch::Background { task_id } => SpawnResult {
                output: launch_result(&task_id),
                task_id,
                success: true,
                error: None,
            },
            Launch::Finished(c) => finished(c),
        })
    }

}

/// The parent's side of a helper: the run the tool call came from.
fn parent_turn(req: &SpawnRequest) -> TurnRequest {
    let key = req.parent_session_key.clone();
    let depth = depth_of(&key);
    let agent_id = req
        .seat
        .grant
        .as_ref()
        .map(|g| g.agent_id.clone())
        .unwrap_or_else(|| types::keyparser::extract_agent_id(&key));
    // Isolation copies the project the parent works in, or the one named.
    let cwd = if req.isolate == "worktree" && !req.workspace.is_empty() {
        Some(req.workspace.clone())
    } else {
        req.seat.cwd.clone()
    };
    TurnRequest {
        mode: if depth > 0 {
            TurnMode::Helper {
                parent_session_key: String::new(),
                kind: HelperKind::General,
                depth,
            }
        } else {
            TurnMode::Chat
        },
        session_key: key,
        input: TurnInput::None,
        seat: SeatRequest {
            agent_id,
            user_id: req.user_id.clone(),
            origin: req.seat.origin,
            door: types::permissions::Door::Chat,
            mode: None,
            ceiling: None,
            cwd,
            seed_taint: req.seat.taint.clone(),
            audience: None,
            tool_allowlist: req.seat.tool_allowlist.clone(),
            tool_denial_hint: req.seat.tool_denial_hint.clone(),
            handoff_depth: req.handoff_depth,
            model_override: req.model_override.clone(),
            model_preference: None,
            personality_snippet: None,
            tool_scope: None,
        },
        delivery: Delivery {
            channel: String::new(),
            channel_ctx: None,
            mention_briefing: None,
        },
        cancel: CancellationToken::new(),
        progress: None,
    }
}

/// A finished helper as the tools report it.
fn finished(c: Completion) -> SpawnResult {
    let error = match &c.status {
        CompletionStatus::Failed { error } => Some(error.clone()),
        CompletionStatus::Stopped => Some("stopped".to_string()),
        CompletionStatus::Done | CompletionStatus::Partial { .. } => None,
    };
    SpawnResult {
        task_id: c.task_id.clone(),
        success: error.is_none(),
        output: notify::render_foreground(&c),
        error,
    }
}

impl SubAgentOrchestrator for HelperDoor {
    fn spawn(&self, req: SpawnRequest) -> Fut<'_, Result<SpawnResult, String>> {
        Box::pin(async move {
            let background = !req.wait;
            self.spawn_one(req, background).await
        })
    }

    fn start_work(&self, req: SpawnRequest, work: tools::orchestrator::Work) -> Fut<'_, Result<SpawnResult, String>> {
        let started = self.helpers.start_work(&parent_turn(&req), &req.description, work);
        Box::pin(async move {
            let (task_id, output) = started?;
            Ok(SpawnResult { task_id, success: true, output, error: None })
        })
    }

    fn cancel(&self, task_id: &str, caller: &str) -> Fut<'_, Result<(), String>> {
        let stopped = self.helpers.stop(caller, task_id).map(|_| ());
        Box::pin(async move { stopped })
    }

    fn status(&self, task_id: &str, caller: &str) -> Fut<'_, Result<String, String>> {
        let status = self.helpers.read_output(caller, task_id);
        Box::pin(async move { status })
    }

    fn send(&self, task_id: &str, message: &str, parent: SpawnRequest) -> Fut<'_, Result<FollowUp, String>> {
        let (task_id, message) = (task_id.to_string(), message.to_string());
        Box::pin(async move {
            let (task_id, message) = (task_id.as_str(), message.as_str());
            let turn = parent_turn(&parent);
            let running = self
                .helpers
                .list(&turn.session_key)
                .iter()
                .any(|h| h.task_id == task_id && h.running);
            let said = self
                .helpers
                .send(&turn, parent.seat.grant.as_deref(), &parent.seat.taint, task_id, message)
                .await?;
            Ok(if running {
                FollowUp::Delivered { task_id: task_id.to_string() }
            } else {
                FollowUp::Continued(SpawnResult {
                    task_id: task_id.to_string(),
                    success: true,
                    output: said,
                    error: None,
                })
            })
        })
    }

    fn list_active(&self, caller: &str) -> Fut<'_, Vec<(String, String, String)>> {
        let listed = self
            .helpers
            .list(caller)
            .into_iter()
            .map(|h| (h.task_id, h.description, if h.running { "running" } else { "finished" }.to_string()))
            .collect();
        Box::pin(async move { listed })
    }

    fn recover(&self) -> Fut<'_, ()> {
        Box::pin(async move {
            let swept = crate::worktree::cleanup_stale(crate::worktree::STALE_AFTER_SECS).await;
            if !swept.is_empty() {
                info!(count = swept.len(), "swept stale helper copies");
            }
            self.helpers.recover();
        })
    }
}

#[cfg(test)]
mod tests {
    //! A helper tool call carries its run's limits through the door into the
    //! one child constructor: the child can only narrow them (#246).

    use std::sync::Mutex;

    use tools::ToolContext;
    use tools::registry::DynTool;
    use types::permissions::{Ceiling, Door, Effect, Grant, Mode, Rule, RuleField, RuleKey, RuleSource, Scope};
    use types::provenance::ProvenanceClass;

    use super::super::child::{Parent, child_request};
    use super::*;

    /// Records what the helper tools asked for.
    #[derive(Default)]
    struct Recorder(Mutex<Vec<SpawnRequest>>);

    /// The recorder as the tools' door.
    struct Recording(Arc<Recorder>);

    fn done() -> SpawnResult {
        SpawnResult { task_id: "h1".into(), success: true, output: "done".into(), error: None }
    }

    impl SubAgentOrchestrator for Recording {
        fn spawn(&self, req: SpawnRequest) -> Fut<'_, Result<SpawnResult, String>> {
            self.0.0.lock().unwrap().push(req);
            Box::pin(async { Ok(done()) })
        }
        fn start_work(&self, req: SpawnRequest, _: tools::orchestrator::Work) -> Fut<'_, Result<SpawnResult, String>> {
            self.0.0.lock().unwrap().push(req);
            Box::pin(async { Ok(done()) })
        }
        fn cancel(&self, _: &str, _: &str) -> Fut<'_, Result<(), String>> {
            Box::pin(async { Ok(()) })
        }
        fn status(&self, _: &str, _: &str) -> Fut<'_, Result<String, String>> {
            Box::pin(async { Ok(String::new()) })
        }
        fn send(&self, _: &str, _: &str, parent: SpawnRequest) -> Fut<'_, Result<FollowUp, String>> {
            self.0.0.lock().unwrap().push(parent);
            Box::pin(async { Ok(FollowUp::Continued(done())) })
        }
        fn list_active(&self, _: &str) -> Fut<'_, Vec<(String, String, String)>> {
            Box::pin(async { Vec::new() })
        }
        fn recover(&self) -> Fut<'_, ()> {
            Box::pin(async {})
        }
    }

    fn rule(key: RuleKey, field: Option<RuleField>, effect: Effect) -> Rule {
        Rule {
            id: uuid::Uuid::new_v4().to_string(),
            scope: Scope::Employee("a1".into()),
            key,
            field,
            effect,
            money: None,
            source: RuleSource::Owner,
            locked: false,
            created_at: 0,
        }
    }

    fn limited_parent() -> ToolContext {
        let mut grant = Grant::new("a1", Mode::Automatic);
        grant.rules = vec![
            rule(RuleKey::Capability("shell".into()), None, Effect::Deny),
            rule(RuleKey::Capability("file".into()), Some(RuleField::Folder("/work/a".into())), Effect::Allow),
        ];
        grant.fence = Some(vec!["/work/a".into()]);
        ToolContext {
            session_id: "s1".into(),
            session_key: "agent:a1:web".into(),
            user_id: "owner:agent:a1".into(),
            grant: Some(Arc::new(grant)),
            tool_whitelist: Some(["read_file".to_string()].into_iter().collect()),
            whitelist_denial_hint: Some("not in this run".into()),
            cwd: Some("/work/a".into()),
            run_taint: vec![ProvenanceClass::Web, ProvenanceClass::ExternalEmail],
            ..Default::default()
        }
    }

    /// What the helper registry builds from a recorded tool call.
    fn child_of(req: &SpawnRequest) -> TurnRequest {
        let turn = parent_turn(req);
        let parent = Parent {
            session_key: &turn.session_key,
            seat: &turn.seat,
            grant: req.seat.grant.as_deref(),
            run_taint: &req.seat.taint,
            cancel: CancellationToken::new(),
        };
        let spec = HelperSpec {
            description: req.description.clone(),
            prompt: req.prompt.clone(),
            kind: HelperKind::General,
            background: true,
            isolation: None,
            skills: Vec::new(),
            speed: None,
        };
        child_request(&parent, "h1", &spec, None, TurnInput::None)
    }

    fn assert_limited_like(child: &TurnRequest, parent: &ToolContext, path: &str) {
        let Some(Ceiling::Parent { grant }) = &child.seat.ceiling else {
            panic!("{path}: the child has no parent ceiling");
        };
        assert_eq!(Some(&**grant), parent.grant.as_deref(), "{path}: the parent's grant (with its fence) is the ceiling");
        assert_eq!(child.seat.tool_allowlist, parent.tool_whitelist, "{path}: tool allowlist lost");
        assert_eq!(child.seat.tool_denial_hint, parent.whitelist_denial_hint, "{path}: denial hint lost");
        assert_eq!(child.seat.cwd, parent.cwd, "{path}: working directory lost");
        assert_eq!(child.seat.seed_taint, parent.run_taint, "{path}: taint laundered");
        assert_eq!(child.seat.user_id, parent.user_id, "{path}: memory scope lost");
        assert_eq!(child.seat.door, Door::Helper, "{path}: a child is a helper");
        assert_eq!(child.seat.origin, tools::Origin::System, "{path}: a child never asks the owner");
        assert_eq!(child.session_key, format!("subagent:{}:h1", parent.session_key), "{path}");
    }

    async fn call(tools: &[Box<dyn DynTool>], ctx: &ToolContext, name: &str, input: serde_json::Value) {
        tools.iter().find(|t| t.name() == name).expect("helper tool").execute_dyn(ctx, input).await;
    }

    fn helper_tools(rec: &Arc<Recorder>) -> (tempfile::TempDir, Vec<Box<dyn DynTool>>) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(db::Store::new(&dir.path().join("t.db").to_string_lossy()).unwrap());
        let handle = tools::orchestrator::new_handle();
        let _ = handle.set(Box::new(Recording(rec.clone())));
        let rail = tools::coworker::new_rail_cell();
        let teams = Arc::new(tools::team_tool::Teams::new(Some(store.clone()), None, None, rail.clone()));
        (dir, tools::helper_tools::Helpers::new(store, handle, teams, rail).tools())
    }

    /// The escalation #246 closed: a helper of an employee with shell off and
    /// a folder fence came back with shell on and no fence. Every way a
    /// helper starts — foreground, background, isolated, a continuation by
    /// send_message — carries the parent's limits.
    #[tokio::test]
    async fn every_helper_path_keeps_its_parents_limits() {
        let rec = Arc::new(Recorder::default());
        let (_dir, tools) = helper_tools(&rec);
        let ctx = limited_parent();
        for (path, name, input) in [
            ("foreground", "delegate", serde_json::json!({"description": "a", "prompt": "a", "background": false})),
            ("background", "delegate", serde_json::json!({"description": "a", "prompt": "a"})),
            ("isolated", "delegate", serde_json::json!({"description": "b", "prompt": "b", "isolation": "worktree"})),
            ("send", "send_message", serde_json::json!({"to": "h1", "message": "and the edge cases"})),
        ] {
            call(&tools, &ctx, name, input).await;
            let reqs = std::mem::take(&mut *rec.0.lock().unwrap());
            assert!(!reqs.is_empty(), "{path}: nothing reached the helper door");
            for req in reqs {
                let child = child_of(&req);
                let Some(Ceiling::Parent { grant }) = &child.seat.ceiling else { unreachable!() };
                assert!(
                    grant.rules.iter().any(|r| r.key == RuleKey::Capability("shell".into()) && r.effect == Effect::Deny),
                    "{path}: shell came back on"
                );
                assert_limited_like(&child, &ctx, path);
            }
        }
    }

    /// Nothing is invented: an unrestricted parent's child is unrestricted,
    /// and the owner's Full Access reaches it.
    #[tokio::test]
    async fn an_unrestricted_parent_has_an_unrestricted_child() {
        let rec = Arc::new(Recorder::default());
        let (_dir, tools) = helper_tools(&rec);
        let ctx = ToolContext {
            session_id: "s1".into(),
            session_key: "agent:assistant:web".into(),
            grant: Some(Arc::new(Grant::new("", Mode::FullAccess))),
            ..Default::default()
        };
        call(&tools, &ctx, "delegate", serde_json::json!({"description": "a", "prompt": "a"})).await;
        let req = rec.0.lock().unwrap().pop().unwrap();
        let child = child_of(&req);
        assert!(child.seat.tool_allowlist.is_none() && child.seat.seed_taint.is_empty());
        let dir = tempfile::tempdir().unwrap();
        let store = db::Store::new(&dir.path().join("t.db").to_string_lossy()).unwrap();
        let grant = crate::harness::seat::run_grant(
            &store,
            crate::harness::seat::GrantRequest {
                agent_id: &child.seat.agent_id,
                origin: child.seat.origin,
                mode: child.seat.mode,
                ceiling: child.seat.ceiling.as_ref(),
                cwd: child.seat.cwd.as_deref(),
            },
        );
        assert_eq!(grant.mode, Mode::FullAccess, "the owner's Full Access did not reach the helper");
    }
}
