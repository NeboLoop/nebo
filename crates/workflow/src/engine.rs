use std::collections::HashMap;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use ai::{ChatRequest, StreamEventType};
use db::Store;
use tools::registry::DynTool;

use crate::parser::{Activity, Fallback, WorkflowDef};
use crate::WorkflowError;

const MAX_ITERATIONS: u32 = 50;

/// Execute a complete workflow run.
///
/// If `existing_run_id` is provided, uses that run record instead of creating a new one.
/// This avoids duplicate run records when the caller (e.g. WorkflowManager) already created one.
///
/// `cancel_token` — checked before each activity; if cancelled, returns `WorkflowError::Cancelled`.
/// `skill_content` — maps skill qualified name → SKILL.md body text, injected into activity prompts.
/// `event_bus` — if provided, an `emit` tool is injected into every activity's tool set.
/// Progress event emitted during workflow execution.
#[derive(Debug, Clone)]
pub enum WorkflowProgress {
    /// Activity-level progress (before each activity starts).
    ActivityStarted {
        activity_id: String,
        activity_index: usize,
        total_activities: usize,
    },
    /// Task-level progress (per-step within an activity).
    TaskUpdated {
        list_id: String,
        task_id: String,
        seq: i64,
        status: String,
    },
}

pub async fn execute_workflow(
    def: &WorkflowDef,
    inputs: serde_json::Value,
    trigger_type: &str,
    trigger_detail: Option<&str>,
    store: &Arc<Store>,
    provider: &dyn ai::Provider,
    resolved_tools: &[Box<dyn DynTool>],
    existing_run_id: Option<&str>,
    cancel_token: Option<&CancellationToken>,
    skill_content: Option<&HashMap<String, String>>,
    event_bus: Option<&tools::EventBus>,
    emit_source: Option<String>,
    progress_tx: Option<tokio::sync::mpsc::UnboundedSender<WorkflowProgress>>,
) -> Result<(String, String), WorkflowError> {
    let run_id = match existing_run_id {
        Some(id) => id.to_string(),
        None => {
            let id = uuid::Uuid::new_v4().to_string();
            let session_key = format!("workflow-{}-{}", def.id, id);
            store
                .create_workflow_run(
                    &id,
                    &def.id,
                    trigger_type,
                    trigger_detail,
                    Some(&inputs.to_string()),
                    Some(&session_key),
                )
                .map_err(|e| WorkflowError::Database(e.to_string()))?;
            id
        }
    };

    // Resolve emit source: prefer explicit parameter, fall back to _emit key in inputs
    let resolved_emit = emit_source.or_else(|| {
        inputs.get("_emit")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    });

    let mut total_tokens: u32 = 0;
    let mut prior_context = String::new();
    let activity_count = def.activities.len();

    // Circuit breaker: abort if 3+ consecutive activities fail with the same error pattern
    const CIRCUIT_BREAKER_THRESHOLD: u32 = 3;
    let mut consecutive_failures: u32 = 0;
    let mut last_failure_pattern: Option<String> = None;

    for (idx, activity) in def.activities.iter().enumerate() {
        let is_last = idx == activity_count - 1;
        let activity_emit = if is_last { resolved_emit.as_deref() } else { None };
        // Check for cancellation before each activity
        if let Some(token) = cancel_token {
            if token.is_cancelled() {
                return Err(WorkflowError::Cancelled);
            }
        }

        info!(
            workflow = def.id.as_str(),
            activity = activity.id.as_str(),
            "executing activity"
        );

        // Send progress event
        if let Some(ref tx) = progress_tx {
            let _ = tx.send(WorkflowProgress::ActivityStarted {
                activity_id: activity.id.clone(),
                activity_index: idx,
                total_activities: activity_count,
            });
        }

        // Update current activity
        if let Err(e) = store.update_workflow_run(
            &run_id,
            Some("running"),
            Some(&activity.id),
            None,
            None,
            None,
        ) {
            warn!(run_id = %run_id, error = %e, "failed to update workflow run status");
        }

        // All resolved tools are available to every activity
        let mut activity_tools: Vec<&Box<dyn DynTool>> = resolved_tools.iter().collect();

        // Inject emit tool if event bus is available (always available, no declaration needed)
        let emit_tool_box: Option<Box<dyn DynTool>> = event_bus
            .map(|bus| Box::new(tools::EmitTool::new(bus.clone())) as Box<dyn DynTool>);
        if let Some(ref emit) = emit_tool_box {
            activity_tools.push(emit);
        }

        // Inject exit tool — always available, every activity can stop cleanly
        let exit_tool_box: Box<dyn DynTool> = Box::new(tools::ExitTool::new());
        activity_tools.push(&exit_tool_box);

        let started_at = chrono::Utc::now().timestamp();

        match execute_activity_with_retry(
            activity,
            &prior_context,
            &inputs,
            provider,
            &activity_tools,
            skill_content,
            activity_emit,
            store,
            &run_id,
            progress_tx.as_ref(),
        )
        .await
        {
            Ok((result_text, tokens_used)) => {
                total_tokens += tokens_used;
                consecutive_failures = 0;
                last_failure_pattern = None;
                prior_context.push_str(&format!(
                    "\n[Activity '{}' result]: {}\n",
                    activity.id, result_text
                ));

                let completed_at = chrono::Utc::now().timestamp();
                if let Err(e) = store.create_activity_result(
                    &run_id,
                    &activity.id,
                    "completed",
                    tokens_used as i64,
                    1,
                    None,
                    started_at,
                    Some(completed_at),
                ) {
                    warn!(run_id = %run_id, activity = %activity.id, error = %e, "failed to record activity result");
                }
            }
            Err(WorkflowError::Exited(reason)) => {
                let completed_at = chrono::Utc::now().timestamp();
                let _ = store.create_activity_result(
                    &run_id, &activity.id, "exited", 0, 1,
                    Some(&reason), started_at, Some(completed_at),
                );
                let _ = store.complete_workflow_run(
                    &run_id, "exited", total_tokens as i64, Some(&reason), Some(&activity.id), Some(&prior_context),
                );
                info!(workflow = def.id.as_str(), run_id = %run_id, reason = %reason, "workflow exited early");
                return Ok((run_id, prior_context));
            }
            Err(e) => {
                let completed_at = chrono::Utc::now().timestamp();
                let err_msg = e.to_string();
                if let Err(db_err) = store.create_activity_result(
                    &run_id,
                    &activity.id,
                    "failed",
                    0,
                    activity.on_error.retry as i64,
                    Some(&err_msg),
                    started_at,
                    Some(completed_at),
                ) {
                    warn!(run_id = %run_id, activity = %activity.id, error = %db_err, "failed to record activity failure");
                }

                // Circuit breaker: track consecutive failures with same pattern
                let pattern = extract_error_pattern(&err_msg);
                if last_failure_pattern.as_deref() == Some(&pattern) {
                    consecutive_failures += 1;
                } else {
                    consecutive_failures = 1;
                    last_failure_pattern = Some(pattern.clone());
                }

                if consecutive_failures >= CIRCUIT_BREAKER_THRESHOLD {
                    let reason = format!(
                        "{} consecutive activities failed with same error: {}",
                        consecutive_failures, pattern
                    );
                    warn!(workflow = def.id.as_str(), run_id = %run_id, "{}", reason);
                    if let Err(db_err) = store.complete_workflow_run(
                        &run_id, "failed", total_tokens as i64,
                        Some(&reason), Some(&activity.id), None,
                    ) {
                        warn!(run_id = %run_id, error = %db_err, "failed to mark workflow run as circuit-broken");
                    }
                    return Err(WorkflowError::CircuitBreak(reason));
                }

                match activity.on_error.fallback {
                    Fallback::Skip => {
                        warn!(
                            activity = activity.id.as_str(),
                            error = %e,
                            "activity failed, skipping"
                        );
                        continue;
                    }
                    Fallback::Abort | Fallback::NotifyOwner => {
                        if let Err(db_err) = store.complete_workflow_run(
                            &run_id,
                            "failed",
                            total_tokens as i64,
                            Some(&err_msg),
                            Some(&activity.id),
                            None,
                        ) {
                            warn!(run_id = %run_id, error = %db_err, "failed to mark workflow run as failed");
                        }
                        return Err(e);
                    }
                }
            }
        }

        // Check total budget
        if def.budget.total_per_run > 0 && total_tokens > def.budget.total_per_run {
            if let Err(e) = store.complete_workflow_run(
                &run_id,
                "failed",
                total_tokens as i64,
                Some("total budget exceeded"),
                None,
                None,
            ) {
                warn!(run_id = %run_id, error = %e, "failed to mark workflow run as budget-exceeded");
            }
            return Err(WorkflowError::BudgetExceeded {
                activity_id: "workflow".into(),
                used: total_tokens,
                limit: def.budget.total_per_run,
            });
        }
    }

    if let Err(e) = store.complete_workflow_run(&run_id, "completed", total_tokens as i64, None, None, Some(&prior_context)) {
        warn!(run_id = %run_id, error = %e, "failed to mark workflow run as completed");
    }

    info!(
        workflow = def.id.as_str(),
        run_id = run_id.as_str(),
        total_tokens,
        "workflow completed"
    );

    Ok((run_id, prior_context))
}

