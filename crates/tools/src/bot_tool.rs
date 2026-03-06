use std::sync::Arc;

use db::Store;
use crate::domain::DomainInput;
use crate::orchestrator::OrchestratorHandle;
use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

/// Trait for advisor deliberation (implemented by agent::advisors::Runner).
/// Defined here to avoid circular dependencies between tools and agent crates.
pub trait AdvisorDeliberator: Send + Sync {
    fn deliberate<'a>(
        &'a self,
        task: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>>;
}

/// Trait for hybrid memory search (implemented by agent::search wrapper).
/// Combines FTS5 text search + vector cosine similarity with adaptive weights.
pub trait HybridSearcher: Send + Sync {
    fn search<'a>(
        &'a self,
        query: &'a str,
        user_id: &'a str,
        limit: usize,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<HybridSearchResult>> + Send + 'a>>;
}

/// Result from hybrid memory search.
#[derive(Debug, Clone)]
pub struct HybridSearchResult {
    pub key: String,
    pub value: String,
    pub namespace: String,
    pub score: f64,
}

/// BotTool is the agent's self-management domain tool.
/// Consolidates: memory, sessions, tasks (work items), profile, context, and ask.
pub struct BotTool {
    store: Arc<Store>,
    orchestrator: OrchestratorHandle,
    advisor_runner: Option<Arc<dyn AdvisorDeliberator>>,
    hybrid_searcher: Option<Arc<dyn HybridSearcher>>,
}

impl BotTool {
    pub fn new(store: Arc<Store>, orchestrator: OrchestratorHandle) -> Self {
        Self { store, orchestrator, advisor_runner: None, hybrid_searcher: None }
    }

    pub fn with_advisor_runner(mut self, runner: Arc<dyn AdvisorDeliberator>) -> Self {
        self.advisor_runner = Some(runner);
        self
    }

    pub fn with_hybrid_searcher(mut self, searcher: Arc<dyn HybridSearcher>) -> Self {
        self.hybrid_searcher = Some(searcher);
        self
    }

    fn infer_resource(&self, action: &str) -> &str {
        match action {
            "store" | "recall" | "search" => "memory",
            "spawn" | "orchestrate" | "status" | "cancel" | "create" | "update" | "delete" => "task",
            "history" | "query" => "session",
            "get" | "open_billing" => "profile",
            "reset" | "compact" | "summary" => "context",
            "deliberate" => "advisors",
            "analyze" => "vision",
            "prompt" | "confirm" | "select" => "ask",
            _ => "",
        }
    }

