use std::path::PathBuf;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use futures::stream::{FuturesUnordered, StreamExt};
use tokio::sync::{RwLock, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

/// Per-worker inactivity guard for parallel spawns: a worker is aborted only
/// after this long with NO stream activity (no text, tool, or usage events) —
/// any event resets the window. Deliberately not a wall-clock cap: a run is
/// already bounded by its iteration budget, and a blanket timeout kills
/// legitimate long-running work.
const WORKER_INACTIVITY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Prefix on the partial output returned when a worker is aborted for
/// inactivity. Callers match on it to record the task as failed for telemetry
/// while still handing the accumulated output to the parent.
/// A blocking or background child that emits nothing for this long is ended
/// with a stall marker; the parent gets the partial output. Longer than the
/// fan-out window because one child may run a whole build, shorter than the
/// dispatcher's [`crate::guardrails::RUN_IDLE_LIMIT`] so the child reports
/// before the parent is ended.
pub const SUBAGENT_INACTIVITY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10 * 60);
/// Every stall marker starts with this; the window follows so the text is
/// true for whichever bound fired.
const STALL_MARKER_PREFIX: &str = "[partial: worker stalled after ";

fn stall_marker(window: std::time::Duration) -> String {
    format!("{STALL_MARKER_PREFIX}{}s of no activity]", window.as_secs())
}

/// Max sub-agent nesting depth. Without this, a weak model told to "work
/// together" delegates, and each spawned agent re-delegates — nesting
/// `subagent:subagent:subagent:…` with no bound, where every node is a full
/// provider run. Beyond this depth a spawn is refused so the agent does the
/// work itself. Tunable.
const MAX_SUBAGENT_DEPTH: usize = 2;

/// Acknowledgement for a `wait: false` spawn. The constraint lives HERE, at
/// the decision point, not only in the distant system prompt: the moment of
/// temptation is right after spawning, when the model narrates onward as if
/// the result already exists. Until the wake arrives it knows nothing.
pub(crate) const BACKGROUND_SPAWN_ACK: &str =
    "Working in the background. When it finishes or fails you will be woken \
     automatically to act on the result and report. Until that wake arrives you know \
     NOTHING about its outcome — do not report, assume, or predict its results. If the \
     owner asks before then, say it is still running (read_output with its id shows \
     where it is).";

/// How deeply a session sits in the sub-agent tree — the number of `subagent:`
/// prefixes on its key. Top-level (user/channel) agents are depth 0; their
/// direct sub-agents are depth 1, and so on.
fn subagent_depth(parent_session_key: &str) -> usize {
    parent_session_key.matches("subagent:").count()
}

use ai::{StreamEventType, ToolCall};
use db::Store;
use tools::{FollowUp, SpawnRequest, SpawnResult, SubAgentOrchestrator};

/// Build a human-readable description from a tool call.
///
/// For STRAP tools, extracts resource/action from the input JSON so
/// the owner's progress line shows "persona: create" instead of just "agent".
fn describe_tool_call(tc: &ToolCall) -> String {
    let input = &tc.input;
    let resource = input.get("resource").and_then(|v| v.as_str()).unwrap_or("");
    let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");

    // Plugin tool: show slug + command prefix
    if tc.name == "plugin" {
        let command = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
        let cmd_prefix = command.split_whitespace().next().unwrap_or("");
        if !resource.is_empty() && !cmd_prefix.is_empty() {
            return format!("{}: {}", resource, cmd_prefix);
        }
        if !resource.is_empty() {
            return resource.to_string();
        }
        return tc.name.clone();
    }

    // STRAP tools: show resource + action
    if !resource.is_empty() && !action.is_empty() {
        return format!("{}: {}", resource, action);
    }
    if !resource.is_empty() {
        return resource.to_string();
    }
    tc.name.clone()
}

use crate::decompose;
use crate::harness::conversation::MidTurnFrom;
use crate::lanes::{self, LaneManager};
use crate::runner::{RunRequest, Runner};
use crate::task_graph::{AgentType, TaskGraph};

/// Maximum characters of dependency context injected per dependency.
const MAX_DEP_CONTEXT_CHARS: usize = 4000;

/// Tracks a running sub-agent.
struct ActiveAgent {
    task_id: String,
    description: String,
    status: String,
    cancel: CancellationToken,
    /// The session a `send` reaches while this child runs. `None` for the
    /// children of a parallel batch or a DAG: the caller waits on the whole
    /// batch and nothing hears a late message for one of them.
    session_key: Option<String>,
}

/// Enough for any realistic pasted payload (the observed SVG + brief was ~1.6K);
/// a cap so a pasted book cannot blow the delegate's context.
const MAX_ORIGINAL_CHARS: usize = 12_000;

/// Pure half of `original_request_block`: dedup against the parent's prompt,
/// clip, and wrap in the authoritative-source framing.
fn compose_original_block(original: &str, prompt: &str) -> String {
    let original = original.trim();
    if original.is_empty() || prompt.contains(original) {
        // Nothing to carry, or the parent quoted it in full already.
        return String::new();
    }
    let clipped: String = original.chars().take(MAX_ORIGINAL_CHARS).collect();
    let truncated = if clipped.len() < original.len() {
        "\n[...truncated]"
    } else {
        ""
    };
    format!(
        "\n\n--- ORIGINAL USER REQUEST (verbatim; authoritative over the summary above — \
         any content it embeds, such as pasted markup, copy, or data, is source material \
         you must use) ---\n{clipped}{truncated}"
    )
}

/// The sub-agent orchestrator: manages lifecycle, DAG execution, concurrency.
pub struct Orchestrator {
    runner: Arc<Runner>,
    store: Arc<Store>,
    active: Arc<RwLock<HashMap<String, ActiveAgent>>>,
    lanes: Option<Arc<LaneManager>>,
    /// Session wake rail (R5): fire-and-forget completions send the parent
    /// session key here after the wake row is persisted; the server pumps it
    /// into `wake::deliver`. The row is durable either way — a dropped
    /// notification is recovered by the boot sweep.
    wake_notify: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    /// Spawn requests of recent children, newest last, so `send` can relaunch
    /// one on its own session with the same skills, tools, and plugins.
    resumable: Arc<RwLock<std::collections::VecDeque<(String, SpawnRequest)>>>,
}

/// Prefixed to a `send` follow-up so the child knows it is the addressee. Its
/// own transcript carries the parent's original request (which may say "send
/// the sub-agent a follow-up"), and a bare follow-up read as an instruction to
/// delegate onward instead of doing the work (live, 2026-09-02).
pub const CONTINUATION_FRAME: &str = "[Follow-up for you, the sub-agent that did the task \
above. Do it yourself with your tools; do not delegate it.]\n";

/// How many recent children stay continuable. Older ones are forgotten;
/// their sessions remain in the store, only the relaunch context is gone.
pub const MAX_RESUMABLE: usize = 64;

/// The copy of a spawn request kept for `send`. The parent's stream sender
/// and cancel token are stripped: `send` supplies the current turn's, and a
/// kept sender holds the parent's event channel open after its turn ended, so
/// the run never completes (live, 2026-09-02: 38 minutes, then killed).
fn resumable_copy(req: &SpawnRequest) -> SpawnRequest {
    SpawnRequest { parent_stream_tx: None, parent_cancel: None, ..req.clone() }
}

fn remember_resumable<T>(
    ring: &mut std::collections::VecDeque<(String, T)>,
    task_id: String,
    value: T,
) {
    ring.push_back((task_id, value));
    while ring.len() > MAX_RESUMABLE {
        ring.pop_front();
    }
}

/// Register `task_id` as running and hand back its cancel token, derived from
/// the parent's so cancelling the parent cascades. The caller holds the
/// `active` lock: `send` decides "running or finished" and admits under the
/// one lock, so two sends cannot both start a run on the child's session.
fn admit(active: &mut HashMap<String, ActiveAgent>, task_id: &str, req: &SpawnRequest) -> CancellationToken {
    let cancel = req
        .parent_cancel
        .as_ref()
        .map(|p| p.child_token())
        .unwrap_or_else(CancellationToken::new);
    active.insert(
        task_id.to_string(),
        ActiveAgent {
            task_id: task_id.to_string(),
            description: req.description.clone(),
            status: "running".to_string(),
            cancel: cancel.clone(),
            session_key: Some(format!("subagent:{}:{}", req.parent_session_key, task_id)),
        },
    );
    cancel
}

