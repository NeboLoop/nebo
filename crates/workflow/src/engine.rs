use std::collections::HashMap;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use ai::{ChatRequest, StreamEventType};
use db::Store;
use tools::registry::DynTool;

use crate::WorkflowError;
use crate::parser::{Activity, WorkflowDef};

const MAX_ITERATIONS: u32 = 50;

/// Decision from the step evaluator (orchestrator between steps).
#[derive(Debug)]
enum EvalDecision {
    Proceed,
    Exit(String),
}

fn parse_eval_response(content: &str) -> EvalDecision {
    let trimmed = content.trim();
    if let Some(reason) = trimmed.strip_prefix("exit:") {
        EvalDecision::Exit(reason.trim().to_string())
    } else {
        EvalDecision::Proceed
    }
}

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

#[allow(unused_assignments)] // circuit breaker state is future-proofed for Fallback::Skip
pub async fn execute_workflow(
    def: &WorkflowDef,
    agent_id: &str,
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
        inputs
            .get("_emit")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    });

    // Explicit connections → deterministic graph execution (forks parallel,
    // joins barriered, condition/loop routing engine-evaluated). No
    // connections → the sequential array-order path below, unchanged.
    if !def.connections.is_empty() {
        return crate::graph::execute_graph(
            def,
            agent_id,
            &inputs,
            store,
            provider,
            resolved_tools,
            &run_id,
            cancel_token,
            skill_content,
            event_bus,
            resolved_emit,
            progress_tx,
        )
        .await;
    }

    let mut total_tokens: u32 = 0;
    let mut prior_context = String::new();
    let activity_count = def.activities.len();

    // Circuit breaker: abort if 3+ consecutive activities fail with the same error pattern
    const CIRCUIT_BREAKER_THRESHOLD: u32 = 3;
    let mut consecutive_failures: u32 = 0;
    let mut last_failure_pattern: Option<String> = None;

    for (idx, activity) in def.activities.iter().enumerate() {
        let is_last = idx == activity_count - 1;
        let activity_emit = if is_last {
            resolved_emit.as_deref()
        } else {
            None
        };
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
        let emit_tool_box: Option<Box<dyn DynTool>> =
            event_bus.map(|bus| Box::new(tools::EmitTool::new(bus.clone())) as Box<dyn DynTool>);
        if let Some(ref emit) = emit_tool_box {
            activity_tools.push(emit);
        }

        // Inject exit tool — always available, every activity can stop cleanly
        let exit_tool_box: Box<dyn DynTool> = Box::new(tools::ExitTool::new());
        activity_tools.push(&exit_tool_box);

        let started_at = chrono::Utc::now().timestamp();

        // Accumulates every token this activity consumes — successful turns,
        // evaluator turns, failed retry attempts, and exit-path turns.
        let mut activity_spent: u32 = 0;

        match execute_activity_with_retry(
            activity,
            &prior_context,
            &inputs,
            provider,
            &activity_tools,
            skill_content,
            activity_emit,
            store,
            agent_id,
            &run_id,
            &def.id,
            progress_tx.as_ref(),
            &mut activity_spent,
        )
        .await
        {
            Ok((result_text, _tokens_used)) => {
                total_tokens += activity_spent;
                consecutive_failures = 0;
                last_failure_pattern = None;

                let completed_at = chrono::Utc::now().timestamp();
                if let Err(e) = store.create_activity_result(
                    &run_id,
                    &activity.id,
                    "completed",
                    activity_spent as i64,
                    1,
                    None,
                    started_at,
                    Some(completed_at),
                ) {
                    warn!(run_id = %run_id, activity = %activity.id, error = %e, "failed to record activity result");
                }

                // n8n-style branch termination: empty output = no downstream execution.
                // If the activity produced no output (even after tool-result synthesis),
                // there is nothing to pass to the next activity — stop the branch.
                if result_text.trim().is_empty() {
                    info!(
                        workflow = def.id.as_str(),
                        activity = activity.id.as_str(),
                        run_id = %run_id,
                        "activity produced no output, terminating branch"
                    );
                    let _ = store.complete_workflow_run(
                        &run_id,
                        "completed",
                        total_tokens as i64,
                        None,
                        Some(&activity.id),
                        Some(&prior_context),
                    );
                    return Ok((run_id, prior_context));
                }

                prior_context.push_str(&format!(
                    "\n[Activity '{}' result]: {}\n",
                    activity.id, result_text
                ));
            }
            Err(WorkflowError::Exited(reason)) => {
                total_tokens += activity_spent;
                let completed_at = chrono::Utc::now().timestamp();
                let _ = store.create_activity_result(
                    &run_id,
                    &activity.id,
                    "exited",
                    activity_spent as i64,
                    1,
                    Some(&reason),
                    started_at,
                    Some(completed_at),
                );
                let _ = store.complete_workflow_run(
                    &run_id,
                    "exited",
                    total_tokens as i64,
                    Some(&reason),
                    Some(&activity.id),
                    Some(&prior_context),
                );
                info!(workflow = def.id.as_str(), run_id = %run_id, reason = %reason, "workflow exited early");
                return Ok((run_id, prior_context));
            }
            Err(e) => {
                total_tokens += activity_spent;
                let completed_at = chrono::Utc::now().timestamp();
                let err_msg = e.to_string();
                if let Err(db_err) = store.create_activity_result(
                    &run_id,
                    &activity.id,
                    "failed",
                    activity_spent as i64,
                    activity.on_error.retry as i64,
                    Some(&err_msg),
                    started_at,
                    Some(completed_at),
                ) {
                    warn!(run_id = %run_id, activity = %activity.id, error = %db_err, "failed to record activity failure");
                }

                // Circuit breaker: track consecutive failures with same pattern.
                // Note: currently dead (abort-on-error policy returns below),
                // but wired for future Fallback::Skip support.
                let pattern = extract_error_pattern(&err_msg);
                if last_failure_pattern.as_deref() == Some(pattern.as_str()) {
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
                        &run_id,
                        "failed",
                        total_tokens as i64,
                        Some(&reason),
                        Some(&activity.id),
                        None,
                    ) {
                        warn!(run_id = %run_id, error = %db_err, "failed to mark workflow run as circuit-broken");
                    }
                    return Err(WorkflowError::CircuitBreak(reason));
                }

                // Always abort: downstream activities depend on prior results,
                // so continuing after a failure produces garbage.
                // Fallback::Skip is kept for future use (independent activities)
                // but currently behaves the same as Abort.
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

    if let Err(e) = store.complete_workflow_run(
        &run_id,
        "completed",
        total_tokens as i64,
        None,
        None,
        Some(&prior_context),
    ) {
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
///
/// `spent` accumulates tokens across ALL attempts (failed retries included) —
/// callers use it for run totals; the Ok tuple's count covers only the
/// successful attempt.
pub(crate) async fn execute_activity_with_retry(
    activity: &Activity,
    prior_context: &str,
    inputs: &serde_json::Value,
    provider: &dyn ai::Provider,
    tools: &[&Box<dyn DynTool>],
    skill_content: Option<&HashMap<String, String>>,
    emit_source: Option<&str>,
    store: &Arc<Store>,
    agent_id: &str,
    run_id: &str,
    workflow_id: &str,
    progress_tx: Option<&tokio::sync::mpsc::UnboundedSender<WorkflowProgress>>,
    spent: &mut u32,
) -> Result<(String, u32), WorkflowError> {
    let max_attempts = activity.on_error.retry.max(1);

    for attempt in 0..max_attempts {
        match execute_activity(
            activity,
            prior_context,
            inputs,
            provider,
            tools,
            skill_content,
            emit_source,
            store,
            agent_id,
            run_id,
            workflow_id,
            progress_tx,
            spent,
        )
        .await
        {
            Ok(result) => return Ok(result),
            // Deliberate stops are not failures — retrying would re-run the
            // activity's tool side effects from scratch.
            Err(e @ (WorkflowError::Exited(_) | WorkflowError::Cancelled)) => return Err(e),
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
    agent_id: &str,
    run_id: &str,
    workflow_id: &str,
    progress_tx: Option<&tokio::sync::mpsc::UnboundedSender<WorkflowProgress>>,
    spent: &mut u32,
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

    // Trace builder — links every LLM call to this agent/run/workflow/action/step
    // so Janus can attribute usage per agent and per workflow. agent_id is "" for
    // standalone (non-agent-bound) workflow runs. step_id is the step index ("" when
    // the activity has no steps).
    let make_trace = |step_id: String| ai::RequestTrace {
        agent_id: agent_id.to_string(),
        run_id: run_id.to_string(),
        workflow_id: workflow_id.to_string(),
        action_id: activity.id.clone(),
        step_id,
    };

    // If activity has steps, execute per-step. Otherwise, single-turn legacy path.
    if activity.steps.is_empty() {
        // No steps — legacy single-turn execution
        let system = build_activity_prompt(
            activity,
            prior_context,
            inputs,
            skill_content,
            emit_source,
            has_browser,
            &tool_names,
        );
        let messages = vec![ai::Message {
            role: "user".into(),
            // Typed nodes may have no intent — the system prompt carries the
            // type contract and parameters; providers reject empty messages.
            content: if activity.intent.trim().is_empty() {
                "Execute this activity as defined by its type and parameters.".to_string()
            } else {
                activity.intent.clone()
            },
            ..Default::default()
        }];
        return run_llm_loop(activity, provider, tools, &tool_defs, &system, messages, spent, make_trace(String::new())).await;
    }

    // --- Per-step execution ---
    let list_id = format!("run:{}:{}", run_id, activity.id);
    let step_strs: Vec<&str> = activity.steps.iter().map(|s| s.as_str()).collect();

    // Seed task_items for all steps
    let task_items = store
        .seed_task_list(&list_id, &step_strs)
        .map_err(|e| WorkflowError::Database(e.to_string()))?;

    // Build system prompt WITHOUT steps (they'll come as individual user messages)
    let system = build_activity_prompt_no_steps(
        activity,
        prior_context,
        inputs,
        skill_content,
        emit_source,
        has_browser,
        &tool_names,
    );

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
            activity,
            provider,
            tools,
            &tool_defs,
            &system,
            messages.clone(),
            spent,
            make_trace(i.to_string()),
        )
        .await
        .map_err(|e| {
            // Exit-by-design (exit tool) is a clean stop, not a step failure —
            // recording it as failed painted successful exited runs red in the UI.
            let status = if matches!(e, WorkflowError::Exited(_)) {
                "exited"
            } else {
                "failed"
            };
            let _ =
                store.update_task_item(&task_item.id, status, None, Some(&e.to_string()), 0, 0);
            if let Some(tx) = progress_tx {
                let _ = tx.send(WorkflowProgress::TaskUpdated {
                    list_id: list_id.clone(),
                    task_id: task_item.id.clone(),
                    seq: task_seq,
                    status: status.to_string(),
                });
            }
            e
        })?;

        // --- Orchestrator evaluation (its tokens count too) ---
        let (eval, eval_tokens) = evaluate_step(
            provider,
            &system,
            step,
            &step_result,
            i,
            total_steps,
            make_trace(i.to_string()),
        )
        .await?;
        *spent += eval_tokens;

        match eval {
            EvalDecision::Proceed => {
                // Normal flow: append result, continue to next step
                messages.push(ai::Message {
                    role: "assistant".into(),
                    content: step_result.clone(),
                    ..Default::default()
                });
            }
            EvalDecision::Exit(reason) => {
                // Record step as completed (it did produce output), then exit
                let tokens_in = (step_tokens as i64) / 2;
                let tokens_out = step_tokens as i64 - tokens_in;
                let _ = store.update_task_item(
                    &task_item.id,
                    "completed",
                    Some(&step_result),
                    None,
                    tokens_in,
                    tokens_out,
                );
                if let Some(tx) = progress_tx {
                    let _ = tx.send(WorkflowProgress::TaskUpdated {
                        list_id: list_id.clone(),
                        task_id: task_item.id.clone(),
                        seq: task_seq,
                        status: "completed".to_string(),
                    });
                }
                info!(
                    activity = %activity.id,
                    step = i,
                    reason = %reason,
                    "orchestrator exited workflow at step"
                );
                return Err(WorkflowError::Exited(
                    format!("Step {}/{} evaluator: {}", i + 1, total_steps, reason),
                ));
            }
        }

        // Record completion
        total_tokens += step_tokens;

        // Cumulative per-activity budget across steps + evaluator turns.
        if activity.token_budget.max > 0 && *spent > activity.token_budget.max {
            return Err(WorkflowError::BudgetExceeded {
                activity_id: activity.id.clone(),
                used: *spent,
                limit: activity.token_budget.max,
            });
        }
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

/// Evaluate a step's output using the same provider (prompt-cached system prompt).
/// Returns Proceed or Exit plus the evaluator's own token usage (previously
/// uncounted — every step paid an invisible evaluation turn).
/// Fails open (Proceed) on any error.
async fn evaluate_step(
    provider: &dyn ai::Provider,
    system: &str,
    step_text: &str,
    step_output: &str,
    step_index: usize,
    total_steps: usize,
    trace: ai::RequestTrace,
) -> Result<(EvalDecision, u32), WorkflowError> {
    let eval_system = format!(
        "{}\n\n## Step Evaluation Mode\n\
         You are evaluating the output of Step {}/{}: \"{}\"\n\n\
         Based on the workflow context above and the step output below, respond with EXACTLY ONE of:\n\
         - proceed — step completed its stated goal, continue to the next step\n\
         - exit:<reason> — the task is inapplicable, the data doesn't match expectations, \
           or continuing would be wasteful or harmful\n\n\
         Respond with ONLY the decision. Nothing else.",
        system, step_index + 1, total_steps, step_text,
    );

    let truncated_output = truncate_at_char_boundary(step_output, 2000);

    let messages = vec![ai::Message {
        role: "user".into(),
        content: format!("Step output:\n\n{}", truncated_output),
        ..Default::default()
    }];

    let req = ChatRequest {
        tool_choice: Default::default(),
        messages,
        tools: vec![],
        max_tokens: 100,
        temperature: 0.0,
        system: eval_system,
        static_system: String::new(),
        model: String::new(),
        enable_thinking: false,
        metadata: None,
        cache_breakpoints: vec![],
        cancel_token: None,
        trace: Some(trace),
    };

    let mut rx = provider
        .stream(&req)
        .await
        .map_err(|e| WorkflowError::Provider(e.to_string()))?;

    let mut response_text = String::new();
    let mut eval_tokens: u32 = 0;
    while let Some(event) = rx.recv().await {
        match event.event_type {
            StreamEventType::Text => response_text.push_str(&event.text),
            StreamEventType::Error => {
                warn!("step evaluator error: {:?}", event.error);
                return Ok((EvalDecision::Proceed, eval_tokens));
            }
            StreamEventType::Done => {
                if let Some(usage) = event.usage {
                    eval_tokens = (usage.input_tokens + usage.output_tokens) as u32;
                }
                break;
            }
            _ => {}
        }
    }

    Ok((parse_eval_response(&response_text), eval_tokens))
}

/// Core LLM multi-turn loop extracted from the original execute_activity.
/// Runs until the LLM produces a response with no tool calls, then returns
/// the final text response and total tokens used.
///
/// `spent` accumulates EVERY token consumed, including turns that later end
/// in an error — error variants can't carry token counts, so callers read
/// the accumulator to keep run totals truthful across exits/failures/retries.
async fn run_llm_loop(
    activity: &Activity,
    provider: &dyn ai::Provider,
    tools: &[&Box<dyn DynTool>],
    tool_defs: &[ai::ToolDefinition],
    system: &str,
    mut messages: Vec<ai::Message>,
    spent: &mut u32,
    trace: ai::RequestTrace,
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
            tool_choice: Default::default(),
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
            trace: Some(trace.clone()),
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
                        let turn = (usage.input_tokens + usage.output_tokens) as u32;
                        tokens_used += turn;
                        *spent += turn;
                    }
                    break;
                }
                _ => {}
            }
        }

        // Per-activity token budget — enforced DURING the loop, not after the
        // activity finishes. A runaway activity stops at its own ceiling
        // instead of spending unboundedly until the workflow-total check.
        if activity.token_budget.max > 0 && tokens_used > activity.token_budget.max {
            return Err(WorkflowError::BudgetExceeded {
                activity_id: activity.id.clone(),
                used: tokens_used,
                limit: activity.token_budget.max,
            });
        }

        // If no tool calls, check if we should force-continue (min_iterations budget)
        if tool_calls.is_empty() {
            if activity.min_iterations > 0
                && iterations < activity.min_iterations
                && !response_text.is_empty()
            {
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
                              Do not summarize or ask to continue. Take the next action."
                        .to_string(),
                    ..Default::default()
                });
                iterations += 1;
                continue;
            }
            // If the LLM produced no text but tool calls were made,
            // synthesize output from tool results so downstream steps/activities
            // get context (n8n-style: empty output = branch termination).
            if response_text.is_empty() && iterations > 0 {
                response_text = synthesize_from_tool_results(&messages);
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

        // Execute each tool call and collect results. Workflow activities are unattended —
        // mark the origin so the ask tool (and any HITL-gated capability) is unavailable;
        // the engine never blocks on a UI prompt.
        let ctx = tools::ToolContext::new(tools::Origin::Workflow);
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

        // Same-tool loop nudge — MUST come after the tool-results message:
        // providers reject a user message wedged between tool_use and
        // tool_result, which would 400 exactly when the model is stuck.
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

        iterations += 1;
    }
}