    async fn handle_memory(&self, input: &serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let action = input["action"].as_str().unwrap_or("");

        match action {
            "store" => {
                let key = input["key"].as_str().unwrap_or("");
                let value = input["value"].as_str().unwrap_or("");
                let namespace = input["namespace"].as_str().unwrap_or("tacit/general");

                if key.is_empty() || value.is_empty() {
                    return ToolResult::error("key and value are required");
                }

                match self.store.upsert_memory(namespace, key, value, None, None, &ctx.user_id) {
                    Ok(_) => ToolResult::ok(format!("Stored memory: [{}] {} = {}", namespace, key, value)),
                    Err(e) => ToolResult::error(format!("Failed to store memory: {}", e)),
                }
            }
            "recall" => {
                let key = input["key"].as_str().unwrap_or("");
                let namespace = input["namespace"].as_str().unwrap_or("tacit/general");
                if key.is_empty() {
                    return ToolResult::error("key is required");
                }

                match self.store.get_memory_by_key_and_user(namespace, key, &ctx.user_id) {
                    Ok(Some(mem)) => {
                        // Increment access count on recall
                        let _ = self.store.increment_memory_access_by_key(namespace, key, &ctx.user_id);
                        ToolResult::ok(format!("[{}] {}: {}", mem.namespace, mem.key, mem.value))
                    }
                    Ok(None) => ToolResult::ok(format!("No memory found for key: {}", key)),
                    Err(e) => ToolResult::error(format!("Failed to recall memory: {}", e)),
                }
            }
            "search" => {
                let query = input["query"].as_str().unwrap_or("");
                let limit = input["limit"].as_i64().unwrap_or(20) as usize;

                if query.is_empty() {
                    return ToolResult::error("query is required for memory search");
                }

                // Use hybrid search (FTS5 + vector) when available
                if let Some(ref searcher) = self.hybrid_searcher {
                    let results = searcher.search(query, &ctx.user_id, limit).await;
                    if results.is_empty() {
                        return ToolResult::ok(format!("No memories found matching: {}", query));
                    }
                    let lines: Vec<String> = results
                        .iter()
                        .map(|r| format!("- [{}] {}: {} (score: {:.2})", r.namespace, r.key, r.value, r.score))
                        .collect();
                    return ToolResult::ok(format!(
                        "Found {} memories:\n{}",
                        results.len(),
                        lines.join("\n")
                    ));
                }

                // Fallback: simple LIKE query
                match self.store.search_memories_by_user(&ctx.user_id, query, limit as i64, 0) {
                    Ok(memories) => {
                        if memories.is_empty() {
                            ToolResult::ok(format!("No memories found matching: {}", query))
                        } else {
                            let lines: Vec<String> = memories
                                .iter()
                                .map(|m| format!("- [{}] {}: {}", m.namespace, m.key, m.value))
                                .collect();
                            ToolResult::ok(format!(
                                "Found {} memories:\n{}",
                                memories.len(),
                                lines.join("\n")
                            ))
                        }
                    }
                    Err(e) => ToolResult::error(format!("Memory search failed: {}", e)),
                }
            }
            "list" => {
                let namespace = input["namespace"].as_str().unwrap_or("");
                let limit = input["limit"].as_i64().unwrap_or(50);

                let memories = if namespace.is_empty() {
                    self.store.list_memories(limit, 0)
                } else {
                    self.store.list_memories_by_namespace(namespace, limit, 0)
                };

                match memories {
                    Ok(mems) => {
                        if mems.is_empty() {
                            ToolResult::ok("No memories stored.")
                        } else {
                            let lines: Vec<String> = mems
                                .iter()
                                .map(|m| format!("- [{}] {}: {}", m.namespace, m.key, m.value))
                                .collect();
                            ToolResult::ok(format!(
                                "{} memories:\n{}",
                                mems.len(),
                                lines.join("\n")
                            ))
                        }
                    }
                    Err(e) => ToolResult::error(format!("Failed to list memories: {}", e)),
                }
            }
            "delete" => {
                let key = input["key"].as_str().unwrap_or("");
                let namespace = input["namespace"].as_str().unwrap_or("tacit/general");

                if key.is_empty() {
                    return ToolResult::error("key is required");
                }
                match self.store.delete_memory_by_key_and_user(namespace, key, &ctx.user_id) {
                    Ok(count) => ToolResult::ok(format!("Deleted {} memory entries for key: {}", count, key)),
                    Err(e) => ToolResult::error(format!("Failed to delete: {}", e)),
                }
            }
            "clear" => {
                let namespace = input["namespace"].as_str().unwrap_or("");
                if namespace.is_empty() {
                    return ToolResult::error("namespace is required to clear memories");
                }
                match self.store.delete_memories_by_namespace_and_user(namespace, &ctx.user_id) {
                    Ok(count) => {
                        ToolResult::ok(format!("Cleared {} memories in namespace: {}", count, namespace))
                    }
                    Err(e) => ToolResult::error(format!("Failed to clear: {}", e)),
                }
            }
            _ => ToolResult::error(format!(
                "Unknown memory action: {}. Available: store, recall, search, list, delete, clear",
                action
            )),
        }
    }