/// Execute an activity with retry support.
async fn execute_activity_with_retry(
    activity: &Activity,
    prior_context: &str,
    inputs: &serde_json::Value,
    provider: &dyn ai::Provider,
    tools: &[&Box<dyn DynTool>],
    skill_content: Option<&HashMap<String, String>>,
    emit_source: Option<&str>,
    store: &Arc<Store>,
    run_id: &str,
    progress_tx: Option<&tokio::sync::mpsc::UnboundedSender<WorkflowProgress>>,
) -> Result<(String, u32), WorkflowError> {
    let max_attempts = activity.on_error.retry.max(1);

    for attempt in 0..max_attempts {
        match execute_activity(activity, prior_context, inputs, provider, tools, skill_content, emit_source, store, run_id, progress_tx).await {
            Ok(result) => return Ok(result),
            Err(e) if attempt + 1 < max_attempts => {
                warn!(
                    activity = activity.id.as_str(),
                    attempt = attempt + 1,
                    error = %e,
                    "activity failed, retrying"
                );
            }
            Err(e) => return Err(e),
        }
    }

    unreachable!()
}

/// Execute a single activity (lean execution path — no steering, no memory).
///
/// If the activity has steps, each step is executed as a separate LLM turn within
/// a shared conversation. Each step's input/output/tokens are tracked in `task_items`.
/// If no steps, executes as a single intent (backward-compatible).
pub async fn execute_activity(
    activity: &Activity,
    prior_context: &str,
    inputs: &serde_json::Value,
    provider: &dyn ai::Provider,
    tools: &[&Box<dyn DynTool>],
    skill_content: Option<&HashMap<String, String>>,
    emit_source: Option<&str>,
    store: &Arc<Store>,
    run_id: &str,
    progress_tx: Option<&tokio::sync::mpsc::UnboundedSender<WorkflowProgress>>,
) -> Result<(String, u32), WorkflowError> {
    // Detect if browser tool is available for this activity
    let has_browser = tools.iter().any(|t| t.name() == "web");
    let tool_names: Vec<String> = tools.iter().map(|t| t.name().to_string()).collect();

    // Build tool definitions (shared across all steps)
    let tool_defs: Vec<ai::ToolDefinition> = tools
        .iter()
        .map(|t| ai::ToolDefinition {
            name: t.name().to_string(),
            description: t.description(),
            input_schema: t.schema(),
        })
        .collect();

    // If activity has steps, execute per-step. Otherwise, single-turn legacy path.
    if activity.steps.is_empty() {
        // No steps — legacy single-turn execution
        let system = build_activity_prompt(activity, prior_context, inputs, skill_content, emit_source, has_browser, &tool_names);
        let messages = vec![ai::Message {
            role: "user".into(),
            content: activity.intent.clone(),
            ..Default::default()
        }];
        return run_llm_loop(activity, provider, tools, &tool_defs, &system, messages).await;
    }

    // --- Per-step execution ---
    let list_id = format!("run:{}:{}", run_id, activity.id);
    let step_strs: Vec<&str> = activity.steps.iter().map(|s| s.as_str()).collect();

    // Seed task_items for all steps
    let task_items = store.seed_task_list(&list_id, &step_strs)
        .map_err(|e| WorkflowError::Database(e.to_string()))?;

    // Build system prompt WITHOUT steps (they'll come as individual user messages)
    let system = build_activity_prompt_no_steps(activity, prior_context, inputs, skill_content, emit_source, has_browser, &tool_names);

    // Shared conversation — messages accumulate across steps
    let mut messages = Vec::new();
    let mut total_tokens: u32 = 0;
    let mut step_outputs: Vec<String> = Vec::new();
    let total_steps = activity.steps.len();

    for (i, step) in activity.steps.iter().enumerate() {
        let task_item = &task_items[i];
        let task_seq = task_item.seq.unwrap_or((i + 1) as i64);

        // Mark in_progress
        if let Err(e) = store.start_task_item(&task_item.id) {
            warn!(task_id = %task_item.id, error = %e, "failed to mark task_item in_progress");
        }
        if let Some(tx) = progress_tx {
            let _ = tx.send(WorkflowProgress::TaskUpdated {
                list_id: list_id.clone(),
                task_id: task_item.id.clone(),
                seq: task_seq,
                status: "in_progress".to_string(),
            });
        }

        // Send step as user message
        let step_msg = format!("Step {}/{}: {}", i + 1, total_steps, step);
        messages.push(ai::Message {
            role: "user".into(),
            content: step_msg,
            ..Default::default()
        });

        // Run LLM loop for this step
        let (step_result, step_tokens) = run_llm_loop(
            activity, provider, tools, &tool_defs, &system, messages.clone(),
        ).await.map_err(|e| {
            // Record failure
            let _ = store.update_task_item(&task_item.id, "failed", None, Some(&e.to_string()), 0, 0);
            if let Some(tx) = progress_tx {
                let _ = tx.send(WorkflowProgress::TaskUpdated {
                    list_id: list_id.clone(),
                    task_id: task_item.id.clone(),
                    seq: task_seq,
                    status: "failed".to_string(),
                });
            }
            e
        })?;

        // Append assistant response to conversation for next step's context
        messages.push(ai::Message {
            role: "assistant".into(),
            content: step_result.clone(),
            ..Default::default()
        });

        // Record completion
        total_tokens += step_tokens;
        let tokens_in = (step_tokens as i64) / 2; // approximate split
        let tokens_out = step_tokens as i64 - tokens_in;
        if let Err(e) = store.update_task_item(
            &task_item.id,
            "completed",
            Some(&step_result),
            None,
            tokens_in,
            tokens_out,
        ) {
            warn!(task_id = %task_item.id, error = %e, "failed to update task_item completed");
        }
        if let Some(tx) = progress_tx {
            let _ = tx.send(WorkflowProgress::TaskUpdated {
                list_id: list_id.clone(),
                task_id: task_item.id.clone(),
                seq: task_seq,
                status: "completed".to_string(),
            });
        }

        step_outputs.push(step_result);
    }

    // Final result is the last step's output (or concatenation if needed for prior_context)
    let final_output = step_outputs.last().cloned().unwrap_or_default();
    Ok((final_output, total_tokens))
}

