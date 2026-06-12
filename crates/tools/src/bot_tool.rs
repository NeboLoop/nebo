use std::sync::Arc;

use crate::domain::DomainInput;
use crate::errors;
use crate::orchestrator::OrchestratorHandle;
use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};
use crate::run_querier::RunQuerierHandle;
use db::Store;
use tracing::{debug, warn};

/// Trait for advisor deliberation (implemented by agent::advisors::Runner).
/// Defined here to avoid circular dependencies between tools and agent crates.
pub trait AdvisorDeliberator: Send + Sync {
    fn deliberate<'a>(
        &'a self,
        task: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + 'a>>;
}

/// One structured sub-agent request for the deep-research harness. The agent does free
/// tool work with the named `aux_tools`, then is FORCED through a schema-validated
/// `StructuredOutput` call (see `agent::structured::agent_structured`).
pub struct StructuredTask {
    pub system: String,
    pub task: String,
    pub schema: serde_json::Value,
    /// STRAP tool names the sub-agent may call during its free phase (e.g. `["web"]`).
    pub aux_tools: Vec<String>,
    /// Browser-tab / session identity for this sub-agent. Aux-tool calls execute under
    /// this key so each sub-agent owns its own tab (the 1:1 sub-agent→tab model), while
    /// siblings keyed `subagent:{parent}:sa-{id}` share the parent's visited-page cache.
    pub tab_key: String,
    /// Optional cap on free-phase tool-use turns. `None` → the agent default. Used to
    /// hold a sub-agent to a single tool call (e.g. the reference deep-research search
    /// agent does ONE WebSearch per angle, not an open-ended browse loop).
    pub max_tool_turns: Option<u32>,
}

/// Trait for running forced-structured-output sub-agents AND executing single tools on
/// their behalf (implemented by `agent::structured_agent::StructuredRunner`). Defined
/// here so the deep-research harness in the tools crate can drive sub-agents without a
/// circular dependency on the agent crate (which owns the providers). Both methods
/// dispatch tool calls through the canonical `Registry::execute` — there is no separate
/// web pathway.
pub trait StructuredAgent: Send + Sync {
    fn run<'a>(
        &'a self,
        task: StructuredTask,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<serde_json::Value, String>> + Send + 'a>,
    >;

    /// Execute one registered tool directly (no LLM) under `tab_key` — for the harness's
    /// deterministic fetch+sanitize step. Returns the canonical [`ToolResult`] (content +
    /// `http_status` + `is_error`) so the caller can branch on rate-limit statuses.
    fn execute_tool<'a>(
        &'a self,
        tab_key: String,
        tool: String,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>>;

    /// Close the browser tab/page this sub-agent opened under `tab_key`, once it has
    /// finished — the 1:1 sub-agent→tab cleanup. No-op if it never opened one.
    fn close_tab<'a>(
        &'a self,
        tab_key: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>>;
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

/// AgentTool is the agent's self-management domain tool.
/// Resources: memory, task, session, context, advisors, ask, registry.
pub struct AgentTool {
    store: Arc<Store>,
    orchestrator: OrchestratorHandle,
    advisor_runner: Option<Arc<dyn AdvisorDeliberator>>,
    hybrid_searcher: Option<Arc<dyn HybridSearcher>>,
    structured_agent: Option<Arc<dyn StructuredAgent>>,
    run_querier: RunQuerierHandle,
    persona: Option<crate::agent_tool::PersonaTool>,
}

impl AgentTool {
    pub fn new(store: Arc<Store>, orchestrator: OrchestratorHandle) -> Self {
        Self {
            store,
            orchestrator,
            advisor_runner: None,
            hybrid_searcher: None,
            structured_agent: None,
            run_querier: crate::run_querier::new_handle(),
            persona: None,
        }
    }

    pub fn with_persona(mut self, persona: crate::agent_tool::PersonaTool) -> Self {
        self.persona = Some(persona);
        self
    }

    pub fn with_advisor_runner(mut self, runner: Arc<dyn AdvisorDeliberator>) -> Self {
        self.advisor_runner = Some(runner);
        self
    }

    pub fn with_structured_agent(mut self, agent: Arc<dyn StructuredAgent>) -> Self {
        self.structured_agent = Some(agent);
        self
    }

    pub fn with_run_querier(mut self, handle: RunQuerierHandle) -> Self {
        self.run_querier = handle;
        self
    }

    pub fn with_hybrid_searcher(mut self, searcher: Arc<dyn HybridSearcher>) -> Self {
        self.hybrid_searcher = Some(searcher);
        self
    }

    fn infer_resource(&self, action: &str) -> &str {
        match action {
            "store" | "save" | "recall" | "search" => "memory",
            "spawn" | "spawn_parallel" | "orchestrate" | "status" | "cancel" | "create"
            | "update" | "delete" => "task",
            "research" | "deep_research" | "submit_findings" => "research",
            "open_billing" => "profile",
            "history" | "query" => "session",
            "reset" | "compact" | "summary" => "context",
            "deliberate" => "advisors",
            "prompt" | "confirm" | "select" => "ask",
            "delegate" | "activate" | "deactivate" | "info" | "install" | "setup" | "reload"
            | "repair" | "stats" => "registry",
            _ => "",
        }
    }

