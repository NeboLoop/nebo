use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use futures::stream::{FuturesUnordered, StreamExt};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use ai::StreamEventType;
use db::Store;
use tools::{SpawnRequest, SpawnResult, SubAgentOrchestrator};

use crate::concurrency::ConcurrencyController;
use crate::decompose;
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
}

/// The sub-agent orchestrator: manages lifecycle, DAG execution, concurrency.
pub struct Orchestrator {
    runner: Arc<Runner>,
    store: Arc<Store>,
    concurrency: Arc<ConcurrencyController>,
    active: Arc<RwLock<HashMap<String, ActiveAgent>>>,
    lanes: Option<Arc<LaneManager>>,
}

impl Orchestrator {
    pub fn new(runner: Arc<Runner>, store: Arc<Store>, concurrency: Arc<ConcurrencyController>) -> Self {
        Self {
            runner,
            store,
            concurrency,
            active: Arc::new(RwLock::new(HashMap::new())),
            lanes: None,
        }
    }

    pub fn with_lanes(mut self, lanes: Arc<LaneManager>) -> Self {
        self.lanes = Some(lanes);
        self
    }

    /// Spawn a single sub-agent.
    async fn spawn_internal(&self, req: SpawnRequest) -> Result<SpawnResult, String> {

        let task_id = format!("sa-{}", uuid::Uuid::new_v4());
        let session_key = format!("subagent:{}:{}", req.parent_session_key, task_id);
        // Derive a child token from the parent so cancelling the parent cascades.
        let cancel = req.parent_cancel.as_ref()
            .map(|p| p.child_token())
            .unwrap_or_else(CancellationToken::new);

        // Persist to pending_tasks
        let _ = self.store.create_pending_task(
            &task_id,
            "subagent",
            &session_key,
            Some(&req.user_id),
            &req.prompt,
            Some(&system_prompt_for_type(&AgentType::from_str(&req.agent_type))),
            Some(&req.description),
            Some("subagent"),
            0,
        );

        // Register in active map
        {
            let mut active = self.active.write().await;
            active.insert(
                task_id.clone(),
                ActiveAgent {
                    task_id: task_id.clone(),
                    description: req.description.clone(),
                    status: "running".to_string(),
                    cancel: cancel.clone(),
                },
            );
        }

        let system = system_prompt_for_type(&AgentType::from_str(&req.agent_type));

        if req.wait {
            // Blocking: run and return result
            let result = self
                .run_subagent(
                    &task_id,
                    &req.prompt,
                    &system,
                    &req.model_override,
                    &req.user_id,
                    "",
                    cancel.clone(),
                    &format!("subagent:{}:{}", req.parent_session_key, task_id),
                )
                .await;

            // Clean up active map
            self.active.write().await.remove(&task_id);

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
            let prompt = req.prompt.clone();
            let model_override = req.model_override.clone();
            let user_id = req.user_id.clone();
            let concurrency = self.concurrency.clone();

            let bg_session_key = format!("subagent:{}:{}", req.parent_session_key, task_id_clone);

            tokio::spawn(async move {
                let _permit = concurrency.acquire_llm_permit().await;

                let run_req = build_subagent_request(
                    &bg_session_key, &prompt, &system, &model_override, &user_id, &cancel,
                );

                let result = run_and_collect(&runner, run_req, cancel).await;

                match result {
                    Ok(output) => {
                        let _ = store.update_task_completed(&task_id_clone, Some(&output));
                    }
                    Err(e) => {
                        let _ = store.update_task_failed(&task_id_clone, &e);
                    }
                }

                active.write().await.remove(&task_id_clone);
            });

            Ok(SpawnResult {
                task_id,
                success: true,
                output: "Sub-agent spawned in background.".to_string(),
                error: None,
            })
        }
    }