/// When the LLM completes via tool calls without a final text response,
/// extract the last tool result contents as the step output.
fn synthesize_from_tool_results(messages: &[ai::Message]) -> String {
    for msg in messages.iter().rev() {
        if msg.role == "tool" {
            if let Some(serde_json::Value::Array(results)) = &msg.tool_results {
                let parts: Vec<&str> = results
                    .iter()
                    .filter_map(|entry| {
                        let is_err = entry
                            .get("is_error")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        if is_err {
                            return None;
                        }
                        entry.get("content").and_then(|v| v.as_str()).filter(|s| !s.is_empty())
                    })
                    .collect();
                if !parts.is_empty() {
                    let joined = parts.join("\n---\n");
                    const MAX_LEN: usize = 4000;
                    if joined.len() > MAX_LEN {
                        return format!("{}...", truncate_at_char_boundary(&joined, MAX_LEN));
                    }
                    return joined;
                }
            }
        }
    }
    String::new()
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
    let mut prompt = build_activity_prompt(
        &stepless,
        prior_context,
        inputs,
        skill_content,
        emit_source,
        has_browser,
        tool_names,
    );

    prompt.push_str("\n## Step Execution Mode\n\
        You will receive instructions one step at a time. You are running autonomously.\n\
        - Execute ONLY what the current step asks. Nothing more.\n\
        - Do NOT ask questions or present options. There is no human to answer.\n\
        - If the task is inapplicable or the data doesn't match, use the exit tool.\n\
        - When done, provide a brief summary of findings/actions and stop.\n\n");

    prompt
}