/// Run a child's turn to its end, then release it from `active`.
///
/// A message its parent sent while it ran is heard inside the turn (the
/// runner's loop reads it at the next step, and before ending the turn). One
/// that lands after the loop has ended — between its last check and here —
/// has no model step after it; the child runs one more turn on its own
/// session to hear it, with no new prompt (the message is already in its
/// thread), and the parent gets both reports. The check and the release
/// happen under the `active` lock `send` delivers under, so a message
/// either lands before the check or finds the child finished.
///
/// A cancelled or stalled child does not go on: its message stays in its
/// thread, and the error or report says it was not heard.
async fn run_child(
    runner: &Arc<Runner>,
    active: &Arc<RwLock<HashMap<String, ActiveAgent>>>,
    task_id: &str,
    session_key: &str,
    first_prompt: String,
    spawn_req: &SpawnRequest,
    cancel: CancellationToken,
    parent_stream_tx: Option<mpsc::Sender<ai::StreamEvent>>,
) -> Result<String, String> {
    let mut prompt = first_prompt;
    let mut reports: Vec<String> = Vec::new();
    loop {
        let run_req = build_subagent_request(spawn_req, session_key, &prompt, &cancel);
        let result = run_and_collect(
            runner,
            run_req,
            cancel.clone(),
            None,
            parent_stream_tx.clone(),
            Some(SUBAGENT_INACTIVITY_TIMEOUT),
        )
        .await;

        let mut active = active.write().await;
        let unheard = runner
            .sessions()
            .resolve_session_id_by_key(session_key)
            .and_then(|id| runner.sessions().get_messages(&id))
            .is_ok_and(|messages| crate::harness::conversation::parent_message_unheard(&messages));
        match result {
            Ok(report) if unheard && !cancel.is_cancelled() => {
                info!(task_id = %task_id, "a message from the parent landed as the turn ended: running a turn to hear it");
                reports.push(report);
                prompt = String::new();
                drop(active);
            }
            Ok(report) => {
                active.remove(task_id);
                reports.push(report);
                if unheard {
                    reports.push(UNHEARD_NOTE.to_string());
                }
                return Ok(reports.join("\n\n"));
            }
            Err(e) => {
                active.remove(task_id);
                return Err(if unheard { format!("{e}. {UNHEARD_NOTE}") } else { e });
            }
        }
    }
}

/// Said to the parent when a child ended without hearing its last message.
const UNHEARD_NOTE: &str = "It stopped before it read your last message; that message is in its \
thread, and a send continues it from there.";

impl Orchestrator {
    pub fn new(runner: Arc<Runner>, store: Arc<Store>) -> Self {
        Self {
            runner,
            store,
            active: Arc::new(RwLock::new(HashMap::new())),
            lanes: None,
            wake_notify: None,
            resumable: Arc::new(RwLock::new(std::collections::VecDeque::new())),
        }
    }

    pub fn with_lanes(mut self, lanes: Arc<LaneManager>) -> Self {
        self.lanes = Some(lanes);
        self
    }

    pub fn with_wake_notify(mut self, tx: tokio::sync::mpsc::UnboundedSender<String>) -> Self {
        self.wake_notify = Some(tx);
        self
    }

    /// The user's own words, carried into every delegation.
    ///
    /// A parent agent writes the sub-agent's prompt as a SUMMARY of what the
    /// user asked, and summaries drop payloads: a pasted SVG logo, exact copy,
    /// a schema, a stack trace. Observed live — a deck task summarised as one
    /// subject line, and the logo the user pasted never reached the delegate,
    /// twice. The delegate must see the source material, not the paraphrase,
    /// so the originating user message travels verbatim with every spawn.
    ///
    /// Empty when there is no parent user message (cron, automation), or when
    /// the parent already embedded the full text in its prompt.
    fn original_request_block(&self, parent_session_id: &str, prompt: &str) -> String {
        let Ok(messages) = self.runner.sessions().get_messages(parent_session_id) else {
            return String::new();
        };
        let Some(user_msg) = messages
            .iter()
            .rev()
            .find(|m| m.role == "user" && !m.content.trim().is_empty())
        else {
            return String::new();
        };
        compose_original_block(&user_msg.content, prompt)
    }

    /// The brief every child gets: its type's prefix, the task, and the
    /// owner's own words (see `original_request_block`). One composition for
    /// every spawn path.
    fn child_prompt(&self, req: &SpawnRequest) -> String {
        let prefix = task_prefix_for_type(&AgentType::from_str(&req.agent_type));
        format!(
            "{}{}{}",
            prefix,
            req.prompt,
            self.original_request_block(&req.parent_session_id, &req.prompt)
        )
    }

    /// Spawn a single sub-agent.
    async fn spawn_internal(&self, req: SpawnRequest) -> Result<SpawnResult, String> {
        // Recursion guard: stop a "work together" prompt from exploding into an
        // unbounded subagent:subagent:subagent… tree (each node a full run).
        if subagent_depth(&req.parent_session_key) >= MAX_SUBAGENT_DEPTH {
            return Err(format!(
                "Sub-agent depth limit reached ({MAX_SUBAGENT_DEPTH} levels). Do this work \
                 yourself with your own tools and report the result — do not spawn another \
                 sub-agent."
            ));
        }
        let task_id = format!("sa-{}", uuid::Uuid::new_v4());
        let session_key = format!("subagent:{}:{}", req.parent_session_key, task_id);
        // Persist to pending_tasks
        let agent_type = AgentType::from_str(&req.agent_type);
        let _ = self.store.create_pending_task(
            &task_id,
            "subagent",
            &session_key,
            Some(&req.user_id),
            &req.prompt,
            Some(task_prefix_for_type(&agent_type).trim()),
            Some(&req.description),
            Some("subagent"),
            0,
            None,
        );

        let prefixed_prompt = self.child_prompt(&req);
        remember_resumable(&mut *self.resumable.write().await, task_id.clone(), resumable_copy(&req));
        self.launch(task_id, req, prefixed_prompt).await
    }

    /// `agent(task, send)`. A running child hears the message at its next
    /// step: it goes into the child's thread the way an owner's mid-turn
    /// message goes into theirs (`harness::conversation::MidTurnFrom`), and the child keeps
    /// working. A finished child continues on its own session with the
    /// message as its next user turn, with no task prefix and no
    /// original-request block: it already has both.
    ///
    /// The decision and the delivery happen under the `active` lock, the same
    /// lock `run_child` takes to check for an unheard message and release the
    /// child — so a message either reaches a child that will hear it, or
    /// finds it finished and continues it. It is never left in a thread no
    /// turn will read.
    async fn send_internal(
        &self,
        task_id: &str,
        message: &str,
        from_session_key: &str,
        taint: Vec<types::provenance::ProvenanceClass>,
        parent_cancel: Option<CancellationToken>,
        parent_stream_tx: Option<mpsc::Sender<ai::StreamEvent>>,
    ) -> Result<FollowUp, String> {
        let remembered = self
            .resumable
            .read()
            .await
            .iter()
            .find(|(id, _)| id == task_id)
            .map(|(_, req)| req.clone());
        let mut active = self.active.write().await;
        if let Some(agent) = active.get(task_id) {
            let Some(session_key) = agent.session_key.as_deref() else {
                return Err(format!(
                    "Sub-agent {task_id} is part of a parallel batch and takes no messages while \
                     the batch runs. Wait for the batch's results, then spawn a new sub-agent \
                     with the follow-up."
                ));
            };
            let sessions = self.runner.sessions();
            let session_id = sessions
                .resolve_session_id_by_key(session_key)
                .map_err(|e| format!("Could not reach sub-agent {task_id}: {e}"))?;
            let from = MidTurnFrom::Parent {
                session_key: from_session_key.to_string(),
                task_id: task_id.to_string(),
                taint,
            };
            sessions
                .append_message(&session_id, "user", message, None, None, Some(&from.metadata()))
                .map_err(|e| format!("Could not deliver the message to sub-agent {task_id}: {e}"))?;
            info!(task_id = %task_id, "message delivered into a running sub-agent");
            return Ok(FollowUp::Delivered { task_id: task_id.to_string() });
        }
        let Some(mut req) = remembered else {
            return Err(format!(
                "No sub-agent {task_id} to continue: it was not spawned from here, or it is not among \
                 the last {MAX_RESUMABLE} sub-agents spawned here. Spawn a new one with the \
                 full task in the prompt."
            ));
        };
        let follow_up = format!("{CONTINUATION_FRAME}{message}");
        req.prompt = follow_up.clone();
        req.parent_cancel = parent_cancel;
        req.parent_stream_tx = parent_stream_tx;
        let cancel = admit(&mut active, task_id, &req);
        drop(active);
        self.start(task_id.to_string(), req, follow_up, cancel)
            .await
            .map(FollowUp::Continued)
    }

    /// Run `req` as a new child `task_id`: admit it, then start it.
    async fn launch(
        &self,
        task_id: String,
        req: SpawnRequest,
        prefixed_prompt: String,
    ) -> Result<SpawnResult, String> {
        let cancel = admit(&mut *self.active.write().await, &task_id, &req);
        self.start(task_id, req, prefixed_prompt, cancel).await
    }