/// Core LLM multi-turn loop extracted from the original execute_activity.
/// Runs until the LLM produces a response with no tool calls, then returns
/// the final text response and total tokens used.
async fn run_llm_loop(
    activity: &Activity,
    provider: &dyn ai::Provider,
    tools: &[&Box<dyn DynTool>],
    tool_defs: &[ai::ToolDefinition],
    system: &str,
    mut messages: Vec<ai::Message>,
) -> Result<(String, u32), WorkflowError> {
    let mut tokens_used: u32 = 0;
    let mut iterations: u32 = 0;
    let mut consecutive_all_not_found: u32 = 0;
    let mut last_tool_name: String = String::new();
    let mut consecutive_same_tool: u32 = 0;

    loop {
        if iterations >= MAX_ITERATIONS {
            return Err(WorkflowError::MaxIterations(activity.id.clone()));
        }
        let req = ChatRequest {
            messages: messages.clone(),
            tools: tool_defs.to_vec(),
            max_tokens: 16384,
            temperature: 0.0,
            system: system.to_string(),
            static_system: String::new(),
            model: activity.model.clone(),
            enable_thinking: false,
            metadata: None,
            cache_breakpoints: vec![],
            cancel_token: None,
        };

        let mut rx = provider
            .stream(&req)
            .await
            .map_err(|e| WorkflowError::Provider(e.to_string()))?;

        let mut response_text = String::new();
        let mut tool_calls: Vec<ai::ToolCall> = Vec::new();

        while let Some(event) = rx.recv().await {
            match event.event_type {
                StreamEventType::Text => {
                    response_text.push_str(&event.text);
                }
                StreamEventType::ToolCall => {
                    if let Some(tc) = event.tool_call {
                        tool_calls.push(tc);
                    }
                }
                StreamEventType::Error => {
                    return Err(WorkflowError::ActivityFailed(
                        activity.id.clone(),
                        event.error.unwrap_or_default(),
                    ));
                }
                StreamEventType::Done => {
                    if let Some(usage) = event.usage {
                        tokens_used += (usage.input_tokens + usage.output_tokens) as u32;
                    }
                    break;
                }
                _ => {}
            }
        }

        // If no tool calls, check if we should force-continue (min_iterations budget)
        if tool_calls.is_empty() {
            if activity.min_iterations > 0 && iterations < activity.min_iterations && !response_text.is_empty() {
                info!(
                    activity_id = %activity.id,
                    iteration = iterations,
                    min = activity.min_iterations,
                    "budget continuation: forcing next iteration"
                );
                messages.push(ai::Message {
                    role: "assistant".into(),
                    content: response_text,
                    ..Default::default()
                });
                messages.push(ai::Message {
                    role: "user".into(),
                    content: "You stopped early but your task is not complete. \
                              Keep working — use your tools to make more progress. \
                              Do not summarize or ask to continue. Take the next action.".to_string(),
                    ..Default::default()
                });
                iterations += 1;
                continue;
            }
            return Ok((response_text, tokens_used));
        }

        // Add assistant message with tool calls
        messages.push(ai::Message {
            role: "assistant".into(),
            content: response_text,
            tool_calls: Some(serde_json::to_value(&tool_calls).unwrap_or_default()),
            ..Default::default()
        });

        // Execute each tool call and collect results
        let ctx = tools::ToolContext::default();
        let mut tool_result_entries = Vec::new();
        for tc in &tool_calls {
            let tool = tools.iter().find(|t| t.name() == tc.name)
                .or_else(|| {
                    let stripped = strip_mcp_prefix(&tc.name);
                    if stripped != tc.name {
                        warn!(requested = %tc.name, resolved = %stripped, "stripped MCP prefix from tool call");
                        tools.iter().find(|t| t.name() == stripped)
                    } else {
                        None
                    }
                });
            let result = match tool {
                Some(t) => t.execute_dyn(&ctx, tc.input.clone()).await,
                None => tools::ToolResult::error(format!("tool not found: {}", tc.name)),
            };

            // Check for exit sentinel
            if !result.is_error {
                if let Some(reason) = result.content.strip_prefix(tools::EXIT_SENTINEL) {
                    return Err(WorkflowError::Exited(reason.to_string()));
                }
            }

            tool_result_entries.push(serde_json::json!({
                "tool_call_id": tc.id,
                "content": result.content,
                "is_error": result.is_error,
            }));
        }

        // Same-tool loop detection
        if let Some(first_call) = tool_calls.first() {
            if first_call.name == last_tool_name {
                consecutive_same_tool += 1;
            } else {
                last_tool_name = first_call.name.clone();
                consecutive_same_tool = 1;
            }
        }
        if consecutive_same_tool >= 3 {
            messages.push(ai::Message {
                role: "user".into(),
                content: format!(
                    "You have called '{}' {} times in a row. Take a different action \
                     or complete this activity by responding without tool calls.",
                    last_tool_name, consecutive_same_tool
                ),
                ..Default::default()
            });
        }

        // Early termination on repeated tool-not-found
        let all_not_found = tool_result_entries.iter().all(|e| {
            e.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false)
                && e.get("content")
                    .and_then(|v| v.as_str())
                    .map_or(false, |s| s.contains("tool not found"))
        });
        if all_not_found {
            consecutive_all_not_found += 1;
            if consecutive_all_not_found >= 3 {
                let names: Vec<&str> = tool_calls.iter().map(|tc| tc.name.as_str()).collect();
                return Err(WorkflowError::ActivityFailed(
                    activity.id.clone(),
                    format!("repeated tool-not-found for: {}", names.join(", ")),
                ));
            }
        } else {
            consecutive_all_not_found = 0;
        }

        messages.push(ai::Message {
            role: "tool".into(),
            content: String::new(),
            tool_results: Some(serde_json::Value::Array(tool_result_entries)),
            ..Default::default()
        });

        iterations += 1;
    }
}