/// How each LLM-driven activity type operates. Deterministic types
/// (condition/loop/wait/http) never reach the LLM — the engine executes them.
fn typed_node_preamble(activity_type: &str) -> Option<&'static str> {
    match activity_type {
        "research" => Some(
            "This is a research activity: gather information per the parameters \
             (depth, sources) using web/search tools. Summarize findings with sources.",
        ),
        "email" => Some(
            "This is an email activity: compose and send using the messaging tools. \
             Recipient/subject parameters are authoritative; template placeholders like \
             {{topic}} resolve from inputs and prior results.",
        ),
        "notify" => Some(
            "This is a notification activity: deliver one concise notification to the \
             owner via the message tool. No follow-up actions.",
        ),
        "code" => Some(
            "This is a code activity: write and run code in the configured language \
             using the os tool. Return the program's output as your summary.",
        ),
        "transform" => Some(
            "This is a data-transform activity: reshape the prior results/inputs as the \
             parameters describe. Output ONLY the transformed data — no commentary.",
        ),
        "agent" => Some(
            "This is a delegation activity: delegate the task to the agent named in the \
             parameters via agent(resource: \"registry\", action: \"delegate\", ...) and \
             relay its result.",
        ),
        "connector" => Some(
            "This is an MCP connector activity: call the configured server's tool \
             (parameters name the server, tool, and input) via the mcp tool and report \
             the result.",
        ),
        _ => None,
    }
}