    /// Run an admitted child in the mode it asked for: blocking returns the
    /// output, background returns the ack and wakes the parent when done.
    /// Shared by a first spawn and a `send` continuation.
    async fn start(
        &self,
        task_id: String,
        req: SpawnRequest,
        prefixed_prompt: String,
        cancel: CancellationToken,
    ) -> Result<SpawnResult, String> {
        let session_key = format!("subagent:{}:{}", req.parent_session_key, task_id);
        if req.wait {
            // Blocking: run and return result
            let _ = self.store.update_task_running(&task_id);
            let result = run_child(
                &self.runner,
                &self.active,
                &task_id,
                &session_key,
                prefixed_prompt,
                &req,
                cancel,
                req.parent_stream_tx.clone(),
            )
            .await;

            match result {
                Ok(output) => {
                    let _ = self.store.update_task_completed(&task_id, Some(&output));
                    Ok(SpawnResult {
                        task_id,
                        success: true,
                        output,
                        error: None,
                    })
                }
                Err(e) => {
                    let _ = self.store.update_task_failed(&task_id, &e);
                    Ok(SpawnResult {
                        task_id,
                        success: false,
                        output: String::new(),
                        error: Some(e),
                    })
                }
            }
        } else {
            // Fire-and-forget: spawn background task
            let runner = self.runner.clone();
            let store = self.store.clone();
            let active = self.active.clone();
            let task_id_clone = task_id.clone();
            let prompt = prefixed_prompt;
            let parent_stream_tx = req.parent_stream_tx.clone();
            let spawn_req = req.clone();

            let parent_session_key = req.parent_session_key.clone();
            let description = req.description.clone();
            let wake_notify = self.wake_notify.clone();

            tokio::spawn(async move {
                let result = run_child(
                    &runner,
                    &active,
                    &task_id_clone,
                    &session_key,
                    prompt,
                    &spawn_req,
                    cancel,
                    parent_stream_tx,
                )
                .await;

                let wake_payload = match &result {
                    Ok(output) => format!(
                        "Task \"{}\" (id {}) FINISHED. Output:\n{}",
                        description, task_id_clone, output
                    ),
                    Err(e) => format!(
                        "Task \"{}\" (id {}) FAILED: {}",
                        description, task_id_clone, e
                    ),
                };
                match result {
                    Ok(output) => {
                        let _ = store.update_task_completed(&task_id_clone, Some(&output));
                    }
                    Err(e) => {
                        let _ = store.update_task_failed(&task_id_clone, &e);
                    }
                }

                // Session wake rail (R5): wake the parent — failure is a
                // completion too. Interactive parents only: workflow-internal
                // and nested-subagent parents are synchronously orchestrated,
                // not asleep.
                if is_interactive_session(&parent_session_key) {
                    let ok = store
                        .engine_enqueue_wake(&parent_session_key, "task_done", &wake_payload, "[]", 0)
                        .is_ok();
                    if ok && let Some(tx) = wake_notify {
                        let _ = tx.send(parent_session_key);
                    }
                }
            });

            Ok(SpawnResult {
                task_id,
                success: true,
                output: BACKGROUND_SPAWN_ACK.to_string(),
                error: None,
            })
        }
    }

    /// Execute a DAG of sub-tasks with reactive scheduling.
    async fn execute_dag_internal(
        &self,
        prompt: &str,
        parent: SpawnRequest,
    ) -> Result<SpawnResult, String> {
        // 1. Decompose task into sub-tasks
        info!("Decomposing task into sub-tasks");
        let nodes = decompose::decompose_task(&self.runner, prompt).await?;

        // Single-task optimization: skip DAG scheduler
        if decompose::is_single_task(&nodes) {
            info!("Single task decomposition — running directly");
            return self.spawn_internal(dag_node_request(&parent, &nodes[0])).await;
        }

        // 2. Build and validate DAG
        let mut graph = TaskGraph::new(nodes);
        graph.validate()?;

        let parent_task_id = format!("dag-{}", uuid::Uuid::new_v4());
        info!(
            task_id = %parent_task_id,
            sub_tasks = graph.len(),
            "Starting DAG execution"
        );

        // 3. Persist parent task
        let _ = self.store.create_pending_task(
            &parent_task_id,
            "dag",
            &parent.parent_session_key,
            Some(&parent.user_id),
            prompt,
            None,
            Some("DAG orchestration"),
            Some("subagent"),
            0,
            None,
        );

        // 4. Shared cancellation for the entire DAG — derived from parent so
        //    cancelling the parent cascades to all DAG tasks.
        let dag_cancel = parent
            .parent_cancel
            .as_ref()
            .map(|p| p.child_token())
            .unwrap_or_else(CancellationToken::new);

        // 5. Reactive scheduling loop
        let mut running: FuturesUnordered<
            Pin<Box<dyn Future<Output = (String, Result<String, String>)> + Send>>,
        > = FuturesUnordered::new();

        loop {
            // Start all tasks whose dependencies are satisfied
            let ready = graph.get_ready_tasks();
            for task_id in ready {
                let node = graph.nodes.get(&task_id).unwrap();
                let dep_context = format_dep_context(&graph.collect_dependency_results(&task_id));
                let task_prefix = task_prefix_for_type(&node.agent_type);
                let node_req = dag_node_request(&parent, node);
                let prompt = self.child_prompt(&node_req);
                let user_id = node_req.user_id.clone();
                let cancel = dag_cancel.clone();
                let session_key = format!("subagent:{}:{}", parent.parent_session_key, task_id);

                let runner = self.runner.clone();
                let store = self.store.clone();
                let child_task_id = format!("{}-{}", parent_task_id, task_id);

                // Persist child task
                let _ = store.create_pending_task(
                    &child_task_id,
                    "subagent",
                    &session_key,
                    Some(&user_id),
                    &prompt,
                    Some(task_prefix.trim()),
                    graph.nodes.get(&task_id).map(|n| n.description.as_str()),
                    Some("subagent"),
                    0,
                    Some(&parent_task_id),
                );

                graph.mark_running(&task_id);

                let tid = task_id.clone();
                running.push(Box::pin(async move {
                    // No permit here: the sub-task's runner takes an LLM permit
                    // for each call it makes. Holding one around the whole
                    // sub-task deadlocked the DAG at the permit floor (auditor
                    // Rule 15.2).
                    let _ = store.update_task_running(&child_task_id);

                    let full_prompt = if dep_context.is_empty() {
                        prompt
                    } else {
                        format!("{}\n\n{}", dep_context, prompt)
                    };

                    let req = build_subagent_request(&node_req, &session_key, &full_prompt, &cancel);

                    let result = run_and_collect(&runner, req, cancel, None, None, None).await;

                    match &result {
                        Ok(output) => {
                            let _ =
                                store.update_task_completed(&child_task_id, Some(output.as_str()));
                        }
                        Err(e) => {
                            let _ = store.update_task_failed(&child_task_id, e);
                        }
                    }

                    (tid, result)
                }));
            }

            // All done?
            if running.is_empty() {
                break;
            }

            // Wait for ANY task to complete (reactive!)
            let (task_id, result) = running.next().await.unwrap();

            match result {
                Ok(output) => {
                    info!(task_id = %task_id, output_len = output.len(), "Sub-task completed");
                    graph.mark_completed(&task_id, output);
                }
                Err(e) => {
                    warn!(task_id = %task_id, error = %e, "Sub-task failed");
                    graph.mark_failed(&task_id, e);
                    // Continue — let dependents see the failure and get blocked naturally
                }
            }

            // Loop back → get_ready_tasks() now includes newly unblocked tasks
        }

        // 6. Synthesize final result
        let output = graph.synthesize_results();
        let success = !graph.has_failures();

        let _ = if success {
            self.store
                .update_task_completed(&parent_task_id, Some(&output))
        } else {
            self.store
                .update_task_failed(&parent_task_id, "One or more sub-tasks failed")
        };

        info!(
            task_id = %parent_task_id,
            success = success,
            output_len = output.len(),
            "DAG execution complete"
        );

        Ok(SpawnResult {
            task_id: parent_task_id,
            success,
            output,
            error: if success {
                None
            } else {
                Some("One or more sub-tasks failed".to_string())
            },
        })
    }

    /// Cancel a running task.
    async fn cancel_internal(&self, task_id: &str) -> Result<(), String> {
        let mut active = self.active.write().await;
        // Cancelling a parent takes its descendants, live or not yet started:
        // the token cascades to running children, the rows to every one.
        if let Some(agent) = active.remove(task_id) {
            agent.cancel.cancel();
            let _ = self.store.cancel_task(task_id);
            let _ = self.store.cancel_child_tasks(task_id);
            info!(task_id = %task_id, "Cancelled sub-agent");
            Ok(())
        } else {
            let _ = self.store.cancel_task(task_id);
            let _ = self.store.cancel_child_tasks(task_id);
            Ok(())
        }
    }

    /// Get status of a task.
    async fn status_internal(&self, task_id: &str) -> Result<String, String> {
        // Check active map first
        {
            let active = self.active.read().await;
            if let Some(agent) = active.get(task_id) {
                return Ok(format!(
                    "Task: {}\nDescription: {}\nStatus: {}",
                    agent.task_id, agent.description, agent.status
                ));
            }
        }

        // Fall back to database
        match self.store.get_pending_task(task_id) {
            Ok(Some(task)) => {
                let mut result = format!(
                    "Task: {}\nType: {}\nStatus: {}\nDescription: {}",
                    task.id,
                    task.task_type,
                    task.status,
                    task.description.as_deref().unwrap_or("")
                );
                if let Some(ref output) = task.output {
                    result.push_str(&format!("\nOutput:\n{}", output));
                }
                if let Some(ref err) = task.last_error {
                    result.push_str(&format!("\nError: {}", err));
                }
                Ok(result)
            }
            Ok(None) => Err(format!("Task '{}' not found", task_id)),
            Err(e) => Err(format!("Failed to get task status: {}", e)),
        }
    }

    /// List all active sub-agents.
    async fn list_active_internal(&self) -> Vec<(String, String, String)> {
        let active = self.active.read().await;
        active
            .values()
            .map(|a| (a.task_id.clone(), a.description.clone(), a.status.clone()))
            .collect()
    }