/// Build the system prompt for a per-step activity (no steps section — steps come as user messages).
fn build_activity_prompt_no_steps(
    activity: &Activity,
    prior_context: &str,
    inputs: &serde_json::Value,
    skill_content: Option<&HashMap<String, String>>,
    emit_source: Option<&str>,
    has_browser: bool,
    tool_names: &[String],
) -> String {
    // Reuse the full builder but with an activity clone that has empty steps
    let mut stepless = activity.clone();
    stepless.steps = vec![];
    build_activity_prompt(&stepless, prior_context, inputs, skill_content, emit_source, has_browser, tool_names)
}

/// Build the system prompt for an activity.
///
/// Spec order: Execution Rules → Skills → Tools → Task → Steps → Inputs → Prior Results → Browser Guide
fn build_activity_prompt(
    activity: &Activity,
    prior_context: &str,
    inputs: &serde_json::Value,
    skill_content: Option<&HashMap<String, String>>,
    emit_source: Option<&str>,
    has_browser: bool,
    tool_names: &[String],
) -> String {
    let mut prompt = String::new();

    // Execution behavior rules — same action-bias as the chat agent
    prompt.push_str("## Execution Rules\n\
        You are an autonomous agent executing a workflow activity. Bias toward action:\n\
        - ZERO text when making tool calls. If you are calling a tool, output ONLY the tool call — no text.\n\
        - After a tool returns results, take the NEXT action immediately. Do not re-read data you already have.\n\
        - Do not call the same tool with identical parameters twice. If you got a result, act on it.\n\
        - When processing a collection (emails, files, records), use batch operations if available. \
          Do NOT process items one at a time when a batch call exists.\n\
        - Track your progress. Do not re-fetch the full list after every single operation.\n\
        - If something fails, diagnose why before retrying. Do not retry the identical call blindly.\n\
        - Complete the ENTIRE task. Do not stop at 10% and ask whether to continue.\n\
        - Do NOT repeat information you already told the user. Each response must contain NEW information only.\n\
        - Report the final result only. No status updates, no intermediate summaries.\n\n");

    // Skills — injected from SKILL.md content
    if let Some(skills) = skill_content {
        let activity_skills: Vec<&str> = activity
            .skills
            .iter()
            .filter_map(|name| skills.get(name.as_str()).map(|body| body.as_str()))
            .collect();
        if !activity_skills.is_empty() {
            prompt.push_str("## Skills\n");
            for body in activity_skills {
                prompt.push_str(body);
                prompt.push_str("\n\n");
            }
        }
    }

    // Available tools — explicit list prevents hallucination
    if !tool_names.is_empty() {
        prompt.push_str("## Available Tools\n");
        prompt.push_str("Your tools (case-sensitive, call ONLY these): ");
        prompt.push_str(&tool_names.join(", "));
        prompt.push_str("\nDo NOT call any tool not in this list. Do NOT prefix tool names with mcp__ or any namespace.\n\n");
    }

    // Intent
    prompt.push_str(&format!("## Task\n{}\n\n", activity.intent));

    // Steps
    if !activity.steps.is_empty() {
        prompt.push_str("## Steps\n");
        for (i, step) in activity.steps.iter().enumerate() {
            prompt.push_str(&format!("{}. {}\n", i + 1, step));
        }
        prompt.push('\n');
    }

    // Inputs (exclude _emit — it's an operational key, not a user input)
    if let serde_json::Value::Object(map) = inputs {
        let user_inputs: Vec<_> = map.iter()
            .filter(|(k, _)| !k.starts_with('_'))
            .collect();
        if !user_inputs.is_empty() {
            prompt.push_str("## Inputs\n");
            for (key, val) in user_inputs {
                prompt.push_str(&format!("- {}: {}\n", key, val));
            }
            prompt.push('\n');
        }
    }

    // Prior activity context
    if !prior_context.is_empty() {
        prompt.push_str("## Prior Results\n");
        prompt.push_str(prior_context);
        prompt.push('\n');
    }

    // Command hints — only mention tools the step actually declares.
    // If emit_source is set (Path B will handle it specifically), skip the
    // generic emit hint to avoid redundant/conflicting instructions.
    let effective_cmds: Vec<&str> = activity.cmds.iter()
        .filter(|cmd| !(cmd.as_str() == "emit" && emit_source.is_some()))
        .map(|s| s.as_str())
        .collect();

    if !effective_cmds.is_empty() {
        prompt.push_str("\n## Workflow Controls\n");
        prompt.push_str("You have access to these workflow control tools:\n");
        for cmd in &effective_cmds {
            match *cmd {
                "exit" => prompt.push_str(
                    "- exit(reason: \"...\") — call this to stop the workflow early if \
                     the condition in your task is not met or there is nothing to do.\n"
                ),
                "emit" => prompt.push_str(
                    "- emit(source: \"...\", payload: {...}) — call this to announce \
                     your result to other workflows. Can be called multiple times, \
                     once per item, if processing a collection.\n"
                ),
                _ => {}
            }
        }
        prompt.push('\n');
    }

    // Browser automation guide — injected when web tool is available
    if has_browser {
        prompt.push_str("\n## Browser Automation Guide\n\
            - Always call read_page FIRST before any click, fill, or navigate action.\n\
            - Use element refs from the read_page output for click/fill/select — never guess selectors.\n\
            - After navigate, wait briefly then read_page to see the new content.\n\
            - For forms: click the field first, then type/fill the value.\n\
            - If you cannot find an element, scroll down and read_page again.\n\
            - Do NOT open new_tab unless you need multiple pages simultaneously.\n\
            - Verify results with a final read_page after completing actions.\n\n");
    }

    // Emit instruction — injected into last activity only when declared
    if let Some(source) = emit_source {
        prompt.push_str(&format!(
            "\n## Output\nWhen you have completed your work, you MUST call the emit tool with:\n- source: \"{}\"\n- payload: your actual output or result (not a summary of what you did — the content itself)\n\nDo not say \"done\" or \"completed\". Call emit with the real output.\n",
            source
        ));
    }

    prompt
}