/// Build the system prompt for an activity.
///
/// Spec order: Execution Rules → Skills → Tools → Type/Params → Task → Steps → Inputs → Prior Results → Browser Guide
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
        - Report the final result only. No status updates, no intermediate summaries.\n\
        - If a prior step already resolved the task (e.g., 'no meeting found', 'not applicable', \
          'nothing to do'), call the exit tool immediately instead of repeating the same conclusion. \
          Do not waste steps re-analyzing data you already evaluated.\n\
        - After completing all tool calls for a step, always end with a brief text summary of what \
          you found or did. Never end a step with zero text output — downstream activities depend \
          on your summary.\n\n");

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

    // Typed-node contract: the type's preamble tells the model HOW this
    // activity kind operates; params are the authoritative configuration.
    // Routing stays with the engine — these only shape the work inside the node.
    if let Some(preamble) = typed_node_preamble(&activity.activity_type) {
        prompt.push_str(&format!(
            "## Activity Type: {}\n{}\n\n",
            activity.activity_type, preamble
        ));
    }
    if let Some(params) = &activity.params {
        if params.as_object().is_some_and(|o| !o.is_empty()) {
            prompt.push_str(&format!(
                "## Parameters\nConfigured parameters for this activity — treat them as authoritative:\n```json\n{}\n```\n\n",
                serde_json::to_string_pretty(params).unwrap_or_else(|_| params.to_string())
            ));
        }
    }

    // Intent — typed nodes may have none; the type + parameters ARE the task.
    if activity.intent.trim().is_empty() {
        prompt.push_str(
            "## Task\nExecute this activity as defined by its type and parameters above.\n\n",
        );
    } else {
        prompt.push_str(&format!("## Task\n{}\n\n", activity.intent));
    }

    // Steps
    if !activity.steps.is_empty() {
        prompt.push_str("## Steps\n");
        for (i, step) in activity.steps.iter().enumerate() {
            prompt.push_str(&format!("{}. {}\n", i + 1, step));
        }
        prompt.push('\n');
    }

    // Inputs — include event payload fields, exclude only internal operational keys
    if let serde_json::Value::Object(map) = inputs {
        let skip_keys = ["_emit"];
        let user_inputs: Vec<_> = map
            .iter()
            .filter(|(k, _)| !skip_keys.contains(&k.as_str()))
            .collect();
        if !user_inputs.is_empty() {
            prompt.push_str("## Inputs\n");
            for (key, val) in &user_inputs {
                let formatted = format_input_value(val);
                prompt.push_str(&format!("### {}\n{}\n\n", key, formatted));
            }
        }
    }

    // Prior activity context
    if !prior_context.is_empty() {
        prompt.push_str("## Prior Results\n");
        prompt.push_str(prior_context);
        prompt.push('\n');
    }

    // Workflow controls — exit is always available (injected at engine level).
    // Emit is opt-in via cmds declaration.
    prompt.push_str("\n## Workflow Controls\n");
    prompt.push_str("You have access to these workflow control tools:\n");
    prompt.push_str(
        "- exit(reason: \"...\") — call this to stop the workflow early if \
         the condition in your task is not met or there is nothing to do.\n",
    );
    let has_emit_cmd = activity.cmds.iter().any(|c| c == "emit");
    if has_emit_cmd && emit_source.is_none() {
        prompt.push_str(
            "- emit(source: \"...\", payload: {...}) — call this to announce \
             your result to other workflows. Can be called multiple times, \
             once per item, if processing a collection.\n",
        );
    }
    prompt.push('\n');

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