    /// Spawn multiple sub-agents in parallel and wait for all to complete.
    /// Sends SubagentStart/Progress/Complete events via progress_tx.
    async fn spawn_parallel_internal(
        &self,
        requests: Vec<SpawnRequest>,
        progress_tx: mpsc::Sender<ai::StreamEvent>,
    ) -> Result<SpawnResult, String> {
        use ai::StreamEvent;

        // Recursion guard (siblings share a parent): a parallel batch spawned
        // from too deep in the tree is refused, same as single spawns.
        if requests
            .first()
            .is_some_and(|r| subagent_depth(&r.parent_session_key) >= MAX_SUBAGENT_DEPTH)
        {
            return Err(format!(
                "Sub-agent depth limit reached ({MAX_SUBAGENT_DEPTH} levels). Do this work \
                 yourself instead of spawning more sub-agents."
            ));
        }

        let parent_task_id = format!("batch-{}", uuid::Uuid::new_v4());
        let total = requests.len();

        // Isolation (P5.3): each child gets its own copy of the project,
        // fenced to it (cwd + the run's fence); merged back after the batch.
        let isolate = requests.first().is_some_and(|r| r.isolate == "worktree");
        // The project to isolate: the one named, else where the parent works.
        let workspace: PathBuf = match requests.first() {
            Some(r) if !r.workspace.is_empty() => PathBuf::from(&r.workspace),
            Some(SpawnRequest { seat: tools::orchestrator::ChildSeat { cwd: Some(cwd), .. }, .. }) => {
                PathBuf::from(cwd)
            }
            _ => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        };
        if isolate && !workspace.is_dir() {
            return Err(format!(
                "workspace {} is not a folder. Pass the project folder to isolate, or leave isolate out.",
                workspace.display()
            ));
        }
        let mut isolations: Vec<crate::worktree::Isolation> = Vec::new();

        // Internal channel for progress from all sub-agents
        let (prog_tx, mut prog_rx) = mpsc::channel::<SubagentProgress>(64);

        // Spawn all sub-agents
        let mut running: FuturesUnordered<
            Pin<
                Box<
                    dyn Future<Output = (String, String, Result<String, String>, usize, i32)>
                        + Send,
                >,
            >,
        > = FuturesUnordered::new();

        for mut req in requests {
            let task_id = format!("sa-{}", uuid::Uuid::new_v4());
            let session_key = format!("subagent:{}:{}", req.parent_session_key, task_id);
            let cancel = req
                .parent_cancel
                .as_ref()
                .map(|p| p.child_token())
                .unwrap_or_else(CancellationToken::new);

            let agent_type = AgentType::from_str(&req.agent_type);
            let task_prefix = task_prefix_for_type(&agent_type);
            let mut prefixed_prompt = self.child_prompt(&req);
            let description = req.description.clone();

            // Persist to DB
            let _ = self.store.create_pending_task(
                &task_id,
                "subagent",
                &session_key,
                Some(&req.user_id),
                &prefixed_prompt,
                Some(task_prefix.trim()),
                Some(&description),
                Some("subagent"),
                0,
                None,
            );

            // Register active
            {
                let mut active = self.active.write().await;
                active.insert(
                    task_id.clone(),
                    ActiveAgent {
                        task_id: task_id.clone(),
                        description: description.clone(),
                        status: "running".to_string(),
                        cancel: cancel.clone(),
                        session_key: None,
                    },
                );
            }

            // Send SubagentStart event (canonical constructor + spawn-batch metrics).
            let mut start_ev = StreamEvent::subagent_start(task_id.as_str(), description.as_str());
            if let Some(serde_json::Value::Object(w)) = start_ev.widgets.as_mut() {
                w.insert("agent_type".to_string(), serde_json::json!(req.agent_type));
                w.insert("total_count".to_string(), serde_json::json!(total));
            }
            let _ = progress_tx.send(start_ev).await;

            let runner = self.runner.clone();
            let store = self.store.clone();
            let active = self.active.clone();
            let tid = task_id.clone();
            let desc = description.clone();
            let prog_tx_clone = prog_tx.clone();

            if isolate {
                // The child works in its own copy, fenced to it — a narrowing
                // of the parent's fence, refused when the project lies outside it.
                let isolated = match crate::worktree::create(&workspace, &task_id).await {
                    Ok(iso) => {
                        let path = iso.path().to_string_lossy().into_owned();
                        prefixed_prompt = format!("{}{}", crate::worktree::preamble(&iso), prefixed_prompt);
                        isolations.push(iso);
                        req.seat.isolate_to(&workspace.to_string_lossy(), &path)
                    }
                    Err(e) => Err(format!("could not isolate {}: {e}", workspace.display())),
                };
                if let Err(e) = isolated {
                    // Undo what this batch already isolated; nothing ran yet.
                    let _ = crate::worktree::merge_all(&isolations, "nebo: aborted batch").await;
                    return Err(e);
                }
            }
            let run_req = build_subagent_request(&req, &session_key, &prefixed_prompt, &cancel);

            running.push(Box::pin(async move {
                let result = run_and_collect(
                    &runner, run_req, cancel,
                    Some((tid.clone(), prog_tx_clone)),
                    None,
                    Some(WORKER_INACTIVITY_TIMEOUT),
                ).await;

                let (tool_count, token_count) = (0usize, 0i32); // final counts come from progress events
                match &result {
                    // Stalled worker: the partial output still flows to the parent,
                    // but the task is recorded as failed so telemetry sees the stall.
                    Ok(output) if output.starts_with(STALL_MARKER_PREFIX) => {
                        warn!(task_id = %tid, "worker stalled: no activity for {}s; keeping partial output", WORKER_INACTIVITY_TIMEOUT.as_secs());
                        let _ = store.update_task_failed(&tid, &stall_marker(WORKER_INACTIVITY_TIMEOUT));
                    }
                    Ok(output) => {
                        let _ = store.update_task_completed(&tid, Some(output.as_str()));
                    }
                    Err(e) => {
                        let _ = store.update_task_failed(&tid, e);
                    }
                }
                active.write().await.remove(&tid);

                (tid, desc, result, tool_count, token_count)
            }));
        }

        // Drop our copy so prog_rx closes when all sub-agents finish
        drop(prog_tx);

        // Collect results and forward progress events
        let mut results: Vec<(String, String, Result<String, String>)> = Vec::new();
        let mut agent_metrics: HashMap<String, (usize, i32)> = HashMap::new();

        loop {
            tokio::select! {
                // Forward progress from sub-agents
                prog = prog_rx.recv() => {
                    if let Some(p) = prog {
                        agent_metrics.insert(p.task_id.clone(), (p.tool_count, p.token_count));
                        let _ = progress_tx.send(StreamEvent { payload: None,
                            provenance: None,
                            event_type: StreamEventType::SubagentProgress,
                            text: p.current_operation.clone(),
                            tool_call: None,
                            error: Some(p.task_id.clone()),
                            usage: None,
                            rate_limit: None,
                            widgets: Some(serde_json::json!({
                                "task_id": p.task_id,
                                "tool_count": p.tool_count,
                                "token_count": p.token_count,
                                "current_operation": p.current_operation,
                            })),
                            provider_metadata: None,
                            stop_reason: None,
                            image_url: None,
                        }).await;
                    }
                }
                // Collect completed sub-agents
                completed = running.next() => {
                    match completed {
                        Some((tid, desc, result, _, _)) => {
                            let (tool_count, token_count) = agent_metrics.get(&tid).copied().unwrap_or((0, 0));
                            let success = result.is_ok();
                            let mut done_ev =
                                StreamEvent::subagent_complete(tid.as_str(), desc.as_str(), success);
                            if let Some(serde_json::Value::Object(w)) = done_ev.widgets.as_mut() {
                                w.insert("tool_count".to_string(), serde_json::json!(tool_count));
                                w.insert("token_count".to_string(), serde_json::json!(token_count));
                            }
                            let _ = progress_tx.send(done_ev).await;
                            results.push((tid, desc, result));
                        }
                        None => break, // All done
                    }
                }
            }
        }

        // Synthesize combined output
        let mut output_parts = Vec::new();
        let mut all_success = true;
        for (_, desc, result) in &results {
            match result {
                Ok(text) => {
                    output_parts.push(format!("## {}\n\n{}", desc, text));
                }
                Err(e) => {
                    all_success = false;
                    output_parts.push(format!("## {} (FAILED)\n\n{}", desc, e));
                }
            }
        }

        let mut combined = format!(
            "{} sub-agents completed ({} succeeded, {} failed):\n\n{}",
            results.len(),
            results.iter().filter(|(_, _, r)| r.is_ok()).count(),
            results.iter().filter(|(_, _, r)| r.is_err()).count(),
            output_parts.join("\n\n---\n\n"),
        );
        if !isolations.is_empty() {
            let outcomes = crate::worktree::merge_all(&isolations, "nebo: parallel batch").await;
            combined.push_str("\n\n## Worktree merges\n\n");
            for (tid, outcome) in &outcomes {
                let desc = results
                    .iter()
                    .find(|(t, _, _)| t == tid)
                    .map(|(_, d, _)| d.as_str())
                    .unwrap_or(tid.as_str());
                combined.push_str(&crate::worktree::render_outcome(desc, outcome));
                combined.push('\n');
                if matches!(outcome, crate::worktree::MergeOutcome::Conflict { .. } | crate::worktree::MergeOutcome::Failed { .. }) {
                    all_success = false;
                }
            }
        }

        Ok(SpawnResult {
            task_id: parent_task_id,
            success: all_success,
            output: combined,
            error: if all_success {
                None
            } else {
                Some("One or more sub-agents failed".to_string())
            },
        })
    }

    /// Check whether a task's session appears complete based on message heuristics.
    fn check_task_completion(&self, session_key: &str) -> bool {
        let messages = match self.store.get_chat_messages(session_key) {
            Ok(m) => m,
            Err(_) => return false,
        };
        check_completion_heuristic(&messages)
    }