    async fn handle_task(&self, input: &serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let action = input["action"].as_str().unwrap_or("");

        match action {
            "spawn" => {
                let task_prompt = input["prompt"].as_str().unwrap_or("");
                let agent_type = input["agent_type"].as_str().unwrap_or("general");
                let description = input["description"].as_str().unwrap_or(
                    &task_prompt[..task_prompt.len().min(80)]
                );
                let model_override = input["model_override"].as_str().unwrap_or("");
                let wait = input["wait"].as_bool().unwrap_or(true);

                if task_prompt.is_empty() {
                    return ToolResult::error("prompt is required for task spawn");
                }

                let orch = match self.orchestrator.get() {
                    Some(o) => o,
                    None => return ToolResult::error("Sub-agent orchestrator not ready"),
                };

                let req = crate::orchestrator::SpawnRequest {
                    prompt: task_prompt.to_string(),
                    description: description.to_string(),
                    agent_type: agent_type.to_string(),
                    model_override: model_override.to_string(),
                    parent_session_id: ctx.session_id.clone(),
                    parent_session_key: ctx.session_key.clone(),
                    user_id: String::new(),
                    wait,
                };

                match orch.spawn(req).await {
                    Ok(result) => {
                        if result.success {
                            ToolResult::ok(format!(
                                "Sub-agent [{}] completed:\n\n{}",
                                result.task_id, result.output
                            ))
                        } else {
                            ToolResult::error(format!(
                                "Sub-agent [{}] failed: {}",
                                result.task_id,
                                result.error.unwrap_or_default()
                            ))
                        }
                    }
                    Err(e) => ToolResult::error(format!("Failed to spawn sub-agent: {}", e)),
                }
            }
            "orchestrate" => {
                let task_prompt = input["prompt"].as_str().unwrap_or("");
                if task_prompt.is_empty() {
                    return ToolResult::error("prompt is required for orchestration");
                }

                let orch = match self.orchestrator.get() {
                    Some(o) => o,
                    None => return ToolResult::error("Sub-agent orchestrator not ready"),
                };

                match orch.execute_dag(task_prompt, "", &ctx.session_id).await {
                    Ok(result) => {
                        if result.success {
                            ToolResult::ok(format!(
                                "Orchestration [{}] completed:\n\n{}",
                                result.task_id, result.output
                            ))
                        } else {
                            ToolResult::error(format!(
                                "Orchestration [{}] had failures:\n\n{}\n\nError: {}",
                                result.task_id,
                                result.output,
                                result.error.unwrap_or_default()
                            ))
                        }
                    }
                    Err(e) => ToolResult::error(format!("Orchestration failed: {}", e)),
                }
            }
            "cancel" => {
                let task_id = input["task_id"].as_str().unwrap_or("");
                if task_id.is_empty() {
                    return ToolResult::error("task_id is required for cancel");
                }

                let orch = match self.orchestrator.get() {
                    Some(o) => o,
                    None => {
                        // Fall back to DB cancel
                        return match self.store.cancel_task(task_id) {
                            Ok(_) => ToolResult::ok(format!("Cancelled task: {}", task_id)),
                            Err(e) => ToolResult::error(format!("Failed to cancel: {}", e)),
                        };
                    }
                };

                match orch.cancel(task_id).await {
                    Ok(()) => ToolResult::ok(format!("Cancelled task: {}", task_id)),
                    Err(e) => ToolResult::error(format!("Failed to cancel: {}", e)),
                }
            }
            "status" => {
                let task_id = input["task_id"].as_str().unwrap_or("");
                if task_id.is_empty() {
                    // List active sub-agents
                    if let Some(orch) = self.orchestrator.get() {
                        let agents = orch.list_active().await;
                        if agents.is_empty() {
                            return ToolResult::ok("No active sub-agents.");
                        }
                        let lines: Vec<String> = agents
                            .iter()
                            .map(|(id, desc, status)| format!("- [{}] {} ({})", id, desc, status))
                            .collect();
                        return ToolResult::ok(format!(
                            "{} active sub-agents:\n{}",
                            agents.len(),
                            lines.join("\n")
                        ));
                    }
                    return ToolResult::error("task_id is required, or orchestrator not ready");
                }

                if let Some(orch) = self.orchestrator.get() {
                    match orch.status(task_id).await {
                        Ok(status) => return ToolResult::ok(status),
                        Err(_) => {} // Fall through to DB lookup
                    }
                }

                // Fall back to DB lookup
                match self.store.get_pending_task(task_id) {
                    Ok(Some(task)) => ToolResult::ok(format!(
                        "Task: {}\nType: {}\nStatus: {}",
                        task.id, task.task_type, task.status
                    )),
                    Ok(None) => ToolResult::error(format!("Task '{}' not found", task_id)),
                    Err(e) => ToolResult::error(format!("Failed to get status: {}", e)),
                }
            }
            "create" => {
                let subject = input["subject"].as_str().unwrap_or("");
                if subject.is_empty() {
                    return ToolResult::error("subject is required for task creation");
                }

                let id = uuid::Uuid::new_v4().to_string();
                match self.store.create_pending_task(
                    &id,
                    "work",
                    &ctx.session_id,
                    None,
                    subject,
                    None,
                    None,
                    None,
                    0,
                ) {
                    Ok(task) => ToolResult::ok(format!("Created task [{}]: {}", task.id, subject)),
                    Err(e) => ToolResult::error(format!("Failed to create task: {}", e)),
                }
            }
            "update" => {
                let task_id = input["task_id"].as_str().unwrap_or("");
                let status = input["status"].as_str().unwrap_or("");

                if task_id.is_empty() || status.is_empty() {
                    return ToolResult::error("task_id and status are required");
                }

                match self.store.update_task_status(task_id, status) {
                    Ok(_) => ToolResult::ok(format!("Updated task {} to {}", task_id, status)),
                    Err(e) => ToolResult::error(format!("Failed to update task: {}", e)),
                }
            }
            "list" => {
                match self.store.get_pending_tasks_by_status("pending") {
                    Ok(tasks) => {
                        if tasks.is_empty() {
                            ToolResult::ok("No pending tasks.")
                        } else {
                            let lines: Vec<String> = tasks
                                .iter()
                                .map(|t| {
                                    format!(
                                        "- [{}] {} — {} ({})",
                                        t.id, t.prompt, t.task_type, t.status
                                    )
                                })
                                .collect();
                            ToolResult::ok(format!(
                                "{} tasks:\n{}",
                                tasks.len(),
                                lines.join("\n")
                            ))
                        }
                    }
                    Err(e) => ToolResult::error(format!("Failed to list tasks: {}", e)),
                }
            }
            "delete" => {
                let task_id = input["task_id"].as_str().unwrap_or("");
                if task_id.is_empty() {
                    return ToolResult::error("task_id is required");
                }
                match self.store.cancel_task(task_id) {
                    Ok(_) => ToolResult::ok(format!("Cancelled task: {}", task_id)),
                    Err(e) => ToolResult::error(format!("Failed to cancel task: {}", e)),
                }
            }
            _ => ToolResult::error(format!(
                "Unknown task action: {}. Available: spawn, orchestrate, status, cancel, create, update, list, delete",
                action
            )),
        }
    }

