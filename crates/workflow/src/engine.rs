use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use ai::{ChatRequest, StreamEventType};
use db::Store;
use tools::registry::DynTool;

use crate::WorkflowError;
use crate::parser::{Activity, WorkflowDef};

const MAX_ITERATIONS: u32 = 50;

/// Open a provider stream, retrying transient/retryable errors (transport
/// blips, 5xx, rate limits) with a short backoff — the same classes the
/// chat runner retries. Terminal errors (auth, usage limit) fail immediately.
async fn stream_with_retry(
    provider: &dyn ai::Provider,
    req: &ChatRequest,
) -> Result<tokio::sync::mpsc::Receiver<ai::StreamEvent>, ai::ProviderError> {
    const MAX_ATTEMPTS: u32 = 3;
    let mut attempt = 1;
    loop {
        match provider.stream(req).await {
            Ok(rx) => return Ok(rx),
            Err(e) if attempt < MAX_ATTEMPTS
                && (e.is_retryable() || ai::is_transient_error(&e)) =>
            {
                warn!(attempt, error = %e, "workflow provider error, retrying");
                attempt += 1;
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
            Err(e) => return Err(e),
        }
    }
}

/// Decision from the step evaluator (orchestrator between steps).
#[derive(Debug)]
enum EvalDecision {
    Proceed,
    Exit(String),
}

fn parse_eval_response(content: &str) -> EvalDecision {
    let trimmed = content.trim();
    if let Some(reason) = trimmed.strip_prefix("exit:") {
        let reason = reason.trim();
        if exit_reason_is_really_proceed(reason) {
            return EvalDecision::Proceed;
        }
        EvalDecision::Exit(reason.to_string())
    } else {
        EvalDecision::Proceed
    }
}

/// An `exit:` whose reason is actually a proceed/skip statement is treated as
/// proceed. Observed live, four times across one workflow: the evaluator wrote
/// `exit: Email is valid ... proceeding to Step 2`, `exit: ... skipping as
/// instructed`, `exit: SKIP` (a step's own sentinel echoed back), and
/// `exit: Report file path identified from payload: /data/.../report.xls` — a
/// healthy, affirmative step result — each of which killed the RUN, not the
/// step, so downstream loops and summaries never executed. Exit is reserved
/// for "this task is inapplicable / a precondition failed / continuing would
/// be harmful"; a reason that says the step succeeded, is proceeding, or is a
/// conditional skip is none of those.
fn exit_reason_is_really_proceed(reason: &str) -> bool {
    let r = reason.to_ascii_lowercase();
    if r.is_empty() {
        return false;
    }
    const PROCEED_SIGNALS: &[&str] = &[
        "proceed",
        "continu",
        "next step",
        "as instructed",
        "not applicable to this",
        "no action needed",
        "nothing to do for this",
        "skipping",
        "skipped",
        "identified",
        "complete",
        "resolved",
        "logged",
        "successfully",
        "ready for",
    ];
    if r == "skip" || r == "skipped" || r == "none" {
        return true;
    }
    // A reason that names a concrete artifact (a path, an id) is a step
    // result, not an exit condition.
    if r.contains('/') && (r.ends_with(".xls") || r.ends_with(".xlsx") || r.ends_with(".csv") || r.ends_with(".json") || r.ends_with(".pdf")) {
        return true;
    }
    PROCEED_SIGNALS.iter().any(|s| r.contains(s))
}

/// Scope an activity's toolset to what it declares and references.
///
/// The full registry (~38 tools, ~21k tokens of schemas) went out with EVERY
/// LLM turn and invited small models to wander — web-searching for
/// instructions the step already spells out, or shelling out to plugin
/// binaries via `os` instead of the plugin tool. A tool is included when:
///
/// - the activity DECLARES it in agent.json — `mcps` entries select that
///   server's proxy tools (`mcp__<server>__*`), `cmds` (plugin commands)
///   select the `plugin` tool — the authored contract comes first;
/// - or its intent/steps/skill docs REFERENCE it — `<tool>(` — directly or
///   through a legacy pre-STRAP name (`organizer(` → `os`, `gws(` →
///   `plugin`; see `tools::registry::legacy_tool_aliases`), so imported
///   workflows authored against old tool names still scope correctly;
/// - or it is `message` (the delivery primitive — steps often say "alert"
///   without naming it).
///
/// When nothing is declared or referenced, fall back to the NON-DEFERRED
/// roster only. Deferred tools (MCP proxies, heavyweight domain tools) are
/// deferred precisely so their schemas don't ship until needed — the old
/// fail-open-with-everything sent every connected MCP server's full schemas
/// (~20k tokens/call) to activities whose agent.json declared `mcps: []`.
pub(crate) fn scoped_activity_tools<'a>(
    activity: &Activity,
    resolved_tools: &'a [Box<dyn DynTool>],
    skill_content: Option<&HashMap<String, String>>,
    deferred: Option<&HashSet<String>>,
) -> Vec<&'a Box<dyn DynTool>> {
    // Explicit declaration wins outright: deterministic, auditable, immune to
    // the text-sniffing gap below (dotted tool names in prose never match
    // `name(` and silently fall back to the full roster).
    if !activity.tools.is_empty() {
        let declared: Vec<&'a Box<dyn DynTool>> = resolved_tools
            .iter()
            .filter(|t| {
                let n = t.name();
                n == "message"
                    || activity.tools.iter().any(|d| {
                        n == d || n.strip_prefix(d.as_str()).is_some_and(|r| r.starts_with('.'))
                    })
            })
            .collect();
        info!(
            activity = activity.id.as_str(),
            tools = declared.len(),
            "scoped activity toolset to declared tools"
        );
        return declared;
    }

    let mut text = activity.intent.clone();
    for s in &activity.steps {
        text.push_str(s);
    }
    if let Some(skills) = skill_content {
        for name in &activity.skills {
            if let Some(body) = skills.get(name.as_str()) {
                text.push_str(body);
            }
        }
    }

    // Declared MCP servers → proxy-name prefixes (server keys are normalized
    // the same way proxy names are built: lowercase, non-alphanumeric → `_`).
    let mcp_prefixes: Vec<String> = activity
        .mcps
        .iter()
        .map(|s| {
            let norm: String = s
                .to_lowercase()
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                .collect();
            format!("mcp__{norm}")
        })
        .collect();
    // Declared plugin commands run through the plugin tool ("emit" is the
    // event primitive, injected separately — it declares no plugin need).
    let wants_plugin = activity.cmds.iter().any(|c| c != "emit");
    // Legacy pre-STRAP names appearing in the text → their absorbing tool.
    let alias_targets: HashSet<&'static str> = tools::registry::legacy_tool_aliases()
        .iter()
        .filter(|(alias, _)| text.contains(&format!("{alias}(")))
        .map(|(_, target)| *target)
        .collect();

    let referenced: Vec<&'a Box<dyn DynTool>> = resolved_tools
        .iter()
        .filter(|t| {
            let n = t.name();
            n == "message"
                || text.contains(&format!("{n}("))
                || alias_targets.contains(n)
                || (wants_plugin && n == "plugin")
                || mcp_prefixes.iter().any(|p| n.to_lowercase().starts_with(p.as_str()))
                || activity.mcps.iter().any(|m| m == n)
        })
        .collect();
    if referenced.iter().any(|t| t.name() != "message") {
        info!(
            activity = activity.id.as_str(),
            tools = referenced.len(),
            "scoped activity toolset to declared + referenced tools"
        );
        referenced
    } else {
        // Fail-soft: active (non-deferred) tools only — never ship deferred
        // schemas an activity neither declared nor referenced. Loud, so a
        // workflow whose steps reference only unknown/stale tool names shows
        // up in logs instead of silently running with a blanket roster.
        let fallback: Vec<&'a Box<dyn DynTool>> = resolved_tools
            .iter()
            .filter(|t| deferred.is_none_or(|d| !d.contains(t.name())))
            .collect();
        warn!(
            activity = activity.id.as_str(),
            tools = fallback.len(),
            "activity declares and references no known tools; using non-deferred roster"
        );
        fallback
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

/// Approval-checkpoint context for a run: the employee's per-operation policy
/// plus, on a post-approval re-run, the one-shot token authorizing exactly the
/// call the owner saw. Matched on operation suffix + exact input hash — a call
/// that drifted on re-derivation re-asks rather than executing something the
/// owner never approved.
#[derive(Debug, Clone, Default)]
pub struct CheckpointCtx {
    pub operation_policy: Option<tools::policy::OperationPolicy>,
    /// The seat binding name (for the suspension row / notification).
    pub binding_name: String,
    /// The run's inputs carry untrusted content (a watch/comm payload) —
    /// the gate decides as `Origin::Comm` instead of trusted Workflow, so a
    /// gated `Always` floors to Approval (WS2-R7: input taint, not just
    /// origin; the payload steering the run arrived from outside).
    pub tainted: bool,
}

/// Durable resume state for a run parked at the approval checkpoint —
/// Temporal-style semantics: the suspension persisted the full conversation,
/// the pending (now owner-approved) tool call, and its exact position; resume
/// rehydrates and continues AT the blocked call. Nothing before the pause
/// re-executes, so non-idempotent side effects can never duplicate.
#[derive(Debug, Clone)]
pub struct ResumeState {
    pub activity_id: String,
    /// Loop scope path the run suspended in ("" outside a loop) — resuming
    /// must re-enter the same iteration, not the activity generally.
    pub iteration: String,
    pub step_index: Option<i64>,
    pub messages: Vec<ai::Message>,
    /// The approved call — executed directly on resume (it IS what the owner
    /// saw; the checkpoint is bypassed for exactly this call id).
    pub pending: ai::ToolCall,
}

#[allow(unused_assignments)] // circuit breaker state is future-proofed for Fallback::Skip
pub async fn execute_workflow(
    def: &WorkflowDef,
    agent_id: &str,
    // Resolved memory scope for tool execution, provided by the caller (the
    // server layer owns the scope derivation — see agent::memory). A bare
    // user_id made every workflow run read/write the global unowned "" scope
    // shared across all agents (isolation audit 2026-08-22, leak class 1).
    // For context-isolated agents a workflow run has no matter, so callers
    // pass writes_disabled=true — fail closed, reads still serve the scope.
    memory_user_id: &str,
    memory_writes_disabled: bool,
    inputs: serde_json::Value,
    trigger_type: &str,
    trigger_detail: Option<&str>,
    store: &Arc<Store>,
    provider: &dyn ai::Provider,
    resolved_tools: &[Box<dyn DynTool>],
    // Names of deferred tools in the registry (MCP proxies etc.) — excluded
    // from the fail-soft roster so their schemas only ship to activities
    // that declare or reference them. `None` = treat all tools as active.
    deferred_tools: Option<&HashSet<String>>,
    existing_run_id: Option<&str>,
    cancel_token: Option<&CancellationToken>,
    skill_content: Option<&HashMap<String, String>>,
    event_bus: Option<&tools::EventBus>,
    emit_source: Option<String>,
    progress_tx: Option<tokio::sync::mpsc::UnboundedSender<WorkflowProgress>>,
    checkpoint: Option<&CheckpointCtx>,
    resume: Option<ResumeState>,
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
            memory_user_id,
            memory_writes_disabled,
            &inputs,
            store,
            provider,
            resolved_tools,
            deferred_tools,
            &run_id,
            cancel_token,
            skill_content,
            event_bus,
            resolved_emit,
            progress_tx,
            checkpoint,
            resume,
        )
        .await;
    }

    let mut total_tokens: u32 = 0;
    // Output tokens only — what budget.total_per_run is enforced in (same
    // semantics as the graph executor and the per-activity budgets). Input is
    // dominated by fixed per-turn overhead (tool schemas, context) resent every
    // call, so metering the run budget in input+output made small budgets trip
    // on the first call regardless of how much work the model actually did.
    let mut total_output_tokens: u32 = 0;
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

        // Scope the toolset to what the activity actually uses (see
        // scoped_activity_tools) — the full registry went out with EVERY LLM
        // turn (~21k tokens of schemas) and invited small models to wander.
        let mut activity_tools: Vec<&Box<dyn DynTool>> =
            scoped_activity_tools(activity, resolved_tools, skill_content, deferred_tools);

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
        // Output tokens only — the unit token budgets are enforced in.
        let mut activity_spent_output: u32 = 0;

        match execute_activity_with_retry(
            activity,
            &prior_context,
            memory_user_id,
            memory_writes_disabled,
            &inputs,
            provider,
            &activity_tools,
            resolved_tools,
            skill_content,
            activity_emit,
            store,
            agent_id,
            &run_id,
            &def.id,
            progress_tx.as_ref(),
            &mut activity_spent,
            &mut activity_spent_output,
            checkpoint,
            resume.as_ref().filter(|r| r.activity_id == activity.id),
            "", // sequential engine has no loop nodes
        )
        .await
        {
            Ok((result_text, _tokens_used)) => {
                total_tokens += activity_spent;
                total_output_tokens += activity_spent_output;
                consecutive_failures = 0;
                last_failure_pattern = None;

                let completed_at = chrono::Utc::now().timestamp();
                if let Err(e) = store.create_activity_result(
                    &run_id,
                    &activity.id,
                    "",
                    "completed",
                    activity_spent as i64,
                    1,
                    None,
                    started_at,
                    Some(completed_at),
                ) {
                    warn!(run_id = %run_id, activity = %activity.id, error = %e, "failed to record activity result");
                }
                // Output content backs the resume fast-forward — a parked run
                // never re-executes an activity whose result is recorded.
                let _ = store.set_activity_result_content(&run_id, &activity.id, "", &result_text);

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
                total_output_tokens += activity_spent_output;
                let completed_at = chrono::Utc::now().timestamp();
                let _ = store.create_activity_result(
                    &run_id,
                    &activity.id,
                    "",
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
            // A suspension is NOT a failure: the engine already parked the run
            // as awaiting_approval and persisted the pending call. Propagate
            // untouched — the failure bookkeeping below would overwrite the
            // parked status and paint the run red.
            Err(e @ WorkflowError::AwaitingApproval { .. }) => {
                return Err(e);
            }
            Err(e) => {
                total_tokens += activity_spent;
                total_output_tokens += activity_spent_output;
                let completed_at = chrono::Utc::now().timestamp();
                let err_msg = e.to_string();
                if let Err(db_err) = store.create_activity_result(
                    &run_id,
                    &activity.id,
                    "",
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

        // Check total budget — output tokens only, matching the graph executor
        // and the per-activity budgets (`total_tokens` keeps full input+output
        // for run reporting).
        if def.budget.total_per_run > 0 && total_output_tokens > def.budget.total_per_run {
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
                used: total_output_tokens,
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
    memory_user_id: &str,
    memory_writes_disabled: bool,
    inputs: &serde_json::Value,
    provider: &dyn ai::Provider,
    tools: &[&Box<dyn DynTool>],
    // Full resolved roster — dispatch fallback only (see resolve_tool_call);
    // schemas advertised to the model come from `tools` (the scoped set).
    roster: &[Box<dyn DynTool>],
    skill_content: Option<&HashMap<String, String>>,
    emit_source: Option<&str>,
    store: &Arc<Store>,
    agent_id: &str,
    run_id: &str,
    workflow_id: &str,
    progress_tx: Option<&tokio::sync::mpsc::UnboundedSender<WorkflowProgress>>,
    spent: &mut u32,
    spent_output: &mut u32,
    checkpoint: Option<&CheckpointCtx>,
    resume: Option<&ResumeState>,
    iteration: &str,
) -> Result<(String, u32), WorkflowError> {
    // Resume fast-forward: an activity this run already completed returns its
    // recorded output instead of re-executing — the Temporal property that a
    // resumed run never re-does finished (possibly non-idempotent) work.
    //
    // Keyed by (activity_id, iteration), NOT activity_id alone: a loop body
    // appends a completed row per item within the SAME run, so matching on the
    // id alone made item 2 replay item 1's output and the body ran exactly once
    // however many items there were. `iteration` is "" outside a loop, so
    // linear workflows behave exactly as before.
    if let Ok(done) = store.completed_activity_contents(run_id) {
        if let Some(content) = done.get(&(activity.id.clone(), iteration.to_string())) {
            info!(activity = activity.id.as_str(), run_id, "resume: skipping completed activity");
            return Ok((content.clone(), 0));
        }
    }
    let max_attempts = activity.on_error.retry.max(1);

    for attempt in 0..max_attempts {
        match execute_activity(
            activity,
            prior_context,
            memory_user_id,
            memory_writes_disabled,
            inputs,
            provider,
            tools,
            roster,
            skill_content,
            emit_source,
            store,
            agent_id,
            run_id,
            workflow_id,
            progress_tx,
            spent,
            spent_output,
            checkpoint,
            resume,
            iteration,
        )
        .await
        {
            Ok(result) => return Ok(result),
            // Deliberate stops are not failures — retrying would re-run the
            // activity's tool side effects from scratch. Blocked is terminal
            // by definition (FRAMES): a retry hits the same wall. A suspension
            // (AwaitingApproval) must surface untouched — the run is parked
            // for the owner, not failed.
            Err(
                e @ (WorkflowError::Exited(_)
                | WorkflowError::Cancelled
                | WorkflowError::Blocked(_)
                | WorkflowError::AwaitingApproval { .. }),
            ) => return Err(e),
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
    memory_user_id: &str,
    memory_writes_disabled: bool,
    inputs: &serde_json::Value,
    provider: &dyn ai::Provider,
    tools: &[&Box<dyn DynTool>],
    // Full resolved roster — dispatch fallback only (see resolve_tool_call);
    // schemas advertised to the model come from `tools` (the scoped set).
    roster: &[Box<dyn DynTool>],
    skill_content: Option<&HashMap<String, String>>,
    emit_source: Option<&str>,
    store: &Arc<Store>,
    agent_id: &str,
    run_id: &str,
    workflow_id: &str,
    progress_tx: Option<&tokio::sync::mpsc::UnboundedSender<WorkflowProgress>>,
    spent: &mut u32,
    spent_output: &mut u32,
    checkpoint: Option<&CheckpointCtx>,
    resume: Option<&ResumeState>,
    iteration: &str,
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

    // Identity + memory continuity for agent-bound runs. Computed per
    // activity so mid-run memory writes surface in later activities.
    let agent_ctx = build_agent_context(store, agent_id);

    // If activity has steps, execute per-step. Otherwise, single-turn legacy path.
    if activity.steps.is_empty() {
        // No steps — legacy single-turn execution
        let system = build_activity_prompt_with_context(
            activity,
            prior_context,
            inputs,
            skill_content,
            emit_source,
            has_browser,
            &tool_names,
            agent_ctx.as_deref(),
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
        let (messages, pending) = match resume {
            Some(r) => (r.messages.clone(), Some(r.pending.clone())),
            None => (messages, None),
        };
        return run_llm_loop(activity, memory_user_id, memory_writes_disabled, provider, tools, roster, &tool_defs, &system, messages, spent, spent_output, make_trace(String::new()), store, checkpoint, pending, iteration).await;
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
        agent_ctx.as_deref(),
    );

    // Shared conversation — messages accumulate across steps
    let mut messages = Vec::new();
    let mut total_tokens: u32 = 0;
    let mut step_outputs: Vec<String> = Vec::new();
    let total_steps = activity.steps.len();

    // Resume rehydration: earlier steps' work lives in the restored messages —
    // skip re-executing them; enter the loop at the suspended step with the
    // persisted conversation and the approved pending call.
    let resume_step = resume.and_then(|r| r.step_index).unwrap_or(-1);
    let mut resume_pending = resume.map(|r| r.pending.clone());
    for (i, step) in activity.steps.iter().enumerate() {
        if resume.is_some() && (i as i64) < resume_step {
            continue;
        }
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

        // Send step as user message — except at the resumed step, whose
        // conversation (step message included) is restored verbatim.
        if resume.is_some() && (i as i64) == resume_step {
            messages = resume.map(|r| r.messages.clone()).unwrap_or_default();
        } else {
            let step_msg = format!("Step {}/{}: {}", i + 1, total_steps, step);
            messages.push(ai::Message {
                role: "user".into(),
                content: step_msg,
                ..Default::default()
            });
        }

        // Run LLM loop for this step
        let (step_result, step_tokens) = run_llm_loop(
            activity,
            memory_user_id,
            memory_writes_disabled,
            provider,
            tools,
            roster,
            &tool_defs,
            &system,
            messages.clone(),
            spent,
            spent_output,
            make_trace(i.to_string()),
            store,
            checkpoint,
            if resume.is_some() && (i as i64) == resume_step { resume_pending.take() } else { None },
            iteration,
        )
        .await
        .map_err(|e| {
            // Exit-by-design (exit tool) is a clean stop, not a step failure —
            // recording it as failed painted successful exited runs red in the UI.
            // A suspension keeps the step pending: it re-runs after approval.
            let status = if matches!(e, WorkflowError::Exited(_)) {
                "exited"
            } else if matches!(e, WorkflowError::AwaitingApproval { .. }) {
                "pending"
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
        // Give the evaluator the remaining steps so it can see that unexecuted
        // work (often the side effects: store, send, record) still exists —
        // without this it exited workflows whose intermediate output merely
        // LOOKED complete (voice profile distilled at step 2/5, never stored).
        let remaining_steps = activity.steps[i + 1..]
            .iter()
            .enumerate()
            .map(|(j, s)| format!("- Step {}: {}", i + j + 2, truncate_at_char_boundary(s, 200)))
            .collect::<Vec<_>>()
            .join("\n");
        let (eval, eval_tokens) = evaluate_step(
            provider,
            &system,
            step,
            &step_result,
            i,
            total_steps,
            &remaining_steps,
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
            EvalDecision::Exit(reason) if i + 1 == total_steps => {
                // FINAL step: there are nothing left to skip — the eval prompt
                // itself says "(none — this is the final step)" — so an exit
                // here can only kill downstream graph nodes (loop re-entry,
                // commit, delivery) for zero benefit. Observed live: the
                // evaluator exited on "Chunk 7 complete: 4 rows resolved..."
                // — an affirmative completion — after the keyword guard in
                // exit_reason_is_really_proceed missed it ("complete:" vs
                // "completed"). Keyword lists leak; position doesn't. Treat a
                // final-step exit as normal completion of the activity.
                info!(
                    activity = %activity.id,
                    step = i,
                    reason = %reason,
                    "evaluator exit on final step demoted to completion"
                );
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

        // Cumulative per-activity budget across steps + evaluator turns —
        // output tokens, same unit as the in-loop check.
        if activity.token_budget.max > 0 && *spent_output > activity.token_budget.max {
            return Err(WorkflowError::BudgetExceeded {
                activity_id: activity.id.clone(),
                used: *spent_output,
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
    remaining_steps: &str,
    trace: ai::RequestTrace,
) -> Result<(EvalDecision, u32), WorkflowError> {
    let remaining_block = if remaining_steps.is_empty() {
        String::from("(none — this is the final step)")
    } else {
        remaining_steps.to_string()
    };
    let eval_system = format!(
        "{}\n\n## Step Evaluation Mode\n\
         You are evaluating the output of Step {}/{}: \"{}\"\n\n\
         Steps that have NOT run yet:\n{}\n\n\
         Based on the workflow context above and the step output below, respond with EXACTLY ONE of:\n\
         - proceed — step completed its stated goal, continue to the next step\n\
         - exit:<reason> — ONLY when the task is inapplicable to this data, a required \
           precondition failed, or continuing would be harmful\n\n\
         NEVER exit because the work so far looks complete or sufficient — the remaining \
         steps exist for a reason and often perform the required side effects (storing, \
         sending, recording). \"Task completed\" and \"no actions required\" are NOT valid \
         exit reasons; if this step met its goal, respond proceed. If the remaining steps \
         carry their own conditional guards (\"only if\", \"always\", \"skip if\"), respond \
         proceed and let the steps self-gate — do not exit on their behalf.\n\n\
         Respond with ONLY the decision. Nothing else.",
        system, step_index + 1, total_steps, step_text, remaining_block,
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

    let mut rx = stream_with_retry(provider, &req)
        .await
        .map_err(|e| WorkflowError::Provider(e.to_string()))?;

    let mut response_text = String::new();
    // Max-merged per field: usage counters are cumulative running totals, and
    // a provider may emit them once (OpenAI final chunk), per chunk (Janus),
    // or split across two events with disjoint fields (input at start, output
    // at end). Max per field is correct for all three; summing or last-wins
    // is not.
    let mut eval_input: i32 = 0;
    let mut eval_output: i32 = 0;
    while let Some(event) = rx.recv().await {
        match event.event_type {
            StreamEventType::Text => response_text.push_str(&event.text),
            StreamEventType::Error => {
                warn!("step evaluator error: {:?}", event.error);
                let eval_tokens = (eval_input.max(0) + eval_output.max(0)) as u32;
                return Ok((EvalDecision::Proceed, eval_tokens));
            }
            // Providers emit usage as a dedicated Usage event; Done carries
            // usage: None everywhere (done()/done_with_reason() construct it
            // that way). Reading usage only on Done left eval_tokens at 0.
            StreamEventType::Usage => {
                if let Some(usage) = event.usage {
                    eval_input = eval_input.max(usage.input_tokens);
                    eval_output = eval_output.max(usage.output_tokens);
                }
            }
            StreamEventType::Done => {
                if let Some(usage) = event.usage {
                    eval_input = eval_input.max(usage.input_tokens);
                    eval_output = eval_output.max(usage.output_tokens);
                }
                break;
            }
            _ => {}
        }
    }

    let eval_tokens = (eval_input.max(0) + eval_output.max(0)) as u32;
    Ok((parse_eval_response(&response_text), eval_tokens))
}

/// Core LLM multi-turn loop extracted from the original execute_activity.
/// Runs until the LLM produces a response with no tool calls, then returns
/// the final text response and total tokens used.
///
/// `spent` accumulates EVERY token consumed, including turns that later end
/// in an error — error variants can't carry token counts, so callers read
/// the accumulator to keep run totals truthful across exits/failures/retries.
///
/// `spent_output` accumulates output tokens only, across all of an activity's
/// steps — the unit token budgets are enforced in. Input tokens are dominated
/// by the fixed tool-schema overhead resent every turn (~30k), so an
/// input-inclusive budget would fail on turn 1 regardless of the model's work.
/// Budgets are opt-in: an activity with no declared budget (max 0) is uncapped.

/// Resolve a model tool call to an executable tool + input.
///
/// The first-tool-call-success contract: a call written against a legacy
/// pre-STRAP name (`organizer(resource: "mail")`) EXECUTES — resolved through
/// the one canonical alias table (tools::registry::resolve_flat_alias) — it
/// never bounces through a correction round-trip. And scoping restricts the
/// advertised MENU, never the executable KITCHEN: a real roster tool called
/// by name runs (with a warning that names the scope miss) even when its
/// schema wasn't shipped for this activity. Small models follow step text
/// literally; their first call has to land.
fn resolve_tool_call<'a>(
    scoped: &[&'a Box<dyn DynTool>],
    roster: &'a [Box<dyn DynTool>],
    call_name: &str,
    input: &serde_json::Value,
) -> (String, serde_json::Value, Option<&'a Box<dyn DynTool>>) {
    let (name, input) = match tools::registry::resolve_flat_alias(call_name) {
        Some((strap, params)) => {
            let mut merged = input.clone();
            if let Some(obj) = merged.as_object_mut() {
                for (k, v) in params {
                    obj.entry(&k).or_insert(v);
                }
            }
            info!(requested = %call_name, resolved = %strap, "legacy tool name resolved at dispatch");
            (strap, merged)
        }
        None => (call_name.to_string(), input.clone()),
    };
    let stripped = strip_mcp_prefix(&name).to_string();
    let found: Option<&'a Box<dyn DynTool>> = scoped
        .iter()
        .find(|t| t.name() == name)
        .or_else(|| scoped.iter().find(|t| t.name() == stripped))
        .copied()
        .or_else(|| {
            roster
                .iter()
                .find(|t| t.name() == name || t.name() == stripped)
                .inspect(|t| {
                    warn!(
                        tool = %t.name(),
                        "tool executed outside activity scope — declare or reference it so its schema ships"
                    );
                })
        });
    (name, input, found)
}

#[allow(clippy::too_many_arguments)]
async fn run_llm_loop(
    activity: &Activity,
    memory_user_id: &str,
    memory_writes_disabled: bool,
    provider: &dyn ai::Provider,
    tools: &[&Box<dyn DynTool>],
    roster: &[Box<dyn DynTool>],
    tool_defs: &[ai::ToolDefinition],
    system: &str,
    mut messages: Vec<ai::Message>,
    spent: &mut u32,
    spent_output: &mut u32,
    trace: ai::RequestTrace,
    store: &Arc<Store>,
    checkpoint: Option<&CheckpointCtx>,
    pending: Option<ai::ToolCall>,
    iteration: &str,
) -> Result<(String, u32), WorkflowError> {
    let mut tokens_used: u32 = 0;
    let mut iterations: u32 = 0;
    // Per-activity turn budget: params.maxIterations overrides the default,
    // same shape the graph loop node already accepts (graph.rs).
    let max_iterations: u32 = activity
        .params
        .as_ref()
        .and_then(|p| p.get("maxIterations"))
        .and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        })
        .map(|v| v as u32)
        .unwrap_or(MAX_ITERATIONS);
    let mut consecutive_all_not_found: u32 = 0;
    let mut last_tool_name: String = String::new();
    let mut consecutive_same_tool: u32 = 0;
    // Tools this activity must actually land before it may finish (see
    // Activity::requires_tools). Only a NON-error result counts: an attempt
    // that failed is exactly the case this guard exists to catch.
    let mut required_pending: std::collections::HashSet<String> =
        activity.requires_tools.iter().cloned().collect();

    // Workflow activities are unattended — Origin::Workflow keeps the ask tool
    // (and any HITL-gated capability) unavailable so the engine never blocks on
    // a UI prompt. Carry the run's agent identity in the session key so tools
    // resolve per-agent state (plugin account profiles, memory scope) — a bare
    // context made every workflow run account-less and scope-less.
    let mut ctx = tools::ToolContext::new(tools::Origin::Workflow).with_session(
        tools::workflow_session_key(&trace.agent_id, &trace.run_id),
        trace.run_id.clone(),
    );
    // Caller-resolved memory scope: without it every workflow activity ran
    // tools against the global unowned "" scope (see execute_workflow docs).
    ctx.user_id = memory_user_id.to_string();
    ctx.memory_writes_disabled = memory_writes_disabled;
    let ctx = ctx;

    // ── Durable resume entry ──────────────────────────────────────────
    // The restored conversation ends at the assistant message whose gated
    // call the owner just approved. Execute exactly that call (checkpoint
    // bypassed — it IS the approved call), append its result, and fall into
    // the normal loop: the next model turn sees the outcome and continues
    // the workflow from precisely where it paused. Nothing earlier re-runs.
    if let Some(tc) = pending {
        let (_name, input, tool) = resolve_tool_call(tools, roster, &tc.name, &tc.input);
        let result = match tool {
            Some(t) => t.execute_dyn(&ctx, input).await,
            None => tools::ToolResult::error(format!("tool not found: {}", tc.name)),
        };
        if result.terminal {
            return Err(WorkflowError::Blocked(result.content.clone()));
        }
        if !result.is_error {
            if let Some(reason) = result.content.strip_prefix(tools::EXIT_SENTINEL) {
                return Err(WorkflowError::Exited(reason.to_string()));
            }
        }
        messages.push(ai::Message {
            role: "tool".into(),
            content: String::new(),
            tool_results: Some(serde_json::Value::Array(vec![serde_json::json!({
                "tool_call_id": tc.id,
                "content": result.content,
                "is_error": result.is_error,
            })])),
            ..Default::default()
        });
    }

    loop {
        if iterations >= max_iterations {
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

        let mut rx = stream_with_retry(provider, &req)
            .await
            .map_err(|e| WorkflowError::Provider(e.to_string()))?;

        let mut response_text = String::new();
        let mut tool_calls: Vec<ai::ToolCall> = Vec::new();
        // Per-turn usage, merged max-per-field across however many Usage
        // events the provider emits. Usage counters are cumulative running
        // totals, so max = final; SUMMING them counted a ~200-token turn as
        // tens of thousands and tripped every budget (the 71613/8000 failures).
        let mut turn_input: i32 = 0;
        let mut turn_output: i32 = 0;

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
                // Providers emit usage as a dedicated Usage event; Done always
                // carries usage: None. Reading usage only on Done recorded 0
                // tokens for every workflow run, so token budgets never
                // enforced and the only runaway stop was the iteration cap.
                StreamEventType::Usage => {
                    if let Some(usage) = event.usage {
                        turn_input = turn_input.max(usage.input_tokens);
                        turn_output = turn_output.max(usage.output_tokens);
                    }
                }
                StreamEventType::Done => {
                    if let Some(usage) = event.usage {
                        turn_input = turn_input.max(usage.input_tokens);
                        turn_output = turn_output.max(usage.output_tokens);
                    }
                    break;
                }
                _ => {}
            }
        }

        // Commit the merged turn usage exactly once, after the stream ends.
        let turn_total = (turn_input.max(0) + turn_output.max(0)) as u32;
        tokens_used += turn_total;
        *spent += turn_total;
        *spent_output += turn_output.max(0) as u32;

        // Per-activity token budget — enforced DURING the loop, not after the
        // activity finishes. A runaway activity stops at its own ceiling
        // instead of spending unboundedly until the workflow-total check.
        // Budgets meter OUTPUT tokens (the model's work), cumulative across
        // the activity's steps; run totals stay full input+output.
        if activity.token_budget.max > 0 && *spent_output > activity.token_budget.max {
            return Err(WorkflowError::BudgetExceeded {
                activity_id: activity.id.clone(),
                used: *spent_output,
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
            // The activity declared an outward effect it never achieved. Fail
            // loudly: a run that "completed" while its email never sent looks
            // healthy in every log and is the worst kind of silent failure.
            if !required_pending.is_empty() {
                let mut missing: Vec<String> = required_pending.into_iter().collect();
                missing.sort();
                return Err(WorkflowError::ActivityFailed(
                    activity.id.clone(),
                    format!(
                        "stopped without a successful {} call — the activity's required effect \
                         never happened. The model's own summary said: {}",
                        missing.join(", "),
                        response_text.chars().take(300).collect::<String>()
                    ),
                ));
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

        // Execute each tool call and collect results (ctx built above the loop
        // with tools::workflow_session_key — supersedes the older inline binding).
        let mut tool_result_entries = Vec::new();
        for tc in &tool_calls {
            let (_name, resolved_input, tool) =
                resolve_tool_call(tools, roster, &tc.name, &tc.input);
            // ── Approval checkpoint (per-employee operation policy) ──────
            // The headless analog of the chat runner's operation gate: a gated
            // interface operation is decided by the SAME OperationPolicy.
            // Always → run; Blocked → refuse (roster omission is the primary
            // control, this is the backstop); Approval → SUSPEND the run
            // (persist the pending call, park as awaiting_approval, notify the
            // owner) instead of failing headless. On the post-approval re-run
            // the one-shot token (operation + exact input hash) admits exactly
            // the call the owner saw — a drifted re-derivation re-asks.
            if tc.name == "plugin" {
                if let Some(cp) = checkpoint {
                    {
                        let op = tc
                            .input
                            .get("operation")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        if !op.is_empty() {
                            let suffix = tools::plugin_tool::port_suffix(&op);
                            // Effective origin (WS2): the engine always runs as
                            // Workflow (trusted — owner-authored automation), but
                            // a run DRIVEN by untrusted content (watch/comm
                            // payload in its inputs) is decided as Comm: the
                            // words steering this run arrived from outside, so a
                            // gated Always floors to Approval (WS2-R7). With no
                            // policy set, decide_optional applies the same rule
                            // (None ⇒ trusted no-gate, untrusted ⇒ safe default).
                            let effective_origin = if cp.tainted {
                                tools::Origin::Comm
                            } else {
                                tools::Origin::Workflow
                            };
                            let access = tools::policy::OperationPolicy::decide_optional(
                                cp.operation_policy.as_ref(),
                                &op,
                                effective_origin,
                            );
                            if let Some(access) = access {
                                match access {
                                    tools::policy::OperationAccess::Always => {}
                                    tools::policy::OperationAccess::Blocked => {
                                        tool_result_entries.push(serde_json::json!({
                                            "tool_call_id": tc.id,
                                            "content": format!(
                                                "The operation '{op}' is turned OFF (Blocked) for this AI employee in its Controls. Do not retry or work around it."
                                            ),
                                            "is_error": true,
                                        }));
                                        continue;
                                    }
                                    tools::policy::OperationAccess::Approval => {
                                        let display = tc
                                            .input
                                            .get("display")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("")
                                            .trim()
                                            .to_string();
                                        if display.is_empty() {
                                            tool_result_entries.push(serde_json::json!({
                                                "tool_call_id": tc.id,
                                                "content": format!(
                                                    "The operation '{op}' needs the owner's approval, and the approval prompt requires a `display` sentence. Retry the SAME call adding display: one plain-language sentence a non-technical person understands — real names and formatted amounts, never raw ids or cents."
                                                ),
                                                "is_error": true,
                                            }));
                                            continue;
                                        }
                                        let messages_json =
                                            serde_json::to_string(&messages).unwrap_or_default();
                                        let pending_json =
                                            serde_json::to_string(tc).unwrap_or_default();
                                        if let Err(e) = store.create_workflow_suspension(
                                            &trace.run_id,
                                            &trace.agent_id,
                                            &cp.binding_name,
                                            &activity.id,
                                            iteration,
                                            trace.step_id.parse::<i64>().ok(),
                                            &messages_json,
                                            &pending_json,
                                            &suffix,
                                            &display,
                                        ) {
                                            // Can't persist the suspension → the run
                                            // cannot park safely; fail loud, never
                                            // silent-run the gated call.
                                            return Err(WorkflowError::Database(format!(
                                                "failed to persist approval suspension: {e}"
                                            )));
                                        }
                                        let _ = store.update_workflow_run(
                                            &trace.run_id,
                                            Some("awaiting_approval"),
                                            Some(&activity.id),
                                            None,
                                            None,
                                            None,
                                        );
                                        return Err(WorkflowError::AwaitingApproval {
                                            operation: suffix,
                                            display,
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }
            let result = match tool {
                Some(t) => t.execute_dyn(&ctx, resolved_input).await,
                None => tools::ToolResult::error(format!("tool not found: {}", tc.name)),
            };

            // Terminal tool result (auth expired, account not connected,
            // permission off — FRAMES classification): the run cannot do its
            // job; end it as blocked instead of feeding the error back for the
            // model to improvise around. The chat runner already does this —
            // the workflow engine let the model "voluntarily" exit, which read
            // as a clean stop and hid hard failures for days.
            if result.terminal {
                return Err(WorkflowError::Blocked(result.content.clone()));
            }

            // Check for exit sentinel
            if !result.is_error {
                if let Some(reason) = result.content.strip_prefix(tools::EXIT_SENTINEL) {
                    return Err(WorkflowError::Exited(reason.to_string()));
                }
                required_pending.remove(&tc.name);
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
#[allow(clippy::too_many_arguments)]
fn build_activity_prompt_no_steps(
    activity: &Activity,
    prior_context: &str,
    inputs: &serde_json::Value,
    skill_content: Option<&HashMap<String, String>>,
    emit_source: Option<&str>,
    has_browser: bool,
    tool_names: &[String],
    agent_context: Option<&str>,
) -> String {
    // Reuse the full builder but with an activity clone that has empty steps
    let mut stepless = activity.clone();
    stepless.steps = vec![];
    let mut prompt = build_activity_prompt_with_context(
        &stepless,
        prior_context,
        inputs,
        skill_content,
        emit_source,
        has_browser,
        tool_names,
        agent_context,
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
            "This is a coworker activity: message the employee named in the parameters \
             via message(resource: \"coworker\", action: \"send\", to: \"<name>\", \
             text: \"<the task>\") and relay their reply.",
        ),
        "connector" => Some(
            "This is an MCP connector activity: call the configured server's tool \
             (parameters name the server, tool, and input) via the mcp tool and report \
             the result.",
        ),
        _ => None,
    }
}

/// Per-agent identity + memory context injected into every activity prompt.
/// Soul is who the agent IS (voice, values, boundaries); the memory slice
/// gives scheduled runs continuity — most-used facts plus what happened most
/// recently, including post-run outcome history. Recall is not learning, so
/// this is NOT gated by learning_mode.
fn build_agent_context(store: &Store, agent_id: &str) -> Option<String> {
    if agent_id.is_empty() {
        return None;
    }
    let soul = store
        .get_agent(agent_id)
        .ok()
        .flatten()
        .and_then(|a| a.soul)
        .filter(|s| !s.trim().is_empty());

    // Base agent scope ONLY — never `:ctx:`-suffixed scopes. Context-isolated
    // agents (law-firm matters, per-client engagements) keep each context's
    // memories sealed from every other; a scheduled run has no case context,
    // so it must see none of them. It gets the agent-wide slice only.
    let mut memories = store.recent_memories_for_agent(agent_id, 8).unwrap_or_default();
    for m in store.list_memories_for_agent(agent_id, 8, 0).unwrap_or_default() {
        if !memories.iter().any(|e| e.id == m.id) {
            memories.push(m);
        }
    }
    memories.retain(|m| !m.user_id.contains(":ctx:"));
    memories.truncate(8);

    if soul.is_none() && memories.is_empty() {
        return None;
    }

    let mut out = String::new();
    if let Some(soul) = soul {
        out.push_str("## Who You Are\n\nEmbody this personality and tone. This is who you ARE — your voice, values, and boundaries.\n\n");
        out.push_str(&soul);
        out.push_str("\n\n");
    }
    if !memories.is_empty() {
        out.push_str("## Your Memory (recent and most-used)\n\n");
        for m in &memories {
            let mut value = m.value.replace('\n', " ");
            if value.len() > 300 {
                value.truncate(300);
                value.push('…');
            }
            out.push_str(&format!("- [{}/{}] {}\n", m.namespace, m.key, value));
        }
        out.push('\n');
    }
    Some(out)
}

#[allow(clippy::too_many_arguments)]
fn build_activity_prompt_with_context(
    activity: &Activity,
    prior_context: &str,
    inputs: &serde_json::Value,
    skill_content: Option<&HashMap<String, String>>,
    emit_source: Option<&str>,
    has_browser: bool,
    tool_names: &[String],
    agent_context: Option<&str>,
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

    // Identity + memory continuity for agent-bound runs (soul, recent history).
    if let Some(ctx) = agent_context {
        prompt.push_str(ctx);
    }

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
        prompt.push_str("\nDo NOT call any tool not in this list. Do NOT prefix tool names with mcp__ or any namespace.\n");
        // A step phrased as a CLI command ("Run: gws calendar +agenda") must go
        // through the plugin tool — running the bare binary via os/shell skips
        // the per-account credential injection and fails with "not
        // authenticated" even when the account is connected.
        if tool_names.iter().any(|t| t == "plugin") {
            prompt.push_str(
                "To run a plugin command (e.g. gws, slack, cos-store), ALWAYS use the plugin tool: \
                 plugin(resource: \"<name>\", action: \"exec\", command: \"<the command>\"). \
                 A step written as a shell command like `gws calendar +agenda --today` means \
                 plugin(resource: \"gws\", action: \"exec\", command: \"calendar +agenda --today\"). \
                 NEVER run a plugin binary through os or shell — only the plugin tool injects the \
                 account credentials, so the shell path fails auth.\n",
            );
        }
        prompt.push('\n');
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

    struct FakeTool(&'static str);
    impl DynTool for FakeTool {
        fn name(&self) -> &str {
            self.0
        }
        fn description(&self) -> String {
            String::new()
        }
        fn schema(&self) -> serde_json::Value {
            serde_json::json!({})
        }
        fn requires_approval(&self) -> bool {
            false
        }
        fn execute_dyn<'a>(
            &'a self,
            _ctx: &'a tools::ToolContext,
            _input: serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = tools::ToolResult> + Send + 'a>>
        {
            Box::pin(async { tools::ToolResult::ok(String::new()) })
        }
    }

    fn fake_registry() -> Vec<Box<dyn DynTool>> {
        ["plugin", "agent", "message", "os", "web", "browser"]
            .iter()
            .map(|n| Box::new(FakeTool(n)) as Box<dyn DynTool>)
            .collect()
    }

    #[test]
    fn test_scoped_activity_tools_filters_to_referenced() {
        let activity: Activity = serde_json::from_value(serde_json::json!({
            "id": "sweep",
            "intent": "Mark noise read",
            "steps": [
                "List: plugin(resource: \"gws\", action: \"exec\", command: \"gmail users messages list\")",
                "Record ids: agent(resource: \"memory\", action: \"store\", key: \"x\")"
            ]
        }))
        .unwrap();
        let registry = fake_registry();
        let scoped = scoped_activity_tools(&activity, &registry, None, None);
        let names: Vec<&str> = scoped.iter().map(|t| t.name()).collect();
        // plugin + agent referenced; message always rides along; os/web/browser stripped
        assert_eq!(names, vec!["plugin", "agent", "message"]);
    }

    #[test]
    fn test_scoped_activity_tools_fails_open_without_references() {
        let activity: Activity = serde_json::from_value(serde_json::json!({
            "id": "compile",
            "intent": "Compile the briefing from prior context",
            "steps": ["Lead with the most important thing", "Keep it scannable"]
        }))
        .unwrap();
        let registry = fake_registry();
        let scoped = scoped_activity_tools(&activity, &registry, None, None);
        // Nothing referenced, no deferral info → full roster (fail soft)
        assert_eq!(scoped.len(), registry.len());
    }

    #[test]
    fn test_scoped_activity_tools_failsoft_excludes_deferred() {
        // A context-compile activity that references nothing must NOT be
        // handed deferred schemas (MCP proxies) it never declared — that was
        // ~20k tokens of Monument schemas on every call of an activity whose
        // agent.json said mcps: [].
        let activity: Activity = serde_json::from_value(serde_json::json!({
            "id": "compile",
            "intent": "Compile the briefing from prior context",
            "steps": ["Keep it scannable"]
        }))
        .unwrap();
        let mut registry = fake_registry();
        registry.push(Box::new(FakeTool("mcp__monument__project")));
        let deferred: HashSet<String> = ["mcp__monument__project".to_string()].into();
        let scoped = scoped_activity_tools(&activity, &registry, None, Some(&deferred));
        let names: Vec<&str> = scoped.iter().map(|t| t.name()).collect();
        assert!(!names.contains(&"mcp__monument__project"));
        assert_eq!(names.len(), registry.len() - 1);
    }

    #[test]
    fn test_scoped_activity_tools_resolves_legacy_alias() {
        // Imported workflows authored pre-STRAP say `organizer(...)` — that
        // tool no longer exists (folded into os). The alias table must scope
        // this to os instead of matching nothing and blanketing the roster.
        let activity: Activity = serde_json::from_value(serde_json::json!({
            "id": "parse-brief",
            "intent": "List unread messages",
            "steps": ["List unread: organizer(resource: \"mail\", action: \"unread\")"]
        }))
        .unwrap();
        let registry = fake_registry();
        let scoped = scoped_activity_tools(&activity, &registry, None, None);
        let names: Vec<&str> = scoped.iter().map(|t| t.name()).collect();
        assert_eq!(names, vec!["message", "os"]);
    }

    #[test]
    fn test_resolve_tool_call_first_call_lands() {
        // The first-call contract: a legacy name EXECUTES (no correction
        // round-trip), and scoping narrows the advertised menu but never the
        // executable kitchen.
        let roster = fake_registry();
        let scoped: Vec<&Box<dyn DynTool>> =
            roster.iter().filter(|t| t.name() == "os" || t.name() == "message").collect();

        // Legacy alias: organizer( → os, params passed through untouched.
        let input = serde_json::json!({"resource": "mail", "action": "unread"});
        let (name, resolved_input, tool) = resolve_tool_call(&scoped, &roster, "organizer", &input);
        assert_eq!(name, "os");
        assert_eq!(resolved_input, input);
        assert_eq!(tool.expect("resolves to a live tool").name(), "os");

        // Single-purpose legacy alias injects its absorbed resource.
        let (_n, resolved_input, tool) =
            resolve_tool_call(&scoped, &roster, "spotlight", &serde_json::json!({"query": "q"}));
        assert_eq!(resolved_input["resource"], "search");
        assert_eq!(tool.unwrap().name(), "os");

        // Out-of-scope but real: plugin isn't in the scoped menu, still runs.
        let (_n, _i, tool) =
            resolve_tool_call(&scoped, &roster, "plugin", &serde_json::json!({}));
        assert_eq!(tool.expect("kitchen is never narrowed").name(), "plugin");

        // Genuinely unknown names still fail (no phantom tools).
        let (_n, _i, tool) =
            resolve_tool_call(&scoped, &roster, "definitely_not_a_tool", &serde_json::json!({}));
        assert!(tool.is_none());
    }

    #[test]
    fn test_scoped_activity_tools_honors_declared_mcps_and_cmds() {
        // agent.json declarations are the authored tool contract: mcps
        // selects that server's proxy tools, cmds selects the plugin tool —
        // even when the step prose never writes a `tool(` call.
        let activity: Activity = serde_json::from_value(serde_json::json!({
            "id": "sync",
            "intent": "Sync project changes",
            "mcps": ["monument"],
            "cmds": ["gws gmail +triage"],
            "steps": ["Pull recent changes and file them"]
        }))
        .unwrap();
        let mut registry = fake_registry();
        registry.push(Box::new(FakeTool("mcp__monument__project")));
        let deferred: HashSet<String> = ["mcp__monument__project".to_string()].into();
        let scoped = scoped_activity_tools(&activity, &registry, None, Some(&deferred));
        let names: Vec<&str> = scoped.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"mcp__monument__project"), "declared mcps scope in: {names:?}");
        assert!(names.contains(&"plugin"), "declared cmds scope the plugin tool in: {names:?}");
        assert!(!names.contains(&"web"), "undeclared tools stay out: {names:?}");
    }

    #[test]
    fn test_scoped_activity_tools_reads_skill_docs() {
        let activity: Activity = serde_json::from_value(serde_json::json!({
            "id": "triage-inbox",
            "intent": "Get unread email summary",
            "skills": ["gws-gmail-triage"],
            "steps": ["Run: gws gmail +triage --max 30 ONCE to get the unread summary."]
        }))
        .unwrap();
        let mut skills = HashMap::new();
        skills.insert(
            "gws-gmail-triage".to_string(),
            "Use plugin(resource: \"gws\", action: \"exec\", ...) to triage.".to_string(),
        );
        let registry = fake_registry();
        // Step text never names a tool, but the skill doc shows plugin( usage
        let scoped = scoped_activity_tools(&activity, &registry, Some(&skills), None);
        let names: Vec<&str> = scoped.iter().map(|t| t.name()).collect();
        assert_eq!(names, vec!["plugin", "message"]);
    }

    fn prompt_with_tools(tool_names: &[&str]) -> String {
        let activity: Activity = serde_json::from_value(serde_json::json!({
            "id": "agenda",
            "intent": "Read today's calendar",
            "steps": ["Run: gws calendar +agenda --today"]
        }))
        .unwrap();
        let names: Vec<String> = tool_names.iter().map(|s| s.to_string()).collect();
        build_activity_prompt_with_context(
            &activity,
            "",
            &serde_json::json!({}),
            None,
            None,
            false,
            &names,
            None,
        )
    }

    #[test]
    fn test_activity_prompt_routes_plugin_commands_through_plugin_tool() {
        // A step written as a bare CLI command must be steered to the plugin
        // tool — os/shell skips per-account credential injection.
        let prompt = prompt_with_tools(&["plugin", "message"]);
        assert!(prompt.contains("ALWAYS use the plugin tool"));
        assert!(prompt.contains("NEVER run a plugin binary through os or shell"));
    }

    #[test]
    fn test_activity_prompt_omits_plugin_guidance_without_plugin_tool() {
        let prompt = prompt_with_tools(&["os", "message"]);
        assert!(!prompt.contains("ALWAYS use the plugin tool"));
        // Section spacing unchanged for the no-plugin case.
        assert!(prompt.contains("or any namespace.\n\n"));
    }

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
        // Contradictory exits — an exit whose reason is a proceed/skip
        // statement — are proceed. Every one of these killed a live run.
        for contradictory in [
            "exit: Email is valid order report - proceeding to Step 2/5 to parse the attachment",
            "exit: Local test payload - no real mailbox message, skipping label operations as instructed",
            "exit: SKIP",
            "exit: Report file path identified from payload: /data/appdata/skills/x/inbox/oor.xls",
            "exit: no action needed for this item, continuing",
        ] {
            match parse_eval_response(contradictory) {
                EvalDecision::Proceed => {}
                other => panic!("expected Proceed for {contradictory:?}, got {:?}", other),
            }
        }
        // Real exits still exit.
        for real in [
            "exit: QuickBooks authentication is not configured; cannot pull transactions",
            "exit: task inapplicable — the email is a shipping notice, not an order",
            "exit: precondition failed: no connected account for gws",
        ] {
            match parse_eval_response(real) {
                EvalDecision::Exit(_) => {}
                other => panic!("expected Exit for {real:?}, got {:?}", other),
            }
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
        let prompt = build_activity_prompt_with_context(
            &activity,
            "",
            &serde_json::json!({}),
            None,
            None,
            false,
            &[],
            None,
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
        let prompt = build_activity_prompt_with_context(
            &plain,
            "",
            &serde_json::json!({}),
            None,
            None,
            false,
            &[],
            None,
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