    /// Recover incomplete tasks from previous crash.
    /// Uses completion heuristic to determine whether to mark complete or re-spawn.
    async fn recover_internal(&self) {
        // Copies a crashed parent left behind (fail-closed: anything with
        // work in it is kept).
        let swept = crate::worktree::cleanup_stale(crate::worktree::STALE_AFTER_SECS).await;
        if !swept.is_empty() {
            info!(count = swept.len(), "swept stale worktrees/scratch copies");
        }
        let tasks = match self.store.get_recoverable_tasks() {
            Ok(t) => t,
            Err(e) => {
                warn!(error = %e, "Failed to load recoverable tasks");
                return;
            }
        };

        let now = chrono::Utc::now().timestamp();

        for task in tasks {
            if task.task_type != "subagent" && task.task_type != "dag" {
                continue;
            }

            let age_secs = now - task.created_at;

            // Skip tasks older than 2 hours
            if age_secs > 2 * 3600 {
                debug!(task_id = %task.id, age_secs, "Skipping stale task");
                let _ = self
                    .store
                    .update_task_failed(&task.id, "Stale: older than 2 hours");
                continue;
            }

            // Skip tasks that exceeded retry limit
            if task.attempts.unwrap_or(0) >= task.max_attempts.unwrap_or(3) {
                debug!(task_id = %task.id, "Skipping exhausted task");
                let _ = self
                    .store
                    .update_task_failed(&task.id, "Max retry attempts exceeded");
                continue;
            }

            // Check completion heuristic — if session looks complete, mark done
            if self.check_task_completion(&task.session_key) {
                info!(task_id = %task.id, "Task session appears complete, marking completed");
                let _ = self.store.update_task_completed(&task.id, None);
                continue;
            }

            // Re-spawn viable tasks
            info!(task_id = %task.id, task_type = %task.task_type, "Re-spawning recovered task");

            // Count this recovery as an attempt (sets status='running', attempts += 1). Without
            // this, a task interrupted mid-run (e.g. a restart killing it) stays attempts=1
            // forever, so the `attempts >= max_attempts` guard above never fires and the task
            // re-runs the SAME work on every restart indefinitely. Bumping it here makes the
            // retry bounded: after max_attempts interruptions the next pass marks it failed.
            let _ = self.store.update_task_running(&task.id);

            let runner = self.runner.clone();
            let store = self.store.clone();
            let task_id = task.id.clone();
            let session_key = task.session_key.clone();
            let prompt = task.prompt.clone();
            let system = task.system_prompt.unwrap_or_default();
            let user_id = task.user_id.unwrap_or_default();
            let lane = task.lane.as_deref().unwrap_or("subagent").to_string();

            let future = async move {
                let req = RunRequest {
                    session_key,
                    prompt,
                    system,
                    user_id,
                    skip_memory_extract: true,
                    origin: tools::Origin::System,
                    channel: "recovery".to_string(),
                    ..Default::default()
                };

                let cancel = CancellationToken::new();
                match run_and_collect(&runner, req, cancel, None, None, None).await {
                    Ok(output) => {
                        let _ = store.update_task_completed(&task_id, Some(&output));
                    }
                    Err(e) => {
                        let _ = store.update_task_failed(&task_id, &e);
                    }
                }
                Ok(())
            };

            // Route through lanes if available, otherwise tokio::spawn
            if let Some(ref lanes) = self.lanes {
                let task = lanes::make_task(&lane, format!("recover:{}", task.id), future);
                lanes.enqueue_async(&lane, task);
            } else {
                tokio::spawn(future);
            }
        }
    }
}

/// Check whether a message list suggests the task completed. Matches Go's heuristic.
fn check_completion_heuristic(messages: &[db::models::ChatMessage]) -> bool {
    // Rule 1: No messages → incomplete
    if messages.is_empty() {
        return false;
    }

    // Rule 2: Has tool calls → complete (side effects may have happened)
    let has_tool_calls = messages
        .iter()
        .any(|m| m.tool_calls.as_ref().map_or(false, |tc| !tc.is_empty()));
    if has_tool_calls {
        return true;
    }

    // Rule 3: Multiple assistant messages with >2 total → complete (at least one loop)
    let assistant_count = messages.iter().filter(|m| m.role == "assistant").count();
    if assistant_count > 0 && messages.len() > 2 {
        return true;
    }

    // Rule 4: Last message from assistant with substantial content → complete
    if let Some(last) = messages.last() {
        if last.role == "assistant" && last.content.len() > 50 {
            return true;
        }
    }

    false
}

/// Build a RunRequest for a sub-agent. Single source of truth for sub-agent request construction.
/// Uses PromptMode::Minimal — sub-agents get identity + capabilities + behavior core,
/// but skip memory docs, tool routing guide, etiquette, comm style, and autonomy sections.
/// A session that can be woken (R5): an owner-facing chat session — not a
/// nested subagent and not a workflow run's ephemeral execution session.
fn is_interactive_session(session_key: &str) -> bool {
    !session_key.is_empty()
        && !session_key.starts_with("subagent:")
        && !session_key.starts_with("workflow-")
        && !session_key.contains(":workflow:")
}

/// The ONE builder of a child's run: a single spawn, a background spawn, a
/// `send` continuation, every member of a parallel batch and every DAG node
/// come through here. The child runs under its parent's limits (`seat`): the
/// parent's grant as its ceiling (mode, rules, money limits), its fence, its
/// tool allowlist and its taint so far. It can only narrow them.
fn build_subagent_request(
    spawn_req: &SpawnRequest,
    session_key: &str,
    prompt: &str,
    cancel: &CancellationToken,
) -> RunRequest {
    let seat = &spawn_req.seat;
    let mut run_req = RunRequest {
        session_key: session_key.to_string(),
        prompt: prompt.to_string(),
        model_override: spawn_req.model_override.clone(),
        user_id: spawn_req.user_id.clone(),
        skip_memory_extract: true,
        origin: tools::Origin::System,
        channel: "subagent".to_string(),
        cancel_token: cancel.clone(),
        prompt_mode: crate::prompt::PromptMode::Minimal,
        max_iterations: spawn_req.max_iterations,
        // A sub-agent stays at the parent's hop depth — a spawn must not
        // restart the coworker chain cap at zero.
        handoff_depth: spawn_req.handoff_depth,
        door: types::permissions::Door::Helper,
        ceiling: seat.grant.as_ref().map(|g| types::permissions::Ceiling::Parent { grant: Box::new((**g).clone()) }),
        fence: seat.fence.clone(),
        tool_allowlist: seat.tool_allowlist.clone(),
        tool_denial_hint: seat.tool_denial_hint.clone(),
        cwd: seat.cwd.clone(),
        seed_taint: seat.taint.clone(),
        ..Default::default()
    };
    if !spawn_req.skills.is_empty() {
        run_req.preload_skills = spawn_req.skills.clone();
    }
    run_req
}

/// A DAG node as a child of `parent`: the node's task on the parent's seat.
/// A node names a model only when the decomposition asked for one; otherwise
/// the whole DAG runs at the parent's.
fn dag_node_request(parent: &SpawnRequest, node: &crate::task_graph::TaskNode) -> SpawnRequest {
    SpawnRequest {
        prompt: node.prompt.clone(),
        description: node.description.clone(),
        agent_type: node.agent_type.as_str().to_string(),
        model_override: if node.model_override.is_empty() {
            parent.model_override.clone()
        } else {
            node.model_override.clone()
        },
        wait: true,
        ..parent.clone()
    }
}

/// Progress update emitted during sub-agent execution.
#[derive(Debug, Clone)]
pub struct SubagentProgress {
    pub task_id: String,
    pub tool_count: usize,
    pub token_count: i32,
    pub current_operation: String,
}