    async fn handle_session(&self, input: &serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let action = input["action"].as_str().unwrap_or("");

        match action {
            "list" => match self.store.list_sessions(50, 0) {
                Ok(sessions) => {
                    if sessions.is_empty() {
                        ToolResult::ok("No sessions.")
                    } else {
                        let lines: Vec<String> = sessions
                            .iter()
                            .map(|s| {
                                let name = s.name.as_deref().unwrap_or("-");
                                let msgs = s.message_count.unwrap_or(0);
                                format!("- {} ({}): {} messages", s.id, name, msgs)
                            })
                            .collect();
                        ToolResult::ok(format!(
                            "{} sessions:\n{}",
                            sessions.len(),
                            lines.join("\n")
                        ))
                    }
                }
                Err(e) => ToolResult::error(format!("Failed to list sessions: {}", e)),
            },
            "history" => {
                let session_id = input["session_id"]
                    .as_str()
                    .unwrap_or(&ctx.session_id);
                // Sessions use chat_messages table with session_id as the chat_id
                match self.store.get_chat_messages(session_id) {
                    Ok(msgs) => {
                        if msgs.is_empty() {
                            return ToolResult::ok(format!("No messages in session: {}", session_id));
                        }
                        let recent: Vec<&db::models::ChatMessage> = msgs.iter().rev().take(50).collect();
                        let lines: Vec<String> = recent
                            .iter()
                            .rev()
                            .map(|m| {
                                let preview = if m.content.len() > 200 {
                                    format!("{}...", &m.content[..200])
                                } else {
                                    m.content.clone()
                                };
                                format!("[{}] {}: {}", m.id, m.role, preview)
                            })
                            .collect();
                        ToolResult::ok(format!(
                            "{} messages:\n{}",
                            msgs.len(),
                            lines.join("\n")
                        ))
                    }
                    Err(e) => ToolResult::error(format!("Failed to get history: {}", e)),
                }
            }
            "status" => ToolResult::ok(format!("Current session: {}", ctx.session_id)),
            "clear" => {
                let session_id = input["session_id"]
                    .as_str()
                    .unwrap_or(&ctx.session_id);
                match self.store.reset_session(session_id) {
                    Ok(_) => ToolResult::ok(format!("Cleared session: {}", session_id)),
                    Err(e) => ToolResult::error(format!("Failed to clear session: {}", e)),
                }
            }
            _ => ToolResult::error(format!(
                "Unknown session action: {}. Available: list, history, status, clear",
                action
            )),
        }
    }