    /// Run a sub-agent and collect its text output.
    async fn run_subagent(
        &self,
        task_id: &str,
        prompt: &str,
        system_prompt: &str,
        model_override: &str,
        user_id: &str,
        dep_context: &str,
        cancel: CancellationToken,
        session_key: &str,
    ) -> Result<String, String> {
        let _ = self.store.update_task_running(task_id);

        let full_prompt = if dep_context.is_empty() {
            prompt.to_string()
        } else {
            format!("{}\n\n{}", dep_context, prompt)
        };

        let req = build_subagent_request(session_key, &full_prompt, system_prompt, model_override, user_id, &cancel);
        run_and_collect(&self.runner, req, cancel).await
    }

    /// Execute a DAG of sub-tasks with reactive scheduling.
    async fn execute_dag_internal(
        &self,
        prompt: &str,
        user_id: &str,
        parent_session_id: &str,
        parent_cancel: Option<CancellationToken>,
    ) -> Result<SpawnResult, String> {
        // 1. Decompose task into sub-tasks
        info!("Decomposing task into sub-tasks");
        let nodes = decompose::decompose_task(&self.runner, prompt).await?;

        // Single-task optimization: skip DAG scheduler
        if decompose::is_single_task(&nodes) {
            info!("Single task decomposition — running directly");
            let node = &nodes[0];
            let req = SpawnRequest {
                prompt: node.prompt.clone(),
                description: node.description.clone(),
                agent_type: node.agent_type.as_str().to_string(),
                model_override: String::new(),
                parent_session_id: parent_session_id.to_string(),
                parent_session_key: parent_session_id.to_string(),
                user_id: user_id.to_string(),
                wait: true,
                parent_cancel: parent_cancel.clone(),
            };
            return self.spawn_internal(req).await;
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
            parent_session_id,
            Some(user_id),
            prompt,
            None,
            Some("DAG orchestration"),
            Some("subagent"),
            0,
        );

        // 4. Shared cancellation for the entire DAG — derived from parent so
        //    cancelling the parent cascades to all DAG tasks.
        let dag_cancel = parent_cancel.as_ref()
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
                let system = system_prompt_for_type(&node.agent_type);
                let prompt = node.prompt.clone();
                let model_override = node.model_override.clone();
                let user_id = user_id.to_string();
                let cancel = dag_cancel.clone();
                let session_key =
                    format!("subagent:{}:{}", parent_session_id, task_id);

                let runner = self.runner.clone();
                let store = self.store.clone();
                let concurrency = self.concurrency.clone();
                let child_task_id = format!("{}-{}", parent_task_id, task_id);

                // Persist child task
                let _ = store.create_pending_task(
                    &child_task_id,
                    "subagent",
                    &session_key,
                    Some(&user_id),
                    &prompt,
                    Some(&system),
                    graph.nodes.get(&task_id).map(|n| n.description.as_str()),
                    Some("subagent"),
                    0,
                );

                graph.mark_running(&task_id);

                let tid = task_id.clone();
                running.push(Box::pin(async move {
                    // Acquire LLM permit (blocks if at capacity)
                    let _permit = concurrency.acquire_llm_permit().await;

                    let _ = store.update_task_running(&child_task_id);

                    let full_prompt = if dep_context.is_empty() {
                        prompt
                    } else {
                        format!("{}\n\n{}", dep_context, prompt)
                    };

                    let req = build_subagent_request(
                        &session_key, &full_prompt, &system, &model_override, &user_id, &cancel,
                    );

                    let result = run_and_collect(&runner, req, cancel).await;

                    match &result {
                        Ok(output) => {
                            let _ = store.update_task_completed(&child_task_id, Some(output.as_str()));
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
            self.store.update_task_completed(&parent_task_id, Some(&output))
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
        if let Some(agent) = active.remove(task_id) {
            agent.cancel.cancel();
            let _ = self.store.cancel_task(task_id);
            info!(task_id = %task_id, "Cancelled sub-agent");
            Ok(())
        } else {
            // Try cancelling children if it's a DAG parent
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
            .map(|a| {
                (
                    a.task_id.clone(),
                    a.description.clone(),
                    a.status.clone(),
                )
            })
            .collect()
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
                match run_and_collect(&runner, req, cancel).await {
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
fn build_subagent_request(
    session_key: &str,
    prompt: &str,
    system: &str,
    model_override: &str,
    user_id: &str,
    cancel: &CancellationToken,
) -> RunRequest {
    RunRequest {
        session_key: session_key.to_string(),
        prompt: prompt.to_string(),
        system: system.to_string(),
        model_override: model_override.to_string(),
        user_id: user_id.to_string(),
        skip_memory_extract: true,
        origin: tools::Origin::System,
        channel: "subagent".to_string(),
        cancel_token: cancel.clone(),
        ..Default::default()
    }
}

/// Run a RunRequest via the Runner and collect text output from the stream.
async fn run_and_collect(
    runner: &Arc<Runner>,
    req: RunRequest,
    cancel: CancellationToken,
) -> Result<String, String> {
    let mut rx = runner
        .run(req)
        .await
        .map_err(|e| format!("Failed to start sub-agent: {}", e))?;

    let mut output = String::new();

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                return Err("Cancelled".to_string());
            }
            event = rx.recv() => {
                match event {
                    Some(e) => {
                        match e.event_type {
                            StreamEventType::Text => output.push_str(&e.text),
                            StreamEventType::Error => {
                                if let Some(err) = e.error {
                                    return Err(err);
                                }
                            }
                            StreamEventType::Done => break,
                            _ => {} // ToolCall, ToolResult, Usage, etc — handled internally by runner
                        }
                    }
                    None => break, // Channel closed
                }
            }
        }
    }

    if output.is_empty() {
        // Check if there are results in the session messages
        Ok("Sub-agent completed (no text output).".to_string())
    } else {
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
        let truncated = if result.len() > MAX_DEP_CONTEXT_CHARS {
            format!("{}...(truncated)", &result[..MAX_DEP_CONTEXT_CHARS])
        } else {
            result.clone()
        };
        parts.push(format!(
            "--- Task \"{}\" (completed) ---\n{}\n",
            desc, truncated
        ));
    }

    parts.push("---\n\nYour task:".to_string());
    parts.join("\n")
}

/// Generate a system prompt based on agent type.
fn system_prompt_for_type(agent_type: &AgentType) -> String {
    let base = "You are a focused sub-agent working on a specific task. \
                Complete your assigned task and report results concisely. \
                Do not take on work outside your task scope. \
                Maximum 50 iterations.";

    match agent_type {
        AgentType::Explore => format!(
            "{}\n\nYou are an EXPLORATION agent. \
             Search, read, and research. Do NOT modify files or execute destructive commands. \
             Report your findings clearly.",
            base
        ),
        AgentType::Plan => format!(
            "{}\n\nYou are a PLANNING agent. \
             Analyze the task, break down steps, identify files and patterns. \
             Produce a clear actionable plan. Do NOT implement anything.",
            base
        ),
        AgentType::General => format!(
            "{}\n\nYou have full tool access. \
             Execute the task using whatever tools are needed.",
            base
        ),
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
        user_id: &str,
        parent_session_id: &str,
        parent_cancel: Option<CancellationToken>,
    ) -> Pin<Box<dyn Future<Output = Result<SpawnResult, String>> + Send + '_>> {
        let prompt = prompt.to_string();
        let user_id = user_id.to_string();
        let parent_session_id = parent_session_id.to_string();
        Box::pin(async move {
            self.execute_dag_internal(&prompt, &user_id, &parent_session_id, parent_cancel)
                .await
        })
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

    fn list_active(
        &self,
    ) -> Pin<Box<dyn Future<Output = Vec<(String, String, String)>> + Send + '_>> {
        Box::pin(async move { self.list_active_internal().await })
    }

    fn recover(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move { self.recover_internal().await })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn test_system_prompts() {
        let explore = system_prompt_for_type(&AgentType::Explore);
        assert!(explore.contains("EXPLORATION"));
        assert!(explore.contains("Do NOT modify"));

        let plan = system_prompt_for_type(&AgentType::Plan);
        assert!(plan.contains("PLANNING"));
        assert!(plan.contains("Do NOT implement"));

        let general = system_prompt_for_type(&AgentType::General);
        assert!(general.contains("full tool access"));
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