    async fn handle_memory(&self, input: &serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let action = input["action"].as_str().unwrap_or("");

        match action {
            // "save" is the most common model misspelling of "store" — accept
            // it rather than burn a correction round-trip.
            "store" | "save" => {
                let key = input["key"].as_str().unwrap_or("");
                let value = input["value"].as_str().unwrap_or("");
                // The advertised `layer` param maps to the canonical namespace
                // for that layer; an explicit `namespace` overrides. Previously
                // `layer` was silently ignored, so every explicit store —
                // preferences, entities, daily facts alike — collapsed into
                // tacit/general and the categorized recall slices stayed empty.
                let layer_ns = match input["layer"].as_str().unwrap_or("") {
                    "daily" => format!("daily/{}", chrono::Local::now().format("%Y-%m-%d")),
                    "entity" => "entity/default".to_string(),
                    _ => "tacit/general".to_string(),
                };
                let namespace = input["namespace"].as_str().unwrap_or(&layer_ns);

                if key.is_empty() || value.is_empty() {
                    return ToolResult::error(errors::missing_param(
                        "store",
                        "key and value",
                        "bot(resource: \"memory\", action: \"store\", key: \"user/name\", value: \"Alice\")",
                    ));
                }

                debug!(
                    namespace = namespace,
                    key = key,
                    value_len = value.len(),
                    user_id = %ctx.user_id,
                    "memory store attempt"
                );

                match self
                    .store
                    .upsert_memory(namespace, key, value, None, None, &ctx.user_id)
                {
                    Ok(_) => {
                        // Verify the write was persisted by reading it back (different pool connection)
                        let verify =
                            self.store
                                .get_memory_by_key_and_user(namespace, key, &ctx.user_id);
                        match &verify {
                            Ok(Some(m)) => debug!(
                                key = key,
                                stored_value = %m.value,
                                "memory store verified on separate connection"
                            ),
                            Ok(None) => {
                                // Data not visible on a different connection — persistence failure
                                let total = self.store.count_memories().unwrap_or(-1);
                                warn!(
                                    namespace = namespace,
                                    key = key,
                                    user_id = %ctx.user_id,
                                    total_memories = total,
                                    "memory store: upsert OK but cross-connection verify found NOTHING"
                                );
                                return ToolResult::error(format!(
                                    "Memory store failed: data not persisted (wrote to [{}] {} but read-back returned nothing). \
                                     Total memories in DB: {}. This may indicate FTS trigger corruption — \
                                     try restarting the server.",
                                    namespace, key, total
                                ));
                            }
                            Err(e) => warn!(
                                key = key,
                                error = %e,
                                "memory store verify read failed"
                            ),
                        }
                        ToolResult::ok(format!(
                            "Stored memory: [{}] {} = {}",
                            namespace, key, value
                        ))
                    }
                    Err(e) => ToolResult::error(format!(
                        "Failed to store memory [{}] {}: {}. Do not retry immediately — this is a database error, not a parameter issue.",
                        namespace, key, e
                    )),
                }
            }
            "recall" => {
                let key = input["key"].as_str().unwrap_or("");
                let namespace = input["namespace"].as_str().unwrap_or("tacit/general");
                if key.is_empty() {
                    return ToolResult::error(errors::missing_param(
                        "recall",
                        "key",
                        "bot(resource: \"memory\", action: \"recall\", key: \"user/name\")",
                    ));
                }

                debug!(
                    namespace = namespace,
                    key = key,
                    user_id = %ctx.user_id,
                    "memory recall attempt"
                );

                match self
                    .store
                    .get_memory_by_key_and_user(namespace, key, &ctx.user_id)
                {
                    Ok(Some(mem)) => {
                        // Increment access count on recall
                        let _ =
                            self.store
                                .increment_memory_access_by_key(namespace, key, &ctx.user_id);
                        ToolResult::ok(format!("[{}] {}: {}", mem.namespace, mem.key, mem.value))
                    }
                    Ok(None) => {
                        // Fallback 1: try without user_id filter
                        let any = self.store.get_memory_by_key(namespace, key);
                        match any {
                            Ok(Some(m)) => {
                                warn!(
                                    namespace = namespace,
                                    key = key,
                                    expected_user_id = %ctx.user_id,
                                    actual_user_id = %m.user_id,
                                    "memory exists but user_id mismatch — returning anyway"
                                );
                                let _ = self
                                    .store
                                    .increment_memory_access_by_key(namespace, key, &m.user_id);
                                ToolResult::ok(format!("[{}] {}: {}", m.namespace, m.key, m.value))
                            }
                            Ok(None) => {
                                // Fallback 2: try key-only lookup across all namespaces
                                match self.store.find_memory_by_key(key) {
                                    Ok(Some(m)) => {
                                        warn!(
                                            expected_namespace = namespace,
                                            actual_namespace = %m.namespace,
                                            key = key,
                                            "memory found in different namespace"
                                        );
                                        ToolResult::ok(format!(
                                            "[{}] {}: {}",
                                            m.namespace, m.key, m.value
                                        ))
                                    }
                                    _ => {
                                        warn!(
                                            namespace = namespace,
                                            key = key,
                                            user_id = %ctx.user_id,
                                            "memory not found at all — not in DB"
                                        );
                                        ToolResult::ok(format!("No memory found for key: {}", key))
                                    }
                                }
                            }
                            Err(_) => ToolResult::ok(format!("No memory found for key: {}", key)),
                        }
                    }
                    Err(e) => ToolResult::error(format!(
                        "Failed to recall memory [{}] {}: {}. Do not retry — this is a database error.",
                        namespace, key, e
                    )),
                }
            }
            "search" => {
                let query = input["query"].as_str().unwrap_or("");
                let limit = input["limit"].as_i64().unwrap_or(20) as usize;

                if query.is_empty() {
                    return ToolResult::error(errors::missing_param(
                        "search",
                        "query",
                        "bot(resource: \"memory\", action: \"search\", query: \"project deadlines\")",
                    ));
                }

                // Use hybrid search (FTS5 + vector) when available
                if let Some(ref searcher) = self.hybrid_searcher {
                    let results = searcher.search(query, &ctx.user_id, limit).await;
                    if !results.is_empty() {
                        let lines: Vec<String> = results
                            .iter()
                            .map(|r| {
                                format!(
                                    "- [{}] {}: {} (score: {:.2})",
                                    r.namespace, r.key, r.value, r.score
                                )
                            })
                            .collect();
                        return ToolResult::ok(format!(
                            "Found {} memories:\n{}",
                            results.len(),
                            lines.join("\n")
                        ));
                    }
                    // Fall through to LIKE query if hybrid returned nothing
                }

                // Fallback: simple LIKE query
                match self
                    .store
                    .search_memories_by_user(&ctx.user_id, query, limit as i64, 0)
                {
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
                    Err(e) => ToolResult::error(format!(
                        "Memory search failed: {}. Do not retry — this is a database error. Try a different query or use action: \"list\" instead.",
                        e
                    )),
                }
            }
            "list" => {
                let namespace = input["namespace"].as_str().unwrap_or("");
                let limit = input["limit"].as_i64().unwrap_or(50);

                // Always scope to current user/agent — never list cross-agent memories
                let ns_prefix = if namespace.is_empty() { "tacit/" } else { namespace };
                let memories = self.store.list_memories_by_user_and_namespace(
                    &ctx.user_id,
                    ns_prefix,
                    limit,
                    0,
                );

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
                    return ToolResult::error(errors::missing_param(
                        "delete",
                        "key",
                        "bot(resource: \"memory\", action: \"delete\", key: \"user/name\")",
                    ));
                }
                // Delete scoped to current user/agent only — never cross-agent
                match self
                    .store
                    .delete_memory_by_key_and_user(namespace, key, &ctx.user_id)
                {
                    Ok(n) if n > 0 => {
                        ToolResult::ok(format!("Deleted {} memory entries for key: {}", n, key))
                    }
                    Ok(_) => ToolResult::ok(format!("No memory found with key: {}", key)),
                    Err(e) => ToolResult::error(format!("Failed to delete: {}", e)),
                }
            }
            "clear" => {
                let namespace = input["namespace"].as_str().unwrap_or("");
                if namespace.is_empty() {
                    return ToolResult::error(errors::missing_param(
                        "clear",
                        "namespace",
                        "bot(resource: \"memory\", action: \"clear\", namespace: \"tacit/general\")",
                    ));
                }
                match self
                    .store
                    .delete_memories_by_namespace_and_user(namespace, &ctx.user_id)
                {
                    Ok(count) => ToolResult::ok(format!(
                        "Cleared {} memories in namespace: {}",
                        count, namespace
                    )),
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
                let description = input["description"]
                    .as_str()
                    .unwrap_or(&task_prompt[..task_prompt.len().min(80)]);
                // No explicit override → inherit the parent run's model so the
                // sub-agent uses the same provider as the conversation that
                // spawned it (not the global default).
                let model_override = match input["model_override"].as_str() {
                    Some(m) if !m.is_empty() => m.to_string(),
                    _ => ctx.model_preference.clone().unwrap_or_default(),
                };
                let wait = input["wait"].as_bool().unwrap_or(true);
                let max_iterations = input["max_iterations"].as_u64().unwrap_or(0) as usize;
                let skills: Vec<String> = input["skills"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                let plugins: Vec<String> = input["plugins"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                let tools: Vec<String> = input["tools"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();

                if task_prompt.is_empty() {
                    return ToolResult::error(errors::missing_param(
                        "spawn",
                        "prompt",
                        "bot(resource: \"task\", action: \"spawn\", prompt: \"Research competitor pricing\")",
                    ));
                }

                let orch = match self.orchestrator.get() {
                    Some(o) => o,
                    None => return ToolResult::error(
                        "Sub-agent orchestrator not ready. The server may still be starting up. \
                         Try again in a moment, or do the work directly instead of delegating to a sub-agent.",
                    ),
                };

                let req = crate::orchestrator::SpawnRequest {
                    prompt: task_prompt.to_string(),
                    description: description.to_string(),
                    agent_type: agent_type.to_string(),
                    model_override: model_override.to_string(),
                    parent_session_id: ctx.session_id.clone(),
                    parent_session_key: ctx.session_key.clone(),
                    user_id: ctx.user_id.clone(),
                    wait,
                    parent_cancel: Some(ctx.cancel_token.clone()),
                    max_iterations,
                    skills,
                    plugins,
                    tools,
                    parent_stream_tx: ctx.stream_tx.clone(),
                    agent_id: String::new(),
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
            "spawn_parallel" => {
                let tasks = match input["tasks"].as_array() {
                    Some(arr) => arr,
                    None => return ToolResult::error(errors::missing_param(
                        "spawn_parallel",
                        "tasks",
                        "bot(resource: \"task\", action: \"spawn_parallel\", tasks: [{\"prompt\": \"task 1\"}, {\"prompt\": \"task 2\"}])",
                    )),
                };

                if tasks.is_empty() {
                    return ToolResult::error(
                        "tasks array must not be empty. Provide at least one task with a prompt.\n\
                         Example: bot(resource: \"task\", action: \"spawn_parallel\", tasks: [{\"prompt\": \"task 1\"}, {\"prompt\": \"task 2\"}])",
                    );
                }

                let orch = match self.orchestrator.get() {
                    Some(o) => o,
                    None => return ToolResult::error(
                        "Sub-agent orchestrator not ready. The server may still be starting up. \
                         Try again in a moment, or spawn tasks individually with action: \"spawn\".",
                    ),
                };

                let stream_tx = match ctx.stream_tx {
                    Some(ref tx) => tx.clone(),
                    None => {
                        return ToolResult::error(
                            "Stream sender not available for progress events. \
                             This usually means the request came from a non-streaming context. \
                             Use action: \"spawn\" for individual tasks instead of spawn_parallel.",
                        );
                    }
                };

                let requests: Vec<crate::orchestrator::SpawnRequest> = tasks
                    .iter()
                    .map(|t| {
                        let prompt = t["prompt"].as_str().unwrap_or("").to_string();
                        let description = t["description"]
                            .as_str()
                            .unwrap_or(&prompt[..prompt.len().min(80)])
                            .to_string();
                        let task_skills: Vec<String> = t["skills"]
                            .as_array()
                            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                            .unwrap_or_default();
                        let task_plugins: Vec<String> = t["plugins"]
                            .as_array()
                            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                            .unwrap_or_default();
                        let task_tools: Vec<String> = t["tools"]
                            .as_array()
                            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                            .unwrap_or_default();
                        crate::orchestrator::SpawnRequest {
                            prompt,
                            description,
                            agent_type: t["agent_type"].as_str().unwrap_or("general").to_string(),
                            model_override: t["model_override"].as_str().unwrap_or("").to_string(),
                            parent_session_id: ctx.session_id.clone(),
                            parent_session_key: ctx.session_key.clone(),
                            user_id: ctx.user_id.clone(),
                            wait: true, // spawn_parallel always waits for all
                            parent_cancel: Some(ctx.cancel_token.clone()),
                            max_iterations: t["max_iterations"].as_u64().unwrap_or(0) as usize,
                            skills: task_skills,
                            plugins: task_plugins,
                            tools: task_tools,
                            parent_stream_tx: ctx.stream_tx.clone(),
                            agent_id: String::new(),
                        }
                    })
                    .collect();

                match orch.spawn_parallel(requests, stream_tx).await {
                    Ok(result) => {
                        if result.success {
                            ToolResult::ok(format!(
                                "Parallel execution [{}] completed:\n\n{}",
                                result.task_id, result.output
                            ))
                        } else {
                            ToolResult::error(format!(
                                "Parallel execution [{}] had failures:\n\n{}\n\nError: {}",
                                result.task_id,
                                result.output,
                                result.error.unwrap_or_default()
                            ))
                        }
                    }
                    Err(e) => ToolResult::error(format!("Failed to spawn parallel agents: {}", e)),
                }
            }
            "orchestrate" => {
                let task_prompt = input["prompt"].as_str().unwrap_or("");
                if task_prompt.is_empty() {
                    return ToolResult::error(errors::missing_param(
                        "orchestrate",
                        "prompt",
                        "bot(resource: \"task\", action: \"orchestrate\", prompt: \"Plan and execute a market analysis\")",
                    ));
                }

                let orch = match self.orchestrator.get() {
                    Some(o) => o,
                    None => return ToolResult::error(
                        "Sub-agent orchestrator not ready. The server may still be starting up. \
                         Try again in a moment, or break the work into individual spawn calls.",
                    ),
                };

                match orch
                    .execute_dag(
                        task_prompt,
                        "",
                        &ctx.session_id,
                        Some(ctx.cancel_token.clone()),
                    )
                    .await
                {
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
                    return ToolResult::error(errors::missing_param(
                        "cancel",
                        "task_id",
                        "bot(resource: \"task\", action: \"cancel\", task_id: \"abc123\")",
                    ));
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
                    return ToolResult::error(errors::missing_param(
                        "status",
                        "task_id",
                        "bot(resource: \"task\", action: \"status\", task_id: \"abc123\")",
                    ));
                }

                if let Some(orch) = self.orchestrator.get() {
                    match orch.status(task_id).await {
                        Ok(status) => return ToolResult::ok(status),
                        Err(_) => {} // Fall through to DB lookup
                    }
                }

                // Fall back to DB lookup
                match self.store.get_pending_task(task_id) {
                    Ok(Some(task)) => {
                        let mut result = format!(
                            "Task: {}\nType: {}\nStatus: {}\nDescription: {}",
                            task.id,
                            task.task_type,
                            task.status,
                            task.description.as_deref().unwrap_or("-"),
                        );
                        if let Some(ref output) = task.output {
                            result.push_str(&format!("\nOutput:\n{}", output));
                        }
                        if let Some(ref err) = task.last_error {
                            result.push_str(&format!("\nError: {}", err));
                        }
                        ToolResult::ok(result)
                    }
                    Ok(None) => ToolResult::error(format!("Task '{}' not found", task_id)),
                    Err(e) => ToolResult::error(format!("Failed to get status: {}", e)),
                }
            }
            "create" => {
                let subject = input["subject"].as_str().unwrap_or("");
                if subject.is_empty() {
                    return ToolResult::error(errors::missing_param(
                        "create",
                        "subject",
                        "bot(resource: \"task\", action: \"create\", subject: \"Draft the quarterly report\")",
                    ));
                }
                let description = input["description"].as_str();
                let list_id = format!("session:{}", ctx.session_id);

                match self.store.create_task_item(&list_id, subject, description) {
                    Ok(task) => ToolResult::ok(format!("Task {} created: {}", task.id, subject)),
                    Err(e) => ToolResult::error(format!("Failed to create task: {}", e)),
                }
            }
            "update" => {
                let task_id = input["task_id"].as_str().unwrap_or("");
                if task_id.is_empty() {
                    return ToolResult::error(errors::missing_param(
                        "update",
                        "task_id",
                        "bot(resource: \"task\", action: \"update\", task_id: \"1\", status: \"completed\")",
                    ));
                }
                let status = input["status"].as_str().unwrap_or("");
                if status.is_empty() {
                    return ToolResult::error(errors::missing_param(
                        "update",
                        "status",
                        "bot(resource: \"task\", action: \"update\", task_id: \"1\", status: \"completed\")\nValid statuses: pending, in_progress, completed, failed",
                    ));
                }
                let output = input["output"].as_str();
                let error = input["error"].as_str();

                match self
                    .store
                    .update_task_item(task_id, status, output, error, 0, 0)
                {
                    Ok(_) => ToolResult::ok(format!("Task {} updated to {}", task_id, status)),
                    Err(e) => ToolResult::error(format!("Failed to update task: {}", e)),
                }
            }
            "list" => {
                let list_id = format!("session:{}", ctx.session_id);
                match self.store.list_task_items(&list_id) {
                    Ok(tasks) => {
                        if tasks.is_empty() {
                            ToolResult::ok("No tasks.")
                        } else {
                            let lines: Vec<String> = tasks
                                .iter()
                                .map(|t| {
                                    let output_hint =
                                        if t.status == "completed" && t.output.is_some() {
                                            " [has output]"
                                        } else {
                                            ""
                                        };
                                    let desc = t.description.as_deref().unwrap_or(&t.prompt);
                                    format!("{} [{}] {}{}", t.id, t.status, desc, output_hint)
                                })
                                .collect();
                            ToolResult::ok(format!("{} tasks:\n{}", tasks.len(), lines.join("\n")))
                        }
                    }
                    Err(e) => ToolResult::error(format!("Failed to list tasks: {}", e)),
                }
            }
            "get" => {
                let task_id = input["task_id"].as_str().unwrap_or("");
                if task_id.is_empty() {
                    return ToolResult::error(errors::missing_param(
                        "get",
                        "task_id",
                        "bot(resource: \"task\", action: \"get\", task_id: \"1\")",
                    ));
                }
                match self.store.get_task_item(task_id) {
                    Ok(Some(t)) => {
                        let desc = t.description.as_deref().unwrap_or(&t.prompt);
                        let mut result = format!("Task {}: {}\nStatus: {}\n", t.id, desc, t.status);
                        if let Some(ref output) = t.output {
                            result.push_str(&format!("Output: {}\n", output));
                        }
                        if let Some(ref error) = t.last_error {
                            result.push_str(&format!("Error: {}\n", error));
                        }
                        ToolResult::ok(result)
                    }
                    Ok(None) => ToolResult::error(format!("Task {} not found", task_id)),
                    Err(e) => ToolResult::error(format!("Failed to get task: {}", e)),
                }
            }
            "delete" => {
                let task_id = input["task_id"].as_str().unwrap_or("");
                if task_id.is_empty() {
                    return ToolResult::error(errors::missing_param(
                        "delete",
                        "task_id",
                        "bot(resource: \"task\", action: \"delete\", task_id: \"1\")",
                    ));
                }
                match self
                    .store
                    .update_task_item(task_id, "skipped", None, None, 0, 0)
                {
                    Ok(_) => ToolResult::ok(format!("Task {} deleted", task_id)),
                    Err(e) => ToolResult::error(format!("Failed to delete task: {}", e)),
                }
            }
            _ => ToolResult::error(format!(
                "Unknown task action: {}. Available: spawn, spawn_parallel, orchestrate, status, cancel, create, update, list, get, delete",
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
                let session_id = input["session_id"].as_str().unwrap_or(&ctx.session_id);
                // Sessions use chat_messages table with session_id as the chat_id
                match self.store.get_chat_messages(session_id) {
                    Ok(msgs) => {
                        if msgs.is_empty() {
                            return ToolResult::ok(format!(
                                "No messages in session: {}",
                                session_id
                            ));
                        }
                        let recent: Vec<&db::models::ChatMessage> =
                            msgs.iter().rev().take(50).collect();
                        let lines: Vec<String> = recent
                            .iter()
                            .rev()
                            .map(|m| {
                                let preview = if m.content.len() > 200 {
                                    format!("{}...", crate::truncate_str(&m.content, 200))
                                } else {
                                    m.content.clone()
                                };
                                format!("[{}] {}: {}", m.id, m.role, preview)
                            })
                            .collect();
                        ToolResult::ok(format!("{} messages:\n{}", msgs.len(), lines.join("\n")))
                    }
                    Err(e) => ToolResult::error(format!("Failed to get history: {}", e)),
                }
            }
            "status" => ToolResult::ok(format!("Current session: {}", ctx.session_id)),
            "clear" => {
                let session_id = input["session_id"].as_str().unwrap_or(&ctx.session_id);
                match self.store.reset_session(session_id) {
                    Ok(_) => ToolResult::ok(format!("Cleared session: {}", session_id)),
                    Err(e) => ToolResult::error(format!("Failed to clear session: {}", e)),
                }
            }
            "query" => {
                let query_text = input["query"].as_str().unwrap_or("");
                if query_text.is_empty() {
                    return ToolResult::error(errors::missing_param(
                        "query",
                        "query",
                        "bot(resource: \"session\", action: \"query\", query: \"meeting notes\")",
                    ));
                }
                let limit = input["limit"].as_i64().unwrap_or(20) as usize;

                // Fallback: list sessions, then search each
                match self.store.list_sessions(100, 0) {
                    Ok(sessions) => {
                        let mut found = Vec::new();
                        for session in &sessions {
                            if let Ok(msgs) = self.store.get_chat_messages(&session.id) {
                                for msg in msgs {
                                    if msg
                                        .content
                                        .to_lowercase()
                                        .contains(&query_text.to_lowercase())
                                    {
                                        let preview = if msg.content.len() > 150 {
                                            format!("{}...", crate::truncate_str(&msg.content, 150))
                                        } else {
                                            msg.content.clone()
                                        };
                                        found.push(format!(
                                            "- [{}] {}: {}",
                                            session.id, msg.role, preview
                                        ));
                                        if found.len() >= limit {
                                            break;
                                        }
                                    }
                                }
                            }
                            if found.len() >= limit {
                                break;
                            }
                        }
                        if found.is_empty() {
                            ToolResult::ok(format!("No messages found matching: {}", query_text))
                        } else {
                            ToolResult::ok(format!(
                                "Found {} messages:\n{}",
                                found.len(),
                                found.join("\n")
                            ))
                        }
                    }
                    Err(e) => ToolResult::error(format!("Cross-session search failed: {}", e)),
                }
            }
            _ => ToolResult::error(format!(
                "Unknown session action: {}. Available: list, history, status, clear, query",
                action
            )),
        }
    }

    async fn handle_context(&self, input: &serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let action = input["action"].as_str().unwrap_or("");
        match action {
            "summary" => {
                // Get recent messages from the current session for a summary
                // Use session_key as the chat_id (messages are stored under session name, not session UUID)
                let chat_id = if !ctx.session_key.is_empty() {
                    &ctx.session_key
                } else {
                    &ctx.session_id
                };
                match self.store.get_chat_messages(chat_id) {
                    Ok(msgs) => {
                        let count = msgs.len();
                        let user_count = msgs.iter().filter(|m| m.role == "user").count();
                        let assistant_count = msgs.iter().filter(|m| m.role == "assistant").count();
                        let tool_count = msgs.iter().filter(|m| m.role == "tool").count();

                        let last_topic = msgs
                            .iter()
                            .rev()
                            .find(|m| m.role == "user")
                            .map(|m| {
                                if m.content.len() > 100 {
                                    format!("{}...", crate::truncate_str(&m.content, 100))
                                } else {
                                    m.content.clone()
                                }
                            })
                            .unwrap_or_else(|| "-".to_string());

                        ToolResult::ok(format!(
                            "Session: {}\nMessages: {} ({} user, {} assistant, {} tool)\nLast user message: {}",
                            ctx.session_id,
                            count,
                            user_count,
                            assistant_count,
                            tool_count,
                            last_topic
                        ))
                    }
                    Err(e) => ToolResult::error(format!("Failed to get context: {}", e)),
                }
            }
            "reset" => match self.store.reset_session(&ctx.session_id) {
                Ok(_) => ToolResult::ok("Session context has been reset."),
                Err(e) => ToolResult::error(format!("Failed to reset: {}", e)),
            },
            "compact" => {
                // Compaction is handled automatically by the agentic loop's sliding window.
                // This explicit call is a no-op signal that the agent wants to reduce context.
                ToolResult::ok(
                    "Context compaction noted. The agentic loop will apply sliding window pruning on the next iteration.",
                )
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
                    return ToolResult::error(errors::missing_param(
                        "deliberate",
                        "task",
                        "bot(resource: \"advisors\", action: \"deliberate\", task: \"Should we use PostgreSQL or SQLite?\")",
                    ));
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
                        let enabled: Vec<_> = advisors.iter().filter(|a| a.enabled != 0).collect();

                        if enabled.is_empty() {
                            return ToolResult::ok(format!(
                                "No advisors configured. Proceeding with own judgment on: {}",
                                task
                            ));
                        }

                        let mut perspectives = Vec::new();
                        for advisor in &enabled {
                            let persona = if advisor.persona.is_empty() {
                                "general advisor"
                            } else {
                                &advisor.persona
                            };
                            let name = &advisor.name;
                            let role = if advisor.role.is_empty() {
                                "advisor"
                            } else {
                                &advisor.role
                            };
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
            "list" => match self.store.list_advisors() {
                Ok(advisors) => {
                    if advisors.is_empty() {
                        ToolResult::ok("No advisors configured.")
                    } else {
                        let lines: Vec<String> = advisors
                            .iter()
                            .map(|a| {
                                let enabled = if a.enabled != 0 {
                                    "enabled"
                                } else {
                                    "disabled"
                                };
                                let desc = if a.description.is_empty() {
                                    "-"
                                } else {
                                    &a.description
                                };
                                format!("- {} [{}] — {}", a.name, enabled, desc)
                            })
                            .collect();
                        ToolResult::ok(format!(
                            "{} advisors:\n{}",
                            advisors.len(),
                            lines.join("\n")
                        ))
                    }
                }
                Err(e) => ToolResult::error(format!("Failed to list advisors: {}", e)),
            },
            _ => ToolResult::error(format!(
                "Unknown advisors action: {}. Available: deliberate, list",
                action
            )),
        }
    }

    async fn handle_ask(&self, input: &serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let action = input["action"].as_str().unwrap_or("prompt");
        match action {
            "prompt" | "confirm" | "select" => {
                let text = input["text"].as_str().unwrap_or("");
                if text.is_empty() {
                    return ToolResult::error(errors::missing_param(
                        action,
                        "text",
                        "bot(resource: \"ask\", action: \"prompt\", text: \"What would you like to do?\")",
                    ));
                }
                let options = input
                    .get("options")
                    .cloned()
                    .unwrap_or(serde_json::json!([]));

                // Build widget definition from action type
                let widget_type = match action {
                    "confirm" => "confirm",
                    "select" => {
                        if options.as_array().map_or(0, |a| a.len()) > 5 {
                            "select"
                        } else {
                            "buttons"
                        }
                    }
                    _ => "buttons",
                };
                let widgets = serde_json::json!([{
                    "type": widget_type,
                    "options": options,
                }]);

                match ctx.ask_user(text, widgets).await {
                    Some(response) => ToolResult::ok(
                        serde_json::json!({
                            "response": response,
                        })
                        .to_string(),
                    ),
                    None => ToolResult::error(
                        "Ask prompt not supported in this context — no UI is connected. \
                         This happens when running as a sub-agent or in a non-interactive channel. \
                         Make a reasonable decision and proceed, or explain your options to the user in your response.",
                    ),
                }
            }
            _ => ToolResult::error(format!(
                "Unknown ask action: {}. Available: prompt, confirm, select",
                action
            )),
        }
    }

    /// Handle runs resource — scoped agent visibility into the global RunRegistry.
    ///
    /// Primary agent ("main") sees all runs. Persona agents see only their own.
    async fn handle_runs(&self, input: &serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let action = input["action"].as_str().unwrap_or("list");

        let querier = match self.run_querier.get() {
            Some(q) => q,
            None => return ToolResult::error(
                "Run registry not available. The server may still be starting up. Try again in a moment.",
            ),
        };

        // Derive caller's entity_id from session_key.
        // Format: "agent:<uuid>:<channel>" → entity is the uuid.
        // Anything else (e.g., "default", "main") → "main" (primary agent).
        let caller_entity_id = if ctx.session_key.starts_with("agent:") {
            ctx.session_key
                .split(':')
                .nth(1)
                .unwrap_or("main")
                .to_string()
        } else {
            "main".to_string()
        };

        match action {
            "list" => {
                let runs = querier.list_runs(&caller_entity_id).await;
                if runs.is_empty() {
                    return ToolResult::ok("No active agent runs.");
                }
                let lines: Vec<String> = runs
                    .iter()
                    .map(|r| {
                        let tool_info = if r.current_tool.is_empty() {
                            String::new()
                        } else {
                            format!(" — running: {}", r.current_tool)
                        };
                        format!(
                            "- [{}] {} ({}) · {} tools · {}s{}",
                            &r.run_id[..8.min(r.run_id.len())],
                            r.entity_name,
                            r.origin,
                            r.tool_call_count,
                            r.elapsed_secs,
                            tool_info,
                        )
                    })
                    .collect();
                ToolResult::ok(format!("{} active runs:\n{}", runs.len(), lines.join("\n")))
            }
            "cancel" => {
                let run_id = input["run_id"].as_str().unwrap_or("");
                if run_id.is_empty() {
                    return ToolResult::error(errors::missing_param(
                        "cancel",
                        "run_id",
                        "agent(resource: \"runs\", action: \"cancel\", run_id: \"abc123\")",
                    ));
                }
                match querier.cancel_run(run_id, &caller_entity_id).await {
                    Ok(true) => ToolResult::ok(format!("Cancelled run {}", run_id)),
                    Ok(false) => ToolResult::error(format!("Run {} not found", run_id)),
                    Err(e) => ToolResult::error(e),
                }
            }
            _ => ToolResult::error(format!(
                "Unknown runs action: {}. Available: list, cancel",
                action
            )),
        }
    }

    /// Owner/account profile: read account info, update the bot's identity, or open billing.
    async fn handle_profile(&self, input: &serde_json::Value) -> ToolResult {
        let action = input["action"].as_str().unwrap_or("get");
        let api = match crate::build_neboai_api(&self.store) {
            Ok(a) => a,
            Err(e) => return ToolResult::error(e),
        };
        match action {
            "get" => {
                let mut out = format!("Bot ID: {}\n", api.bot_id());
                match api.billing_subscription().await {
                    Ok(v) => {
                        if let Some(plan) = v.get("plan").and_then(|p| p.as_str()) {
                            out.push_str(&format!("Plan: {}\n", plan));
                        }
                        out.push_str(&format!("Subscription: {}", v));
                    }
                    Err(e) => out.push_str(&format!("(plan unavailable: {})", e)),
                }
                ToolResult::ok(out)
            }
            "update" => {
                let name = input["name"].as_str().unwrap_or("");
                let role = input["role"].as_str().unwrap_or("");
                if name.is_empty() && role.is_empty() {
                    return ToolResult::error(errors::missing_param(
                        "update",
                        "name",
                        "agent(resource: \"profile\", action: \"update\", name: \"...\", role: \"...\")",
                    ));
                }
                match api.update_bot_identity(name, role).await {
                    Ok(_) => ToolResult::ok(format!(
                        "Updated bot identity (name: {:?}, role: {:?})",
                        name, role
                    )),
                    Err(e) => ToolResult::error(format!("Failed to update profile: {}", e)),
                }
            }
            "open_billing" => match api.billing_portal().await {
                Ok(v) => {
                    let url = v.get("portalUrl").and_then(|u| u.as_str()).unwrap_or("");
                    if url.is_empty() {
                        return ToolResult::error("Billing portal URL not available.");
                    }
                    open_url(url);
                    ToolResult::ok(format!("Opened billing portal: {}", url))
                }
                Err(e) => ToolResult::error(format!("Failed to open billing: {}", e)),
            },
            other => ToolResult::error(format!(
                "Unknown profile action: {}. Available: get, update, open_billing",
                other
            )),
        }
    }

    async fn handle_research(&self, input: &serde_json::Value, ctx: &ToolContext) -> ToolResult {
        let action = input["action"].as_str().unwrap_or("");

        match action {
            "deep_research" => self.handle_deep_research(input, ctx).await,
            "research" => {
                let query = input["query"].as_str().unwrap_or("");
                if query.is_empty() {
                    return ToolResult::error(errors::missing_param(
                        "research",
                        "query",
                        "bot(resource: \"research\", action: \"research\", query: \"What are the latest trends in AI?\")",
                    ));
                }

                let data_dir = match config::data_dir() {
                    Ok(d) => d,
                    Err(e) => {
                        return ToolResult::error(format!("Cannot determine data dir: {}", e));
                    }
                };

                let run_id = format!("research-{}", uuid::Uuid::new_v4().as_simple());

                let run_dir = match crate::research::create_run_dir(&data_dir, &run_id, query) {
                    Ok(d) => d,
                    Err(e) => {
                        return ToolResult::error(format!("Failed to create research dir: {}", e));
                    }
                };

                ToolResult::ok(format!(
                    "Research mode active. Run ID: {}. Dir: {}.\n\n{}",
                    run_id,
                    run_dir.display(),
                    crate::research::RESEARCH_LEAD_PROMPT,
                ))
            }
            "submit_findings" => {
                let subtask_id = input["subtask_id"].as_str().unwrap_or("");
                if subtask_id.is_empty() {
                    return ToolResult::error(errors::missing_param(
                        "submit_findings",
                        "subtask_id",
                        "bot(resource: \"research\", action: \"submit_findings\", subtask_id: \"sub-1\", findings: [{\"claim\": \"...\", \"source_url\": \"...\"}])",
                    ));
                }

                // Parse findings from JSON
                let findings_arr = match input["findings"].as_array() {
                    Some(arr) => arr,
                    None => return ToolResult::error(errors::missing_param(
                        "submit_findings",
                        "findings",
                        "bot(resource: \"research\", action: \"submit_findings\", subtask_id: \"sub-1\", findings: [{\"claim\": \"...\", \"source_url\": \"...\", \"confidence\": 0.9}])",
                    )),
                };

                let mut findings = Vec::new();
                for (i, f) in findings_arr.iter().enumerate() {
                    findings.push(crate::research::Finding {
                        claim: f["claim"].as_str().unwrap_or("").to_string(),
                        source_url: f["source_url"].as_str().unwrap_or("").to_string(),
                        source_ref: f["source_ref"].as_str().unwrap_or("").to_string(),
                        confidence: f["confidence"].as_f64().unwrap_or(0.5) as f32,
                        quote: f["quote"].as_str().unwrap_or("").to_string(),
                    });
                    if findings[i].claim.is_empty() {
                        return ToolResult::error(format!("findings[{}].claim is empty", i));
                    }
                }

                let gaps: Vec<String> = input["gaps"]
                    .as_array()
                    .unwrap_or(&vec![])
                    .iter()
                    .filter_map(|g| g.as_str().map(String::from))
                    .collect();

                let worker_findings = crate::research::WorkerFindings {
                    subtask_id: subtask_id.to_string(),
                    findings,
                    gaps: gaps.clone(),
                };

                // Find the active research dir — look for it in the session key context.
                // The research dir is passed to workers in their prompt, so they'll include it.
                // For now, scan <data_dir>/research/ for the most recent run with status "running".
                let data_dir = match config::data_dir() {
                    Ok(d) => d,
                    Err(e) => {
                        return ToolResult::error(format!("Cannot determine data dir: {}", e));
                    }
                };

                let research_dir = data_dir.join("research");
                let run_dir = match crate::research::find_active_run_dir(&research_dir) {
                    Some(d) => d,
                    None => {
                        return ToolResult::error(
                            "No active research run found. You must start a research run first with:\n\
                             bot(resource: \"research\", action: \"research\", query: \"your research question\")\n\
                             Then submit findings from worker sub-agents using submit_findings.",
                        );
                    }
                };

                match crate::research::write_worker_findings(&run_dir, &worker_findings) {
                    Ok(()) => ToolResult::ok(format!(
                        "Findings submitted. {} claims, {} gaps.",
                        worker_findings.findings.len(),
                        gaps.len()
                    )),
                    Err(e) => ToolResult::error(format!("Failed to write findings: {}", e)),
                }
            }
            other => ToolResult::error(format!(
                "Unknown research action: {:?}. Available: deep_research, research, submit_findings",
                other
            )),
        }
    }

    /// Run the deterministic deep-research harness: scope → search → fetch/extract →
    /// 3-vote adversarial verify → synthesize. Unlike `research` (a manual lead-prompt
    /// flow the agent drives itself), this is a single bounded, self-contained action.
    async fn handle_deep_research(
        &self,
        input: &serde_json::Value,
        ctx: &ToolContext,
    ) -> ToolResult {
        let query = input["query"].as_str().unwrap_or("").trim();
        if query.is_empty() {
            return ToolResult::error(errors::missing_param(
                "deep_research",
                "query",
                "bot(resource: \"research\", action: \"deep_research\", query: \"How effective is X for Y?\", depth: \"standard\")",
            ));
        }

        let agent = match &self.structured_agent {
            Some(a) => a.clone(),
            None => {
                return ToolResult::error(
                    "Deep research is unavailable: no structured-output-capable AI provider is configured. \
                     Configure a provider (Anthropic/OpenAI/Gemini/Janus) and retry.",
                );
            }
        };

        let depth = input["depth"].as_str().unwrap_or("standard");
        let cfg = crate::deep_research::Config::for_depth(depth);

        let data_dir = match config::data_dir() {
            Ok(d) => d,
            Err(e) => return ToolResult::error(format!("Cannot determine data dir: {}", e)),
        };
        let run_id = format!("research-{}", uuid::Uuid::new_v4().as_simple());

        // ── Scope checkpoint: decompose the question, then confirm the plan with the user
        // before the (slow, credit-costly) fan-out. Only gate INTERACTIVE (User-origin) runs:
        // automations/cron/channels/sub-agents (and an explicit `confirm: false`) proceed
        // without prompting — an automation must never block on a UI button.
        let confirm_gate = matches!(ctx.origin, crate::origin::Origin::User)
            && input["confirm"].as_bool().unwrap_or(true);
        let scope = crate::deep_research::scope(&agent, query, &cfg).await;
        let angle_labels: Vec<&str> = scope.angles.iter().map(|a| a.label.as_str()).collect();
        let mut plan = format!(
            "I'll research \"{}\" across {} angles at **{}** depth — a verified multi-source \
             search that takes a couple of minutes.\n\nAngles: {}.",
            query,
            scope.angles.len(),
            depth,
            angle_labels.join(", "),
        );
        if scope.clarifying_questions.is_empty() {
            plan.push_str("\n\nStart the research, or refine the plan first?");
        } else {
            plan.push_str("\n\nA few details would sharpen this:");
            for q in &scope.clarifying_questions {
                plan.push_str(&format!("\n• {q}"));
            }
            plan.push_str("\n\nStart as-is, refine the plan, or cancel?");
        }
        if confirm_gate {
            let widgets = serde_json::json!([{ "type": "buttons", "options": ["Start research", "Refine the plan", "Cancel"] }]);
            if let Some(resp) = ctx.ask_user(&plan, widgets).await {
                let r = resp.to_lowercase();
                if r.contains("cancel") {
                    return ToolResult::ok(format!(
                        "Research not started. I was going to cover: {}. Add any constraints \
                         (budget, region, use-case, time window) and ask again.",
                        angle_labels.join(", ")
                    ));
                }
                if r.contains("refine") {
                    // Hand control back to the conversation: the agent asks what to change,
                    // then re-invokes deep_research with the refinement folded into the query.
                    return ToolResult::ok(format!(
                        "The user wants to adjust this research plan before running — do NOT start \
                         the research yet. Ask them what to change: narrow or broaden the topic, \
                         add or drop an angle, or change depth (quick/standard/deep). Then call \
                         deep_research again with their changes folded into the query. The current \
                         plan was {} angles ({}) at {} depth.",
                        scope.angles.len(),
                        angle_labels.join(", "),
                        depth
                    ));
                }
            }
        }

        // Pre-compute the run's report path + a unique, readable Work-panel filename
        // (the harness writes a generic report.md per run-dir, which would collide in files/).
        let report_src = data_dir.join("research").join(&run_id).join("report.md");
        let files_dir = data_dir.join("files");
        let short = run_id.rsplit('-').next().unwrap_or(&run_id);
        let work_name = format!(
            "{}-{}.md",
            research_slug(query),
            &short[..short.len().min(8)]
        );

        match crate::deep_research::run(
            agent,
            data_dir,
            run_id,
            query.to_string(),
            cfg,
            ctx.cancel_token.clone(),
            ctx.stream_tx.clone(),
            Some(scope),
        )
        .await
        {
            Ok(report) => {
                let mut result = ToolResult::ok(crate::deep_research::format_report(&report));
                // Surface the report in the Work panel under its unique name.
                if report_src.exists() {
                    let _ = std::fs::create_dir_all(&files_dir);
                    let dest = files_dir.join(&work_name);
                    if std::fs::copy(&report_src, &dest).is_ok() {
                        result = result.with_image_url(dest.to_string_lossy().to_string());
                    }
                }
                result
            }
            Err(e) => ToolResult::error(format!("Deep research failed: {}", e)),
        }
    }
}

/// Slugify a research question into a readable filename stem (lowercase, alnum + single
/// dashes, capped). Falls back to "research" if the question has no usable characters.
fn research_slug(question: &str) -> String {
    let mut out = String::new();
    for c in question.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
        if out.len() >= 40 {
            break;
        }
    }
    let slug = out.trim_matches('-').to_string();
    if slug.is_empty() { "research".to_string() } else { slug }
}

impl DynTool for AgentTool {
    fn name(&self) -> &str {
        "agent"
    }

    fn description(&self) -> String {
        "Agent self-management — memory, tasks, sub-agents, sessions, context, advisors, vision, and ask.\n\
         USE THIS when: spawning sub-agents, tracking multi-step work, searching memory, managing sessions, analyzing images, or asking the user a question.\n\n\
         Sub-agents (parallel work):\n\
         - agent(resource: \"task\", action: \"spawn\", prompt: \"Research competitor pricing\") — Spawn and wait (default)\n\
         - agent(resource: \"task\", action: \"spawn\", prompt: \"Draft an NDA for...\", skills: [\"docx-generation\", \"contract-summary\"]) — Spawn with skills pre-loaded\n\
         - agent(resource: \"task\", action: \"spawn\", prompt: \"Check inbox for urgent items\", plugins: [\"PLUG-PJ3Z-ECFV\"], tools: [\"loop\", \"message\"]) — Spawn with plugin and tool docs\n\
         - agent(resource: \"task\", action: \"spawn\", prompt: \"...\", wait: false) — Background; result delivered when done\n\
         - agent(resource: \"task\", action: \"status\", task_id: \"...\") — Check background agent status\n\
         - agent(resource: \"task\", action: \"cancel\", task_id: \"...\") — Cancel a running sub-agent\n\
         - agent(resource: \"task\", action: \"spawn_parallel\", tasks: [{\"prompt\": \"...\", \"tools\": [\"web\"]}, ...]) — Run multiple sub-agents concurrently, return all results\n\
         IMPORTANT: Always pass plugins and tools the sub-agent needs. Sub-agents are born blind — they only know what you tell them.\n\
         - plugins: install codes for plugins the sub-agent should use (from your agent config or current session)\n\
         - tools: STRAP tool names the sub-agent needs (\"web\", \"loop\", \"message\", \"system\", etc.)\n\
         - skills: skill names for SKILL.md instructions pre-loaded into context\n\
         ALWAYS spawn when: comparing across multiple websites, researching independent topics, any task with 2+ independent web lookups.\n\
         Spawn when: multiple independent tasks, long-running research, skill-heavy work. Do it yourself when: simple task, dependent results.\n\n\
         Work tracking:\n\
         - agent(resource: \"task\", action: \"create\", subject: \"Test shell tool\") — Create a trackable step\n\
         - agent(resource: \"task\", action: \"update\", task_id: \"1\", status: \"completed\") — Mark done\n\
         - agent(resource: \"task\", action: \"list\") — See all tasks and sub-agents\n\n\
         Memory (3-tier persistence):\n\
         - agent(resource: \"memory\", action: \"store\", key: \"user/name\", value: \"Alice\", layer: \"tacit\") — Store a fact\n\
         - agent(resource: \"memory\", action: \"recall\", key: \"user/name\") — Recall a specific fact\n\
         - agent(resource: \"memory\", action: \"search\", query: \"...\") — Search across all memories\n\
         Layers: \"tacit\" (long-term preferences — MOST COMMON), \"daily\" (today's facts, auto-expires), \"entity\" (people/places/things)\n\
         For likes/dislikes/working-style facts, store with namespace: \"tacit/preferences\" — those inject into every conversation automatically.\n\n\
         Sessions:\n\
         - agent(resource: \"session\", action: \"list\") / history / status / clear / query\n\n\
         Profile:\n\
         - agent(resource: \"profile\", action: \"get\") / update / open_billing\n\n\
         Advisors (internal deliberation):\n\
         - agent(resource: \"advisors\", action: \"deliberate\", task: \"Should we use PostgreSQL or SQLite?\")\n\n\
         Research (deterministic deep-research harness — fans out searches, fetches sources, adversarially fact-checks claims, returns a cited report):\n\
         - agent(resource: \"research\", action: \"deep_research\", query: \"How effective is X for Y?\", depth: \"standard\") — Run the full harness; depth: \"quick\"|\"standard\"|\"deep\". Use when the user wants a fact-checked, multi-source report. Interactive runs confirm the plan first; pass confirm: false to skip that gate (for automations / unattended runs).\n\n\
         Context:\n\
         - agent(resource: \"context\", action: \"summary\") / compact / reset\n\n\
         Ask (user input):\n\
         - agent(resource: \"ask\", action: \"prompt\", text: \"What would you like?\")\n\
         - agent(resource: \"ask\", action: \"confirm\", text: \"Proceed with deletion?\")\n\
         - agent(resource: \"ask\", action: \"select\", text: \"Pick a color\", options: [\"red\", \"blue\"])\n\n\
         GUARDRAILS: When storing memory, use the exact phrasing the user used. Do not paraphrase.\n\n\
         Registry (installed agent management + delegation):\n\
         - agent(resource: \"registry\", action: \"delegate\", name: \"chief-of-staff\", prompt: \"Check my email\") — Delegate to a named agent\n\
         - agent(resource: \"registry\", action: \"list\") — List installed agents\n\
         - agent(resource: \"registry\", action: \"activate\", name: \"...\") — Activate an agent\n\
         - agent(resource: \"registry\", action: \"info\", name: \"...\") — Show agent details\n\
         - agent(resource: \"registry\", action: \"install\", code: \"AGNT-XXXX-XXXX\") — Install from marketplace\n\
         - agent(resource: \"registry\", action: \"create\", name: \"...\", description: \"...\") — Create a user agent"
            .to_string()
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "resource": {
                    "type": "string",
                    "description": "REQUIRED. The agent resource category — determines which actions are available.",
                    "enum": ["memory", "task", "session", "context", "advisors", "ask", "research", "registry", "runs", "profile"]
                },
                "action": {
                    "type": "string",
                    "description": "The operation to perform on the selected resource. Never put a resource name here."
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
                "skills": { "type": "array", "items": { "type": "string" }, "description": "Skill names to pre-load in the sub-agent's context. The full SKILL.md is injected so the sub-agent has instructions without needing to discover them." },
                "plugins": { "type": "array", "items": { "type": "string" }, "description": "Plugin install codes (e.g. PLUG-XXXX-XXXX) to give the sub-agent. Plugin docs and capabilities are injected so it knows how to use them." },
                "tools": { "type": "array", "items": { "type": "string" }, "description": "STRAP domain tool names (e.g. \"web\", \"loop\", \"message\") the sub-agent needs. Tool docs are injected so it knows resources, actions, and usage." },
                "wait": { "type": "boolean", "description": "Wait for sub-agent to complete (default: true)" },
                "session_id": { "type": "string", "description": "Session ID" },
                "task": { "type": "string", "description": "Task description for advisor deliberation" },
                "text": { "type": "string", "description": "Text for ask prompts" },
                "options": { "type": "array", "items": { "type": "string" }, "description": "Options for select action" },
                "subtask_id": { "type": "string", "description": "Subtask ID for submit_findings" },
                "findings": { "type": "array", "items": { "type": "object", "properties": { "claim": { "type": "string" }, "source_url": { "type": "string" }, "source_ref": { "type": "string" }, "confidence": { "type": "number" }, "quote": { "type": "string" } }, "required": ["claim"] }, "description": "Array of findings from research worker" },
                "gaps": { "type": "array", "items": { "type": "string" }, "description": "Array of gaps (unanswered questions) from research worker" },
                "max_iterations": { "type": "integer", "description": "Max iterations for sub-agent (default: 100)" },
                "tasks": { "type": "array", "items": { "type": "object", "properties": { "prompt": { "type": "string" }, "description": { "type": "string" }, "tools": { "type": "array", "items": { "type": "string" } }, "plugins": { "type": "array", "items": { "type": "string" } }, "skills": { "type": "array", "items": { "type": "string" } } }, "required": ["prompt"] }, "description": "Array of tasks for spawn_parallel — each runs as a concurrent sub-agent" },
                "name": { "type": "string", "description": "Name (registry agent name, or profile update bot name)" },
                "role": { "type": "string", "description": "For profile update: the bot's role/description" }
            },
            "required": ["resource", "action"]
        })
    }

    fn requires_approval(&self) -> bool {
        false
    }

    fn is_concurrent_safe(&self, input: &serde_json::Value) -> bool {
        let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");
        let resource = input.get("resource").and_then(|v| v.as_str()).unwrap_or("");
        let resource = if resource.is_empty() {
            self.infer_resource(action)
        } else {
            resource
        };
        match resource {
            "task" => matches!(action, "list" | "status"),
            "memory" => matches!(action, "recall" | "search"),
            "session" => matches!(action, "list" | "status" | "history" | "query"),
            "context" => matches!(action, "summary"),
            "profile" => matches!(action, "get"),
            "registry" => matches!(action, "list" | "info" | "stats"),
            _ => false,
        }
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

            let mut input = input;
            let resource = {
                let corrected = crate::domain::auto_correct_resource(
                    &domain_input,
                    &mut input,
                    &["memory", "task", "session", "context", "advisors", "ask", "research", "registry", "runs", "profile"],
                );
                if corrected.is_empty() {
                    self.infer_resource(&domain_input.action).to_string()
                } else {
                    corrected
                }
            };

            if resource.is_empty() {
                return ToolResult::error(
                    "Resource is required. Available: memory, task, session, context, advisors, ask, research, registry",
                );
            }

            match resource.as_str() {
                "memory" => self.handle_memory(&input, ctx).await,
                "task" => self.handle_task(&input, ctx).await,
                "session" => self.handle_session(&input, ctx).await,
                "context" => self.handle_context(&input, ctx).await,
                "advisors" => self.handle_advisors(&input).await,
                "ask" => self.handle_ask(&input, ctx).await,
                "runs" => self.handle_runs(&input, ctx).await,
                "research" => self.handle_research(&input, ctx).await,
                "profile" => self.handle_profile(&input).await,
                "registry" => {
                    if let Some(ref persona) = self.persona {
                        persona.handle_action(&input, ctx).await
                    } else {
                        ToolResult::error("Agent registry not configured")
                    }
                }
                other => ToolResult::error(format!(
                    "Resource {:?} not available. Available: memory, task, session, context, advisors, ask, runs, research, profile, registry",
                    other
                )),
            }
        })
    }
}

/// Open a URL in the system browser (best-effort, cross-platform).
fn open_url(url: &str) {
    #[cfg(target_os = "macos")]
    let cmd = ("open", vec![url]);
    #[cfg(target_os = "linux")]
    let cmd = ("xdg-open", vec![url]);
    #[cfg(target_os = "windows")]
    let cmd = ("cmd", vec!["/C", "start", url]);
    let _ = std::process::Command::new(cmd.0).args(cmd.1).spawn();
}