/// Format an input value for the activity prompt.
///
/// Scalar values are printed inline. JSON objects are smart-formatted: scalar
/// fields first (always visible), then nested objects/arrays truncated if large.
/// This ensures key data like `snippet`, `id`, `from` is never buried under
/// massive nested structures (e.g. raw Gmail API responses with MIME/DKIM noise).
const INPUT_VALUE_MAX_CHARS: usize = 4_000;

fn format_input_value(val: &serde_json::Value) -> String {
    match val {
        serde_json::Value::String(s) => {
            if s.len() <= INPUT_VALUE_MAX_CHARS {
                s.clone()
            } else {
                format!(
                    "{}\n\n... (truncated — {} total chars)",
                    truncate_at_char_boundary(s, INPUT_VALUE_MAX_CHARS),
                    s.len()
                )
            }
        }
        serde_json::Value::Object(map) => {
            // Separate scalars from nested structures so key fields are always visible
            let mut scalars = serde_json::Map::new();
            let mut nested = serde_json::Map::new();
            for (k, v) in map {
                match v {
                    serde_json::Value::Object(_) | serde_json::Value::Array(_) => {
                        nested.insert(k.clone(), v.clone());
                    }
                    _ => {
                        scalars.insert(k.clone(), v.clone());
                    }
                }
            }
            // Build: scalars always shown, nested truncated
            let mut result = String::new();
            if !scalars.is_empty() {
                let scalar_obj = serde_json::Value::Object(scalars);
                let pretty = serde_json::to_string_pretty(&scalar_obj)
                    .unwrap_or_else(|_| scalar_obj.to_string());
                result.push_str("```json\n");
                result.push_str(&pretty);
                result.push_str("\n```\n");
            }
            if !nested.is_empty() {
                let nested_obj = serde_json::Value::Object(nested);
                let pretty = serde_json::to_string_pretty(&nested_obj)
                    .unwrap_or_else(|_| nested_obj.to_string());
                if pretty.len() <= INPUT_VALUE_MAX_CHARS {
                    result.push_str("```json\n");
                    result.push_str(&pretty);
                    result.push_str("\n```");
                } else {
                    result.push_str("```json\n");
                    result.push_str(truncate_at_char_boundary(&pretty, INPUT_VALUE_MAX_CHARS));
                    result.push_str("\n```\n");
                    result.push_str(&format!(
                        "... (nested data truncated — {} total chars)",
                        pretty.len()
                    ));
                }
            }
            result
        }
        serde_json::Value::Array(_) => {
            let pretty =
                serde_json::to_string_pretty(val).unwrap_or_else(|_| val.to_string());
            if pretty.len() <= INPUT_VALUE_MAX_CHARS {
                format!("```json\n{}\n```", pretty)
            } else {
                format!(
                    "```json\n{}\n```\n... (truncated — {} total chars)",
                    truncate_at_char_boundary(&pretty, INPUT_VALUE_MAX_CHARS),
                    pretty.len()
                )
            }
        }
        other => other.to_string(),
    }
}

