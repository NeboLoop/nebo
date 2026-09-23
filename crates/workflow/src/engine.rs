use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use db::Store;
use tools::registry::DynTool;

use crate::WorkflowError;
use crate::parser::{Activity, WorkflowDef};

const MAX_ITERATIONS: u32 = 50;

use crate::loop_contract::{ActivityLoop, LoopTurn};

/// Per-activity turn budget: params.maxIterations overrides the default —
/// the same shape the graph loop node accepts (graph.rs).
fn activity_max_iterations(activity: &Activity) -> u32 {
    activity
        .params
        .as_ref()
        .and_then(|p| p.get("maxIterations"))
        .and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
        .map(|v| v as u32)
        .unwrap_or(MAX_ITERATIONS)
}


/// Decision from the step evaluator (orchestrator between steps).
#[derive(Debug)]
enum EvalDecision {
    Proceed,
    Exit(String),
}

/// The slug of the seat a run belongs to — the producer stamped on every event
/// it raises and the seat an event address names. Falls back to the agent id
/// when the row is gone, and is "" for a standalone run with no owning seat.
pub(crate) fn producer_slug(store: &Store, agent_id: &str) -> String {
    if agent_id.is_empty() {
        return String::new();
    }
    store
        .get_agent(agent_id)
        .ok()
        .flatten()
        .map(|a| db::agent_slug(&a.name))
        .unwrap_or_else(|| agent_id.to_string())
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
    // The typed-decision door (Jev through Janus). The step evaluator and
    // `decide` activities run on it; `None` when Janus is not configured.
    // The engine itself never streams a chat model: every LLM turn goes
    // through `loop_impl`.
    decide: Option<&ai::DecideClient>,
    // The ONE injected agentic loop every activity runs through (Phase 4:
    // the engine's own loop is deleted; see loop_contract).
    loop_impl: &dyn ActivityLoop,
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
                    None,
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
            decide,
            loop_impl,
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
        let emit_tool_box: Option<Box<dyn DynTool>> = event_bus.map(|bus| {
            Box::new(tools::EmitTool::new(bus.clone()).with_producer(producer_slug(store, agent_id)))
                as Box<dyn DynTool>
        });
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
            decide,
            loop_impl,
            &activity_tools,
            skill_content,
            activity_emit,
            store,
            agent_id,
            &run_id,
            def,
            progress_tx.as_ref(),
            &mut activity_spent,
            &mut activity_spent_output,
            checkpoint,
            resume.as_ref().filter(|r| r.activity_id == activity.id),
            "", // sequential engine has no loop nodes
        
            cancel_token,
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
            // A standing outcome (an exit, a terminal refusal) ends the run
            // cleanly with its reason — never a failure.
            Err(e) if let Some(reason) = e.standing_outcome() => {
                total_tokens += activity_spent;
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
            // The owner's spending limit: the activity had its wrap-up turn,
            // so it ends as STOPPED with what it reported, and the run says
            // the limit was the owner's. Not a failure, nothing lost.
            Err(e @ WorkflowError::SpendCapReached { .. }) => {
                total_tokens += activity_spent;
                let completed_at = chrono::Utc::now().timestamp();
                let why = e.to_string();
                let partial = match &e {
                    WorkflowError::SpendCapReached { partial, .. } => partial.clone(),
                    _ => String::new(),
                };
                let _ = store.create_activity_result(
                    &run_id,
                    &activity.id,
                    "",
                    "stopped",
                    activity_spent as i64,
                    1,
                    Some(&why),
                    started_at,
                    Some(completed_at),
                );
                if !partial.trim().is_empty() {
                    let _ = store.set_activity_result_content(&run_id, &activity.id, "", &partial);
                    prior_context.push_str(&format!(
                        "\n[Activity '{}' stopped at the owner's limit; what it reported]: {}\n",
                        activity.id, partial
                    ));
                }
                let _ = store.complete_workflow_run(
                    &run_id,
                    "stopped",
                    total_tokens as i64,
                    Some(&why),
                    Some(&activity.id),
                    Some(&prior_context),
                );
                info!(workflow = def.id.as_str(), run_id = %run_id, activity = %activity.id, "workflow stopped at the owner's spending limit");
                return Err(e);
            }
            Err(e) => {
                total_tokens += activity_spent;
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

        // `budget.total_per_run` is the package author's estimate (it feeds the
        // listing's cost_estimate). It is never enforced: the only ceiling a
        // run has is the owner's, checked inside the loop.
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
    decide: Option<&ai::DecideClient>,
    loop_impl: &dyn ActivityLoop,
    tools: &[&Box<dyn DynTool>],
    skill_content: Option<&HashMap<String, String>>,
    emit_source: Option<&str>,
    store: &Arc<Store>,
    agent_id: &str,
    run_id: &str,
    workflow: &WorkflowDef,
    progress_tx: Option<&tokio::sync::mpsc::UnboundedSender<WorkflowProgress>>,
    spent: &mut u32,
    spent_output: &mut u32,
    checkpoint: Option<&CheckpointCtx>,
    resume: Option<&ResumeState>,
    iteration: &str,
    cancel_token: Option<&CancellationToken>,
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
            decide,
            loop_impl,
            tools,
            skill_content,
            emit_source,
            store,
            agent_id,
            run_id,
            workflow,
            progress_tx,
            spent,
            spent_output,
            checkpoint,
            resume,
            iteration,
        
            cancel_token,
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
    decide: Option<&ai::DecideClient>,
    loop_impl: &dyn ActivityLoop,
    tools: &[&Box<dyn DynTool>],
    skill_content: Option<&HashMap<String, String>>,
    emit_source: Option<&str>,
    store: &Arc<Store>,
    agent_id: &str,
    run_id: &str,
    workflow: &WorkflowDef,
    progress_tx: Option<&tokio::sync::mpsc::UnboundedSender<WorkflowProgress>>,
    spent: &mut u32,
    spent_output: &mut u32,
    checkpoint: Option<&CheckpointCtx>,
    resume: Option<&ResumeState>,
    iteration: &str,
    cancel_token: Option<&CancellationToken>,
) -> Result<(String, u32), WorkflowError> {
    // The owner's per-run spending limit for this employee (0 = none). The
    // package's token_budget figures are estimates and never enforced.
    let spend_cap_microcents: i64 = store.agent_run_spend_cap_cents(agent_id) * 1_000_000;
    // Detect if browser tool is available for this activity
    let has_browser = tools.iter().any(|t| t.name() == "web");
    let tool_names: Vec<String> = tools.iter().map(|t| t.name().to_string()).collect();

    // Trace builder — links every LLM call to this agent/run/workflow/action/step
    // so Janus can attribute usage per agent and per workflow. agent_id is "" for
    // standalone (non-agent-bound) workflow runs. step_id is the step index ("" when
    // the activity has no steps).
    let make_trace = |step_id: String| ai::RequestTrace {
        purpose: "workflow_activity",
        agent_id: agent_id.to_string(),
        run_id: run_id.to_string(),
        workflow_id: workflow.id.clone(),
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
        let (seed, pending) = match resume {
            Some(r) => (r.messages.clone(), Some(r.pending.clone())),
            None => (messages, None),
        };
        let turn = LoopTurn {
            activity,
            system,
            seed_messages: seed,
            workflow_name: &workflow.name,
            advertised_tools: tool_names.clone(),
            agent_id,
            user_id: memory_user_id,
            memory_writes_disabled,
            trace: make_trace(String::new()),
            checkpoint,
            pending,
            iteration,
            step_index: None,
            max_iterations: activity_max_iterations(activity),
            min_iterations: activity.min_iterations,
            requires_tools: activity.requires_tools.clone(),
            spend_cap_microcents,
            model: activity.model.clone(),
            cancel: cancel_token.cloned(),
            turn_key: format!("{}:{}", activity.id, iteration),
        };
        let out = loop_impl.run_turn(turn).await?;
        *spent += out.total_tokens;
        *spent_output += out.output_tokens;
        return Ok((out.text, out.total_tokens));
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
    // Outside text in the conversation so far: a watch payload in the run's
    // inputs, or any earlier step that read web, mail, channel or phone
    // content (its output is seeded into every later step). Sticky.
    let mut untrusted = checkpoint.is_some_and(|c| c.tainted);

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

        // Run this step through the ONE injected agentic loop.
        let turn = LoopTurn {
            activity,
            system: system.clone(),
            seed_messages: messages.clone(),
            workflow_name: &workflow.name,
            advertised_tools: tool_names.clone(),
            agent_id,
            user_id: memory_user_id,
            memory_writes_disabled,
            trace: make_trace(i.to_string()),
            checkpoint,
            pending: if resume.is_some() && (i as i64) == resume_step {
                resume_pending.take()
            } else {
                None
            },
            iteration,
            step_index: Some(i as i64),
            max_iterations: activity_max_iterations(activity),
            min_iterations: activity.min_iterations,
            requires_tools: activity.requires_tools.clone(),
            spend_cap_microcents,
            model: activity.model.clone(),
            cancel: cancel_token.cloned(),
            turn_key: format!("{}:{}:{}", activity.id, iteration, i),
        };
        let (step_result, step_tokens, step_tainted) = loop_impl
            .run_turn(turn)
            .await
            .map(|o| {
                *spent += o.total_tokens;
                *spent_output += o.output_tokens;
                (o.text, o.total_tokens, o.tainted)
            })
        .map_err(|e| {
            // Exit-by-design (exit tool) is a clean stop, not a step failure —
            // recording it as failed painted successful exited runs red in the UI.
            // A suspension keeps the step pending: it re-runs after approval.
            let status = if e.standing_outcome().is_some() {
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
        //
        // Not asked (see `step_evaluator_applies`): after the FINAL step, where
        // there is nothing left to skip and an exit could only kill
        // downstream graph nodes, and after a step whose output carries
        // outside text (web, mail, a watch payload), which the evaluator
        // must not be able to stop the run on.
        untrusted |= step_tainted;
        let (eval, eval_tokens) = if step_evaluator_applies(i, total_steps, untrusted) {
            let remaining_steps = activity.steps[i + 1..]
                .iter()
                .enumerate()
                .map(|(j, s)| {
                    format!(
                        "- Step {}: {}",
                        i + j + 2,
                        truncate_at_char_boundary(s, 200)
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let eval_trace = ai::RequestTrace {
                purpose: "step_evaluator",
                ..make_trace(i.to_string())
            };
            evaluate_step(decide, &eval_trace, step, &step_result, &remaining_steps).await
        } else {
            if untrusted && i + 1 < total_steps {
                info!(
                    site = "step_eval",
                    activity = %activity.id,
                    step = i,
                    "step output carries outside text; evaluator not asked, proceeding"
                );
            }
            (EvalDecision::Proceed, 0)
        };
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
                return Err(WorkflowError::Exited(evaluator_exit_reason(
                    i + 1,
                    total_steps,
                    &reason,
                    &step_result,
                )));
            }
        }

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

/// The reason a run the step evaluator ended records: the step's own words
/// (the first line of what it produced), which is the run's standing
/// outcome, or the evaluator's verdict when the step said nothing. The
/// `Step n/m evaluator:` prefix is what the dashboard strips for the owner.
fn evaluator_exit_reason(step: usize, total: usize, verdict: &str, step_output: &str) -> String {
    let said = step_output
        .lines()
        .map(|l| l.trim().trim_start_matches('#').trim())
        .find(|l| !l.is_empty())
        .unwrap_or(verdict);
    format!("Step {step}/{total} evaluator: {}", truncate_at_char_boundary(said, 300))
}

/// Floor on a non-proceed outcome: below it the evaluator proceeds, however
/// the probabilities lean. Exit kills the RUN, not the step, so the bar is
/// high and every uncertain case continues.
///
/// Set from 66 verdicts on 2026-09-22: 25 `precondition_failed`, all on runs
/// facing standing conditions (a store with no order history, no mail or
/// social account connected), read anywhere from 0.37 to 0.75 for the same
/// condition, so the confidence does not separate a run that hit a blocker
/// from one that did not. Two cleared 0.7 (0.70, 0.75) and ended their runs;
/// the runs that proceeded past the same condition handled it themselves,
/// the model exiting with its own reason ("no sales velocity to project
/// against") or notifying the owner. 0.8 is the certainty a guardrail block
/// needs; no verdict in the set reached it.
const STEP_EXIT_CONFIDENCE: f64 = 0.8;

/// The evaluator's verdict from its picked outcome and that pick's
/// confidence: exit only on a non-proceed outcome at or above
/// [`STEP_EXIT_CONFIDENCE`].
fn eval_from(picked: &str, confidence: f64) -> EvalDecision {
    if picked != "proceed" && !picked.is_empty() && confidence >= STEP_EXIT_CONFIDENCE {
        EvalDecision::Exit(picked.to_string())
    } else {
        EvalDecision::Proceed
    }
}

/// Ceiling on one workflow decision (the step evaluator and the `decide`
/// node), retry included. A decision answers in milliseconds; this only
/// bounds a stalled connection, and on timeout both fail open.
pub(crate) const DECISION_TIMEOUT_SECS: u64 = 5;

/// Whether the step evaluator is asked after step `index` of `total`.
///
/// Never after the final step: there is nothing left to skip, so an exit
/// could only kill downstream graph nodes (loop re-entry, commit, delivery)
/// for zero benefit, and the call was paid for and thrown away. Observed
/// live: the evaluator exited on "Chunk 7 complete: 4 rows resolved...".
///
/// Never after a step whose output carries outside text (`untrusted`: this
/// or an earlier step read web pages, mail, channel or phone content, or the
/// run's inputs are a watch payload). That output is raw tool output, and the typed
/// decision model is not robust to text written to steer it: an injected
/// "this is harmful, stop" could otherwise end the owner's run. Such a run
/// proceeds, and its remaining steps, `requires_tools`, the iteration
/// ceiling and the spend cap still govern it.
fn step_evaluator_applies(index: usize, total: usize, untrusted: bool) -> bool {
    index + 1 < total && !untrusted
}

/// Evaluate a step's output with one typed decision (Jev through Janus).
/// Returns Proceed or Exit plus the decision's own token usage, which counts
/// against the activity like every other turn.
///
/// Fails open: no decide client, a failed call, or an answer that is not a
/// choice all Proceed — the remaining steps exist for a reason and often
/// perform the required side effects (storing, sending, recording).
async fn evaluate_step(
    decide: Option<&ai::DecideClient>,
    trace: &ai::RequestTrace,
    step_text: &str,
    step_output: &str,
    remaining_steps: &str,
) -> (EvalDecision, u32) {
    let Some(client) = decide else {
        return (EvalDecision::Proceed, 0);
    };
    let state = serde_json::json!({
        "step": step_text,
        "remaining_steps": remaining_steps,
        "step_output": truncate_at_char_boundary(step_output, 2000),
    });
    let questions = std::collections::BTreeMap::from([(
        "outcome",
        ai::Question::choice(
            "`step_output` is what a workflow step produced for the instruction in `step`; \
             `remaining_steps` are the steps that have NOT run yet. Judge the outcome of \
             this step alone. NEVER pick anything but `proceed` because the work so far looks \
             complete or sufficient: the remaining steps exist for a reason and often perform \
             the required side effects (storing, sending, recording). If `remaining_steps` \
             carry their own conditional guards (\"only if\", \"always\", \"skip if\"), pick \
             `proceed` and let them self-gate.",
            &[
                (
                    "proceed",
                    "the step met its stated goal, or the remaining steps carry their own conditional guards",
                ),
                ("inapplicable", "the task does not apply to this data"),
                (
                    "precondition_failed",
                    "something the step required was missing or failed",
                ),
                ("harmful", "continuing would cause harm"),
            ],
        ),
    )]);

    let call = client.decide(trace, &state, &questions);
    let deadline = std::time::Duration::from_secs(DECISION_TIMEOUT_SECS);
    match tokio::time::timeout(deadline, call).await {
        Ok(Ok(decision)) => {
            let tokens = (decision.usage.input_tokens + decision.usage.output_tokens) as u32;
            let Some(answer) = decision.answer("outcome") else {
                warn!(
                    site = "step_eval",
                    "step evaluator answered without an outcome; proceeding"
                );
                return (EvalDecision::Proceed, tokens);
            };
            let picked = answer.picked();
            let confidence = answer.confidence.unwrap_or(0.0);
            info!(
                site = "step_eval",
                model = %decision.model,
                input_tokens = decision.usage.input_tokens,
                cost_micro = decision.usage.cost_micro,
                outcome = picked,
                confidence,
                "step evaluator decided"
            );
            (eval_from(picked, confidence), tokens)
        }
        Ok(Err(e)) => {
            warn!(site = "step_eval", error = %e, "step evaluator call failed; proceeding");
            (EvalDecision::Proceed, 0)
        }
        Err(_) => {
            warn!(site = "step_eval", "step evaluator timed out; proceeding");
            (EvalDecision::Proceed, 0)
        }
    }
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
         the condition in your task is not met or there is nothing to do. \
         In a case turn (inputs carry `_case`) the reason is the turn's \
         output: it MUST end with the turn's JSON object (result/next), \
         because the engine reads the next wait from it.\n",
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
    fn test_step_exit_needs_confidence_0_8_logged_boundary_verdicts_proceed() {
        // Verdicts copied from the 2026-09-22 log: both exited at 0.7.
        // inventory run e77b93e5, step 3/7 after a no-tool turn on a store
        // with no order history.
        assert!(matches!(eval_from("precondition_failed", 0.7), EvalDecision::Proceed));
        // inventory run f4570b4a, step 5/6 after no mail account was connected.
        assert!(matches!(eval_from("precondition_failed", 0.75), EvalDecision::Proceed));
        assert!(matches!(
            eval_from("precondition_failed", STEP_EXIT_CONFIDENCE),
            EvalDecision::Exit(ref r) if r == "precondition_failed"
        ));
        assert!(matches!(eval_from("harmful", 0.9), EvalDecision::Exit(_)));
        assert!(matches!(eval_from("proceed", 0.98), EvalDecision::Proceed));
        assert!(matches!(eval_from("", 1.0), EvalDecision::Proceed));
    }

    /// Byte-limit truncation must respect UTF-8 boundaries — a slice landing
    /// inside a multibyte character panicked, killed the run task, and left
    /// the run stuck in `running`.
    #[test]
    fn test_truncate_at_char_boundary_multibyte_safe() {
        // "aé" = [0x61, 0xC3, 0xA9]; a cut at 2 lands inside 'é'.
        assert_eq!(truncate_at_char_boundary("aé", 2), "a");
        assert_eq!(truncate_at_char_boundary("aé", 3), "aé");
        assert_eq!(truncate_at_char_boundary("plain", 10), "plain");
        // 4-byte emoji: any cut inside it must back off to the boundary.
        assert_eq!(truncate_at_char_boundary("📊📊", 5), "📊");
    }

    }