/// Extract a normalized error pattern for circuit breaker comparison.
///
/// Takes the first segment before `:`, lowercased, max 60 chars.
fn extract_error_pattern(err: &str) -> String {
    let seg = err.split(':').next().unwrap_or(err);
    let pattern = seg.trim().to_lowercase();
    if pattern.len() > 60 {
        let mut end = 60;
        while !pattern.is_char_boundary(end) { end -= 1; }
        pattern[..end].to_string()
    } else {
        pattern
    }
}

/// Strip MCP namespace prefix from tool names.
/// `mcp__{server}__{tool}` → `{tool}`
/// e.g. "mcp__nebo-agent__plugin" → "plugin"
fn strip_mcp_prefix(name: &str) -> &str {
    if !name.starts_with("mcp__") {
        return name;
    }
    let parts: Vec<&str> = name.splitn(3, "__").collect();
    if parts.len() == 3 {
        parts[2]
    } else {
        name
    }
}

#[cfg(test)]
mod engine_tests {
    use super::*;

    #[test]
    fn test_strip_mcp_prefix() {
        assert_eq!(strip_mcp_prefix("plugin"), "plugin");
        assert_eq!(strip_mcp_prefix("os"), "os");
        assert_eq!(strip_mcp_prefix("mcp__nebo-agent__plugin"), "plugin");
        assert_eq!(strip_mcp_prefix("mcp__nebo-agent__os"), "os");
        assert_eq!(strip_mcp_prefix("mcp__monument_sh__project"), "project");
        assert_eq!(strip_mcp_prefix("mcp__only_one"), "mcp__only_one");
    }
}
