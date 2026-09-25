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

    async fn spec(&self, req: &SpawnRequest, turn: &TurnRequest, background: bool) -> HelperSpec {
        HelperSpec {
            description: req.description.clone(),
            prompt: req.prompt.clone(),
            kind: HelperKind::parse(&req.agent_type).unwrap_or(HelperKind::General),
            background,
            isolation: (req.isolate == "worktree").then_some(Isolation::Worktree),
            model: None,
            skills: self.skills(&req.skills, &turn.seat.agent_id).await,
        }
    }

    async fn spawn_one(&self, req: SpawnRequest, background: bool) -> Result<SpawnResult, String> {
        let turn = parent_turn(&req);
        let spec = self.spec(&req, &turn, background).await;
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

    /// Break `prompt` into a graph of helpers with one model call.
    async fn decompose(&self, prompt: &str, agent_id: &str) -> Result<Vec<crate::task_graph::TaskNode>, String> {
        let provider = self
            .harness
            .providers
            .read()
            .await
            .first()
            .cloned()
            .ok_or("No AI provider is configured to plan the work.")?;
        let trace = ai::RequestTrace {
            agent_id: agent_id.to_string(),
            ..ai::RequestTrace::new("task_decompose")
        };
        crate::decompose::decompose_task(provider.as_ref(), trace, prompt).await
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

    fn execute_dag(&self, prompt: &str, parent: SpawnRequest) -> Fut<'_, Result<SpawnResult, String>> {
        let prompt = prompt.to_string();
        Box::pin(async move {
            let turn = parent_turn(&parent);
            let nodes = self.decompose(&prompt, &turn.seat.agent_id).await?;
            info!(nodes = nodes.len(), parent = %turn.session_key, "orchestrating a decomposed job");
            let (output, success) = self
                .helpers
                .orchestrate(&turn, parent.seat.grant.as_deref(), &parent.seat.taint, nodes)
                .await?;
            Ok(SpawnResult {
                task_id: format!("job-{}", &uuid::Uuid::new_v4().simple().to_string()[..12]),
                success,
                output,
                error: (!success).then(|| "One or more parts failed".to_string()),
            })
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

    fn spawn_parallel(&self, requests: Vec<SpawnRequest>) -> Fut<'_, Result<SpawnResult, String>> {
        Box::pin(async move {
            let descriptions: Vec<String> = requests.iter().map(|r| r.description.clone()).collect();
            let launched =
                futures::future::join_all(requests.into_iter().map(|req| self.spawn_one(req, false))).await;
            let mut parts = Vec::new();
            let mut all_ok = true;
            for (description, result) in descriptions.iter().zip(launched) {
                // Each part is headed by its description; a failed one says so.
                let (ok, body) = match result {
                    Ok(r) => (r.success, r.output),
                    Err(e) => (false, format!("The helper could not start: {e}")),
                };
                all_ok &= ok;
                parts.push(format!("## {description}{}\n\n{body}", if ok { "" } else { " (FAILED)" }));
            }
            Ok(SpawnResult {
                task_id: format!("batch-{}", &uuid::Uuid::new_v4().simple().to_string()[..12]),
                success: all_ok,
                output: parts.join("\n\n---\n\n"),
                error: (!all_ok).then(|| "One or more helpers failed".to_string()),
            })
        })
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