    async fn handle_profile(&self, input: &serde_json::Value) -> ToolResult {
        let action = input["action"].as_str().unwrap_or("get");

        match action {
            "get" => match self.store.get_agent_profile() {
                Ok(Some(profile)) => ToolResult::ok(format!(
                    "Name: {}\nRole: {}\nPersonality: {}\nEmoji: {}\nCreature: {}\nVibe: {}",
                    profile.name,
                    profile.role.as_deref().unwrap_or("-"),
                    profile.personality_preset.as_deref().unwrap_or("-"),
                    profile.emoji.as_deref().unwrap_or("-"),
                    profile.creature.as_deref().unwrap_or("-"),
                    profile.vibe.as_deref().unwrap_or("-"),
                )),
                Ok(None) => ToolResult::ok("No agent profile configured."),
                Err(e) => ToolResult::error(format!("Failed to get profile: {}", e)),
            },
            "update" => {
                let name = input.get("name").and_then(|v| v.as_str());
                let role = input.get("role").and_then(|v| v.as_str());
                let emoji = input.get("emoji_char").and_then(|v| v.as_str());
                let creature = input.get("creature").and_then(|v| v.as_str());
                let vibe = input.get("vibe").and_then(|v| v.as_str());

                match self.store.update_agent_profile(
                    name, None, None, None, None, None, None, None,
                    emoji, creature, vibe, role, None, None, None, None, None,
                ) {
                    Ok(_) => ToolResult::ok("Profile updated."),
                    Err(e) => ToolResult::error(format!("Failed to update profile: {}", e)),
                }
            }
            _ => ToolResult::error(format!(
                "Unknown profile action: {}. Available: get, update",
                action
            )),
        }
    }

    async fn handle_context(&self, input: &serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let action = input["action"].as_str().unwrap_or("");
        match action {
            "summary" => {
                // Get recent messages from the current session for a summary
                match self.store.get_chat_messages(&ctx.session_id) {
                    Ok(msgs) => {
                        let count = msgs.len();
                        let user_count = msgs.iter().filter(|m| m.role == "user").count();
                        let assistant_count = msgs.iter().filter(|m| m.role == "assistant").count();
                        let tool_count = msgs.iter().filter(|m| m.role == "tool").count();

                        let last_topic = msgs.iter().rev()
                            .find(|m| m.role == "user")
                            .map(|m| {
                                if m.content.len() > 100 {
                                    format!("{}...", &m.content[..100])
                                } else {
                                    m.content.clone()
                                }
                            })
                            .unwrap_or_else(|| "-".to_string());

                        ToolResult::ok(format!(
                            "Session: {}\nMessages: {} ({} user, {} assistant, {} tool)\nLast user message: {}",
                            ctx.session_id, count, user_count, assistant_count, tool_count, last_topic
                        ))
                    }
                    Err(e) => ToolResult::error(format!("Failed to get context: {}", e)),
                }
            }
            "reset" => {
                match self.store.reset_session(&ctx.session_id) {
                    Ok(_) => ToolResult::ok("Session context has been reset."),
                    Err(e) => ToolResult::error(format!("Failed to reset: {}", e)),
                }
            }
            "compact" => {
                // Compaction is handled automatically by the agentic loop's sliding window.
                // This explicit call is a no-op signal that the agent wants to reduce context.
                ToolResult::ok("Context compaction noted. The agentic loop will apply sliding window pruning on the next iteration.")
            }
            _ => ToolResult::error(format!(
                "Unknown context action: {}. Available: summary, reset, compact",
                action
            )),
        }
    }