/// Run a RunRequest via the Runner and collect text output from the stream.
/// Accepts an optional progress sender for tracking tool counts and current operations.
/// When `inactivity_timeout` is set, the run is aborted (token cancelled) after that
/// long with no stream activity, and the accumulated output is returned as `Ok`
/// prefixed with the stall marker (`stall_marker`) — any event resets the window.
async fn run_and_collect(
    runner: &Arc<Runner>,
    req: RunRequest,
    cancel: CancellationToken,
    progress_tx: Option<(String, mpsc::Sender<SubagentProgress>)>,
    parent_stream_tx: Option<mpsc::Sender<ai::StreamEvent>>,
    inactivity_timeout: Option<std::time::Duration>,
) -> Result<String, String> {
    // The child's own id, for the progress events its parent's screen shows.
    let child_id = req.session_key.rsplit(':').next().unwrap_or_default().to_string();
    let mut rx = runner
        .run(req)
        .await
        .map_err(|e| format!("Failed to start sub-agent: {}", e))?;

    let mut output = String::new();
    let mut tool_count: usize = 0;
    let mut token_count: i32 = 0;
    let mut last_activity = tokio::time::Instant::now();
    let mut stalled = false;

    loop {
        // Recomputed each iteration so any stream event below resets the window.
        let stall_deadline = inactivity_timeout.map(|window| last_activity + window);
        tokio::select! {
            _ = cancel.cancelled() => {
                return Err("Cancelled".to_string());
            }
            // Inactivity guard: fires only when enabled and no stream event has
            // arrived for the whole window.
            _ = async {
                match stall_deadline {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                cancel.cancel();
                stalled = true;
                break;
            }
            event = rx.recv() => {
                match event {
                    Some(e) => {
                        last_activity = tokio::time::Instant::now();
                        match e.event_type {
                            StreamEventType::Text => output.push_str(&e.text),
                            StreamEventType::ToolCall => {
                                // Progress is a UI event on the parent's
                                // stream, never text in its reply.
                                if let (Some(ptx), Some(tc)) = (&parent_stream_tx, &e.tool_call) {
                                    let op = describe_tool_call(tc);
                                    let mut ev = ai::StreamEvent::subagent_start(child_id.as_str(), op.as_str());
                                    ev.event_type = StreamEventType::SubagentProgress;
                                    ev.widgets = Some(serde_json::json!({
                                        "task_id": child_id,
                                        "tool_count": tool_count,
                                        "current_operation": op,
                                    }));
                                    let _ = ptx.send(ev).await;
                                }
                                if let Some((ref tid, ref tx)) = progress_tx {
                                    let op = e.tool_call.as_ref()
                                        .map(describe_tool_call)
                                        .unwrap_or_default();
                                    let _ = tx.send(SubagentProgress {
                                        task_id: tid.clone(),
                                        tool_count,
                                        token_count,
                                        current_operation: op,
                                    }).await;
                                }
                            }
                            StreamEventType::ToolResult => {
                                tool_count += 1;
                                if let Some((ref tid, ref tx)) = progress_tx {
                                    let _ = tx.send(SubagentProgress {
                                        task_id: tid.clone(),
                                        tool_count,
                                        token_count,
                                        current_operation: String::new(),
                                    }).await;
                                }
                            }
                            StreamEventType::Usage => {
                                if let Some(ref usage) = e.usage {
                                    token_count += usage.input_tokens + usage.output_tokens;
                                }
                            }
                            StreamEventType::Error => {
                                if let Some(err) = e.error {
                                    return Err(err);
                                }
                            }
                            StreamEventType::AskRequest
                            | StreamEventType::ApprovalRequest
                            | StreamEventType::PlanApproval => {
                                // Forward permission/ask events to parent so they reach the user
                                if let Some(ref ptx) = parent_stream_tx {
                                    let _ = ptx.send(e).await;
                                }
                            }
                            StreamEventType::Done => break,
                            _ => {}
                        }
                    }
                    None => break, // Channel closed
                }
            }
        }
    }

    if stalled {
        // Return what was accumulated instead of discarding it; the marker
        // prefix lets the caller record the task as stalled for telemetry.
        output = if output.is_empty() {
            format!("{} (no output produced)", stall_marker(inactivity_timeout.unwrap_or_default()))
        } else {
            format!("{}\n\n{output}", stall_marker(inactivity_timeout.unwrap_or_default()))
        };
    }

    if output.is_empty() {
        Ok("Sub-agent completed (no text output).".to_string())
    } else {
        // Cap sub-agent output to prevent flooding parent context.
        // Default 32K — parent can Read the full file if it needs more.
        const MAX_SUBAGENT_OUTPUT: usize = 32_000;
        if output.len() > MAX_SUBAGENT_OUTPUT {
            let total = output.len();
            output.truncate(MAX_SUBAGENT_OUTPUT);
            // Find a clean char boundary
            while !output.is_char_boundary(output.len()) {
                output.pop();
            }
            output.push_str(&format!(
                "\n\n[Sub-agent output truncated: {} chars total, showing first {}.]",
                total, MAX_SUBAGENT_OUTPUT
            ));
        }
        Ok(output)
    }
}

/// Format dependency results as context for a dependent task.
fn format_dep_context(deps: &[(String, String)]) -> String {
    if deps.is_empty() {
        return String::new();
    }

    let mut parts = vec!["[Results from prerequisite tasks]\n".to_string()];

    for (desc, result) in deps {
        let truncated = match crate::harness::delegation::collect::clip_chars(result, MAX_DEP_CONTEXT_CHARS) {
            Some(head) => format!("{head}...(truncated)"),
            None => result.clone(),
        };
        parts.push(format!(
            "--- Task \"{}\" (completed) ---\n{}\n",
            desc, truncated
        ));
    }

    parts.push("---\n\nYour task:".to_string());
    parts.join("\n")
}

/// Generate a task prefix based on agent type.
/// These constraints are prepended to the user's prompt rather than the system prompt,
/// so sub-agents get the standard Minimal system prompt (identity, capabilities, behavior)
/// plus task-specific instructions in the user message.
fn task_prefix_for_type(agent_type: &AgentType) -> &'static str {
    match agent_type {
        AgentType::Explore => {
            "[EXPLORE agent — a fast, read-only file & code search specialist.\n\
             READ-ONLY: only search and read. Do NOT create, edit, or delete files, and do NOT run destructive or state-changing commands.\n\
             Be FAST: search smartly, and wherever possible issue MULTIPLE PARALLEL tool calls for grepping and reading files in a single response. Start broad, then narrow; check multiple locations and naming conventions.\n\
             Report your findings clearly and concisely — the caller relays your final message, so include the essentials: file paths, line numbers, and what you found.]\n\n"
        }
        AgentType::Plan => {
            "[PLANNING agent — analyze, break down steps, identify files and patterns. Produce a clear actionable plan. Do NOT implement anything.]\n\n"
        }
        AgentType::General => "[Execute the task using whatever tools are needed.]\n\n",
    }
}

/// Implement the SubAgentOrchestrator trait for use via OrchestratorHandle.
impl SubAgentOrchestrator for Orchestrator {
    fn spawn(
        &self,
        req: SpawnRequest,
    ) -> Pin<Box<dyn Future<Output = Result<SpawnResult, String>> + Send + '_>> {
        Box::pin(async move { self.spawn_internal(req).await })
    }

    fn execute_dag(
        &self,
        prompt: &str,
        parent: SpawnRequest,
    ) -> Pin<Box<dyn Future<Output = Result<SpawnResult, String>> + Send + '_>> {
        let prompt = prompt.to_string();
        Box::pin(async move { self.execute_dag_internal(&prompt, parent).await })
    }

    fn cancel(
        &self,
        task_id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
        let task_id = task_id.to_string();
        Box::pin(async move { self.cancel_internal(&task_id).await })
    }

    fn status(
        &self,
        task_id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + '_>> {
        let task_id = task_id.to_string();
        Box::pin(async move { self.status_internal(&task_id).await })
    }

    fn send(
        &self,
        task_id: &str,
        message: &str,
        from_session_key: &str,
        taint: Vec<types::provenance::ProvenanceClass>,
        parent_cancel: Option<CancellationToken>,
        parent_stream_tx: Option<mpsc::Sender<ai::StreamEvent>>,
    ) -> Pin<Box<dyn Future<Output = Result<FollowUp, String>> + Send + '_>> {
        let task_id = task_id.to_string();
        let message = message.to_string();
        let from_session_key = from_session_key.to_string();
        Box::pin(async move {
            self.send_internal(&task_id, &message, &from_session_key, taint, parent_cancel, parent_stream_tx)
                .await
        })
    }

    fn list_active(
        &self,
    ) -> Pin<Box<dyn Future<Output = Vec<(String, String, String)>> + Send + '_>> {
        Box::pin(async move { self.list_active_internal().await })
    }

    fn spawn_parallel(
        &self,
        requests: Vec<SpawnRequest>,
        progress_tx: mpsc::Sender<ai::StreamEvent>,
    ) -> Pin<Box<dyn Future<Output = Result<SpawnResult, String>> + Send + '_>> {
        Box::pin(async move { self.spawn_parallel_internal(requests, progress_tx).await })
    }

    fn recover(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move { self.recover_internal().await })
    }
}

#[cfg(test)]
mod child_limits {
    use super::*;
    use tools::ToolContext;
    use types::provenance::ProvenanceClass;
    type Fut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

