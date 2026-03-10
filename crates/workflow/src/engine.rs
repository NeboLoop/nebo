use std::collections::HashMap;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use ai::{ChatRequest, StreamEventType};
use db::Store;
use tools::registry::DynTool;

use crate::parser::{Activity, Fallback, WorkflowDef};
use crate::WorkflowError;

const MAX_ITERATIONS: u32 = 20;

/// Execute a complete workflow run.
///
/// If `existing_run_id` is provided, uses that run record instead of creating a new one.
/// This avoids duplicate run records when the caller (e.g. WorkflowManager) already created one.
///
/// `cancel_token` — checked before each activity; if cancelled, returns `WorkflowError::Cancelled`.
/// `skill_content` — maps skill qualified name → SKILL.md body text, injected into activity prompts.
/// `event_bus` — if provided, an `emit` tool is injected into every activity's tool set.
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
) -> Result<String, WorkflowError> {
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

    let mut total_tokens: u32 = 0;
    let mut prior_context = String::new();

    for activity in &def.activities {
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

        let started_at = chrono::Utc::now().timestamp();

        match execute_activity_with_retry(
            activity,
            &prior_context,
            &inputs,
            provider,
            &activity_tools,
            skill_content,
        )
        .await
        {
            Ok((result_text, tokens_used)) => {
                total_tokens += tokens_used;
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

    if let Err(e) = store.complete_workflow_run(&run_id, "completed", total_tokens as i64, None, None) {
        warn!(run_id = %run_id, error = %e, "failed to mark workflow run as completed");
    }

    info!(
        workflow = def.id.as_str(),
        run_id = run_id.as_str(),
        total_tokens,
        "workflow completed"
    );

    Ok(run_id)
}

/// Execute an activity with retry support.
async fn execute_activity_with_retry(
    activity: &Activity,
    prior_context: &str,
    inputs: &serde_json::Value,
    provider: &dyn ai::Provider,
    tools: &[&Box<dyn DynTool>],
    skill_content: Option<&HashMap<String, String>>,
) -> Result<(String, u32), WorkflowError> {
    let max_attempts = activity.on_error.retry.max(1);

    for attempt in 0..max_attempts {
        match execute_activity(activity, prior_context, inputs, provider, tools, skill_content).await {
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
pub async fn execute_activity(
    activity: &Activity,
    prior_context: &str,
    inputs: &serde_json::Value,
    provider: &dyn ai::Provider,
    tools: &[&Box<dyn DynTool>],
    skill_content: Option<&HashMap<String, String>>,
) -> Result<(String, u32), WorkflowError> {
    let mut tokens_used: u32 = 0;
    let mut iterations: u32 = 0;

    // Build system prompt: skill content + intent + steps (NO steering, NO memory)
    let system = build_activity_prompt(activity, prior_context, inputs, skill_content);

    // Build tool definitions
    let tool_defs: Vec<ai::ToolDefinition> = tools
        .iter()
        .map(|t| ai::ToolDefinition {
            name: t.name().to_string(),
            description: t.description(),
            input_schema: t.schema(),
        })
        .collect();

    let mut messages = vec![ai::Message {
        role: "user".into(),
        content: activity.intent.clone(),
        ..Default::default()
    }];

    loop {
        if iterations >= MAX_ITERATIONS {
            return Err(WorkflowError::MaxIterations(activity.id.clone()));
        }
        if tokens_used >= activity.token_budget.max {
            return Err(WorkflowError::BudgetExceeded {
                activity_id: activity.id.clone(),
                used: tokens_used,
                limit: activity.token_budget.max,
            });
        }

        let remaining = activity.token_budget.max.saturating_sub(tokens_used);

        let req = ChatRequest {
            messages: messages.clone(),
            tools: tool_defs.clone(),
            max_tokens: remaining as i32,
            temperature: 0.0,
            system: system.clone(),
            static_system: String::new(),
            model: activity.model.clone(),
            enable_thinking: false,
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

        // If no tool calls, we're done
        if tool_calls.is_empty() {
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
            let tool = tools.iter().find(|t| t.name() == tc.name);
            let result = match tool {
                Some(t) => t.execute_dyn(&ctx, tc.input.clone()).await,
                None => tools::ToolResult::error(format!("tool not found: {}", tc.name)),
            };

            tool_result_entries.push(serde_json::json!({
                "tool_call_id": tc.id,
                "content": result.content,
                "is_error": result.is_error,
            }));
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

/// Build the system prompt for an activity (lean — no steering, memory, or personality).
///
/// Spec order: Skills → Task → Steps → Inputs → Prior Results
fn build_activity_prompt(
    activity: &Activity,
    prior_context: &str,
    inputs: &serde_json::Value,
    skill_content: Option<&HashMap<String, String>>,
) -> String {
    let mut prompt = String::new();

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

    // Inputs
    if let serde_json::Value::Object(map) = inputs {
        if !map.is_empty() {
            prompt.push_str("## Inputs\n");
            for (key, val) in map {
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

    prompt
}