    async fn handle_advisors(&self, input: &serde_json::Value) -> ToolResult {
        let action = input["action"].as_str().unwrap_or("deliberate");
        match action {
            "deliberate" => {
                let task = input["task"].as_str().unwrap_or("");
                if task.is_empty() {
                    return ToolResult::error("task description is required for advisor deliberation");
                }

                // Use the advisor runner if available (real LLM deliberation)
                if let Some(ref runner) = self.advisor_runner {
                    return match runner.deliberate(task).await {
                        Ok(output) => {
                            if output.is_empty() {
                                ToolResult::ok(format!(
                                    "No advisors configured. Proceeding with own judgment on: {}",
                                    task
                                ))
                            } else {
                                ToolResult::ok(output)
                            }
                        }
                        Err(e) => ToolResult::error(format!("Advisor deliberation failed: {}", e)),
                    };
                }

                // Fallback: format personas from DB (no LLM calls)
                match self.store.list_advisors() {
                    Ok(advisors) => {
                        let enabled: Vec<_> = advisors.iter()
                            .filter(|a| a.enabled != 0)
                            .collect();

                        if enabled.is_empty() {
                            return ToolResult::ok(format!(
                                "No advisors configured. Proceeding with own judgment on: {}",
                                task
                            ));
                        }

                        let mut perspectives = Vec::new();
                        for advisor in &enabled {
                            let persona = if advisor.persona.is_empty() { "general advisor" } else { &advisor.persona };
                            let name = &advisor.name;
                            let role = if advisor.role.is_empty() { "advisor" } else { &advisor.role };
                            perspectives.push(format!(
                                "**{}** ({}): Consider this task from the perspective of {}.",
                                name, role, persona,
                            ));
                        }

                        ToolResult::ok(format!(
                            "Advisor deliberation for: {}\n\n{}\n\nSynthesize these perspectives to form your approach.",
                            task,
                            perspectives.join("\n\n"),
                        ))
                    }
                    Err(e) => ToolResult::error(format!("Failed to load advisors: {}", e)),
                }
            }
            "list" => {
                match self.store.list_advisors() {
                    Ok(advisors) => {
                        if advisors.is_empty() {
                            ToolResult::ok("No advisors configured.")
                        } else {
                            let lines: Vec<String> = advisors.iter().map(|a| {
                                let enabled = if a.enabled != 0 { "enabled" } else { "disabled" };
                                let desc = if a.description.is_empty() { "-" } else { &a.description };
                                format!("- {} [{}] — {}", a.name, enabled, desc)
                            }).collect();
                            ToolResult::ok(format!("{} advisors:\n{}", advisors.len(), lines.join("\n")))
                        }
                    }
                    Err(e) => ToolResult::error(format!("Failed to list advisors: {}", e)),
                }
            }
            _ => ToolResult::error(format!(
                "Unknown advisors action: {}. Available: deliberate, list",
                action
            )),
        }
    }

    async fn handle_ask(&self, input: &serde_json::Value) -> ToolResult {
        let action = input["action"].as_str().unwrap_or("prompt");
        match action {
            "prompt" | "confirm" | "select" => {
                let text = input["text"].as_str().unwrap_or("");
                if text.is_empty() {
                    return ToolResult::error("text is required for ask operations");
                }
                // In a web context, the frontend would render this as a UI prompt.
                // Here we log it and return the question as a structured response
                // that the frontend can intercept.
                let options = input.get("options").cloned().unwrap_or(serde_json::json!([]));
                ToolResult::ok(serde_json::json!({
                    "type": "ask",
                    "action": action,
                    "text": text,
                    "options": options,
                    "awaitingResponse": true,
                }).to_string())
            }
            _ => ToolResult::error(format!(
                "Unknown ask action: {}. Available: prompt, confirm, select",
                action
            )),
        }
    }
}