    /// Records what the agent tool hands the orchestrator: every child
    /// request, and the parent a decomposition was asked for from.
    #[derive(Default, Clone)]
    struct Recorder(Arc<std::sync::Mutex<Vec<SpawnRequest>>>);
    impl Recorder {
        fn take(&self) -> Vec<SpawnRequest> {
            std::mem::take(&mut *self.0.lock().unwrap())
        }
    }
    fn recorded() -> Result<SpawnResult, String> {
        Ok(SpawnResult { task_id: "t".into(), success: true, output: "done".into(), error: None })
    }
    impl SubAgentOrchestrator for Recorder {
        fn spawn(&self, req: SpawnRequest) -> Fut<'_, Result<SpawnResult, String>> {
            self.0.lock().unwrap().push(req);
            Box::pin(async { recorded() })
        }
        fn execute_dag(&self, _: &str, parent: SpawnRequest) -> Fut<'_, Result<SpawnResult, String>> {
            self.0.lock().unwrap().push(parent);
            Box::pin(async { recorded() })
        }
        fn cancel(&self, _: &str) -> Fut<'_, Result<(), String>> {
            Box::pin(async { Ok(()) })
        }
        fn status(&self, _: &str) -> Fut<'_, Result<String, String>> {
            Box::pin(async { Ok(String::new()) })
        }
        fn send(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: Vec<ProvenanceClass>,
            _: Option<CancellationToken>,
            _: Option<mpsc::Sender<ai::StreamEvent>>,
        ) -> Fut<'_, Result<FollowUp, String>> {
            Box::pin(async { recorded().map(FollowUp::Continued) })
        }
        fn list_active(&self) -> Fut<'_, Vec<(String, String, String)>> {
            Box::pin(async { Vec::new() })
        }
        fn spawn_parallel(
            &self,
            requests: Vec<SpawnRequest>,
            _: mpsc::Sender<ai::StreamEvent>,
        ) -> Fut<'_, Result<SpawnResult, String>> {
            self.0.lock().unwrap().extend(requests);
            Box::pin(async { recorded() })
        }
        fn recover(&self) -> Fut<'_, ()> {
            Box::pin(async {})
        }
    }

    /// The helper tools (`delegate`, `orchestrate`, …) over the recorder.
    fn helper_tools(rec: &Recorder) -> (tempfile::TempDir, Vec<Box<dyn tools::registry::DynTool>>) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(db::Store::new(&dir.path().join("t.db").to_string_lossy()).unwrap());
        let handle = tools::new_handle();
        let _ = handle.set(Box::new(rec.clone()));
        (dir, tools::helper_tools::Helpers::new(store, handle).tools())
    }

    fn rule(key: types::permissions::RuleKey, field: Option<types::permissions::RuleField>, effect: types::permissions::Effect) -> types::permissions::Rule {
        types::permissions::Rule {
            id: format!("{}-{}", key.value(), effect.as_str()),
            scope: types::permissions::Scope::Employee("a1".into()),
            key,
            field,
            effect,
            money: None,
            source: types::permissions::RuleSource::Owner,
            locked: false,
            created_at: 0,
        }
    }

    /// An employee with shell denied, the browser denied, one operation
    /// denied, a folder fence, a tool allowlist, and web content already in
    /// its run.
    fn limited_parent() -> ToolContext {
        use types::permissions::{Effect, Grant, Mode, RuleField, RuleKey};
        let (tx, _rx) = mpsc::channel(8);
        let mut grant = Grant::new("a1", Mode::Automatic);
        grant.rules = vec![
            rule(RuleKey::Capability("shell".into()), None, Effect::Deny),
            rule(RuleKey::Tool("browser_*".into()), None, Effect::Deny),
            rule(RuleKey::Operation("payments.transfer.send".into()), None, Effect::Deny),
            rule(RuleKey::Capability("file".into()), Some(RuleField::Folder("/work/a".into())), Effect::Allow),
        ];
        grant.fence = Some(vec!["/work/a".into()]);
        ToolContext {
            session_id: "s1".into(),
            session_key: "agent:a1:web".into(),
            user_id: "owner:agent:a1".into(),
            grant: Some(std::sync::Arc::new(grant)),
            tool_whitelist: Some(["agent".to_string(), "os".to_string()].into_iter().collect()),
            whitelist_denial_hint: Some("not in this run".into()),
            cwd: Some("/work/a".into()),
            run_taint: vec![ProvenanceClass::Web, ProvenanceClass::ExternalEmail],
            stream_tx: Some(tx),
            ..Default::default()
        }
    }

    /// The child's run carries every limit its parent ran under: the
    /// parent's grant is its ceiling.
    fn assert_limited_like(child: &RunRequest, parent: &ToolContext, path: &str) {
        let ceiling = match &child.ceiling {
            Some(types::permissions::Ceiling::Parent { grant }) => grant,
            other => panic!("{path}: the child has no parent ceiling: {other:?}"),
        };
        assert_eq!(Some(&**ceiling), parent.grant.as_deref(), "{path}: the parent's grant was not the ceiling");
        assert_eq!(child.fence, parent.grant.as_ref().and_then(|g| g.fence.clone()), "{path}: fence lost");
        assert_eq!(child.tool_allowlist, parent.tool_whitelist, "{path}: tool allowlist lost");
        assert_eq!(child.tool_denial_hint, parent.whitelist_denial_hint, "{path}: denial hint lost");
        assert_eq!(child.cwd, parent.cwd, "{path}: working directory lost");
        assert_eq!(child.seed_taint, parent.run_taint, "{path}: taint laundered");
        assert_eq!(child.user_id, parent.user_id, "{path}: memory scope lost");
        assert_eq!(child.door, types::permissions::Door::Helper, "{path}: a child is a helper");
        assert_eq!(child.origin, tools::Origin::System, "{path}: a child never asks the owner");
    }

    async fn call(
        tools: &[Box<dyn tools::registry::DynTool>],
        ctx: &ToolContext,
        name: &str,
        input: serde_json::Value,
    ) -> tools::ToolResult {
        tools.iter().find(|t| t.name() == name).expect("helper tool").execute_dyn(ctx, input).await
    }

    /// The escalation: a sub-agent of an employee with shell OFF and a path
    /// fence came back with shell ON and no fence, because the child request
    /// carried none of the parent's limits and "unset" means allowed.
    #[tokio::test]
    async fn a_sub_agent_keeps_its_parents_limits() {
        let rec = Recorder::default();
        let (_dir, tools) = helper_tools(&rec);
        let ctx = limited_parent();
        call(&tools, &ctx, "delegate", serde_json::json!({"description": "list", "prompt": "list the files", "background": false})).await;
        let req = rec.take().pop().expect("spawn reached the orchestrator");
        let child = build_subagent_request(&req, "subagent:agent:a1:web:sa-1", "p", &CancellationToken::new());
        let denied = match &child.ceiling {
            Some(types::permissions::Ceiling::Parent { grant }) => grant.rules.iter().any(|r| {
                r.key == types::permissions::RuleKey::Capability("shell".into())
                    && r.effect == types::permissions::Effect::Deny
            }),
            _ => false,
        };
        assert!(denied, "shell came back ON");
        assert_limited_like(&child, &ctx, "spawn");
    }

    /// Every way a child is made — single, background, each member of a
    /// parallel batch, each DAG node, and a `send` continuation — ends in the
    /// same builder with the same limits.
    #[tokio::test]
    async fn every_spawn_path_inherits_the_parents_limits() {
        let rec = Recorder::default();
        let (_dir, tools) = helper_tools(&rec);
        let ctx = limited_parent();
        let cancel = CancellationToken::new();
        let key = "subagent:agent:a1:web:sa-1";

        let cases: [(&str, &str, serde_json::Value); 4] = [
            ("foreground", "delegate", serde_json::json!({"description": "a", "prompt": "a", "background": false})),
            ("background", "delegate", serde_json::json!({"description": "a", "prompt": "a"})),
            ("isolated", "delegate", serde_json::json!({"description": "b", "prompt": "b", "isolation": "worktree"})),
            ("orchestrate", "orchestrate", serde_json::json!({"prompt": "a then b"})),
        ];
        for (path, name, input) in cases {
            call(&tools, &ctx, name, input).await;
            let reqs = rec.take();
            assert!(!reqs.is_empty(), "{path}: nothing reached the orchestrator");
            for req in reqs {
                let req = if path == "orchestrate" {
                    let node = crate::task_graph::TaskNode {
                        id: "n1".into(),
                        prompt: "a".into(),
                        description: "a".into(),
                        agent_type: AgentType::Explore,
                        model_override: String::new(),
                        depends_on: vec![],
                        status: crate::task_graph::TaskStatus::Pending,
                        result: None,
                        error: None,
                    };
                    dag_node_request(&req, &node)
                } else {
                    req
                };
                assert_limited_like(&build_subagent_request(&req, key, "p", &cancel), &ctx, path);
                // The same child continued later by `send`.
                let resumed = resumable_copy(&req);
                assert_limited_like(&build_subagent_request(&resumed, key, "p", &cancel), &ctx, "send");
            }
        }
    }

    /// Nothing is invented: an unrestricted parent's child is unrestricted,
    /// and a parent with Full Access hands it on. The parent's own run is
    /// untouched by spawning.
    #[tokio::test]
    async fn an_unrestricted_parent_has_an_unrestricted_child() {
        let rec = Recorder::default();
        let (_dir, tools) = helper_tools(&rec);
        let grant = types::permissions::Grant::new("", types::permissions::Mode::FullAccess);
        let ctx = ToolContext {
            session_id: "s1".into(),
            session_key: "agent:assistant:web".into(),
            grant: Some(std::sync::Arc::new(grant)),
            ..Default::default()
        };
        call(&tools, &ctx, "delegate", serde_json::json!({"description": "a", "prompt": "a"})).await;
        let req = rec.take().pop().unwrap();
        let child = build_subagent_request(&req, "subagent:agent:assistant:web:sa-1", "p", &CancellationToken::new());
        assert!(child.tool_allowlist.is_none() && child.fence.is_none() && child.seed_taint.is_empty());
        let dir = tempfile::tempdir().unwrap();
        let store = db::Store::new(&dir.path().join("t.db").to_string_lossy()).unwrap();
        assert_eq!(
            crate::harness::seat::run_grant(&store, &child).mode,
            types::permissions::Mode::FullAccess,
            "the owner's Full Access did not reach the child"
        );
    }

    /// Taint the parent picked up travels down: a child of a run that read
    /// the web starts out as having read the web.
    #[tokio::test]
    async fn taint_travels_to_the_child() {
        let rec = Recorder::default();
        let (_dir, tools) = helper_tools(&rec);
        let ctx = ToolContext {
            session_key: "agent:a1:web".into(),
            run_taint: vec![ProvenanceClass::Channel],
            ..Default::default()
        };
        call(&tools, &ctx, "delegate", serde_json::json!({"description": "a", "prompt": "a"})).await;
        let req = rec.take().pop().unwrap();
        let child = build_subagent_request(&req, "subagent:agent:a1:web:sa-1", "p", &CancellationToken::new());
        assert_eq!(child.seed_taint, vec![ProvenanceClass::Channel]);
    }

    /// A DAG node runs at its own model when the decomposition named one,
    /// else at the parent's — and keeps the parent's limits either way.
    #[test]
    fn a_dag_node_keeps_the_seat_and_takes_its_own_model() {
        let parent = SpawnRequest {
            model_override: "janus/parent".into(),
            parent_session_key: "agent:a1:web".into(),
            seat: tools::orchestrator::ChildSeat { fence: Some(vec!["/work/a".into()]), ..Default::default() },
            ..Default::default()
        };
        let mut node = crate::task_graph::TaskNode {
            id: "n1".into(),
            prompt: "a".into(),
            description: "a".into(),
            agent_type: AgentType::General,
            model_override: String::new(),
            depends_on: vec![],
            status: crate::task_graph::TaskStatus::Pending,
            result: None,
            error: None,
        };
        assert_eq!(dag_node_request(&parent, &node).model_override, "janus/parent");
        node.model_override = "janus/node".into();
        let req = dag_node_request(&parent, &node);
        assert_eq!(req.model_override, "janus/node");
        assert_eq!(req.seat.fence, Some(vec!["/work/a".into()]));
        assert_eq!(req.parent_session_key, "agent:a1:web");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The background-spawn acknowledgement must carry the no-prediction
    /// constraint AT THE DECISION POINT — a spawn ack that only says "you
    /// will be woken" invites the model to narrate results it does not have.
    #[tokio::test]
    async fn remembered_child_does_not_hold_the_parent_stream_open() {
        let (tx, mut rx) = mpsc::channel::<ai::StreamEvent>(1);
        let req = SpawnRequest {
            prompt: "p".into(),
            description: "d".into(),
            agent_type: "general".into(),
            model_override: String::new(),
            parent_session_id: "s".into(),
            parent_session_key: "k".into(),
            user_id: "u".into(),
            wait: true,
            parent_cancel: Some(CancellationToken::new()),
            max_iterations: 1,
            skills: vec![],
            parent_stream_tx: Some(tx),
            handoff_depth: 0,
            isolate: String::new(),
            workspace: String::new(),
            seat: Default::default(),
        };
        let kept = resumable_copy(&req);
        drop(req);
        // With the turn's sender gone the channel must close; a kept clone
        // would leave recv() pending forever, which is exactly the live hang.
        let closed = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await;
        assert!(matches!(closed, Ok(None)), "parent stream still open: {closed:?}");
        assert!(kept.parent_stream_tx.is_none() && kept.parent_cancel.is_none());
    }

    #[test]
    fn resumable_ring_keeps_the_newest_and_drops_the_oldest() {
        let mut ring = std::collections::VecDeque::new();
        for i in 0..MAX_RESUMABLE + 3 {
            remember_resumable(&mut ring, format!("sa-{i}"), i);
        }
        assert_eq!(ring.len(), MAX_RESUMABLE);
        assert_eq!(ring.front().map(|(id, _)| id.as_str()), Some("sa-3"));
        let newest = format!("sa-{}", MAX_RESUMABLE + 2);
        assert!(ring.iter().any(|(id, _)| *id == newest));
    }

    #[test]
    fn background_ack_forbids_predicting_results() {
        assert!(BACKGROUND_SPAWN_ACK.contains("do not report, assume, or predict"));
        assert!(BACKGROUND_SPAWN_ACK.contains("woken"), "the wake promise must stay");
        assert!(BACKGROUND_SPAWN_ACK.contains("still running"), "must offer the honest fallback");
    }

    /// The depth guard counts `subagent:` prefixes in the session key — the
    /// ONLY thing standing between "work together" prompts and an unbounded
    /// subagent:subagent:… tree where every node is a full provider run.
    #[test]
    fn subagent_depth_counts_nesting_and_hits_the_cap() {
        assert_eq!(subagent_depth("agent:assistant:web"), 0);
        assert_eq!(subagent_depth("subagent:agent:assistant:web:sa-1"), 1);
        assert_eq!(
            subagent_depth("subagent:subagent:agent:assistant:web:sa-1:sa-2"),
            2
        );
        // At MAX_SUBAGENT_DEPTH the spawn path refuses — the boundary itself.
        assert!(subagent_depth("subagent:subagent:agent:a:web:x:y") >= MAX_SUBAGENT_DEPTH);
        assert!(subagent_depth("agent:assistant:web") < MAX_SUBAGENT_DEPTH);
    }

    // The observed failure: the user pasted an SVG logo, the parent summarised
    // the delegation as one line, and the logo never reached the sub-agent.
    #[test]
    fn original_request_travels_with_the_spawn() {
        let user = "Make a deck. This is our logo: <svg viewBox=\"0 0 1 1\"><path d=\"M0 0\"/></svg>";
        let prompt = "Create an 8-slide recruiting deck spec using the Bold Split pack.";
        let block = compose_original_block(user, prompt);
        assert!(block.contains("<svg viewBox="), "the pasted payload must survive");
        assert!(block.contains("ORIGINAL USER REQUEST"), "framed as authoritative source");
    }

    #[test]
    fn quoted_in_full_adds_nothing() {
        let user = "Summarise this thread.";
        let prompt = format!("Do the following exactly: Summarise this thread.");
        assert_eq!(compose_original_block(user, &prompt), "");
        assert_eq!(compose_original_block("", "anything"), "");
        assert_eq!(compose_original_block("   ", "anything"), "");
    }

    #[test]
    fn a_pasted_book_is_clipped_not_forwarded_whole() {
        let user = "x".repeat(MAX_ORIGINAL_CHARS * 2);
        let block = compose_original_block(&user, "summary");
        assert!(block.contains("[...truncated]"));
        assert!(block.len() < MAX_ORIGINAL_CHARS + 400, "cap holds: {}", block.len());
    }

    #[test]
    fn test_format_dep_context_empty() {
        assert_eq!(format_dep_context(&[]), "");
    }

    #[test]
    fn test_format_dep_context_with_results() {
        let deps = vec![
            ("Research X".to_string(), "X is great".to_string()),
            ("Research Y".to_string(), "Y is good".to_string()),
        ];
        let ctx = format_dep_context(&deps);
        assert!(ctx.contains("Research X"));
        assert!(ctx.contains("X is great"));
        assert!(ctx.contains("Research Y"));
        assert!(ctx.contains("Your task:"));
    }

    #[test]
    fn test_format_dep_context_truncation() {
        let long_result = "x".repeat(5000);
        let deps = vec![("Task".to_string(), long_result)];
        let ctx = format_dep_context(&deps);
        assert!(ctx.contains("truncated"));
        assert!(ctx.len() < 5500);
    }

    /// Multi-byte text past the clip point must not split a character (it
    /// used to byte-slice at 4000 and panic).
    #[test]
    fn dep_context_clip_is_char_safe() {
        let long_result = format!("a{}", "é".repeat(MAX_DEP_CONTEXT_CHARS));
        let ctx = format_dep_context(&[("Task".to_string(), long_result)]);
        assert!(ctx.contains(&format!("a{}...(truncated)", "é".repeat(MAX_DEP_CONTEXT_CHARS - 1))));
    }

    #[test]
    fn test_task_prefixes() {
        let explore = task_prefix_for_type(&AgentType::Explore);
        assert!(explore.contains("EXPLORE"));
        assert!(explore.contains("READ-ONLY"));

        let plan = task_prefix_for_type(&AgentType::Plan);
        assert!(plan.contains("PLANNING"));
        assert!(plan.contains("Do NOT implement"));

        let general = task_prefix_for_type(&AgentType::General);
        assert!(general.contains("Execute the task"));
    }

    fn make_msg(role: &str, content: &str, tool_calls: Option<&str>) -> db::models::ChatMessage {
        db::models::ChatMessage {
            id: "test".to_string(),
            chat_id: "test".to_string(),
            role: role.to_string(),
            content: content.to_string(),
            metadata: None,
            created_at: 0,
            day_marker: None,
            tool_calls: tool_calls.map(String::from),
            tool_results: None,
            token_estimate: None,
            html: None,
        }
    }

    #[test]
    fn test_completion_empty_messages() {
        assert!(!check_completion_heuristic(&[]));
    }

    #[test]
    fn test_completion_with_tool_calls() {
        let messages = vec![
            make_msg("user", "do something", None),
            make_msg("assistant", "ok", Some("[{\"id\":\"1\"}]")),
        ];
        assert!(check_completion_heuristic(&messages));
    }

    #[test]
    fn test_completion_multiple_messages() {
        let messages = vec![
            make_msg("user", "question", None),
            make_msg("assistant", "answer", None),
            make_msg("user", "follow up", None),
        ];
        assert!(check_completion_heuristic(&messages));
    }

    #[test]
    fn test_completion_long_assistant_reply() {
        let long_content = "x".repeat(60);
        let messages = vec![
            make_msg("user", "question", None),
            make_msg("assistant", &long_content, None),
        ];
        assert!(check_completion_heuristic(&messages));
    }

    #[test]
    fn test_completion_short_incomplete() {
        let messages = vec![
            make_msg("user", "question", None),
            make_msg("assistant", "ok", None),
        ];
        assert!(!check_completion_heuristic(&messages));
    }

    #[test]
    fn test_completion_only_user_message() {
        let messages = vec![make_msg("user", "question", None)];
        assert!(!check_completion_heuristic(&messages));
    }
}