/// Truncate a string at a byte limit without splitting a UTF-8 character.
/// Direct byte slicing (`&s[..n]`) panics when `n` lands inside a multibyte
/// character — tool output routinely contains emoji and non-ASCII text, and
/// a panic here kills the run task, leaving the run stuck in `running`.
fn truncate_at_char_boundary(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Extract a normalized error pattern for circuit breaker comparison.
///
/// Takes the first segment before `:`, lowercased, max 60 chars.
fn extract_error_pattern(err: &str) -> String {
    let seg = err.split(':').next().unwrap_or(err);
    let pattern = seg.trim().to_lowercase();
    if pattern.len() > 60 {
        let mut end = 60;
        while !pattern.is_char_boundary(end) {
            end -= 1;
        }
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
    if parts.len() == 3 { parts[2] } else { name }
}

#[cfg(test)]
mod engine_tests {
    use super::*;

    #[test]
    fn test_parse_eval_response() {
        match parse_eval_response("proceed") {
            EvalDecision::Proceed => {}
            other => panic!("expected Proceed, got {:?}", other),
        }
        match parse_eval_response("  proceed\n") {
            EvalDecision::Proceed => {}
            other => panic!("expected Proceed, got {:?}", other),
        }
        match parse_eval_response("exit:SENT email, task inapplicable") {
            EvalDecision::Exit(reason) => assert_eq!(reason, "SENT email, task inapplicable"),
            other => panic!("expected Exit, got {:?}", other),
        }
        match parse_eval_response("  exit: nothing to do  ") {
            EvalDecision::Exit(reason) => assert_eq!(reason, "nothing to do"),
            other => panic!("expected Exit, got {:?}", other),
        }
        // Unknown responses default to Proceed (fail-open)
        match parse_eval_response("maybe continue?") {
            EvalDecision::Proceed => {}
            other => panic!("expected Proceed, got {:?}", other),
        }
    }

    #[test]
    fn test_typed_node_prompt_injection() {
        let activity: Activity = serde_json::from_value(serde_json::json!({
            "id": "send-summary",
            "type": "email",
            "params": { "to": "owner@example.com", "subject": "Daily {{topic}}" }
        }))
        .unwrap();
        let prompt = build_activity_prompt(
            &activity,
            "",
            &serde_json::json!({}),
            None,
            None,
            false,
            &[],
        );
        assert!(prompt.contains("## Activity Type: email"));
        assert!(prompt.contains("## Parameters"));
        assert!(prompt.contains("owner@example.com"));
        // Empty intent gets the deterministic fallback task line.
        assert!(prompt.contains("Execute this activity as defined by its type and parameters"));

        // Plain activities are unchanged: no type/params sections.
        let plain: Activity = serde_json::from_value(serde_json::json!({
            "id": "a", "intent": "Do the thing"
        }))
        .unwrap();
        let prompt = build_activity_prompt(
            &plain,
            "",
            &serde_json::json!({}),
            None,
            None,
            false,
            &[],
        );
        assert!(!prompt.contains("## Activity Type"));
        assert!(!prompt.contains("## Parameters"));
        assert!(prompt.contains("## Task\nDo the thing"));
    }

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