impl DynTool for BotTool {
    fn name(&self) -> &str {
        "bot"
    }

    fn description(&self) -> String {
        "Agent self-management — memory, tasks, sessions, profile, and context.\n\n\
         Resources and Actions:\n\
         - memory: store, recall, search, list, delete, clear\n\
         - task: spawn, orchestrate, status, cancel, create, update, list, delete\n\
         - session: list, history, status, clear\n\
         - profile: get, update\n\
         - context: summary, reset, compact\n\
         - advisors: deliberate\n\n\
         Examples:\n  \
         bot(resource: \"memory\", action: \"store\", key: \"user_name\", value: \"Alice\")\n  \
         bot(resource: \"memory\", action: \"recall\", key: \"user_name\")\n  \
         bot(resource: \"task\", action: \"create\", subject: \"Research topic X\")\n  \
         bot(resource: \"session\", action: \"list\")\n  \
         bot(resource: \"profile\", action: \"get\")"
            .to_string()
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "resource": {
                    "type": "string",
                    "description": "Resource type",
                    "enum": ["memory", "task", "session", "profile", "context", "advisors", "ask"]
                },
                "action": {
                    "type": "string",
                    "description": "Action to perform"
                },
                "key": { "type": "string", "description": "Memory key" },
                "value": { "type": "string", "description": "Memory value or field value" },
                "namespace": { "type": "string", "description": "Memory namespace (e.g. tacit/general, entity/people)" },
                "query": { "type": "string", "description": "Search query" },
                "limit": { "type": "integer", "description": "Max results" },
                "subject": { "type": "string", "description": "Task subject" },
                "status": { "type": "string", "description": "Task status: pending, in_progress, completed" },
                "task_id": { "type": "string", "description": "Task ID for updates" },
                "prompt": { "type": "string", "description": "Sub-agent prompt or orchestration task description" },
                "description": { "type": "string", "description": "Short description of the sub-agent task" },
                "agent_type": { "type": "string", "description": "Sub-agent type: general, explore, plan" },
                "model_override": { "type": "string", "description": "Model override for sub-agent" },
                "wait": { "type": "boolean", "description": "Wait for sub-agent to complete (default: true)" },
                "session_id": { "type": "string", "description": "Session ID" },
                "name": { "type": "string", "description": "Profile name" },
                "role": { "type": "string", "description": "Profile role" },
                "task": { "type": "string", "description": "Task description for advisor deliberation" }
            },
            "required": ["action"]
        })
    }

    fn requires_approval(&self) -> bool {
        false
    }

    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let domain_input: DomainInput = match serde_json::from_value(input.clone()) {
                Ok(v) => v,
                Err(e) => return ToolResult::error(format!("Failed to parse input: {}", e)),
            };

            let resource = if domain_input.resource.is_empty() {
                self.infer_resource(&domain_input.action).to_string()
            } else {
                domain_input.resource
            };

            if resource.is_empty() {
                return ToolResult::error(
                    "Resource is required. Available: memory, task, session, profile, context, advisors, ask",
                );
            }

            match resource.as_str() {
                "memory" => self.handle_memory(&input, ctx).await,
                "task" => self.handle_task(&input, ctx).await,
                "session" => self.handle_session(&input, ctx).await,
                "profile" => self.handle_profile(&input).await,
                "context" => self.handle_context(&input, ctx).await,
                "advisors" => self.handle_advisors(&input).await,
                "ask" => self.handle_ask(&input).await,
                "vision" => {
                    // Vision requires multimodal input which is handled at the provider level
                    ToolResult::error(
                        "Vision analysis requires image input. Send images directly in your message to the agent.",
                    )
                }
                other => ToolResult::error(format!(
                    "Resource {:?} not available. Available: memory, task, session, profile, context, advisors, ask",
                    other
                )),
            }
        })
    }
}
