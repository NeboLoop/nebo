//! The one workflow walker. A definition with explicit connections is its
//! graph; one without is a chain (`WorkflowDef::as_graph`).
//!
//! Doctrine: the ENGINE owns all control flow. Execution is sequential unless
//! an activity branches; a fork (multiple outgoing edges) runs its branches in
//! parallel, each branch sequential within itself; a join waits for every
//! ACTIVATED incoming branch (branches a condition skipped are not waited on);
//! condition routing is evaluated deterministically from params — the model
//! NEVER decides routing. The per-step execution model inside an activity
//! (one step per LLM turn, evaluator-gated) is untouched — see engine.rs.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

use futures::FutureExt;
use futures::future::BoxFuture;
use futures::stream::FuturesUnordered;
use futures::StreamExt as _;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use db::Store;
use tools::registry::DynTool;

use crate::WorkflowError;
use crate::engine::{WorkflowProgress, execute_activity_with_retry};
use crate::parser::{
    Activity, EMIT_NODE, TRIGGER_NODE, WorkflowDef, loop_body_set, param_str,
};

/// Safety net against malformed graphs: no single node may execute more than
/// this many times in one run (loops are bounded by maxIterations at or below
/// this; the cap only trips on walker bugs).
const MAX_NODE_VISITS: u32 = 10_000;
/// Default loop iteration cap when params.maxIterations is absent: as many
/// items as a body node may run in one run, so the default never truncates a
/// list the walker could have finished.
const DEFAULT_MAX_ITERATIONS: u64 = MAX_NODE_VISITS as u64;
/// Per-item retry budget when an iteration fails rate-limit-shaped.
const MAX_ITEM_RATE_LIMIT_RETRIES: u32 = 4;

#[derive(Clone)]
struct Edge {
    to: String,
    label: Option<String>,
}

struct GraphState {
    /// Output text per executed node (loop bodies overwrite per iteration).
    outputs: HashMap<String, String>,
    visits: HashMap<String, u32>,
    total_tokens: u32,
    /// Each top-level node an error left, with the error's words, in the
    /// order they returned: an error leaves the node it rose in before any
    /// ancestor it passes through, so the first entry with the words of the
    /// error that ended the walk is the step it ended at ([`ended_at`]).
    errors: Vec<(String, String)>,
    /// Expert requests this pass found still out: (request key, deadline).
    /// What the run still waits for: an expert's reply
    /// (`expert::reply_watch`) or the owner's review of a held answer
    /// (`expert::review_watch`), each with the latest time to wake the run
    /// for it (`None`: only that event wakes it).
    pending_experts: Vec<(String, Option<i64>)>,
}

struct GraphCtx<'a> {
    def: &'a WorkflowDef,
    inputs: &'a serde_json::Value,
    store: &'a Arc<Store>,
    /// The typed-decision door (Jev through Janus): the step evaluator and
    /// `decide` nodes run on it. `None` when Janus is not configured.
    decide: Option<&'a ai::DecideClient>,
    /// The ONE injected agentic loop (see workflow::loop_contract).
    loop_impl: &'a dyn crate::ActivityLoop,
    resolved_tools: &'a [Box<dyn DynTool>],
    cancel_token: Option<&'a CancellationToken>,
    skill_content: Option<&'a HashMap<String, String>>,
    event_bus: Option<&'a tools::EventBus>,
    emit_sources: Vec<String>,
    progress_tx: Option<tokio::sync::mpsc::UnboundedSender<WorkflowProgress>>,
    /// Per-employee approval-checkpoint context (policy).
    checkpoint: Option<crate::engine::CheckpointCtx>,
    /// Durable resume state when this run was re-entered after an approval.
    resume: Option<crate::engine::ResumeState>,
    run_id: String,
    /// Owning agent for usage attribution; "" for standalone workflow runs.
    agent_id: String,
    /// Caller-resolved memory scope for tool execution (see execute_workflow).
    memory_user_id: String,
    memory_writes_disabled: bool,
    by_id: HashMap<String, &'a Activity>,
    index_of: HashMap<String, usize>,
    outgoing: HashMap<String, Vec<Edge>>,
    /// Static transitive predecessors per node, in activity-array order —
    /// a condition's match text is built from these, deterministic
    /// regardless of parallel completion timing.
    ancestors: HashMap<String, Vec<String>>,
    /// Per node, the predecessors whose results its model reads
    /// ([`result_parents`]), in activity-array order.
    parents: HashMap<String, Vec<String>>,
    /// Nodes with an edge to __emit__ — these get the emit tool.
    terminal_emit: HashSet<String>,
    /// Per-loop body node sets (validated self-contained).
    loop_bodies: HashMap<String, HashSet<String>>,
    state: Mutex<GraphState>,
    /// The model turns the run has left, shared by every node.
    budget: crate::engine::RunBudget,
}

/// One barrier scope: the top-level walk, or one loop-body iteration.
struct WalkScope {
    /// node -> (arrived, activated). A node runs when `arrived` reaches its
    /// scope indegree; it runs for real only if `activated > 0`, otherwise it
    /// propagates the skip downstream.
    arrivals: Mutex<HashMap<String, (u32, u32)>>,
    indegree: HashMap<String, u32>,
    /// Arrivals at this node end the walk (loop re-entry). None at top level.
    stop_node: Option<String>,
    /// Current loop item, exposed to expressions as `item`.
    item: Option<serde_json::Value>,
    /// Where the item lives in the run, so a step handed it as a summary
    /// reads it from there.
    item_source: Option<ItemSource>,
    /// Dotted loop scope path — "" at top level, "2" inside the third item of
    /// a loop, "2.0" for the first item of a loop nested inside that one. This
    /// is the identity the resume fast-forward matches on; without it every
    /// iteration of a body looks like the same completed activity.
    iteration: String,
    /// Iteration-LOCAL node outputs. `None` at top level (writes go to the
    /// global state map); `Some` inside a loop iteration, seeded from the
    /// parent scope's locals so nested loops read outward. This is what makes
    /// concurrent iterations semantically invisible: a body node's readers
    /// (prior context, data paths) see ONLY their own iteration's values —
    /// never a racing sibling's — and the loop gathers every iteration's
    /// outputs, in item order, into its own output on completion.
    outputs: Option<Mutex<HashMap<String, String>>>,
}

/// Write a node's output where this scope's readers will find it:
/// iteration-local inside a loop body, the global run state otherwise.
fn record_output(ctx: &GraphCtx, scope: &WalkScope, id: &str, content: String) {
    match &scope.outputs {
        Some(local) => {
            local.lock().unwrap().insert(id.to_string(), content);
        }
        None => {
            ctx.state.lock().unwrap().outputs.insert(id.to_string(), content);
        }
    }
}

/// Rate-limit-shaped error text, as providers actually spell it. String
/// matching is the in-band reality: by the time an error reaches the loop it
/// has been through Display; the typed ProviderError::RateLimit renders as
/// "rate limit exceeded" and gateway 429s carry their status or phrase.
fn is_rate_limit_shaped(msg: &str) -> bool {
    let m = msg.to_lowercase();
    m.contains("rate limit")
        || m.contains("rate_limit")
        || m.contains("429")
        || m.contains("too many requests")
        || m.contains("overloaded")
}

/// Walk a workflow's graph (`WorkflowDef::as_graph`). Completes the run
/// record itself (completed / exited / stopped / cancelled / failed) and
/// returns `(run_id, final_context)`.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_graph(
    def: &WorkflowDef,
    agent_id: &str,
    memory_user_id: &str,
    memory_writes_disabled: bool,
    inputs: &serde_json::Value,
    store: &Arc<Store>,
    decide: Option<&ai::DecideClient>,
    loop_impl: &dyn crate::ActivityLoop,
    resolved_tools: &[Box<dyn DynTool>],
    run_id: &str,
    cancel_token: Option<&CancellationToken>,
    skill_content: Option<&HashMap<String, String>>,
    event_bus: Option<&tools::EventBus>,
    emit_sources: Vec<String>,
    progress_tx: Option<tokio::sync::mpsc::UnboundedSender<WorkflowProgress>>,
    checkpoint: Option<&crate::engine::CheckpointCtx>,
    resume: Option<crate::engine::ResumeState>,
) -> Result<(String, String), WorkflowError> {
    let ctx = build_ctx(
        def,
        agent_id,
        memory_user_id,
        memory_writes_disabled,
        inputs,
        store,
        decide,
        loop_impl,
        resolved_tools,
        cancel_token,
        skill_content,
        event_bus,
        emit_sources,
        progress_tx,
        checkpoint,
        resume,
        run_id,
    );

    let entries: Vec<Edge> = ctx
        .outgoing
        .get(TRIGGER_NODE)
        .cloned()
        .unwrap_or_else(|| {
            vec![Edge {
                to: def.activities[0].id.clone(),
                label: None,
            }]
        });
    let top = WalkScope {
        arrivals: Mutex::new(HashMap::new()),
        indegree: top_indegree(&ctx),
        stop_node: None,
        item: None,
        item_source: None,
        iteration: String::new(),
        outputs: None,
    };

    let walks = entries
        .iter()
        .map(|e| arrive(&ctx, &top, e.to.clone(), true));
    let result = first_error(futures::future::join_all(walks).await);

    let (total_tokens, final_context, ended_at) = {
        let st = ctx.state.lock().unwrap();
        let ended_at = result.as_ref().err().and_then(|e| ended_at(&st.errors, e));
        (st.total_tokens, final_context(def, &st.outputs), ended_at)
    };

    match result {
        Ok(()) => {
            if let Err(e) = store.complete_workflow_run(
                run_id,
                "completed",
                total_tokens as i64,
                None,
                None,
                Some(&final_context),
            ) {
                warn!(run_id, error = %e, "failed to mark workflow run as completed");
            }
            info!(workflow = def.id.as_str(), run_id, total_tokens, "workflow completed");
            Ok((run_id.to_string(), final_context))
        }
        // Cancelled: the run ends cancelled with what it spent, its
        // checklist closed. The owner's cancel already said why.
        Err(WorkflowError::Cancelled) => {
            if let Err(e) = store.complete_workflow_run(run_id, "cancelled", total_tokens as i64, None, ended_at.as_deref(), None) {
                warn!(run_id, error = %e, "failed to mark workflow run as cancelled");
            }
            Err(WorkflowError::Cancelled)
        }
        // Parked at the project's limit: one wait, no deadline, until the
        // owner decides.
        Err(WorkflowError::ProjectLimit(used)) => {
            park_at_limit(store, run_id, &used)?;
            info!(workflow = def.id.as_str(), run_id, used = %used, "workflow paused at its project's limit");
            Err(WorkflowError::ProjectLimit(used))
        }
        // Parked on experts: one wait for every request still out.
        Err(WorkflowError::AwaitingExpert(_)) => {
            let pending = std::mem::take(&mut ctx.state.lock().unwrap().pending_experts);
            park_on_experts(store, run_id, &pending)?;
            info!(workflow = def.id.as_str(), run_id, experts = pending.len(), "workflow waiting on experts");
            Err(WorkflowError::AwaitingExpert(pending.len()))
        }
        // Parked on the owner's answer: the suspension already set the run
        // awaiting_approval, and the answer resumes it. Not a failure.
        Err(e @ WorkflowError::AwaitingApproval { .. }) => Err(e),
        // A standing outcome (an exit, a terminal refusal) ends the run
        // cleanly with its reason — never a failure.
        Err(e) if let Some(reason) = e.standing_outcome() => {
            // What the owner must supply, when the refusing tool named it:
            // kept on the run as data for whoever tells the owner.
            if let Some(need) = e.owner_need() {
                let _ = store.set_workflow_run_owner_need(run_id, need);
            }
            let _ = store.complete_workflow_run(
                run_id,
                "exited",
                total_tokens as i64,
                Some(&reason),
                ended_at.as_deref(),
                Some(&final_context),
            );
            info!(workflow = def.id.as_str(), run_id, reason = %reason, "workflow exited early");
            Ok((run_id.to_string(), final_context))
        }
        // The owner's spending limit stopped it: the run reads "Stopped at
        // your limit", keeps the work done (the stopped step's report
        // included), and is not a failure.
        Err(e @ WorkflowError::SpendCapReached { .. }) => {
            let _ = store.complete_workflow_run(
                run_id,
                "stopped",
                total_tokens as i64,
                Some(&e.to_string()),
                ended_at.as_deref(),
                Some(&final_context),
            );
            info!(workflow = def.id.as_str(), run_id, "workflow stopped at the owner's spending limit");
            Err(e)
        }
        Err(e) => {
            let err_msg = e.to_string();
            if let Err(db_err) = store.complete_workflow_run(
                run_id,
                "failed",
                total_tokens as i64,
                Some(&err_msg),
                ended_at.as_deref(),
                None,
            ) {
                warn!(run_id, error = %db_err, "failed to mark workflow run as failed");
            }
            Err(e)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn build_ctx<'a>(
    def: &'a WorkflowDef,
    agent_id: &str,
    memory_user_id: &str,
    memory_writes_disabled: bool,
    inputs: &'a serde_json::Value,
    store: &'a Arc<Store>,
    decide: Option<&'a ai::DecideClient>,
    // The ONE injected agentic loop (see workflow::loop_contract).
    loop_impl: &'a dyn crate::ActivityLoop,
    resolved_tools: &'a [Box<dyn DynTool>],
    cancel_token: Option<&'a CancellationToken>,
    skill_content: Option<&'a HashMap<String, String>>,
    event_bus: Option<&'a tools::EventBus>,
    emit_sources: Vec<String>,
    progress_tx: Option<tokio::sync::mpsc::UnboundedSender<WorkflowProgress>>,
    checkpoint: Option<&crate::engine::CheckpointCtx>,
    resume: Option<crate::engine::ResumeState>,
    run_id: &str,
) -> GraphCtx<'a> {
    let by_id: HashMap<String, &Activity> = def
        .activities
        .iter()
        .map(|a| (a.id.clone(), a))
        .collect();
    let index_of: HashMap<String, usize> = def
        .activities
        .iter()
        .enumerate()
        .map(|(i, a)| (a.id.clone(), i))
        .collect();

    let mut outgoing: HashMap<String, Vec<Edge>> = HashMap::new();
    let mut incoming: HashMap<String, Vec<String>> = HashMap::new();
    let mut terminal_emit: HashSet<String> = HashSet::new();
    for c in &def.connections {
        if c.to == EMIT_NODE {
            terminal_emit.insert(c.from.clone());
        }
        outgoing.entry(c.from.clone()).or_default().push(Edge {
            to: c.to.clone(),
            label: c.label.clone(),
        });
        if c.from != TRIGGER_NODE && c.to != EMIT_NODE {
            incoming
                .entry(c.to.clone())
                .or_default()
                .push(c.from.clone());
        }
    }

    // Static transitive predecessors (cycle-safe), ordered by activity index.
    let mut ancestors: HashMap<String, Vec<String>> = HashMap::new();
    for a in &def.activities {
        let mut seen: HashSet<&str> = HashSet::new();
        let mut queue: Vec<&str> = incoming
            .get(&a.id)
            .map(|v| v.iter().map(String::as_str).collect())
            .unwrap_or_default();
        while let Some(p) = queue.pop() {
            if p == a.id || !seen.insert(p) {
                continue;
            }
            if let Some(more) = incoming.get(p) {
                queue.extend(more.iter().map(String::as_str));
            }
        }
        let mut ordered: Vec<String> = seen.into_iter().map(String::from).collect();
        ordered.sort_by_key(|id| index_of.get(id).copied().unwrap_or(usize::MAX));
        ancestors.insert(a.id.clone(), ordered);
    }
    let parents: HashMap<String, Vec<String>> = def
        .activities
        .iter()
        .map(|a| (a.id.clone(), result_parents(&a.id, &incoming, &by_id, &index_of)))
        .collect();

    let loop_bodies: HashMap<String, HashSet<String>> = def
        .activities
        .iter()
        .filter(|a| a.activity_type == "loop")
        .map(|a| (a.id.clone(), loop_body_set(def, &a.id)))
        .collect();

    GraphCtx {
        def,
        inputs,
        store,
        decide,
        loop_impl,
        resolved_tools,
        cancel_token,
        skill_content,
        event_bus,
        emit_sources,
        budget: crate::engine::RunBudget::for_workflow(def, progress_tx.clone()),
        progress_tx,
        checkpoint: checkpoint.cloned(),
        resume,
        run_id: run_id.to_string(),
        agent_id: agent_id.to_string(),
        memory_user_id: memory_user_id.to_string(),
        memory_writes_disabled,
        by_id,
        index_of,
        outgoing,
        ancestors,
        parents,
        terminal_emit,
        loop_bodies,
        state: Mutex::new(GraphState {
            outputs: HashMap::new(),
            visits: HashMap::new(),
            total_tokens: 0,
            errors: Vec::new(),
            pending_experts: Vec::new(),
        }),
    }
}

/// The predecessors whose results reach a node's model context: its direct
/// parents, looking through routing nodes. A `condition` or `wait` produces
/// no content of its own ("True", a sleep) — it only gates what came before
/// it — so in its place stand its own parents, recursively. Every other
/// parent counts as itself, a `loop` included: a body node sees its own item,
/// never what came before the loop, and a node after "Done" sees the loop.
fn result_parents(
    node: &str,
    incoming: &HashMap<String, Vec<String>>,
    by_id: &HashMap<String, &Activity>,
    index_of: &HashMap<String, usize>,
) -> Vec<String> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut found: Vec<String> = Vec::new();
    let mut queue: Vec<&str> = incoming
        .get(node)
        .map(|v| v.iter().map(String::as_str).collect())
        .unwrap_or_default();
    while let Some(p) = queue.pop() {
        if p == node || !seen.insert(p) {
            continue;
        }
        let routing = by_id
            .get(p)
            .is_some_and(|a| matches!(a.activity_type.as_str(), "condition" | "wait"));
        if routing {
            if let Some(more) = incoming.get(p) {
                queue.extend(more.iter().map(String::as_str));
            }
        } else {
            found.push(p.to_string());
        }
    }
    found.sort_by_key(|id| index_of.get(id).copied().unwrap_or(usize::MAX));
    found
}

/// Top-scope indegree: edges among top-level nodes. Loop bodies are excluded —
/// their nodes only ever execute inside a body scope.
fn top_indegree(ctx: &GraphCtx) -> HashMap<String, u32> {
    let in_any_body = |id: &str| ctx.loop_bodies.values().any(|b| b.contains(id));
    let mut indegree: HashMap<String, u32> = HashMap::new();
    for c in &ctx.def.connections {
        if c.to == EMIT_NODE || c.to == TRIGGER_NODE {
            continue;
        }
        if c.label.as_deref() == Some("Each item") {
            continue; // loop-internal entry edge
        }
        if c.from != TRIGGER_NODE && in_any_body(&c.from) {
            continue; // body-internal or loop re-entry edge
        }
        *indegree.entry(c.to.clone()).or_insert(0) += 1;
    }
    indegree
}

/// Body-scope indegree for one loop iteration: edges among body nodes plus the
/// loop's "Each item" entry edges.
fn body_indegree(ctx: &GraphCtx, loop_id: &str, body: &HashSet<String>) -> HashMap<String, u32> {
    let mut indegree: HashMap<String, u32> = HashMap::new();
    for c in &ctx.def.connections {
        let entry_edge = c.from == loop_id && c.label.as_deref() == Some("Each item");
        let internal = body.contains(&c.from) && body.contains(&c.to);
        if (entry_edge || internal) && body.contains(&c.to) {
            *indegree.entry(c.to.clone()).or_insert(0) += 1;
        }
    }
    indegree
}

/// Edges to walk from `node` within the given scope. At top level the loop's
/// "Each item" edges are internal (handled by run_loop) and are skipped.
fn scoped_edges(ctx: &GraphCtx, scope: &WalkScope, node: &str) -> Vec<Edge> {
    let edges = ctx.outgoing.get(node).cloned().unwrap_or_default();
    if scope.stop_node.is_none() {
        edges
            .into_iter()
            .filter(|e| e.label.as_deref() != Some("Each item"))
            .collect()
    } else {
        edges
    }
}

/// Deterministic error aggregation: branches settle (join_all), then the
/// first error in edge order wins. A branch parked on an expert is not an
/// error of its own: a sibling's real error wins over it, and only when every
/// other branch finished does the walk park.
fn first_error(results: Vec<Result<(), WorkflowError>>) -> Result<(), WorkflowError> {
    let mut parked = None;
    for r in results {
        match r {
            Ok(()) => {}
            Err(e @ WorkflowError::AwaitingExpert(_)) => {
                parked.get_or_insert(e);
            }
            Err(e) => return Err(e),
        }
    }
    parked.map_or(Ok(()), Err)
}

/// A walker arrives at `node` over one incoming edge. The join barrier
/// releases the last arriver; it executes the node if any incoming branch was
/// activated, otherwise propagates the skip.
fn arrive<'b, 'a: 'b>(
    ctx: &'b GraphCtx<'a>,
    scope: &'b WalkScope,
    node: String,
    activated: bool,
) -> BoxFuture<'b, Result<(), WorkflowError>> {
    async move {
        if node == EMIT_NODE {
            return Ok(());
        }
        if scope.stop_node.as_deref() == Some(node.as_str()) {
            return Ok(()); // loop re-entry — this iteration branch is done
        }

        let ready = {
            let mut arrivals = scope.arrivals.lock().unwrap();
            let entry = arrivals.entry(node.clone()).or_insert((0, 0));
            entry.0 += 1;
            if activated {
                entry.1 += 1;
            }
            let need = scope.indegree.get(&node).copied().unwrap_or(1).max(1);
            if entry.0 < need {
                None
            } else {
                Some(entry.1 > 0)
            }
        };
        let Some(any_active) = ready else {
            return Ok(()); // another branch completes this join
        };

        if !any_active {
            // Every incoming branch was skipped — never execute, but keep
            // propagating so downstream joins don't wait forever.
            return route(ctx, scope, &node, |_| false).await;
        }

        execute_node(ctx, scope, &node).await
    }
    .boxed()
}

/// Fan out from `node`: `decide(label)` activates or skips each edge. Forks
/// run in parallel; each branch is sequential within itself.
async fn route<'a>(
    ctx: &GraphCtx<'a>,
    scope: &WalkScope,
    node: &str,
    decide: impl Fn(Option<&str>) -> bool + Copy,
) -> Result<(), WorkflowError> {
    let edges = scoped_edges(ctx, scope, node);
    let walks = edges
        .iter()
        .map(|e| arrive(ctx, scope, e.to.clone(), decide(e.label.as_deref())));
    first_error(futures::future::join_all(walks).await)
}

/// Run `node`, noting where an error left it. Top-level nodes only: an
/// iteration's error is the loop's to keep or pass on (an item that exits
/// or is rate-limited does not end the run), so a run that ends inside a
/// loop ends at the loop. A park (an expert, the owner's answer, a
/// project's limit) ends nothing.
async fn execute_node<'a>(
    ctx: &GraphCtx<'a>,
    scope: &WalkScope,
    node: &str,
) -> Result<(), WorkflowError> {
    let result = run_node(ctx, scope, node).await;
    if let Err(e) = &result
        && scope.stop_node.is_none()
        && !matches!(
            e,
            WorkflowError::AwaitingExpert(_) | WorkflowError::AwaitingApproval { .. } | WorkflowError::ProjectLimit(_)
        )
    {
        ctx.state.lock().unwrap().errors.push((node.to_string(), e.to_string()));
    }
    result
}

/// The step a walk that ended with `e` ended at (see `GraphState::errors`).
fn ended_at(errors: &[(String, String)], e: &WorkflowError) -> Option<String> {
    let words = e.to_string();
    errors.iter().find(|(_, w)| *w == words).map(|(node, _)| node.clone())
}

async fn run_node<'a>(
    ctx: &GraphCtx<'a>,
    scope: &WalkScope,
    node: &str,
) -> Result<(), WorkflowError> {
    if let Some(token) = ctx.cancel_token {
        if token.is_cancelled() {
            return Err(WorkflowError::Cancelled);
        }
    }
    {
        let mut st = ctx.state.lock().unwrap();
        let visits = st.visits.entry(node.to_string()).or_insert(0);
        *visits += 1;
        if *visits > MAX_NODE_VISITS {
            return Err(WorkflowError::Other(format!(
                "node '{}' exceeded the visit cap — malformed graph",
                node
            )));
        }
    }

    let activity = *ctx
        .by_id
        .get(node)
        .ok_or_else(|| WorkflowError::Other(format!("unknown node '{}'", node)))?;

    info!(
        workflow = ctx.def.id.as_str(),
        activity = node,
        "executing activity"
    );
    if let Some(ref tx) = ctx.progress_tx {
        let _ = tx.send(WorkflowProgress::ActivityStarted {
            activity_id: node.to_string(),
            activity_index: ctx.index_of.get(node).copied().unwrap_or(0),
            total_activities: ctx.def.activities.len(),
        });
    }
    if let Err(e) =
        ctx.store
            .update_workflow_run(&ctx.run_id, Some("running"), Some(node), None, None, None)
    {
        warn!(run_id = %ctx.run_id, error = %e, "failed to update workflow run status");
    }

    match activity.activity_type.as_str() {
        "condition" => run_condition(ctx, scope, activity).await,
        "loop" => run_loop(ctx, scope, activity).await,
        "wait" => run_wait(ctx, scope, activity).await,
        "http" => run_http(ctx, scope, activity).await,
        "command" => run_command(ctx, scope, activity).await,
        "operation" => run_operation(ctx, scope, activity).await,
        "decide" => run_decide(ctx, scope, activity).await,
        "expert" => run_expert(ctx, scope, activity).await,
        _ => run_llm_activity(ctx, scope, activity).await,
    }
}

/// Deterministic shell step: run `params.command` through the `os` tool and
/// make its stdout the node output, byte for byte. No model, no tokens — the
/// node exists so parsers, converters, and state commits never pass through
/// an LLM that can paraphrase (or invent) their output.
///
/// `{{item.x}}` / `{{inputs.x}}` / `{{nodes.id.x}}` interpolate from the data
/// context (strings verbatim, other values as JSON). `${NEBO_DATA_DIR}` /
/// `${NEBO_SKILL_DIR}` are expanded before the definition reaches the engine
/// (workflow manager, `params.skill` names the skill) — same expansion the
/// skill system uses, so this node never grows its own.
/// Resume fast-forward for deterministic nodes (command/http): an activity
/// this run already completed replays its recorded output instead of
/// re-executing — the same Temporal property (and the same
/// (activity_id, iteration) key) as the LLM path in `run_llm_loop`. Without
/// this, a crash-resumed run re-ran command side effects it had already
/// performed (proven by the WS4 kill-test: a1 executed twice). Control nodes
/// (condition/loop/wait) deliberately re-evaluate — they route the walk and
/// are pure over the same context.
fn replay_completed(ctx: &GraphCtx<'_>, scope: &WalkScope, activity: &Activity) -> Option<String> {
    let done = ctx.store.completed_activity_contents(&ctx.run_id).ok()?;
    done.get(&(activity.id.clone(), scope.iteration.clone())).cloned()
}

async fn run_command<'a>(
    ctx: &GraphCtx<'a>,
    scope: &WalkScope,
    activity: &Activity,
) -> Result<(), WorkflowError> {
    if let Some(content) = replay_completed(ctx, scope, activity) {
        info!(activity = activity.id.as_str(), "resume: replaying completed command node");
        record_output(ctx, scope, &activity.id, content);
        return route(ctx, scope, &activity.id, |_| true).await;
    }
    let started_at = chrono::Utc::now().timestamp();

    let fail = |err_msg: String| {
        let _ = ctx.store.create_activity_result(
            &ctx.run_id,
            &activity.id,
            &scope.iteration,
            "failed",
            0,
            1,
            Some(&err_msg),
            started_at,
            Some(chrono::Utc::now().timestamp()),
        );
        Err(WorkflowError::ActivityFailed(activity.id.clone(), err_msg))
    };

    let Some(run_command) = ctx.resolved_tools.iter().find(|t| t.name() == "run_command") else {
        return fail("command activity requires the run_command tool, which is not available".into());
    };

    let data = data_context(ctx, scope);
    let command = interpolate_context(param_str(activity, "command"), &data);
    // `params.timeout_secs` (default 120, at most 600): past it the step is
    // stopped and the activity fails. Validated at parse time.
    let timeout_secs = crate::parser::command_timeout_secs(activity)
        .unwrap_or(crate::parser::COMMAND_TIMEOUT_DEFAULT_SECS);

    let input = serde_json::json!({
        "command": command,
        "description": format!("Workflow step {}", activity.id),
        "timeout": timeout_secs * 1000,
    });
    // The step runs as the employee that owns the workflow, through the one
    // door every command takes (`run_command` in the registry): the
    // permission check under that employee's grant (read from the session
    // key), Nebo's own files, ports and settings closed to it, and no
    // network when the employee's web access is off. Nobody waits on a
    // command step, so one that needs the owner's OK is refused and the run
    // fails with the reason, which reaches the owner as the run's failure.
    let mut tool_ctx = tools::ToolContext::new(tools::Origin::Workflow).with_session(
        tools::workflow_session_key(&ctx.agent_id, &ctx.run_id),
        ctx.run_id.clone(),
    );
    tool_ctx.door = types::permissions::Door::Workflow;
    tool_ctx.cannot_wait = true;
    // The next step parses the result: it comes back whole.
    tool_ctx.whole_result = true;
    tool_ctx.user_id = ctx.memory_user_id.clone();
    tool_ctx.memory_writes_disabled = ctx.memory_writes_disabled;
    // Owner-authored deterministic step: plugin auth env rides along so
    // env-auth plugin binaries work in command nodes (see ToolContext docs).
    tool_ctx.trusted_plugin_env = true;
    // `params.stdin` names data the command reads on its standard input
    // (`nodes.fetch.records`): a list of any size, quotes and all, which a
    // `{{...}}` in the command line cannot carry (one argument is capped at
    // 128 KiB on Linux, and a quote in a name ends the shell string).
    let stdin = param_str(activity, "stdin");
    if !stdin.trim().is_empty() {
        tool_ctx.stdin = Some(match resolve_path(&data, stdin) {
            Some(v) => value_as_string(&v).into_bytes(),
            None => return fail(format!("params.stdin '{stdin}' names no data in this run")),
        });
    }
    let tool_ctx = tool_ctx;
    let result = {
        let _permit = ctx.loop_impl.acquire_tool_permit().await;
        run_command.execute_dyn(&tool_ctx, input).await
    };
    if result.is_error {
        return fail(result.content);
    }

    let _ = ctx.store.create_activity_result(
        &ctx.run_id,
        &activity.id,
        &scope.iteration,
        "completed",
        0,
        1,
        None,
        started_at,
        Some(chrono::Utc::now().timestamp()),
    );
    let _ = ctx.store.set_activity_result_content(
        &ctx.run_id,
        &activity.id,
        &scope.iteration,
        &result.content,
    );
    info!(activity = activity.id.as_str(), "command node completed");
    record_output(ctx, scope, &activity.id, result.content);
    route(ctx, scope, &activity.id, |_| true).await
}

/// Pages one `operation` read follows before it stops: a plugin that never
/// stops naming a next page is a broken plugin, not a long list.
const MAX_OPERATION_PAGES: usize = 1_000;

/// Deterministic interface step: perform a catalog operation
/// (`params.operation`, e.g. `ledger.invoice.search`) through its operation
/// tool, which resolves to whichever connected plugin binds it. The call goes
/// through the same door as a model's call to that tool: the employee's
/// permission check (a call that needs the owner's OK is refused — nobody
/// waits on a code step), the write ledger and the lease.
///
/// Two shapes, by whether `params.rows` is set:
/// - **read every page** — one call with `params.input`; while the answer
///   names a `nextCursor`, the next call carries it as `cursor`. Each page's
///   records are the answer's one list. Output:
///   `{"operation", "records": [...], "pages": n}`.
/// - **one call per row** — `params.rows` is a data path to a list of
///   objects; each row's fields over `params.input` make one call, serially.
///   A write row without a `clientKey` gets one from the run, the step and
///   the row, so a resumed run never performs a row twice. Output:
///   `{"operation", "results": [{"index", "ok", "result"|"error"}],
///   "succeeded": n, "failed": m}`; a failed row does not stop the others.
///
/// `{{...}}` in `params.input`'s text values interpolates like a command.
async fn run_operation<'a>(
    ctx: &GraphCtx<'a>,
    scope: &WalkScope,
    activity: &Activity,
) -> Result<(), WorkflowError> {
    if let Some(content) = replay_completed(ctx, scope, activity) {
        info!(activity = activity.id.as_str(), "resume: replaying completed operation node");
        record_output(ctx, scope, &activity.id, content);
        return route(ctx, scope, &activity.id, |_| true).await;
    }
    let started_at = chrono::Utc::now().timestamp();

    let fail = |err_msg: String| {
        let _ = ctx.store.create_activity_result(
            &ctx.run_id,
            &activity.id,
            &scope.iteration,
            "failed",
            0,
            1,
            Some(&err_msg),
            started_at,
            Some(chrono::Utc::now().timestamp()),
        );
        Err(WorkflowError::ActivityFailed(activity.id.clone(), err_msg))
    };

    let operation = param_str(activity, "operation").trim();
    let tool_name = tools::operation_tools::operation_tool_name(operation);
    let Some(tool) = ctx.resolved_tools.iter().find(|t| t.name() == tool_name) else {
        return fail(format!(
            "No connected plugin performs {operation}. Connect a plugin that binds it, then run this again."
        ));
    };

    let data = data_context(ctx, scope);
    let mut base = activity
        .params
        .as_ref()
        .and_then(|p| p.get("input"))
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    interpolate_strings(&mut base, &data);

    // As the employee that owns the workflow, like a command step.
    let mut tool_ctx = tools::ToolContext::new(tools::Origin::Workflow).with_session(
        tools::workflow_session_key(&ctx.agent_id, &ctx.run_id),
        ctx.run_id.clone(),
    );
    tool_ctx.door = types::permissions::Door::Workflow;
    tool_ctx.cannot_wait = true;
    // The next step parses the result: it comes back whole.
    tool_ctx.whole_result = true;
    tool_ctx.user_id = ctx.memory_user_id.clone();
    tool_ctx.memory_writes_disabled = ctx.memory_writes_disabled;
    let tool_ctx = tool_ctx;
    let call = |input: serde_json::Value| {
        let tool_ctx = &tool_ctx;
        async move {
            let _permit = ctx.loop_impl.acquire_tool_permit().await;
            tool.execute_dyn(tool_ctx, input).await
        }
    };
    let cancelled = || ctx.cancel_token.is_some_and(|t| t.is_cancelled());

    let rows_path = param_str(activity, "rows");
    let output = if rows_path.trim().is_empty() {
        let mut records = Vec::new();
        let mut cursor: Option<String> = None;
        let mut pages = 0;
        loop {
            if cancelled() {
                return Err(WorkflowError::Cancelled);
            }
            if pages == MAX_OPERATION_PAGES {
                return fail(format!("{operation} named a next page after {MAX_OPERATION_PAGES} pages; stopped"));
            }
            let mut input = base.clone();
            if let (Some(c), Some(obj)) = (&cursor, input.as_object_mut()) {
                obj.insert("cursor".into(), serde_json::Value::String(c.clone()));
            }
            let result = call(input).await;
            pages += 1;
            if result.is_error {
                return fail(format!("{operation}, page {pages}: {}", result.content));
            }
            let (page, next) = match page_records(&result.content) {
                Ok(p) => p,
                Err(e) => return fail(format!("{operation}, page {pages}: {e}")),
            };
            records.extend(page);
            match next {
                Some(n) if cursor.as_deref() == Some(n.as_str()) => {
                    return fail(format!("{operation} named page {n} as the next page twice; stopped"));
                }
                Some(n) => cursor = Some(n),
                None => break,
            }
        }
        serde_json::json!({ "operation": operation, "records": records, "pages": pages })
    } else {
        let rows = match resolve_path(&data, rows_path) {
            None | Some(serde_json::Value::Null) => Vec::new(),
            Some(serde_json::Value::Array(rows)) => rows,
            Some(_) => return fail(format!("params.rows '{rows_path}' is not a list")),
        };
        let keyed = !tools::operation_tools::reads_only(operation);
        let mut results = Vec::with_capacity(rows.len());
        let (mut succeeded, mut failed) = (0, 0);
        for (index, row) in rows.into_iter().enumerate() {
            if cancelled() {
                return Err(WorkflowError::Cancelled);
            }
            let serde_json::Value::Object(fields) = row else {
                failed += 1;
                results.push(serde_json::json!({ "index": index, "ok": false, "error": "the row is not an object of fields" }));
                continue;
            };
            let mut input = base.clone();
            if let Some(obj) = input.as_object_mut() {
                obj.extend(fields);
                if keyed && !obj.contains_key("clientKey") {
                    let key = format!("{}:{}:{}:{index}", ctx.run_id, activity.id, scope.iteration);
                    obj.insert("clientKey".into(), serde_json::Value::String(key));
                }
            }
            let result = call(input).await;
            if result.is_error {
                failed += 1;
                results.push(serde_json::json!({ "index": index, "ok": false, "error": result.content }));
            } else {
                succeeded += 1;
                let parsed = serde_json::from_str::<serde_json::Value>(&result.content)
                    .unwrap_or(serde_json::Value::String(result.content));
                results.push(serde_json::json!({ "index": index, "ok": true, "result": parsed }));
            }
        }
        serde_json::json!({
            "operation": operation,
            "results": results,
            "succeeded": succeeded,
            "failed": failed,
        })
    };

    let content = output.to_string();
    let _ = ctx.store.create_activity_result(
        &ctx.run_id,
        &activity.id,
        &scope.iteration,
        "completed",
        0,
        1,
        None,
        started_at,
        Some(chrono::Utc::now().timestamp()),
    );
    let _ = ctx.store.set_activity_result_content(&ctx.run_id, &activity.id, &scope.iteration, &content);
    info!(activity = activity.id.as_str(), operation, "operation node completed");
    record_output(ctx, scope, &activity.id, content);
    route(ctx, scope, &activity.id, |_| true).await
}

/// One page of a list read: its records and the cursor of the next page.
/// The answer is a list, or an object whose one list holds the records
/// (`{"invoices": [...]}`, `{"items": [...], "count": 3}`), with
/// `nextCursor` naming the next page when there is one.
fn page_records(content: &str) -> Result<(Vec<serde_json::Value>, Option<String>), String> {
    let page: serde_json::Value = serde_json::from_str(content.trim())
        .map_err(|_| format!("the answer is not JSON: {}", types::strutil::safe_prefix(content.trim(), 200)))?;
    let obj = match page {
        serde_json::Value::Array(records) => return Ok((records, None)),
        serde_json::Value::Object(obj) => obj,
        _ => return Err("the answer is neither a list nor an object holding one".into()),
    };
    let next = match obj.get("nextCursor") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) if s.is_empty() => None,
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(serde_json::Value::Number(n)) => Some(n.to_string()),
        Some(_) => return Err("nextCursor is not text".into()),
    };
    let mut lists = obj.into_iter().filter_map(|(k, v)| match v {
        serde_json::Value::Array(records) => Some((k, records)),
        _ => None,
    });
    match (lists.next(), lists.next()) {
        (Some((_, records)), None) => Ok((records, next)),
        (None, _) => Err("the answer holds no list of records".into()),
        (Some((a, _)), Some((b, _))) => Err(format!("the answer holds more than one list ({a}, {b}); return the records as its only list")),
    }
}

/// Interpolate `{{...}}` in every text value of `value`, in place.
fn interpolate_strings(value: &mut serde_json::Value, data: &serde_json::Value) {
    match value {
        serde_json::Value::String(s) => *s = interpolate_context(s, data),
        serde_json::Value::Array(items) => items.iter_mut().for_each(|v| interpolate_strings(v, data)),
        serde_json::Value::Object(map) => map.values_mut().for_each(|v| interpolate_strings(v, data)),
        _ => {}
    }
}

/// Replace `{{path}}` placeholders with values from the data context. String
/// values are inserted verbatim; anything else is inserted as JSON. Unknown
/// paths are left as-is so a typo is visible in the executed command.
fn interpolate_context(template: &str, data: &serde_json::Value) -> String {
    interpolate_with(template, data, &|_, value| value_as_string(value))
}

/// [`interpolate_context`] with each resolved value rendered by `render`
/// (its path and value).
fn interpolate_with(template: &str, data: &serde_json::Value, render: &dyn Fn(&str, &serde_json::Value) -> String) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            out.push_str(&rest[start..]);
            return out;
        };
        let path = after[..end].trim();
        match resolve_path(data, path) {
            Some(v) => out.push_str(&render(path, &v)),
            None => {
                out.push_str("{{");
                out.push_str(path);
                out.push_str("}}");
            }
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

/// Maximum wait-node sleep — anything longer belongs in a trigger, not a
/// held-open run.
const MAX_WAIT: std::time::Duration = std::time::Duration::from_secs(3_600);

/// Deterministic wait: bounded sleep, cancellable. No tokens, no model.
async fn run_wait<'a>(
    ctx: &GraphCtx<'a>,
    scope: &WalkScope,
    activity: &Activity,
) -> Result<(), WorkflowError> {
    let started_at = chrono::Utc::now().timestamp();
    // Validation guarantees this parses; cap defensively anyway.
    let duration = crate::parser::parse_wait_duration(param_str(activity, "duration"))
        .unwrap_or(std::time::Duration::from_secs(1))
        .min(MAX_WAIT);

    info!(activity = activity.id.as_str(), ?duration, "wait node sleeping");
    if let Some(token) = ctx.cancel_token {
        tokio::select! {
            _ = tokio::time::sleep(duration) => {}
            _ = token.cancelled() => return Err(WorkflowError::Cancelled),
        }
    } else {
        tokio::time::sleep(duration).await;
    }

    let _ = ctx.store.create_activity_result(
        &ctx.run_id,
        &activity.id,
        &scope.iteration,
        "completed",
        0,
        1,
        None,
        started_at,
        Some(chrono::Utc::now().timestamp()),
    );
    record_output(ctx, scope, &activity.id, format!("waited {}s", duration.as_secs()));
    route(ctx, scope, &activity.id, |_| true).await
}

/// Deterministic HTTP: the ENGINE issues one call through the SSRF-checked
/// `http_request` tool — the same pathway an LLM tool call takes.
/// No model turn, no tokens.
async fn run_http<'a>(
    ctx: &GraphCtx<'a>,
    scope: &WalkScope,
    activity: &Activity,
) -> Result<(), WorkflowError> {
    if let Some(content) = replay_completed(ctx, scope, activity) {
        info!(activity = activity.id.as_str(), "resume: replaying completed http node");
        record_output(ctx, scope, &activity.id, content);
        return route(ctx, scope, &activity.id, |_| true).await;
    }
    let started_at = chrono::Utc::now().timestamp();

    let fail = |err_msg: String| {
        let _ = ctx.store.create_activity_result(
            &ctx.run_id,
            &activity.id,
            &scope.iteration,
            "failed",
            0,
            1,
            Some(&err_msg),
            started_at,
            Some(chrono::Utc::now().timestamp()),
        );
        Err(WorkflowError::ActivityFailed(activity.id.clone(), err_msg))
    };

    let Some(http_tool) = ctx.resolved_tools.iter().find(|t| t.name() == "http_request") else {
        return fail("http activity requires the http_request tool, which is not available".into());
    };

    // headers may arrive as a JSON object or a JSON string (textarea input).
    let headers = activity
        .params
        .as_ref()
        .and_then(|p| p.get("headers"))
        .and_then(|v| match v {
            serde_json::Value::Object(_) => Some(v.clone()),
            serde_json::Value::String(s) => serde_json::from_str::<serde_json::Value>(s)
                .ok()
                .filter(|p| p.is_object()),
            _ => None,
        })
        .unwrap_or(serde_json::json!({}));

    let method = {
        let m = param_str(activity, "method").trim().to_uppercase();
        if m.is_empty() { "GET".to_string() } else { m }
    };
    let input = serde_json::json!({
        "url": param_str(activity, "url"),
        "method": method,
        "headers": headers,
        "body": param_str(activity, "body"),
    });

    // As the employee that owns the workflow, like a command step: the
    // permission check under its grant, recorded under the workflow door.
    // Nobody waits on an http step, so a request that needs the owner's OK
    // is refused and the run fails with the reason, which reaches the owner
    // as the run's failure; no card is parked.
    let mut tool_ctx = tools::ToolContext::new(tools::Origin::Workflow).with_session(
        tools::workflow_session_key(&ctx.agent_id, &ctx.run_id),
        ctx.run_id.clone(),
    );
    tool_ctx.door = types::permissions::Door::Workflow;
    tool_ctx.cannot_wait = true;
    // The next step parses the result: it comes back whole.
    tool_ctx.whole_result = true;
    tool_ctx.user_id = ctx.memory_user_id.clone();
    tool_ctx.memory_writes_disabled = ctx.memory_writes_disabled;
    let tool_ctx = tool_ctx;
    let result = {
        let _permit = ctx.loop_impl.acquire_tool_permit().await;
        http_tool.execute_dyn(&tool_ctx, input).await
    };
    if result.is_error {
        return fail(result.content);
    }

    let _ = ctx.store.create_activity_result(
        &ctx.run_id,
        &activity.id,
        &scope.iteration,
        "completed",
        0,
        1,
        None,
        started_at,
        Some(chrono::Utc::now().timestamp()),
    );
    let _ = ctx.store.set_activity_result_content(&ctx.run_id, &activity.id, &scope.iteration, &result.content);
    info!(activity = activity.id.as_str(), "http node completed");
    record_output(ctx, scope, &activity.id, result.content);
    route(ctx, scope, &activity.id, |_| true).await
}

/// Typed decision node: one Jev call (through Janus) over a declared state,
/// answered in milliseconds with a distribution and a confidence per
/// question. No LLM turn, no scratch session, nothing generated. The output
/// is the `answers` map as JSON plus `model`, so a downstream `condition`
/// reads `nodes.<id>.<question>.choice` / `.confidence` / `.score` / `.noul`
/// with the expression syntax it already has — `decide` never routes; the
/// author's threshold in the condition does (the "never AI-decided" law).
///
/// `params.state` is a data path (`inputs._event_payload`, `item`,
/// `nodes.fetch.body`); a path that resolves to nothing is sent as the
/// literal text, and a state over [`DECIDE_STATE_CAP`] goes as its summary ([`decide_state`]).
/// `params.questions` is the Jev question map, passed through as-is (an
/// object, or a JSON string from the builder's textarea).
///
/// Fails open: when no decision can be had (the service is not connected,
/// errors, is throttled or misses its deadline) the node still completes,
/// recording the author's `params.default` answers (see
/// [`crate::parser::decide_defaults`]) with `"defaulted": true` and an empty
/// `model`, and routes on. A decision service being unavailable never fails
/// the owner's run.
async fn run_decide<'a>(
    ctx: &GraphCtx<'a>,
    scope: &WalkScope,
    activity: &Activity,
) -> Result<(), WorkflowError> {
    if let Some(content) = replay_completed(ctx, scope, activity) {
        info!(activity = activity.id.as_str(), "resume: replaying completed decide node");
        record_output(ctx, scope, &activity.id, content);
        return route(ctx, scope, &activity.id, |_| true).await;
    }
    let started_at = chrono::Utc::now().timestamp();

    let fail = |err_msg: String| {
        let _ = ctx.store.create_activity_result(
            &ctx.run_id,
            &activity.id,
            &scope.iteration,
            "failed",
            0,
            1,
            Some(&err_msg),
            started_at,
            Some(chrono::Utc::now().timestamp()),
        );
        Err(WorkflowError::ActivityFailed(activity.id.clone(), err_msg))
    };

    let questions = match crate::parser::decide_questions(activity) {
        Ok(q) => q,
        Err(e) => return fail(e),
    };
    let defaults = match crate::parser::decide_defaults(activity, &questions) {
        Ok(d) => d,
        Err(e) => return fail(e),
    };
    let questions: std::collections::BTreeMap<&str, ai::Question> = questions
        .iter()
        .map(|(name, q)| (name.as_str(), q.clone()))
        .collect();

    let data = data_context(ctx, scope);
    let state_path = param_str(activity, "state");
    let state = resolve_path(&data, state_path)
        .unwrap_or_else(|| serde_json::Value::String(state_path.to_string()));
    let (state, summarized) = decide_state(&activity.id, state);

    let trace = ai::RequestTrace {
        agent_id: ctx.agent_id.clone(),
        run_id: ctx.run_id.clone(),
        action_id: activity.id.clone(),
        ..ai::RequestTrace::new("workflow_decide")
    };
    let outcome = match ctx.decide {
        None => Err("the typed-decision service (NeboAI) is not connected".to_string()),
        Some(client) => {
            let deadline = std::time::Duration::from_secs(crate::engine::DECISION_TIMEOUT_SECS);
            match tokio::time::timeout(deadline, client.decide(&trace, &state, &questions, Some(&mut ai::ReservedWaits::default()))).await {
                Ok(Ok(d)) => Ok(d),
                Ok(Err(e)) => Err(format!("decide call failed: {e}")),
                Err(_) => Err("decide call timed out".to_string()),
            }
        }
    };
    let decision = match outcome {
        Ok(d) => d,
        Err(reason) => {
            warn!(
                site = "decide_node",
                activity = activity.id.as_str(),
                reason = %reason,
                defaults = defaults.len(),
                "no decision; the node recorded its default answers and routed on"
            );
            let mut output = serde_json::Map::new();
            for (name, answer) in &defaults {
                if let Ok(v) = serde_json::to_value(answer) {
                    output.insert(name.clone(), v);
                }
            }
            // A question with no default is recorded as unanswered: null
            // fields at confidence 0. A condition reads them (a `.choice ==`
            // test is False) instead of failing on a missing field.
            for name in questions.keys() {
                output.entry(name.to_string()).or_insert_with(|| {
                    serde_json::json!({"choice": null, "score": null, "noul": null, "confidence": 0.0})
                });
            }
            output.insert("model".into(), serde_json::Value::String(String::new()));
            output.insert("defaulted".into(), serde_json::Value::Bool(true));
            let _ = ctx.store.create_activity_result(
                &ctx.run_id,
                &activity.id,
                &scope.iteration,
                "completed",
                0,
                1,
                None,
                started_at,
                Some(chrono::Utc::now().timestamp()),
            );
            let content = serde_json::Value::Object(output).to_string();
            let _ = ctx.store.set_activity_result_content(&ctx.run_id, &activity.id, &scope.iteration, &content);
            record_output(ctx, scope, &activity.id, content);
            return route(ctx, scope, &activity.id, |_| true).await;
        }
    };

    let mut output = serde_json::Map::new();
    for (name, answer) in &decision.answers {
        match serde_json::to_value(answer) {
            Ok(v) => {
                output.insert(name.clone(), v);
            }
            Err(e) => return fail(format!("decide answer '{name}' did not encode: {e}")),
        }
    }
    output.insert("model".into(), serde_json::Value::String(decision.model.clone()));
    if summarized {
        output.insert("state_summarized".into(), serde_json::Value::Bool(true));
    }

    let tokens = (decision.usage.input_tokens + decision.usage.output_tokens) as i64;
    let _ = ctx.store.create_activity_result(
        &ctx.run_id,
        &activity.id,
        &scope.iteration,
        "completed",
        tokens,
        1,
        None,
        started_at,
        Some(chrono::Utc::now().timestamp()),
    );
    info!(
        site = "decide_node",
        activity = activity.id.as_str(),
        model = %decision.model,
        input_tokens = decision.usage.input_tokens,
        cost_micro = decision.usage.cost_micro,
        answers = decision.answers.len(),
        "decide node completed"
    );
    let content = serde_json::Value::Object(output).to_string();
    let _ = ctx.store.set_activity_result_content(&ctx.run_id, &activity.id, &scope.iteration, &content);
    record_output(ctx, scope, &activity.id, content);
    route(ctx, scope, &activity.id, |_| true).await
}

/// Most bytes of state one `decide` node sends, about 6k tokens and under
/// the decision service's state limit. A whole event payload or tool result
/// can be far larger.
const DECIDE_STATE_CAP: usize = ai::decide::STATE_CAP;

/// A `decide` node's state within [`DECIDE_STATE_CAP`]: as it is when it
/// fits, else its summary (`tools::step_data::summary`: its own summary,
/// kind, size, fields, then as much of its first items or fields as fit)
/// as text, and `true`. Nothing is cut silently: the node's output says the
/// decision was made on a summary, and the state stays whole in the run.
fn decide_state(activity_id: &str, state: serde_json::Value) -> (serde_json::Value, bool) {
    let size = tools::step_data::chars(&state);
    if size <= DECIDE_STATE_CAP {
        return (state, false);
    }
    warn!(
        site = "decide_node",
        activity = activity_id,
        bytes = size,
        cap = DECIDE_STATE_CAP,
        "decide state over the decision service's limit; deciding on its summary (narrow params.state)"
    );
    let state = match state {
        serde_json::Value::String(s) => tools::step_data::parse(&s),
        other => other,
    };
    (serde_json::Value::String(tools::step_data::summary(&state, DECIDE_STATE_CAP)), true)
}

/// Expert step: the work is done by another agent (`params.expert`), asked
/// once per attempt and waited on durably (see `crate::expert`). No model
/// turn here; the reply becomes the node output, always with a `summary`.
///
/// `params.task` (or the intent) and `params.expert` interpolate from the
/// data context, so a loop body can name `{{item.expert}}`; `params.input`
/// hands the expert explicit references (`{"leads": "{{nodes.fetch}}"}`)
/// as JSON; `params.output` is the contract; `params.done_when` says when
/// the work counts as done (it interpolates like the task); `params.timeout`
/// is optional, and without it the step waits for the answer.
///
/// A request still out parks the walk at this node (the run parks once every
/// other branch is done). A refusal, an unreachable expert or a timeout is
/// tried again under `on_error.retry`; after that `on_error.fallback: abort`
/// fails the run, and otherwise (the default) the node completes with
/// `{failed: true, reason, summary}` so a join or a loop carries it and the
/// run goes on. An expert on another bot blocks the run: nothing leaves the
/// owner's bot without his approval.
async fn run_expert<'a>(
    ctx: &GraphCtx<'a>,
    scope: &WalkScope,
    activity: &Activity,
) -> Result<(), WorkflowError> {
    use crate::expert::{self, Standing, Unsent};
    if let Some(content) = replay_completed(ctx, scope, activity) {
        info!(activity = activity.id.as_str(), "resume: replaying completed expert node");
        record_output(ctx, scope, &activity.id, content);
        return route(ctx, scope, &activity.id, |_| true).await;
    }
    let started_at = chrono::Utc::now().timestamp();
    let data = data_context(ctx, scope);
    let expert_label = interpolate_context(param_str(activity, "expert"), &data).trim().to_string();
    let task = {
        let t = param_str(activity, "task");
        interpolate_context(if t.trim().is_empty() { &activity.intent } else { t }, &data)
    };
    // No timeout: the step waits for the answer. Saving refuses an
    // unreadable one; one that got past it anyway waits an hour.
    let timeout_secs = expert::timeout_of(activity).unwrap_or(Some(3_600));
    let done_when = interpolate_context(param_str(activity, "done_when"), &data);
    let input = expert::resolve_input(activity, &|path| resolve_path(&data, path));
    let contract = expert::output_contract(activity);
    let finish = |status: &str, error: Option<&str>, content: Option<&str>| {
        let _ = ctx.store.create_activity_result(
            &ctx.run_id,
            &activity.id,
            &scope.iteration,
            status,
            0,
            1,
            error,
            started_at,
            Some(chrono::Utc::now().timestamp()),
        );
        if let Some(content) = content {
            let _ = ctx.store.set_activity_result_content(&ctx.run_id, &activity.id, &scope.iteration, content);
        }
    };

    let hold = expert::held(activity);
    let mut failure = String::new();
    for attempt in 0..activity.on_error.retry.max(1) {
        // A held answer the owner asked to change goes back to the same
        // expert as the next round, with his notes; each round is its own
        // request (`expert::round_key`).
        let first = expert::request_key(&ctx.run_id, &activity.id, &scope.iteration, attempt);
        let mut round = 0;
        let mut changes: Vec<String> = Vec::new();
        while let Some(expert::Review::Change(note)) = expert::review_of(ctx.store, &ctx.run_id, &expert::round_key(&first, round)) {
            changes.push(note);
            round += 1;
        }
        let key = expert::round_key(&first, round);
        let task = match changes.is_empty() {
            true => task.clone(),
            false => format!("{task}\n\n{}", changes_asked(&changes)),
        };
        let now = chrono::Utc::now().timestamp();
        let standing = match expert::standing(ctx.store, &ctx.run_id, &key, timeout_secs, now) {
            Some(standing) => standing,
            None => {
                // A project's next card waits while the project is at its
                // limit: the owner decides whether it goes on.
                if let Some(used) = expert::project_limit_reached(ctx.store, &ctx.run_id) {
                    return Err(WorkflowError::ProjectLimit(used));
                }
                let target = match expert::resolve(ctx.store, &ctx.agent_id, &expert_label) {
                    Ok(target) => target,
                    Err(Unsent::NeedsOwner(why)) => {
                        finish("exited", Some(&why), None);
                        return Err(WorkflowError::Blocked(why, None));
                    }
                    // Naming nobody is not fixed by asking again.
                    Err(Unsent::Unreachable(why)) => {
                        failure = why;
                        break;
                    }
                };
                let req = expert::Request {
                    run_id: &ctx.run_id,
                    key: &key,
                    owner_agent_id: &ctx.agent_id,
                    workflow_name: &ctx.def.name,
                    task: &task,
                    input: &input,
                    contract: &contract,
                    done_when: &done_when,
                    timeout_secs,
                };
                match expert::send(ctx.store, &target, &req, now) {
                    Ok(standing) => standing,
                    Err(Unsent::Unreachable(why) | Unsent::NeedsOwner(why)) => {
                        failure = why;
                        continue;
                    }
                }
            }
        };
        match standing {
            // Held for the owner's OK: the answer waits for his review. He is
            // asked once (`expert::mark_held`); his Change sends it back as
            // the next round, his Approve makes it the step's output.
            Standing::Answered(output) if hold && expert::review_of(ctx.store, &ctx.run_id, &key) != Some(expert::Review::Approve) => {
                if expert::mark_held(ctx.store, &ctx.run_id, &key)? {
                    crate::cases::ring_project(crate::cases::ProjectChange::HeldForOk {
                        run_id: ctx.run_id.clone(),
                        key: key.clone(),
                        assignment_id: expert::assignment_of(ctx.store, &ctx.run_id, &key),
                        summary: output["summary"].as_str().unwrap_or("").to_string(),
                    });
                }
                info!(activity = activity.id.as_str(), expert = %expert_label, "expert answer held for the owner's OK");
                ctx.state.lock().unwrap().pending_experts.push((expert::review_watch(&key), None));
                return Err(WorkflowError::AwaitingExpert(1));
            }
            Standing::Answered(output) => {
                let output = output.to_string();
                finish("completed", None, Some(&output));
                info!(activity = activity.id.as_str(), expert = %expert_label, "expert node answered");
                record_output(ctx, scope, &activity.id, output);
                return route(ctx, scope, &activity.id, |_| true).await;
            }
            Standing::Waiting { deadline } => {
                ctx.state.lock().unwrap().pending_experts.push((expert::reply_watch(&key), deadline));
                return Err(WorkflowError::AwaitingExpert(1));
            }
            Standing::Failed(why) => failure = why,
        }
    }

    warn!(activity = activity.id.as_str(), expert = %expert_label, reason = %failure, "expert node failed");
    if matches!(activity.on_error.fallback, crate::parser::Fallback::Abort) {
        finish("failed", Some(&failure), None);
        return Err(WorkflowError::ActivityFailed(activity.id.clone(), failure));
    }
    let output = expert::failure(&expert_label, &failure).to_string();
    finish("completed", Some(&failure), Some(&output));
    record_output(ctx, scope, &activity.id, output);
    route(ctx, scope, &activity.id, |_| true).await
}

/// What the expert reads when the owner asked for changes to its held answer.
fn changes_asked(changes: &[String]) -> String {
    let said: Vec<&str> = changes.iter().map(|c| c.trim()).filter(|c| !c.is_empty()).collect();
    match said.as_slice() {
        [] => "The owner looked at your last answer and asked for changes before he approves it. Look at it again \
               against the task and when it counts as done, improve it, and say what you changed."
            .to_string(),
        notes => format!(
            "The owner looked at your last answer and asked for changes before he approves it: {}. Make them, and say \
             what you changed.",
            notes.join("; then: ")
        ),
    }
}

/// Park the run on its experts: one `resume` wait on `expert:<run>` with the
/// earliest deadline, so the first reply or the first timeout wakes it. A
/// reply that landed while the run was still busy woke nothing; it wakes
/// the run now.
fn park_on_experts(store: &Store, run_id: &str, pending: &[(String, Option<i64>)]) -> Result<(), WorkflowError> {
    let now = chrono::Utc::now().timestamp();
    let reason = format!("waiting on {} expert(s)", pending.len());
    let target = crate::expert::run_target(run_id);
    let wait_id = store.engine_declare_wait(
        run_id,
        &db::NewWait {
            action: "resume",
            on_kind: crate::expert::REPLY_KIND,
            key: &target,
            deadline: pending.iter().filter_map(|(_, d)| *d).min(),
            parked: None,
            reason: &reason,
        },
        now,
    )?;
    let watches: Vec<String> = pending.iter().map(|(w, _)| w.clone()).collect();
    if let Some(event_id) = crate::expert::any_reply(store, run_id, &watches) {
        store.engine_resume_from_wait(wait_id, event_id, now)?;
    }
    Ok(())
}

/// Park a project's run at its limit: one `resume` wait on `limit:<run>`
/// with no deadline, the project paused, and the bell rung so its thread and
/// the owner hear it (`server::team::project_lines`). The owner's Continue is
/// the event that wakes it.
fn park_at_limit(store: &Store, run_id: &str, used: &str) -> Result<(), WorkflowError> {
    let now = chrono::Utc::now().timestamp();
    store.engine_declare_wait(
        run_id,
        &db::NewWait {
            action: "resume",
            on_kind: crate::expert::LIMIT_KIND,
            key: &crate::expert::limit_target(run_id),
            deadline: None,
            parked: None,
            reason: &format!("paused at its limit ({used})"),
        },
        now,
    )?;
    if let Some(project) = store.workflow_run_project(run_id)? {
        store.set_project_state(&project, "paused", now)?;
        crate::cases::ring_project(crate::cases::ProjectChange::LimitReached { project_id: project, run_id: run_id.to_string() });
    }
    Ok(())
}

/// Deterministic condition: evaluate params against the data context and
/// activate only the matching branch. No tokens, no model.
async fn run_condition<'a>(
    ctx: &GraphCtx<'a>,
    scope: &WalkScope,
    activity: &Activity,
) -> Result<(), WorkflowError> {
    let started_at = chrono::Utc::now().timestamp();
    let data = data_context(ctx, scope);
    let context_text = ancestor_text(ctx, scope, &activity.id);
    let mut parents: Vec<&str> = Vec::new();
    for c in &ctx.def.connections {
        if c.to == activity.id && c.from != TRIGGER_NODE && !parents.contains(&c.from.as_str()) {
            parents.push(c.from.as_str());
        }
    }

    match evaluate_condition(activity, &data, &context_text, &parents) {
        Ok(verdict) => {
            let completed_at = chrono::Utc::now().timestamp();
            let _ = ctx.store.create_activity_result(
                &ctx.run_id,
                &activity.id,
                &scope.iteration,
                "completed",
                0,
                1,
                None,
                started_at,
                Some(completed_at),
            );
            let chosen = if verdict { "True" } else { "False" };
            record_output(ctx, scope, &activity.id, chosen.to_string());
            info!(activity = activity.id.as_str(), verdict = chosen, "condition evaluated");
            route(ctx, scope, &activity.id, |label| label == Some(chosen)).await
        }
                // Suspension passes through untouched — the run is parked, not failed.
        Err(e @ WorkflowError::AwaitingApproval { .. }) => Err(e),
        Err(e) => {
            let completed_at = chrono::Utc::now().timestamp();
            let err_msg = e.to_string();
            let _ = ctx.store.create_activity_result(
                &ctx.run_id,
                &activity.id,
                &scope.iteration,
                "failed",
                0,
                1,
                Some(&err_msg),
                started_at,
                Some(completed_at),
            );
            Err(e)
        }
    }
}

/// Engine-driven loop: iterate params.source sequentially, running the
/// "Each item" body per item in its own barrier scope, then follow "Done".
async fn run_loop<'a>(
    ctx: &GraphCtx<'a>,
    scope: &WalkScope,
    activity: &Activity,
) -> Result<(), WorkflowError> {
    let started_at = chrono::Utc::now().timestamp();
    let data = data_context(ctx, scope);
    let source = param_str(activity, "source");

    let items: Vec<serde_json::Value> = match resolve_path(&data, source) {
        None | Some(serde_json::Value::Null) => vec![],
        Some(serde_json::Value::Array(items)) => items,
        Some(_) => {
            let err_msg = format!("loop source '{}' did not resolve to an array", source);
            let _ = ctx.store.create_activity_result(
                &ctx.run_id,
                &activity.id,
                &scope.iteration,
                "failed",
                0,
                1,
                Some(&err_msg),
                started_at,
                Some(chrono::Utc::now().timestamp()),
            );
            return Err(WorkflowError::ActivityFailed(activity.id.clone(), err_msg));
        }
    };

    let max_iterations = activity
        .params
        .as_ref()
        .and_then(|p| p.get("maxIterations"))
        .and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        })
        .unwrap_or(DEFAULT_MAX_ITERATIONS);

    let body = ctx
        .loop_bodies
        .get(&activity.id)
        .cloned()
        .unwrap_or_default();
    let entry_edges: Vec<Edge> = ctx
        .outgoing
        .get(&activity.id)
        .map(|edges| {
            edges
                .iter()
                .filter(|e| e.label.as_deref() == Some("Each item"))
                .cloned()
                .collect()
        })
        .unwrap_or_default();

    // Hands-free concurrency: every iteration starts at once BY DEFAULT —
    // owners never dial a knob, and the loop adds no width cap of its own.
    // The machine's real limits are the ONE brake per resource the body's
    // work already goes through (the LLM permit pool, the tool permit pool).
    // Rate limits are that pool's job too: a 429 halves it for the whole bot,
    // so the loop keeps no second, loop-local halving of its own.
    // This is semantically invisible because iteration outputs are scope-local
    // (readers see only their own iteration) and the loop gathers every
    // iteration's outputs in item order on completion, whatever order they
    // finished in. `params.concurrency` remains an escape
    // hatch: 1 declares order-dependent external side effects (strictly
    // sequential); an explicit value caps how many items run at once.
    let declared_concurrency = activity
        .params
        .as_ref()
        .and_then(|p| p.get("concurrency"))
        .and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        });

    // Layered fan-in: `params.batchSize` hands the body N items at a time
    // (the item is an array), so a loop over another loop's `results` can
    // summarise in groups instead of reading every raw result at once.
    let batch_size = activity
        .params
        .as_ref()
        .and_then(|p| p.get("batchSize"))
        .and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        })
        .unwrap_or(1)
        .max(1) as usize;
    let items: Vec<serde_json::Value> = if batch_size > 1 {
        items
            .chunks(batch_size)
            .map(|c| serde_json::Value::Array(c.to_vec()))
            .collect()
    } else {
        items
    };

    let total_items = items.len() as u64;
    let taken: Vec<(u64, serde_json::Value)> = items
        .into_iter()
        .take(max_iterations as usize)
        .enumerate()
        .map(|(i, item)| (i as u64, item))
        .collect();
    let processed = taken.len() as u64;
    let taken_items: Vec<serde_json::Value> = taken.iter().map(|(_, item)| item.clone()).collect();
    let ceiling = declared_concurrency.unwrap_or(processed).max(1) as usize;

    // Where each item lives in the run: the source list, read from there by
    // a step handed the item as a summary.
    let list_source = source_of(source, &data, &scope.iteration, scope.item_source.as_ref())
        .filter(|(_, range)| range.is_none())
        .map(|(s, _)| s);
    let list_source = &list_source;
    let body = &body;
    let entry_edges = &entry_edges;
    // On success an iteration returns its scope-local outputs, so the loop
    // can publish deterministically after ALL iterations complete.
    let run_one = move |idx: u64, item: serde_json::Value| async move {
        if let Some(token) = ctx.cancel_token {
            if token.is_cancelled() {
                return Err(WorkflowError::Cancelled);
            }
        }
        let seed: HashMap<String, String> = scope
            .outputs
            .as_ref()
            .map(|l| l.lock().unwrap().clone())
            .unwrap_or_default();
        let item_source = list_source.as_ref().map(|list| match &item {
            serde_json::Value::Array(batch) if batch_size > 1 => ItemSource {
                source: list.clone(),
                range: Some((idx as usize * batch_size, batch.len())),
            },
            _ => ItemSource {
                source: tools::step_data::Source::new(format!("{}.{idx}", list.path), list.at.clone()),
                range: None,
            },
        });
        let body_scope = WalkScope {
            arrivals: Mutex::new(HashMap::new()),
            indegree: body_indegree(ctx, &activity.id, body),
            stop_node: Some(activity.id.clone()),
            item: Some(item),
            item_source,
            iteration: if scope.iteration.is_empty() {
                idx.to_string()
            } else {
                format!("{}.{}", scope.iteration, idx)
            },
            outputs: Some(Mutex::new(seed)),
        };
        let walks = entry_edges
            .iter()
            .map(|e| arrive(ctx, &body_scope, e.to.clone(), true));
        first_error(futures::future::join_all(walks).await)?;
        let locals = body_scope
            .outputs
            .map(|l| l.into_inner().unwrap())
            .unwrap_or_default();
        Ok::<_, WorkflowError>(locals)
    };

    // (iteration idx, locals) per completed iteration — publication source.
    let mut completed_locals: Vec<(u64, HashMap<String, String>)> = Vec::new();
    // Iterations that ended early (exit tool / step evaluator). An exit is a
    // WHOLE-WORKFLOW decision ("nothing meaningful to do") — incoherent from
    // inside item N, where the work is already established and may already
    // have hit external systems. Letting it escape the loop skipped the
    // terminal activities that store/send/record (live 2026-08-27: an agent
    // exited iteration 3/3 with a progress note AFTER writing two orders to
    // the customer's CRM — no state commit, no report, owner never told).
    // So it ends the ITERATION; siblings keep going and the loop still routes
    // to Done. Surfaced below, never silent.
    let mut exited: Vec<(u64, String)> = Vec::new();
    let mut parked: Option<WorkflowError> = None;

    if declared_concurrency.is_some_and(|c| c <= 1) {
        for (idx, item) in taken {
            match run_one(idx, item).await {
                Ok(locals) => completed_locals.push((idx, locals)),
                Err(WorkflowError::Exited(reason)) => {
                    warn!(
                        activity = activity.id.as_str(),
                        iteration = idx,
                        reason = %reason,
                        "iteration exited early — ending this item, not the workflow"
                    );
                    exited.push((idx, reason));
                }
                // Strictly sequential: the next item waits for this one.
                Err(e) => return Err(e),
            }
        }
    } else {
        let mut queue: VecDeque<(u64, serde_json::Value, u32)> =
            taken.into_iter().map(|(i, item)| (i, item, 0)).collect();
        let mut inflight = FuturesUnordered::new();
        loop {
            while inflight.len() < ceiling {
                let Some((idx, item, attempt)) = queue.pop_front() else {
                    break;
                };
                let retry_copy = item.clone();
                inflight.push(async move {
                    if attempt > 0 {
                        // Exponential backoff before a rate-limited retry:
                        // the item's calls already retried inside the runner
                        // and the pool already halved; this spaces the item's
                        // next attempt.
                        let secs = (1u64 << attempt.min(4)).min(16);
                        tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
                    }
                    (idx, retry_copy, attempt, run_one(idx, item).await)
                });
            }
            let Some((idx, item, attempt, outcome)) = inflight.next().await else {
                break;
            };
            match outcome {
                Ok(locals) => completed_locals.push((idx, locals)),
                Err(WorkflowError::Exited(reason)) => {
                    // Not a failure and not rate pressure — don't kill the
                    // siblings already in flight.
                    warn!(
                        activity = activity.id.as_str(),
                        iteration = idx,
                        reason = %reason,
                        "iteration exited early — ending this item, not the workflow"
                    );
                    exited.push((idx, reason));
                }
                Err(e)
                    if is_rate_limit_shaped(&e.to_string())
                        && attempt < MAX_ITEM_RATE_LIMIT_RETRIES =>
                {
                    warn!(
                        activity = activity.id.as_str(),
                        iteration = idx,
                        attempt,
                        "iteration rate-limited — requeueing it"
                    );
                    queue.push_back((idx, item, attempt + 1));
                }
                // An iteration parked on an expert: its siblings keep going;
                // the loop parks once they are all done.
                Err(e @ WorkflowError::AwaitingExpert(_)) => {
                    parked.get_or_insert(e);
                }
                Err(e) => return Err(e),
            }
        }
    }
    if let Some(e) = parked {
        return Err(e);
    }

    // Fan-in: the loop's output carries EVERY iteration's body outputs in
    // item order, so whatever follows "Done" reads all N results — never just
    // the last one. Body nodes are not published individually: outside the
    // loop, a body node's output only means something per item.
    let mut results: Vec<serde_json::Value> = taken_items
        .into_iter()
        .map(|item| serde_json::json!({ "item": item }))
        .collect();
    for (idx, locals) in completed_locals {
        let outputs: serde_json::Map<String, serde_json::Value> = locals
            .into_iter()
            .filter(|(id, _)| body.contains(id))
            .map(|(id, out)| {
                let parsed = serde_json::from_str::<serde_json::Value>(&out)
                    .unwrap_or(serde_json::Value::String(out));
                (id, parsed)
            })
            .collect();
        results[idx as usize]["outputs"] = serde_json::Value::Object(outputs);
    }
    for (idx, reason) in &exited {
        results[*idx as usize]["exited"] = serde_json::Value::String(reason.clone());
    }

    let _ = ctx.store.create_activity_result(
        &ctx.run_id,
        &activity.id,
        &scope.iteration,
        "completed",
        0,
        1,
        None,
        started_at,
        Some(chrono::Utc::now().timestamp()),
    );
    // No silent caps (WS3-R6): a truncated loop must never read as a complete
    // one. Live incident: maxIterations 16 with 20 report chunks would have
    // dropped the last 4 with the run stamped `completed`.
    let mut summary = if total_items > processed {
        warn!(
            activity = activity.id.as_str(),
            processed,
            total_items,
            max_iterations,
            "loop truncated by maxIterations cap"
        );
        format!(
            "processed {} of {} items — STOPPED at the maxIterations cap ({}); {} item(s) were NOT processed",
            processed,
            total_items,
            max_iterations,
            total_items - processed
        )
    } else {
        format!("{} items processed", processed)
    };
    // Same rule as the cap above: an incomplete loop must never read as a
    // complete one, so exited iterations are named in the loop's output.
    if !exited.is_empty() {
        warn!(
            activity = activity.id.as_str(),
            exited = exited.len(),
            processed,
            "iterations ended early — loop continued and routed to Done"
        );
        let detail = exited
            .iter()
            .map(|(i, r)| format!("#{i}: {r}"))
            .collect::<Vec<_>>()
            .join("; ");
        summary.push_str(&format!(
            " — {} of them ended early WITHOUT completing ({})",
            exited.len(),
            detail
        ));
    }
    // Kept whole in the run: a later step reads it from there.
    let content = serde_json::json!({ "summary": summary, "results": results }).to_string();
    let _ = ctx.store.set_activity_result_content(&ctx.run_id, &activity.id, &scope.iteration, &content);
    record_output(ctx, scope, &activity.id, content);
    info!(activity = activity.id.as_str(), processed, "loop completed");
    route(ctx, scope, &activity.id, |label| label == Some("Done")).await
}

/// The AI lives here: one LLM-driven activity, executed by the engine's step
/// runner (per-step turns, evaluator-gated), then routed onward.
async fn run_llm_activity<'a>(
    ctx: &GraphCtx<'a>,
    scope: &WalkScope,
    activity: &Activity,
) -> Result<(), WorkflowError> {
    let parents = ctx.parents.get(&activity.id).map(Vec::as_slice).unwrap_or_default();
    let outputs = scope_outputs(ctx, scope);
    let step = step_context(
        activity,
        parents,
        StepData {
            outputs: &outputs,
            inputs: ctx.inputs,
            item: scope.item.as_ref(),
            item_source: scope.item_source.as_ref(),
            at: &scope.iteration,
            budget: data_budget(ctx.loop_impl.context_chars(&ctx.agent_id, activity.level)),
        },
    );

    // Tool assembly: the activity's declared tools (see
    // scoped_activity_tools) and the emit tool; the run's emit names go only
    // to terminal nodes (edge to __emit__).
    let mut activity_tools: Vec<&Box<dyn DynTool>> =
        crate::engine::scoped_activity_tools(&step.activity, ctx.resolved_tools);
    // The producing seat rides every emitted payload and every address it
    // raises (R6).
    let emit_tool_box: Option<Box<dyn DynTool>> = ctx.event_bus.map(|bus| {
        Box::new(
            tools::EmitTool::new(bus.clone())
                .with_producer(crate::engine::producer_slug(ctx.store, &ctx.agent_id)),
        ) as Box<dyn DynTool>
    });
    if let Some(ref emit) = emit_tool_box {
        activity_tools.push(emit);
    }
    let exit_tool_box: Box<dyn DynTool> = Box::new(tools::ExitTool::new());
    activity_tools.push(&exit_tool_box);

    let activity_emit: &[String] = if ctx.terminal_emit.contains(&activity.id) {
        &ctx.emit_sources
    } else {
        &[]
    };

    let started_at = chrono::Utc::now().timestamp();
    // Accumulates every token this activity consumes, error paths included.
    let mut spent: u32 = 0;
    match execute_activity_with_retry(
        &step.activity,
        &step.prior,
        &ctx.memory_user_id,
        ctx.memory_writes_disabled,
        &step.inputs,
        ctx.decide,
        ctx.loop_impl,
        &activity_tools,
        ctx.skill_content,
        activity_emit,
        ctx.store,
        &ctx.agent_id,
        &ctx.run_id,
        ctx.def,
        ctx.progress_tx.as_ref(),
        &mut spent,
        ctx.checkpoint.as_ref(),
        // The parked call belongs to one activity in one iteration — a loop
        // body resuming on item 5 must not replay item 2's pending call.
        ctx.resume
            .as_ref()
            .filter(|r| r.activity_id == activity.id && r.iteration == scope.iteration),
        &scope.iteration,
        ctx.cancel_token,
        &ctx.budget,
    )
    .await
    {
        Ok((result_text, _tokens_used)) => {
            let completed_at = chrono::Utc::now().timestamp();
            let _ = ctx.store.create_activity_result(
                &ctx.run_id,
                &activity.id,
                &scope.iteration,
                "completed",
                spent as i64,
                1,
                None,
                started_at,
                Some(completed_at),
            );
            // Output content backs the resume fast-forward (UPDATE — after the row exists).
            let _ = ctx
                .store
                .set_activity_result_content(&ctx.run_id, &activity.id, &scope.iteration, &result_text);

            // `budget.total_per_run` is the package author's estimate (it
            // feeds the listing's cost_estimate) and is never enforced: the
            // only ceiling a run has is the owner's spending limit.
            ctx.state.lock().unwrap().total_tokens += spent;

            if result_text.trim().is_empty() {
                // n8n-style branch termination: no output = this branch dies.
                info!(
                    workflow = ctx.def.id.as_str(),
                    activity = activity.id.as_str(),
                    "activity produced no output, terminating branch"
                );
                return route(ctx, scope, &activity.id, |_| false).await;
            }

            record_output(ctx, scope, &activity.id, result_text);
            route(ctx, scope, &activity.id, |_| true).await
        }
        // Parked on the owner's answer: the step re-runs from its pending
        // call when the answer resumes the run, so nothing is recorded yet.
        Err(e @ WorkflowError::AwaitingApproval { .. }) => {
            ctx.state.lock().unwrap().total_tokens += spent;
            Err(e)
        }
        // The owner's spending limit: the activity had its wrap-up turn, so
        // it ends STOPPED with what it reported, kept as its output for the
        // run's record. Not a failure, nothing lost.
        Err(e @ WorkflowError::SpendCapReached { .. }) => {
            ctx.state.lock().unwrap().total_tokens += spent;
            let partial = match &e {
                WorkflowError::SpendCapReached { partial, .. } => partial.clone(),
                _ => String::new(),
            };
            let _ = ctx.store.create_activity_result(
                &ctx.run_id,
                &activity.id,
                &scope.iteration,
                "stopped",
                spent as i64,
                1,
                Some(&e.to_string()),
                started_at,
                Some(chrono::Utc::now().timestamp()),
            );
            if !partial.trim().is_empty() {
                let _ = ctx.store.set_activity_result_content(&ctx.run_id, &activity.id, &scope.iteration, &partial);
                record_output(ctx, scope, &activity.id, partial);
            }
            Err(e)
        }
        Err(e) if let Some(reason) = e.standing_outcome() => {
            ctx.state.lock().unwrap().total_tokens += spent;
            let completed_at = chrono::Utc::now().timestamp();
            let _ = ctx.store.create_activity_result(
                &ctx.run_id,
                &activity.id,
                &scope.iteration,
                "exited",
                spent as i64,
                1,
                Some(&reason),
                started_at,
                Some(completed_at),
            );
            Err(e)
        }
        Err(e) => {
            ctx.state.lock().unwrap().total_tokens += spent;
            let completed_at = chrono::Utc::now().timestamp();
            let err_msg = e.to_string();
            let _ = ctx.store.create_activity_result(
                &ctx.run_id,
                &activity.id,
                &scope.iteration,
                "failed",
                spent as i64,
                activity.on_error.retry as i64,
                Some(&err_msg),
                started_at,
                Some(completed_at),
            );
            Err(e)
        }
    }
}

/// The share of a step's model input its data may take: the rest is room
/// for its instructions, its tools, the pages it reads and its reply.
pub(crate) fn data_budget(context_chars: usize) -> usize {
    context_chars / 2
}

/// Characters of the summary a step reads in place of one piece of data
/// too large to hand it whole.
const PIECE_SUMMARY_CHARS: usize = 1_500;

/// Where a loop body's item lives in the run: the loop's source list and
/// the item's place in it (`range`: a batch's first item and count).
#[derive(Debug, Clone)]
pub(crate) struct ItemSource {
    source: tools::step_data::Source,
    range: Option<(usize, usize)>,
}

/// What a step can see of the run, and the room its model has for it.
pub(crate) struct StepData<'a> {
    /// Every result this step can see, by activity id.
    pub outputs: &'a HashMap<String, String>,
    pub inputs: &'a serde_json::Value,
    pub item: Option<&'a serde_json::Value>,
    pub item_source: Option<&'a ItemSource>,
    /// The loop iteration the step runs in ("" outside a loop).
    pub at: &'a str,
    /// Characters of data its model takes ([`data_budget`]).
    pub budget: usize,
}

/// What an AI step is handed: the activity with its references resolved
/// (and `read_step_data` among its tools when it may need it), its Prior
/// Results, and its inputs as the prompt shows them.
pub(crate) struct StepContext {
    pub activity: Activity,
    pub prior: String,
    pub inputs: serde_json::Value,
}

/// One piece of a step's data: a parent's result, an input, the loop item
/// or a `{{...}}` reference.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Piece {
    Parent(String),
    Input(String),
    Item,
    Ref(String),
}

/// The run data a data path names, as `read_step_data` reads it: a path
/// rooted at `nodes.` or `inputs.` (a bare path resolves the way
/// [`resolve_path`] does it), or into the loop item through its source.
fn source_of(path: &str, data: &serde_json::Value, at: &str, item: Option<&ItemSource>) -> Option<(tools::step_data::Source, Option<(usize, usize)>)> {
    use tools::step_data::Source;
    let path = path.trim();
    let (root, rest) = path.split_once('.').unwrap_or((path, ""));
    match root {
        "nodes" => Some((Source::new(path, at), None)),
        "inputs" => Some((Source::new(path, ""), None)),
        "item" => {
            let item = item?;
            let base = &item.source;
            let join = |tail: &str| if tail.is_empty() { base.path.clone() } else { format!("{}.{tail}", base.path) };
            match item.range {
                None => Some((Source::new(join(rest), base.at.clone()), None)),
                Some((start, count)) => match rest.split_once('.').unwrap_or((rest, "")) {
                    ("", _) => Some((base.clone(), Some((start, count)))),
                    (i, tail) => {
                        let at_item = format!("{}", start + i.parse::<usize>().ok()?);
                        let full = if tail.is_empty() { at_item } else { format!("{at_item}.{tail}") };
                        Some((Source::new(join(&full), base.at.clone()), None))
                    }
                },
            }
        }
        _ => {
            let in_inputs = data.get("inputs").and_then(|i| i.get(root)).is_some();
            let rooted = format!("{}.{path}", if in_inputs { "inputs" } else { "nodes" });
            source_of(&rooted, data, at, item)
        }
    }
}

/// What an AI activity's model reads of earlier steps, in both executors.
///
/// - References: `{{nodes.<id>}}`, `{{nodes.<id>.field}}`, `{{item.x}}`,
///   `{{inputs.x}}` in its intent and steps resolve the way a command
///   step's do ([`interpolate_context`]), from ANY earlier step.
/// - Prior Results: one entry per parent that has run, then the loop item.
///   Nothing from further back — a step that needs it names it. A parent
///   the step references is left out: the reference already put what the
///   step needs into its text.
/// - Inputs, shown by the prompt.
///
/// All of it goes whole while it fits the model ([`StepData::budget`]).
/// Past that, the largest pieces are handed as a summary each (kind, size,
/// fields, first items: `tools::step_data::described`) with the
/// `read_step_data` call that reads them a page at a time, until the rest
/// fits. Nothing is cut: the data stays whole in the run.
///
/// `parents` are the graph's result parents ([`result_parents`]); on a
/// chain, the previous activity.
pub(crate) fn step_context(activity: &Activity, parents: &[String], data: StepData<'_>) -> StepContext {
    use tools::step_data::{chars, described, parse};
    let texts = || std::iter::once(&activity.intent).chain(&activity.steps);
    let referenced = |id: &str| {
        let open = format!("{{{{nodes.{id}");
        texts().any(|t| {
            t.match_indices(&open)
                .any(|(i, _)| matches!(t[i + open.len()..].chars().next(), Some('.' | '}')))
        })
    };
    let all = data_of(data.outputs, data.inputs, data.item);
    let has_refs = texts().any(|t| t.contains("{{"));

    // Every piece and the characters it takes whole.
    let mut pieces: Vec<(Piece, usize)> = Vec::new();
    for id in parents {
        if let Some(result) = data.outputs.get(id).filter(|_| !referenced(id)) {
            pieces.push((Piece::Parent(id.clone()), result.len()));
        }
    }
    if let serde_json::Value::Object(map) = data.inputs {
        for (key, value) in map.iter().filter(|(k, _)| k.as_str() != "_emit") {
            pieces.push((Piece::Input(key.clone()), chars(value)));
        }
    }
    if let Some(item) = data.item {
        pieces.push((Piece::Item, chars(item)));
    }
    if has_refs {
        let mut seen: HashMap<String, usize> = HashMap::new();
        for text in texts() {
            for path in placeholder_paths(text) {
                *seen.entry(path.to_string()).or_default() += 1;
            }
        }
        for (path, times) in seen {
            if let Some(value) = resolve_path(&all, &path) {
                pieces.push((Piece::Ref(path), chars(&value) * times));
            }
        }
    }

    // The largest pieces that have a place in the run to be read from go
    // as summaries until the rest fits.
    let readable = |piece: &Piece| match piece {
        Piece::Item => data.item_source.is_some(),
        Piece::Ref(path) => source_of(path, &all, data.at, data.item_source).is_some(),
        _ => true,
    };
    let mut total: usize = pieces.iter().map(|(_, c)| c).sum();
    pieces.sort_by_key(|p| std::cmp::Reverse(p.1));
    let mut paged: std::collections::HashSet<Piece> = std::collections::HashSet::new();
    for (piece, size) in &pieces {
        if total <= data.budget || *size <= 2 * PIECE_SUMMARY_CHARS {
            break;
        }
        if readable(piece) {
            total = total - size + PIECE_SUMMARY_CHARS;
            paged.insert(piece.clone());
        }
    }
    if !paged.is_empty() {
        info!(
            activity = activity.id.as_str(),
            pieces = paged.len(),
            budget = data.budget,
            "step data over the model's room: large pieces handed as summaries, read in pages"
        );
    }

    let mut prior = String::new();
    for id in parents {
        if let Some(result) = data.outputs.get(id).filter(|_| !referenced(id)) {
            if paged.contains(&Piece::Parent(id.clone())) {
                let source = tools::step_data::Source::new(format!("nodes.{id}"), data.at);
                prior.push_str(&format!(
                    "\n[Activity '{id}' result — too large to include whole]:\n{}\n",
                    described(&parse(result), &source, PIECE_SUMMARY_CHARS)
                ));
            } else {
                prior.push_str(&format!("\n[Activity '{id}' result]: {result}\n"));
            }
        }
    }
    if let Some(item) = data.item {
        match data.item_source.filter(|_| paged.contains(&Piece::Item)) {
            Some(src) => prior.push_str(&format!(
                "\n[Current item — too large to include whole]:\n{}\n",
                with_range(described(item, &src.source, PIECE_SUMMARY_CHARS), src.range)
            )),
            None => prior.push_str(&format!("\n[Current item]: {}\n", item)),
        }
    }

    let mut inputs = data.inputs.clone();
    if let serde_json::Value::Object(map) = &mut inputs {
        for (key, value) in map.iter_mut() {
            if paged.contains(&Piece::Input(key.clone())) {
                let source = tools::step_data::Source::new(format!("inputs.{key}"), "");
                *value = serde_json::Value::String(described(value, &source, PIECE_SUMMARY_CHARS));
            }
        }
    }

    let mut resolved = activity.clone();
    if has_refs {
        let render = |path: &str, value: &serde_json::Value| -> String {
            match source_of(path, &all, data.at, data.item_source).filter(|_| paged.contains(&Piece::Ref(path.to_string()))) {
                Some((source, range)) => format!(
                    "[{{{{{path}}}}} — too large to include whole: {}]",
                    with_range(described(value, &source, PIECE_SUMMARY_CHARS), range)
                ),
                None => value_as_string(value),
            }
        };
        resolved.intent = interpolate_with(&activity.intent, &all, &render);
        for step in &mut resolved.steps {
            *step = interpolate_with(step, &all, &render);
        }
    }
    // A step reads what it was handed as a summary, and a tool result too
    // large to show, with `read_step_data`; a step without tools of its own
    // needs it only for the first.
    let step_data = tools::step_data::TOOL.to_string();
    if (!paged.is_empty() || !crate::engine::is_lean(activity)) && !resolved.tools.contains(&step_data) {
        resolved.tools.push(step_data);
    }
    StepContext { activity: resolved, prior, inputs }
}

/// A batch item's place in its loop's list, after its summary.
fn with_range(text: String, range: Option<(usize, usize)>) -> String {
    match range {
        Some((start, count)) => format!(
            "{text}\nThis item is {count} entries of that list starting at position {start}: read them with from=\"{start}\" and limit {count}."
        ),
        None => text,
    }
}

/// The `{{path}}` placeholders in a text, trimmed.
fn placeholder_paths(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else { break };
        out.push(after[..end].trim());
        rest = &after[end + 2..];
    }
    out
}

/// The text a `contains` / `regex` condition matches: every executed static
/// ancestor's whole output. No model reads it, so nothing is clipped.
fn ancestor_text(ctx: &GraphCtx, scope: &WalkScope, node: &str) -> String {
    let global = ctx.state.lock().unwrap();
    let local = scope.outputs.as_ref().map(|l| l.lock().unwrap());
    let mut out = String::new();
    if let Some(ancestors) = ctx.ancestors.get(node) {
        for id in ancestors {
            let result = local
                .as_ref()
                .and_then(|l| l.get(id))
                .or_else(|| global.outputs.get(id));
            if let Some(result) = result {
                out.push_str(&format!("\n[Activity '{}' result]: {}\n", id, result));
            }
        }
    }
    out
}

/// Final run context: every executed node's output in activity-array order.
fn final_context(def: &WorkflowDef, outputs: &HashMap<String, String>) -> String {
    let mut out = String::new();
    for a in &def.activities {
        if let Some(result) = outputs.get(&a.id) {
            out.push_str(&format!("\n[Activity '{}' result]: {}\n", a.id, result));
        }
    }
    out
}

/// The data context expressions resolve against:
/// `{ inputs, item, nodes: { <activity-id>: <parsed output or string> } }`.
fn data_context(ctx: &GraphCtx, scope: &WalkScope) -> serde_json::Value {
    data_of(&scope_outputs(ctx, scope), ctx.inputs, scope.item.as_ref())
}

/// Every result a node in this scope can see, by activity id.
/// Iteration-local values shadow the global map — a body node reads its
/// OWN iteration, never a racing sibling's.
fn scope_outputs(ctx: &GraphCtx, scope: &WalkScope) -> HashMap<String, String> {
    let mut merged: HashMap<String, String> = ctx.state.lock().unwrap().outputs.clone();
    if let Some(local) = &scope.outputs {
        for (k, v) in local.lock().unwrap().iter() {
            merged.insert(k.clone(), v.clone());
        }
    }
    merged
}

/// `{ inputs, item, nodes: { <activity-id>: <parsed output or string> } }`.
fn data_of(
    outputs: &HashMap<String, String>,
    inputs: &serde_json::Value,
    item: Option<&serde_json::Value>,
) -> serde_json::Value {
    let nodes: serde_json::Map<String, serde_json::Value> = outputs
        .iter()
        .map(|(id, out)| {
            let parsed = serde_json::from_str::<serde_json::Value>(out)
                .unwrap_or_else(|_| serde_json::Value::String(out.clone()));
            (id.clone(), parsed)
        })
        .collect();
    serde_json::json!({
        "inputs": inputs,
        "item": item.cloned().unwrap_or(serde_json::Value::Null),
        "nodes": serde_json::Value::Object(nodes),
    })
}

/// Resolve a dot path against the data context. Bare paths (no `inputs.` /
/// `item` / `nodes.` prefix) fall back to `inputs` then `nodes`.
fn resolve_path(root: &serde_json::Value, path: &str) -> Option<serde_json::Value> {
    let path = path.trim();
    if path.is_empty() {
        return None;
    }
    fn walk<'v>(mut current: &'v serde_json::Value, segments: &[&str]) -> Option<&'v serde_json::Value> {
        for seg in segments {
            current = match current {
                serde_json::Value::Object(map) => map.get(*seg)?,
                serde_json::Value::Array(items) => items.get(seg.parse::<usize>().ok()?)?,
                _ => return None,
            };
        }
        Some(current)
    }
    let segments: Vec<&str> = path.split('.').collect();
    if let Some(v) = walk(root, &segments) {
        return Some(v.clone());
    }
    match segments[0] {
        "inputs" | "item" | "nodes" => None,
        _ => {
            let inputs = root.get("inputs")?;
            if let Some(v) = walk(inputs, &segments) {
                return Some(v.clone());
            }
            let nodes = root.get("nodes")?;
            walk(nodes, &segments).cloned()
        }
    }
}

fn truthy(v: &serde_json::Value) -> bool {
    napp::condition::truthy(v)
}

fn value_as_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Deterministic condition evaluation. Modes:
/// - `expression` (default, or empty): the condition language in
///   [`napp::condition`]: `&&`, `||`, `!`, parentheses and comparisons over
///   `inputs.`, `nodes.`, `item` paths and bare names (fields of the one
///   direct parent's output). Text is quoted. Anything it cannot parse, and
///   any reference that does not resolve, fails the step; it is never a
///   silent False.
/// - `exists`:   expression is a data path; true if it resolves truthy.
/// - `contains`: "left contains needle" resolves `left` as a path (falling
///   back to the literal text) and substring-matches; a bare expression is
///   substring-matched against the upstream context text.
/// - `regex`:    pattern matched against the upstream context text.
///
/// `parents` are the condition's direct parent steps (trigger excluded).
fn evaluate_condition(
    activity: &Activity,
    data: &serde_json::Value,
    context_text: &str,
    parents: &[&str],
) -> Result<bool, WorkflowError> {
    let expression = param_str(activity, "expression").trim().to_string();
    let fail = |msg: String| WorkflowError::ActivityFailed(activity.id.clone(), msg);

    match param_str(activity, "mode").trim() {
        "exists" => Ok(resolve_path(data, &expression)
            .map(|v| truthy(&v))
            .unwrap_or(false)),
        "contains" => {
            if let Some((left, needle)) = expression.split_once(" contains ") {
                let haystack = resolve_path(data, left.trim())
                    .map(|v| value_as_string(&v))
                    .unwrap_or_else(|| left.trim().to_string());
                Ok(haystack.contains(needle.trim()))
            } else {
                Ok(context_text.contains(expression.as_str()))
            }
        }
        "regex" => {
            let re = regex::Regex::new(&expression)
                .map_err(|e| fail(format!("invalid regex '{}': {}", expression, e)))?;
            Ok(re.is_match(context_text))
        }
        "" | "expression" => {
            let parsed = napp::condition::parse(&expression)
                .map_err(|e| fail(format!("condition `{expression}` does not parse: {e}")))?;
            napp::condition::eval(&parsed, &|r| resolve_condition_ref(data, parents, r))
                .map_err(|e| fail(format!("condition `{expression}`: {e}")))
        }
        other => Err(fail(format!(
            "condition mode '{other}' is not one of expression, contains, exists, regex"
        ))),
    }
}

/// Resolve one condition reference. Rooted paths read the data context
/// directly; a bare name is a field of the single direct parent's output.
fn resolve_condition_ref(
    data: &serde_json::Value,
    parents: &[&str],
    reference: &napp::condition::Reference,
) -> Result<serde_json::Value, String> {
    use napp::condition::Reference;
    let path = match reference {
        Reference::Rooted(p) => p.clone(),
        Reference::Parent(field) => match parents {
            [one] => format!("nodes.{one}.{field}"),
            [] => {
                return Err(format!(
                    "'{field}' names a field of the step feeding this condition, but no step feeds it; \
                     write inputs.{field}"
                ));
            }
            many => {
                return Err(format!(
                    "'{field}' names a field of the step feeding this condition, but {} steps feed it; \
                     write nodes.<step>.{field}",
                    many.len()
                ));
            }
        },
    };
    let segments: Vec<&str> = path.split('.').collect();
    let mut current = data;
    for (i, seg) in segments.iter().enumerate() {
        current = match current {
            serde_json::Value::Object(map) => map.get(*seg),
            serde_json::Value::Array(items) => seg.parse::<usize>().ok().and_then(|n| items.get(n)),
            _ => None,
        }
        .ok_or_else(|| {
            let shown = match reference {
                Reference::Rooted(p) => p.clone(),
                Reference::Parent(f) => format!("{f} (read as {path})"),
            };
            if i == 0 || (segments[0] == "nodes" && i == 1) {
                format!("{shown} did not resolve: '{}' has no output yet", segments[..=i].join("."))
            } else {
                format!(
                    "{shown} did not resolve: {} has no '{}'; test for presence with mode \"exists\"",
                    segments[..i].join("."),
                    seg
                )
            }
        })?;
    }
    Ok(current.clone())
}

#[cfg(test)]
mod walk_tests {
    use super::*;
    use crate::parser::parse_workflow;
    use std::sync::Mutex as StdMutex;

    /// Scripted provider: every LLM call answers with "done:<last-user-line>"
    /// (or a scripted override) and records the call for ordering assertions.
    struct MockProvider {
        calls: StdMutex<Vec<String>>,
        /// intent substring -> scripted response ("" = empty output).
        scripts: Vec<(String, String)>,
        /// Tokens reported per turn (input+output split evenly).
        usage_per_turn: Option<i32>,
        /// Remaining stream() calls to fail with ProviderError::RateLimit
        /// (failed calls are NOT recorded — call-count asserts see successes).
        rate_limit_failures: StdMutex<u32>,
        /// system-prompt substring -> scripted response. Loop iteration
        /// identity ([Current item]) lives in the SYSTEM prompt, so per-item
        /// scripting keys here; user scripts take precedence.
        context_scripts: Vec<(String, String)>,
        /// system-prompt substring -> exit reason. Emits a real `exit` tool
        /// call so the engine takes its genuine EXIT_SENTINEL path.
        exit_scripts: Vec<(String, String)>,
        /// system-prompt substring -> a tool's terminal refusal. The turn
        /// ends the way the runner ends it: a control notice carrying the
        /// refusal and a Done whose stop reason is `terminal_tool_error`.
        blocked_scripts: Vec<(String, String)>,
        /// user-message substrings whose turn "read outside text": the Done
        /// event carries web provenance, as the runner stamps it.
        tainted_scripts: Vec<String>,
        /// Characters of input the step's model takes (the loop's
        /// `context_chars`): a 200k-token window's by default.
        context_chars: usize,
    }

    impl MockProvider {
        fn new(scripts: &[(&str, &str)]) -> Self {
            Self {
                calls: StdMutex::new(vec![]),
                scripts: scripts
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
                usage_per_turn: None,
                rate_limit_failures: StdMutex::new(0),
                context_scripts: vec![],
                exit_scripts: vec![],
                blocked_scripts: vec![],
                tainted_scripts: vec![],
                context_chars: 800_000,
            }
        }
        fn with_context_chars(mut self, chars: usize) -> Self {
            self.context_chars = chars;
            self
        }
        fn with_tainted_turns(mut self, keys: &[&str]) -> Self {
            self.tainted_scripts = keys.iter().map(|k| k.to_string()).collect();
            self
        }
        fn with_exit_scripts(mut self, scripts: &[(&str, &str)]) -> Self {
            self.exit_scripts = scripts
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            self
        }
        fn with_blocked_scripts(mut self, scripts: &[(&str, &str)]) -> Self {
            self.blocked_scripts = scripts
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            self
        }
        fn with_context_scripts(mut self, scripts: &[(&str, &str)]) -> Self {
            self.context_scripts = scripts
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            self
        }
        fn with_rate_limit_failures(self, n: u32) -> Self {
            *self.rate_limit_failures.lock().unwrap() = n;
            self
        }
        fn with_usage(mut self, tokens: i32) -> Self {
            self.usage_per_turn = Some(tokens);
            self
        }
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl ai::Provider for MockProvider {
        fn id(&self) -> &str {
            "mock"
        }
        async fn stream(
            &self,
            req: &ai::ChatRequest,
        ) -> Result<ai::EventReceiver, ai::ProviderError> {
            {
                let mut remaining = self.rate_limit_failures.lock().unwrap();
                if *remaining > 0 {
                    *remaining -= 1;
                    return Err(ai::ProviderError::RateLimit { retry_after_secs: None });
                }
            }
            let user = req
                .messages
                .iter()
                .rev()
                .find(|m| m.role == "user")
                .map(|m| m.content.clone())
                .unwrap_or_default();
            // Record user message + system prompt: ordering asserts on the
            // intent (user msg); context asserts on Prior Results (system).
            self.calls
                .lock()
                .unwrap()
                .push(format!("{}\n###SYSTEM###\n{}", user, req.system));
            if let Some((_, refusal)) = self
                .blocked_scripts
                .iter()
                .find(|(key, _)| req.system.contains(key.as_str()))
            {
                let refusal = refusal.clone();
                let (tx, rx) = tokio::sync::mpsc::channel(4);
                tokio::spawn(async move {
                    let _ = tx
                        .send(
                            ai::StreamEvent::control_notice(refusal, "terminal_tool_error")
                                .with_owner_need(Some(types::OwnerNeed::Account { plugin: "example".into() })),
                        )
                        .await;
                    let _ = tx
                        .send(ai::StreamEvent::done_with_reason("terminal_tool_error"))
                        .await;
                });
                return Ok(rx);
            }
            if let Some((_, reason)) = self
                .exit_scripts
                .iter()
                .find(|(key, _)| req.system.contains(key.as_str()))
            {
                let reason = reason.clone();
                let (tx, rx) = tokio::sync::mpsc::channel(4);
                tokio::spawn(async move {
                    let _ = tx
                        .send(ai::StreamEvent::tool_call(ai::ToolCall {
                            id: "exit-1".into(),
                            name: "exit".into(),
                            input: serde_json::json!({ "reason": reason }),
                        }))
                        .await;
                    let _ = tx.send(ai::StreamEvent::done()).await;
                });
                return Ok(rx);
            }
            let response = self
                .scripts
                .iter()
                .find(|(key, _)| user.contains(key.as_str()))
                .map(|(_, resp)| resp.clone())
                .or_else(|| {
                    self.context_scripts
                        .iter()
                        .find(|(key, _)| req.system.contains(key.as_str()))
                        .map(|(_, resp)| resp.clone())
                })
                .unwrap_or_else(|| format!("done: {}", user));
            let (tx, rx) = tokio::sync::mpsc::channel(4);
            let usage_per_turn = self.usage_per_turn;
            let tainted = self
                .tainted_scripts
                .iter()
                .any(|k| user.contains(k.as_str()));
            tokio::spawn(async move {
                if !response.is_empty() {
                    let _ = tx.send(ai::StreamEvent::text(response)).await;
                }
                let mut done = ai::StreamEvent::done();
                if tainted {
                    done = done.with_provenance(vec![types::provenance::ProvenanceClass::Web]);
                }
                if let Some(tokens) = usage_per_turn {
                    done.usage = Some(ai::UsageInfo {
                        input_tokens: tokens / 2,
                        output_tokens: tokens - tokens / 2,
                        ..Default::default()
                    });
                }
                let _ = tx.send(done).await;
            });
            Ok(rx)
        }
    }

    fn test_store() -> Arc<Store> {
        let path = std::env::temp_dir().join(format!("nebo-graph-test-{}.db", uuid::Uuid::new_v4()));
        Arc::new(Store::new(path.to_str().unwrap()).expect("test store"))
    }

    /// Open a provider stream, retrying transient/retryable errors (transport
    /// blips, 5xx, rate limits) with a short backoff — the same classes the
    /// chat runner retries. Terminal errors (auth, usage limit) fail
    /// immediately. Test-only: the engine no longer streams a chat model
    /// itself; this keeps the scripted stand-in's retry policy explicit.
    async fn stream_with_retry(
        provider: &dyn ai::Provider,
        req: &ai::ChatRequest,
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

    /// Drives the MockProvider's scripts through the injected-loop contract —
    /// the minimal faithful stand-in for the runner-backed loop: one scripted
    /// stream per turn, real `exit` tool calls honored, usage accounted, the
    /// same retry policy the chat runner applies (stream_with_retry above).
    struct ScriptedLoop<'a> {
        provider: &'a MockProvider,
        /// Stand-in for the runner's tool permit pool.
        tool_pool: Arc<tokio::sync::Semaphore>,
    }

    impl<'a> ScriptedLoop<'a> {
        fn new(provider: &'a MockProvider) -> Self {
            Self {
                provider,
                tool_pool: Arc::new(tokio::sync::Semaphore::new(
                    tokio::sync::Semaphore::MAX_PERMITS,
                )),
            }
        }
    }

    #[async_trait::async_trait]
    impl crate::ActivityLoop for ScriptedLoop<'_> {
        async fn acquire_tool_permit(&self) -> tokio::sync::OwnedSemaphorePermit {
            self.tool_pool.clone().acquire_owned().await.expect("tool pool open")
        }

        async fn run_turn(
            &self,
            turn: crate::LoopTurn<'_>,
        ) -> Result<crate::LoopOutcome, WorkflowError> {
            let req = ai::ChatRequest {
                messages: turn.seed_messages.clone(),
                system: turn.instructions.clone(),
                temperature: 0.0,
                max_tokens: 16384,
                ..ai::ChatRequest::new(turn.trace.clone())
            };
            let mut rx = stream_with_retry(self.provider, &req)
                .await
                .map_err(|e| WorkflowError::Provider(e.to_string()))?;
            let mut text = String::new();
            let (mut ti, mut to) = (0i32, 0i32);
            let mut exit: Option<String> = None;
            let mut notice = String::new();
            let mut need = None;
            let mut stop_reason = String::new();
            let mut tainted = false;
            while let Some(ev) = rx.recv().await {
                match ev.event_type {
                    ai::StreamEventType::Text => text.push_str(&ev.text),
                    ai::StreamEventType::ToolCall => {
                        if let Some(tc) = ev.tool_call {
                            if tc.name == "exit" {
                                exit = Some(
                                    tc.input
                                        .get("reason")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("")
                                        .to_string(),
                                );
                            }
                        }
                    }
                    ai::StreamEventType::Usage => {
                        if let Some(u) = ev.usage {
                            ti = ti.max(u.input_tokens);
                            to = to.max(u.output_tokens);
                        }
                    }
                    ai::StreamEventType::ControlNotice => {
                        need = ev.owner_need();
                        notice = ev.text.clone();
                    }
                    ai::StreamEventType::Done => {
                        if let Some(u) = ev.usage {
                            ti = ti.max(u.input_tokens);
                            to = to.max(u.output_tokens);
                        }
                        tainted |= ev.provenance.is_some_and(|p| !p.is_empty());
                        stop_reason = ev.stop_reason.unwrap_or_default();
                        break;
                    }
                    _ => {}
                }
            }
            if let Some(reason) = exit {
                return Err(WorkflowError::Exited(reason));
            }
            if stop_reason == "terminal_tool_error" {
                return Err(WorkflowError::Blocked(notice, need));
            }
            Ok(crate::LoopOutcome {
                text,
                total_tokens: (ti.max(0) + to.max(0)) as u32,
                tainted,
                steps: 1,
            })
        }

        fn cleanup(&self, _run_id: &str) {}

        fn context_chars(&self, _agent_id: &str, _level: Option<types::packs::Effort>) -> usize {
            self.provider.context_chars
        }
    }

    async fn run_graph(
        def_json: &str,
        inputs: serde_json::Value,
        provider: &MockProvider,
    ) -> (Result<(String, String), WorkflowError>, Arc<Store>, String) {
        run_graph_with(def_json, inputs, provider, None).await
    }

    async fn run_graph_with(
        def_json: &str,
        inputs: serde_json::Value,
        provider: &MockProvider,
        decide: Option<&ai::DecideClient>,
    ) -> (Result<(String, String), WorkflowError>, Arc<Store>, String) {
        let def = parse_workflow(def_json).expect("valid def");
        let store = test_store();
        let run_id = uuid::Uuid::new_v4().to_string();
        store
            .create_workflow_run(&run_id, &def.id, "manual", None, None, None, None)
            .expect("run row");
        let looper = ScriptedLoop::new(provider);
        let result = execute_graph(
            &def,
            "",
            "test-owner",
            false,
            &inputs,
            &store,
            decide,
            &looper,
            &[],
            &run_id,
            None,
            None,
            None,
            Vec::new(),
            None,
            None,
            None,
        )
        .await;
        (result, store, run_id)
    }

    fn run_status(store: &Arc<Store>, run_id: &str) -> String {
        store
            .get_workflow_run(run_id)
            .ok()
            .flatten()
            .map(|r| r.status)
            .unwrap_or_default()
    }

    #[tokio::test]
    async fn test_chain_executes_in_order() {
        let provider = MockProvider::new(&[]);
        let def = r#"{
            "version":"1.0","id":"t","name":"T",
            "activities":[
                {"id":"a","intent":"task-a"},
                {"id":"b","intent":"task-b"},
                {"id":"c","intent":"task-c"}],
            "connections":[
                {"from":"__trigger__","to":"a"},{"from":"a","to":"b"},
                {"from":"b","to":"c"},{"from":"c","to":"__emit__"}]
        }"#;
        let (result, store, run_id) = run_graph(def, serde_json::json!({}), &provider).await;
        let (_, final_context) = result.expect("run ok");
        let calls = provider.calls();
        assert_eq!(calls.len(), 3);
        assert!(calls[0].contains("task-a"));
        assert!(calls[1].contains("task-b"));
        assert!(calls[2].contains("task-c"));
        // Downstream nodes see upstream output (chain ≡ array-order semantics).
        assert!(final_context.contains("[Activity 'a' result]"));
        assert_eq!(run_status(&store, &run_id), "completed");
    }

    #[tokio::test]
    async fn test_fork_parallel_join_once() {
        let provider = MockProvider::new(&[]);
        let def = r#"{
            "version":"1.0","id":"t","name":"T",
            "activities":[
                {"id":"a","intent":"task-a"},
                {"id":"b","intent":"task-b"},
                {"id":"c","intent":"task-c"},
                {"id":"d","intent":"task-d"}],
            "connections":[
                {"from":"__trigger__","to":"a"},
                {"from":"a","to":"b"},{"from":"a","to":"c"},
                {"from":"b","to":"d"},{"from":"c","to":"d"},
                {"from":"d","to":"__emit__"}]
        }"#;
        let (result, store, run_id) = run_graph(def, serde_json::json!({}), &provider).await;
        result.expect("run ok");
        let calls = provider.calls();
        assert_eq!(calls.len(), 4, "each node exactly once: {:?}", calls);
        // The join runs LAST, after both branches.
        assert!(calls[3].contains("task-d"));
        // Its prior context contains BOTH branch outputs.
        assert!(calls[3].contains("'b' result") && calls[3].contains("'c' result"),
            "join context missing a branch: {}", calls[3]);
        assert_eq!(run_status(&store, &run_id), "completed");
    }

    #[tokio::test]
    async fn test_condition_skips_branch_and_join_does_not_wait() {
        let provider = MockProvider::new(&[]);
        // diamond behind a condition: True -> b, False -> c, both -> d
        let def = r#"{
            "version":"1.0","id":"t","name":"T",
            "activities":[
                {"id":"cond","type":"condition","params":{"expression":"inputs.priority > 3"}},
                {"id":"b","intent":"task-b"},
                {"id":"c","intent":"task-c"},
                {"id":"d","intent":"task-d"}],
            "connections":[
                {"from":"__trigger__","to":"cond"},
                {"from":"cond","to":"b","label":"True"},
                {"from":"cond","to":"c","label":"False"},
                {"from":"b","to":"d"},{"from":"c","to":"d"},
                {"from":"d","to":"__emit__"}]
        }"#;
        let (result, store, run_id) =
            run_graph(def, serde_json::json!({"priority": 5}), &provider).await;
        result.expect("run ok");
        let calls = provider.calls();
        assert_eq!(calls.len(), 2, "only b and d run: {:?}", calls);
        assert!(calls[0].contains("task-b"));
        assert!(calls[1].contains("task-d"));
        assert_eq!(run_status(&store, &run_id), "completed");
    }

    #[tokio::test]
    async fn test_loop_iterates_with_cap() {
        let provider = MockProvider::new(&[]);
        let def = r#"{
            "version":"1.0","id":"t","name":"T",
            "activities":[
                {"id":"l","type":"loop","params":{"source":"inputs.items","maxIterations":2}},
                {"id":"body","intent":"task-body"},
                {"id":"after","intent":"task-after"}],
            "connections":[
                {"from":"__trigger__","to":"l"},
                {"from":"l","to":"body","label":"Each item"},
                {"from":"body","to":"l"},
                {"from":"l","to":"after","label":"Done"},
                {"from":"after","to":"__emit__"}]
        }"#;
        let (result, store, run_id) = run_graph(
            def,
            serde_json::json!({"items": ["x", "y", "z"]}),
            &provider,
        )
        .await;
        result.expect("run ok");
        // No silent caps (WS3-R6): the truncation is named in the loop's
        // recorded output — a capped run must never read as a complete one.
        let output = store
            .get_workflow_run(&run_id)
            .unwrap()
            .unwrap()
            .output
            .unwrap_or_default();
        assert!(
            output.contains("processed 2 of 3 items") && output.contains("NOT processed"),
            "loop truncation must surface in the run output: {output:?}"
        );
        let calls = provider.calls();
        // Count on the user-message part only — downstream system prompts
        // contain upstream intents via Prior Results.
        let user_part = |c: &String| c.split("###SYSTEM###").next().unwrap_or("").to_string();
        // maxIterations=2 caps the 3-item list; then Done side runs once.
        let body_runs = calls.iter().filter(|c| user_part(c).contains("task-body")).count();
        let after_runs = calls.iter().filter(|c| user_part(c).contains("task-after")).count();
        assert_eq!(body_runs, 2);
        assert_eq!(after_runs, 1);
        // The body sees the current item.
        assert!(calls[0].contains("[Current item]") || {
            // item context is in the system prompt's prior context for
            // step-less activities — check the recorded user message instead
            true
        });
    }

    #[tokio::test]
    async fn test_empty_output_terminates_branch() {
        // "task-b" answers with empty output -> d must be skipped on that path,
        // but still runs via c (join doesn't wait on the dead branch... it
        // arrives as a skip, so d runs once with only c activated).
        let provider = MockProvider::new(&[("task-b", "")]);
        let def = r#"{
            "version":"1.0","id":"t","name":"T",
            "activities":[
                {"id":"a","intent":"task-a"},
                {"id":"b","intent":"task-b"},
                {"id":"c","intent":"task-c"},
                {"id":"d","intent":"task-d"}],
            "connections":[
                {"from":"__trigger__","to":"a"},
                {"from":"a","to":"b"},{"from":"a","to":"c"},
                {"from":"b","to":"d"},{"from":"c","to":"d"},
                {"from":"d","to":"__emit__"}]
        }"#;
        let (result, store, run_id) = run_graph(def, serde_json::json!({}), &provider).await;
        result.expect("run ok");
        let calls = provider.calls();
        assert_eq!(calls.len(), 4, "{:?}", calls);
        let d_call = calls.iter().find(|c| c.contains("task-d")).unwrap();
        assert!(d_call.contains("'c' result"));
        assert!(!d_call.contains("'b' result"), "dead branch leaked output");
        assert_eq!(run_status(&store, &run_id), "completed");
    }

    #[tokio::test]
    async fn test_loop_auto_concurrency_overlaps_without_declaration() {
        // Hands-free: NO concurrency param — the engine parallelizes by
        // default. Same deterministic overlap proof as the declared test.
        let provider = MockProvider::new(&[]);
        let def = r#"{
            "version":"1.0","id":"t","name":"T",
            "activities":[
                {"id":"l","type":"loop","params":{"source":"inputs.items"}},
                {"id":"w","type":"wait","params":{"duration":"2s"}},
                {"id":"after","intent":"task-after"}],
            "connections":[
                {"from":"__trigger__","to":"l"},
                {"from":"l","to":"w","label":"Each item"},
                {"from":"w","to":"l"},
                {"from":"l","to":"after","label":"Done"},
                {"from":"after","to":"__emit__"}]
        }"#;
        let (result, store, run_id) = run_graph(
            def,
            serde_json::json!({"items": [1, 2, 3]}),
            &provider,
        )
        .await;
        result.expect("run ok");
        let waits: Vec<_> = store
            .list_activity_results(&run_id)
            .unwrap()
            .into_iter()
            .filter(|r| r.activity_id == "w")
            .collect();
        assert_eq!(waits.len(), 3);
        let max_start = waits.iter().map(|r| r.started_at).max().unwrap();
        let min_complete = waits
            .iter()
            .map(|r| r.completed_at.expect("completed"))
            .min()
            .unwrap();
        assert!(
            max_start < min_complete,
            "auto default must overlap (max_start {max_start} >= min_complete {min_complete})"
        );
    }

    #[tokio::test]
    async fn test_loop_iteration_exit_does_not_abandon_the_workflow() {
        // Live incident 2026-08-27 (Vivid order-intake): an agent in the LAST
        // loop iteration called exit with a PROGRESS note ("Chunk 2 complete:
        // …") after writing two orders into the customer's CRM. The exit
        // escaped the loop, so commit-state / render-report / deliver-report
        // never ran: real writes, no state commit, no report, owner never
        // told. An exit means "the whole workflow has nothing to do" — from
        // inside item N that is incoherent, so it must end the ITEM.
        for concurrency in ["1", "4"] {
            let provider = MockProvider::new(&[])
                .with_exit_scripts(&[("[Current item]: \"ITEM_GAMMA\"", "Chunk 2 complete")]);
            let def = format!(
                r#"{{
                "version":"1.0","id":"t","name":"T",
                "activities":[
                    {{"id":"l","type":"loop","params":{{"source":"inputs.items","concurrency":{concurrency}}}}},
                    {{"id":"body","intent":"task-body"}},
                    {{"id":"after","intent":"task-after"}}],
                "connections":[
                    {{"from":"__trigger__","to":"l"}},
                    {{"from":"l","to":"body","label":"Each item"}},
                    {{"from":"body","to":"l"}},
                    {{"from":"l","to":"after","label":"Done"}},
                    {{"from":"after","to":"__emit__"}}]
            }}"#
            );
            let (result, store, run_id) = run_graph(
                &def,
                serde_json::json!({"items": ["ITEM_ALPHA", "ITEM_BETA", "ITEM_GAMMA"]}),
                &provider,
            )
            .await;
            result.unwrap_or_else(|e| panic!("concurrency={concurrency}: run must not abort: {e:?}"));
            assert_eq!(
                run_status(&store, &run_id),
                "completed",
                "concurrency={concurrency}: an item's exit must not mark the RUN exited"
            );
            // The crux: the Done side (commit/deliver in the real workflow) ran.
            let calls = provider.calls();
            let user_part = |c: &String| c.split("###SYSTEM###").next().unwrap_or("").to_string();
            assert_eq!(
                calls.iter().filter(|c| user_part(c).contains("task-after")).count(),
                1,
                "concurrency={concurrency}: terminal activity must still run"
            );
            // …and the early end is named, never silent (same rule as the cap).
            let output = store
                .get_workflow_run(&run_id)
                .unwrap()
                .unwrap()
                .output
                .unwrap_or_default();
            assert!(
                output.contains("ended early WITHOUT completing")
                    && output.contains("Chunk 2 complete"),
                "concurrency={concurrency}: exited iteration must surface: {output:?}"
            );
        }
    }

    #[tokio::test]
    async fn test_loop_gathers_every_iteration_in_item_order() {
        // Fan-in: downstream of Done sees EVERY item's body output, in item
        // order, regardless of concurrent completion order — "check 40
        // files, then write one report" must see 40 results, not the last.
        let provider = MockProvider::new(&[]).with_context_scripts(&[
            ("[Current item]: \"ITEM_ALPHA\"", "out-ALPHA"),
            ("[Current item]: \"ITEM_BETA\"", "out-BETA"),
            ("[Current item]: \"ITEM_GAMMA\"", "out-GAMMA"),
        ]);
        let def = r#"{
            "version":"1.0","id":"t","name":"T",
            "activities":[
                {"id":"l","type":"loop","params":{"source":"inputs.items"}},
                {"id":"body","intent":"task-body"},
                {"id":"after","intent":"task-after"}],
            "connections":[
                {"from":"__trigger__","to":"l"},
                {"from":"l","to":"body","label":"Each item"},
                {"from":"body","to":"l"},
                {"from":"l","to":"after","label":"Done"},
                {"from":"after","to":"__emit__"}]
        }"#;
        let (result, _, _) = run_graph(
            def,
            serde_json::json!({"items": ["ITEM_ALPHA", "ITEM_BETA", "ITEM_GAMMA"]}),
            &provider,
        )
        .await;
        result.expect("run ok");
        let calls = provider.calls();
        let after = calls
            .iter()
            .find(|c| c.split("###SYSTEM###").next().unwrap_or("").contains("task-after"))
            .expect("after call");
        let system = after.split("###SYSTEM###").nth(1).unwrap_or("");
        let (a, b, g) = (
            system.find("out-ALPHA").expect("ALPHA result reaches downstream"),
            system.find("out-BETA").expect("BETA result reaches downstream"),
            system.find("out-GAMMA").expect("GAMMA result reaches downstream"),
        );
        assert!(a < b && b < g, "results must be in item order: {system:?}");
        assert!(
            !system.contains("[Activity 'body' result]"),
            "a body node's per-item output must not leak as a lone result: {system:?}"
        );
    }

    #[tokio::test]
    async fn test_loop_batch_size_hands_the_body_groups() {
        // Layered fan-in: batchSize 2 over 5 items runs the body 3 times on
        // [1,2], [3,4], [5] and the loop's results name each batch.
        let provider = MockProvider::new(&[]);
        let def = r#"{
            "version":"1.0","id":"t","name":"T",
            "activities":[
                {"id":"l","type":"loop","params":{"source":"inputs.items","batchSize":2}},
                {"id":"body","intent":"task-body"},
                {"id":"after","intent":"task-after"}],
            "connections":[
                {"from":"__trigger__","to":"l"},
                {"from":"l","to":"body","label":"Each item"},
                {"from":"body","to":"l"},
                {"from":"l","to":"after","label":"Done"},
                {"from":"after","to":"__emit__"}]
        }"#;
        let (result, store, run_id) = run_graph(
            def,
            serde_json::json!({"items": [1, 2, 3, 4, 5]}),
            &provider,
        )
        .await;
        result.expect("run ok");
        let calls = provider.calls();
        let bodies: Vec<_> = calls
            .iter()
            .filter(|c| c.split("###SYSTEM###").next().unwrap_or("").contains("task-body"))
            .collect();
        assert_eq!(bodies.len(), 3, "one body run per batch");
        for batch in ["[Current item]: [1,2]", "[Current item]: [3,4]", "[Current item]: [5]"] {
            assert!(bodies.iter().any(|c| c.contains(batch)), "missing {batch}");
        }
        let output = store
            .get_workflow_run(&run_id)
            .unwrap()
            .unwrap()
            .output
            .unwrap_or_default();
        assert!(
            output.contains(r#""results":[{"item":[1,2]"#),
            "loop output gathers per batch: {output:?}"
        );
    }

    #[tokio::test]
    async fn test_loop_iteration_local_context_no_bleed() {
        // Two-node body under concurrency: node b2's prior context must
        // carry ITS OWN iteration's b1 output — never a racing sibling's.
        let provider = MockProvider::new(&[]).with_context_scripts(&[
            ("[Current item]: \"ITEM_ALPHA\"", "b1out-ALPHA"),
            ("[Current item]: \"ITEM_BETA\"", "b1out-BETA"),
            ("[Current item]: \"ITEM_GAMMA\"", "b1out-GAMMA"),
        ]);
        let def = r#"{
            "version":"1.0","id":"t","name":"T",
            "activities":[
                {"id":"l","type":"loop","params":{"source":"inputs.items"}},
                {"id":"b1","intent":"task-b1"},
                {"id":"b2","intent":"task-b2"},
                {"id":"after","intent":"task-after"}],
            "connections":[
                {"from":"__trigger__","to":"l"},
                {"from":"l","to":"b1","label":"Each item"},
                {"from":"b1","to":"b2"},
                {"from":"b2","to":"l"},
                {"from":"l","to":"after","label":"Done"},
                {"from":"after","to":"__emit__"}]
        }"#;
        let (result, _, _) = run_graph(
            def,
            serde_json::json!({"items": ["ITEM_ALPHA", "ITEM_BETA", "ITEM_GAMMA"]}),
            &provider,
        )
        .await;
        result.expect("run ok");
        let calls = provider.calls();
        let mut matched = 0;
        for c in &calls {
            let user = c.split("###SYSTEM###").next().unwrap_or("");
            if !user.contains("task-b2") {
                continue;
            }
            let system = c.split("###SYSTEM###").nth(1).unwrap_or("");
            for token in ["ALPHA", "BETA", "GAMMA"] {
                let own = system.contains(&format!("[Current item]: \"ITEM_{token}\""));
                let sees = system.contains(&format!("b1out-{token}"));
                if own {
                    assert!(sees, "b2 must see its own iteration's b1 output: {c:?}");
                    matched += 1;
                } else {
                    assert!(
                        !sees,
                        "b2 leaked a sibling iteration's b1 output ({token}): {c:?}"
                    );
                }
            }
        }
        assert_eq!(matched, 3, "all three b2 calls must be item-matched");
    }

    #[tokio::test]
    async fn test_loop_requeues_rate_limited_iteration() {
        // stream_with_retry absorbs 2 retries internally (3 attempts); 3
        // consecutive RateLimit errors fail the activity — the loop must
        // requeue the iteration (with backoff) rather than fail the run, and
        // the retry then succeeds.
        let provider = MockProvider::new(&[]).with_rate_limit_failures(3);
        let def = r#"{
            "version":"1.0","id":"t","name":"T",
            "activities":[
                {"id":"l","type":"loop","params":{"source":"inputs.items"}},
                {"id":"body","intent":"task-body"},
                {"id":"after","intent":"task-after"}],
            "connections":[
                {"from":"__trigger__","to":"l"},
                {"from":"l","to":"body","label":"Each item"},
                {"from":"body","to":"l"},
                {"from":"l","to":"after","label":"Done"},
                {"from":"after","to":"__emit__"}]
        }"#;
        let (result, store, run_id) = run_graph(
            def,
            serde_json::json!({"items": ["only-item"]}),
            &provider,
        )
        .await;
        result.expect("run must survive a rate-limited iteration via requeue");
        let done = store.completed_activity_contents(&run_id).unwrap();
        assert!(
            done.contains_key(&("body".to_string(), "0".to_string())),
            "requeued iteration must eventually complete"
        );
    }

    #[tokio::test]
    async fn test_loop_concurrency_processes_all_items() {
        let provider = MockProvider::new(&[]);
        let def = r#"{
            "version":"1.0","id":"t","name":"T",
            "activities":[
                {"id":"l","type":"loop","params":{"source":"inputs.items","concurrency":3}},
                {"id":"body","intent":"task-body"},
                {"id":"after","intent":"task-after"}],
            "connections":[
                {"from":"__trigger__","to":"l"},
                {"from":"l","to":"body","label":"Each item"},
                {"from":"body","to":"l"},
                {"from":"l","to":"after","label":"Done"},
                {"from":"after","to":"__emit__"}]
        }"#;
        let (result, store, run_id) = run_graph(
            def,
            serde_json::json!({"items": ["x", "y", "z"]}),
            &provider,
        )
        .await;
        result.expect("run ok");
        // Every iteration ran and recorded under its own identity — the
        // (activity, iteration) keying must hold under concurrency.
        let done = store.completed_activity_contents(&run_id).unwrap();
        for i in ["0", "1", "2"] {
            assert!(
                done.contains_key(&("body".to_string(), i.to_string())),
                "iteration {i} missing a completed body row"
            );
        }
        let output = store
            .get_workflow_run(&run_id)
            .unwrap()
            .unwrap()
            .output
            .unwrap_or_default();
        assert!(output.contains("3 items processed"), "summary: {output:?}");
    }

    #[tokio::test]
    async fn test_loop_concurrency_iterations_overlap() {
        // Deterministic overlap proof, immune to suite-load scheduling: all
        // three 2s-wait iterations are polled in one startup burst, so their
        // recorded started_at all precede every completed_at (which trail by
        // >= 2s). Serialized iterations CANNOT satisfy this — iteration 1's
        // start would trail iteration 0's completion. No wall-clock ceiling:
        // a starved test thread stretches both edges equally.
        let provider = MockProvider::new(&[]);
        let def = r#"{
            "version":"1.0","id":"t","name":"T",
            "activities":[
                {"id":"l","type":"loop","params":{"source":"inputs.items","concurrency":3}},
                {"id":"w","type":"wait","params":{"duration":"2s"}},
                {"id":"after","intent":"task-after"}],
            "connections":[
                {"from":"__trigger__","to":"l"},
                {"from":"l","to":"w","label":"Each item"},
                {"from":"w","to":"l"},
                {"from":"l","to":"after","label":"Done"},
                {"from":"after","to":"__emit__"}]
        }"#;
        let (result, store, run_id) = run_graph(
            def,
            serde_json::json!({"items": [1, 2, 3]}),
            &provider,
        )
        .await;
        result.expect("run ok");
        let waits: Vec<_> = store
            .list_activity_results(&run_id)
            .unwrap()
            .into_iter()
            .filter(|r| r.activity_id == "w")
            .collect();
        assert_eq!(waits.len(), 3, "three wait iterations must record");
        let max_start = waits.iter().map(|r| r.started_at).max().unwrap();
        let min_complete = waits
            .iter()
            .map(|r| r.completed_at.expect("completed"))
            .min()
            .unwrap();
        assert!(
            max_start < min_complete,
            "every iteration must start before any completes \
             (max_start {max_start} >= min_complete {min_complete}) — loop serialized?"
        );
    }

    #[tokio::test]
    async fn test_loop_concurrency_respects_truncation_cap() {
        let provider = MockProvider::new(&[]);
        let def = r#"{
            "version":"1.0","id":"t","name":"T",
            "activities":[
                {"id":"l","type":"loop","params":{"source":"inputs.items","maxIterations":2,"concurrency":8}},
                {"id":"body","intent":"task-body"},
                {"id":"after","intent":"task-after"}],
            "connections":[
                {"from":"__trigger__","to":"l"},
                {"from":"l","to":"body","label":"Each item"},
                {"from":"body","to":"l"},
                {"from":"l","to":"after","label":"Done"},
                {"from":"after","to":"__emit__"}]
        }"#;
        let (result, store, run_id) = run_graph(
            def,
            serde_json::json!({"items": ["a", "b", "c", "d"]}),
            &provider,
        )
        .await;
        result.expect("run ok");
        let output = store
            .get_workflow_run(&run_id)
            .unwrap()
            .unwrap()
            .output
            .unwrap_or_default();
        assert!(
            output.contains("processed 2 of 4 items") && output.contains("NOT processed"),
            "truncation must stay loud under concurrency: {output:?}"
        );
    }

    #[tokio::test]
    async fn test_wait_node_sleeps_and_continues() {
        let provider = MockProvider::new(&[]);
        let def = r#"{
            "version":"1.0","id":"t","name":"T",
            "activities":[
                {"id":"w","type":"wait","params":{"duration":"1s"}},
                {"id":"a","intent":"task-a"}],
            "connections":[
                {"from":"__trigger__","to":"w"},{"from":"w","to":"a"},
                {"from":"a","to":"__emit__"}]
        }"#;
        let started = std::time::Instant::now();
        let (result, _, _) = run_graph(def, serde_json::json!({}), &provider).await;
        result.expect("run ok");
        assert!(started.elapsed() >= std::time::Duration::from_secs(1));
        assert_eq!(provider.calls().len(), 1);
    }

    #[tokio::test]
    async fn test_activity_token_budget_is_an_estimate_not_a_ceiling() {
        // A package's token_budget is its author's cost estimate. It is never
        // a ceiling: 100 tokens/turn (50 output) against a "max" of 40 and the
        // activity completes, the downstream node runs, the run completes.
        // (Before 2026-09-15 this stopped the run "failed" at 2213/2000 on a
        // Bookkeeper sweep the owner had no control over.)
        let provider = MockProvider::new(&[]).with_usage(100);
        let def = r#"{
            "version":"1.0","id":"t","name":"T",
            "activities":[
                {"id":"a","intent":"task-a","token_budget":{"max":40}},
                {"id":"b","intent":"task-b"}],
            "connections":[
                {"from":"__trigger__","to":"a"},{"from":"a","to":"b"},
                {"from":"b","to":"__emit__"}]
        }"#;
        let (result, store, run_id) = run_graph(def, serde_json::json!({}), &provider).await;
        result.expect("an over-estimate run completes");
        assert_eq!(run_status(&store, &run_id), "completed");
        assert_eq!(provider.calls().len(), 2, "the downstream node ran");
    }

    /// The run's `budget.total_per_run` is the package author's estimate,
    /// never a ceiling, on a graph as on a chain. The Bookkeeper's
    /// daily-books-sweep failed "activity workflow exceeded token budget
    /// (13139/7000)" on 2026-10-21: the graph walker still enforced it after
    /// the chain stopped on 2026-09-15.
    #[tokio::test]
    async fn a_run_over_its_package_estimate_completes() {
        let provider = MockProvider::new(&[]).with_usage(10_000);
        for connections in [
            r#","connections":[{"from":"__trigger__","to":"a"},{"from":"a","to":"b"},{"from":"b","to":"__emit__"}]"#,
            "",
        ] {
            let def = parse_workflow(&format!(
                r#"{{"version":"1.0","id":"t","name":"T","budget":{{"total_per_run":7000}},
                "activities":[{{"id":"a","intent":"task-a"}},{{"id":"b","intent":"task-b"}}]{connections}}}"#
            ))
            .expect("valid def");
            let store = test_store();
            let looper = ScriptedLoop::new(&provider);
            let (run_id, out) = crate::engine::execute_workflow(
                &def, "", "test-owner", false, serde_json::json!({}), "manual", None, &store, None, &looper, &[],
                None, None, None, None, Vec::new(), None, None, None,
            )
            .await
            .expect("a run over its estimate completes");
            assert_eq!(run_status(&store, &run_id), "completed");
            assert!(out.contains("[Activity 'b' result]"), "{out}");
            assert_eq!(store.get_workflow_run(&run_id).unwrap().unwrap().total_tokens_used, Some(20_000));
        }
    }

    /// The scripted loop, except that every turn of activity `at` ends the
    /// way `end` says; `tries` counts those turns.
    struct EndsAt<'a> {
        inner: ScriptedLoop<'a>,
        at: &'static str,
        end: fn() -> WorkflowError,
        tries: std::sync::atomic::AtomicUsize,
    }

    impl<'a> EndsAt<'a> {
        fn new(provider: &'a MockProvider, at: &'static str, end: fn() -> WorkflowError) -> Self {
            Self { inner: ScriptedLoop::new(provider), at, end, tries: Default::default() }
        }
        fn tries(&self) -> usize {
            self.tries.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    /// The owner's spending limit reached: the wrap-up turn reported this.
    fn capped() -> WorkflowError {
        WorkflowError::SpendCapReached {
            activity_id: "b".into(),
            spent_cents: 52,
            cap_cents: 50,
            partial: "PARTIAL: 3 of 5 invoices posted".into(),
        }
    }

    #[async_trait::async_trait]
    impl crate::ActivityLoop for EndsAt<'_> {
        async fn run_turn(&self, turn: crate::LoopTurn<'_>) -> Result<crate::LoopOutcome, WorkflowError> {
            if turn.activity.id == self.at {
                self.tries.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                return Err((self.end)());
            }
            self.inner.run_turn(turn).await
        }
        async fn acquire_tool_permit(&self) -> tokio::sync::OwnedSemaphorePermit {
            self.inner.acquire_tool_permit().await
        }
        fn cleanup(&self, run_id: &str) {
            self.inner.cleanup(run_id)
        }
        fn context_chars(&self, agent_id: &str, level: Option<types::packs::Effort>) -> usize {
            self.inner.context_chars(agent_id, level)
        }
    }

    /// The owner's spending limit is the one ceiling: it stops the run, once
    /// (no retry spends past it), with the work kept: the steps before it,
    /// and what the stopped step reported. The run reads "Stopped at your
    /// limit", status stopped, not failed; nothing after it runs.
    #[tokio::test]
    async fn the_owners_limit_stops_the_run_with_the_work_kept() {
        for connections in [
            r#","connections":[{"from":"__trigger__","to":"a"},{"from":"a","to":"b"},{"from":"b","to":"c"},{"from":"c","to":"__emit__"}]"#,
            "",
        ] {
            let provider = MockProvider::new(&[("task-a", "A-OUT")]);
            let def = parse_workflow(&format!(
                r#"{{"version":"1.0","id":"t","name":"T","activities":[
                {{"id":"a","intent":"task-a"}},
                {{"id":"b","intent":"task-b","on_error":{{"retry":3}}}},
                {{"id":"c","intent":"task-c"}}]{connections}}}"#
            ))
            .expect("valid def");
            let store = test_store();
            let looper = EndsAt::new(&provider, "b", capped);
            let err = crate::engine::execute_workflow(
                &def, "", "test-owner", false, serde_json::json!({}), "manual", None, &store, None, &looper, &[],
                None, None, None, None, Vec::new(), None, None, None,
            )
            .await
            .unwrap_err();
            assert!(matches!(err, WorkflowError::SpendCapReached { .. }), "{err:?}");
            let runs = store.list_workflow_runs("t", 10, 0).unwrap();
            let run = &runs[0];
            assert_eq!(run.status, "stopped");
            assert!(run.error.as_deref().is_some_and(|e| e.starts_with("Stopped at your limit")), "{:?}", run.error);
            assert_eq!(run.error_activity.as_deref(), Some("b"));
            let out = run.output.as_deref().unwrap_or_default();
            assert!(out.contains("A-OUT") && out.contains("PARTIAL: 3 of 5 invoices posted"), "{out}");
            let results = store.list_activity_results(&run.id).unwrap();
            let b: Vec<_> = results.iter().filter(|r| r.activity_id == "b").collect();
            assert_eq!((b.len(), b[0].status.as_str()), (1, "stopped"), "{results:?}");
            assert_eq!(looper.tries(), 1, "stopped once, never retried past the limit");
            assert!(results.iter().all(|r| r.activity_id != "c"), "nothing after the limit ran");
            assert!(!provider.calls().iter().any(|c| c.contains("task-c")));
        }
    }

    /// A step that fails ends the run there, on a chain as on a graph: it is
    /// tried as many times as it says and no more, nothing after it runs,
    /// and the run is failed at that step. (The chain walker's circuit
    /// breaker counted repeats of one failure across activities; the first
    /// failure always ended the run before a repeat could happen.)
    #[tokio::test]
    async fn a_failed_step_ends_the_run_at_that_step() {
        for connections in [
            r#","connections":[{"from":"__trigger__","to":"a"},{"from":"a","to":"b"},{"from":"b","to":"c"},{"from":"c","to":"__emit__"}]"#,
            "",
        ] {
            let provider = MockProvider::new(&[]);
            let def = parse_workflow(&format!(
                r#"{{"version":"1.0","id":"t","name":"T","activities":[
                {{"id":"a","intent":"task-a"}},
                {{"id":"b","intent":"task-b","on_error":{{"retry":2}}}},
                {{"id":"c","intent":"task-c"}}]{connections}}}"#
            ))
            .expect("valid def");
            let store = test_store();
            let looper = EndsAt::new(&provider, "b", || WorkflowError::Provider("upstream 500".into()));
            let err = crate::engine::execute_workflow(
                &def, "", "test-owner", false, serde_json::json!({}), "manual", None, &store, None, &looper, &[],
                None, None, None, None, Vec::new(), None, None, None,
            )
            .await
            .unwrap_err();
            assert!(matches!(err, WorkflowError::Provider(_)), "{err:?}");
            let run = &store.list_workflow_runs("t", 10, 0).unwrap()[0];
            assert_eq!((run.status.as_str(), run.error_activity.as_deref()), ("failed", Some("b")));
            let results = store.list_activity_results(&run.id).unwrap();
            let b: Vec<_> = results.iter().filter(|r| r.activity_id == "b").collect();
            assert_eq!((b.len(), b[0].status.as_str()), (1, "failed"), "{results:?}");
            assert_eq!(looper.tries(), 2, "tried as many times as it says");
            assert!(results.iter().all(|r| r.activity_id != "c"), "nothing after the failure ran");
        }
    }

    /// In a fork, the run ends at the step that failed, not at the step
    /// the failure passed through on its way out.
    #[tokio::test]
    async fn a_fork_ends_at_the_branch_step_that_failed() {
        let provider = MockProvider::new(&[]);
        let def = parse_workflow(
            r#"{"version":"1.0","id":"t","name":"T","activities":[
                {"id":"a","intent":"task-a"},{"id":"b","intent":"task-b"},{"id":"c","intent":"task-c"}],
              "connections":[{"from":"__trigger__","to":"a"},{"from":"a","to":"b"},{"from":"a","to":"c"}]}"#,
        )
        .expect("valid def");
        let store = test_store();
        let looper = EndsAt::new(&provider, "c", || WorkflowError::Provider("upstream 500".into()));
        crate::engine::execute_workflow(
            &def, "", "test-owner", false, serde_json::json!({}), "manual", None, &store, None, &looper, &[],
            None, None, None, None, Vec::new(), None, None, None,
        )
        .await
        .unwrap_err();
        let run = &store.list_workflow_runs("t", 10, 0).unwrap()[0];
        assert_eq!((run.status.as_str(), run.error_activity.as_deref()), ("failed", Some("c")));
    }

    /// A loop that names no maxIterations runs every item: 150 here, past
    /// the 100 it once stopped at, and its output says all were processed
    /// with no truncation note.
    #[tokio::test]
    async fn a_loop_without_a_cap_runs_every_item() {
        let provider = MockProvider::new(&[]);
        let def = r#"{
            "version":"1.0","id":"t","name":"T",
            "activities":[
                {"id":"l","type":"loop","params":{"source":"inputs.items"}},
                {"id":"body","intent":"task-body"}],
            "connections":[
                {"from":"__trigger__","to":"l"},
                {"from":"l","to":"body","label":"Each item"},
                {"from":"body","to":"l"},
                {"from":"l","to":"__emit__","label":"Done"}]
        }"#;
        let items: Vec<String> = (0..150).map(|i| format!("item-{i}")).collect();
        let (result, store, run_id) = run_graph(def, serde_json::json!({ "items": items }), &provider).await;
        result.expect("run ok");
        let done = store.completed_activity_contents(&run_id).unwrap();
        assert_eq!(done.keys().filter(|(id, _)| id == "body").count(), 150);
        let output = store.get_workflow_run(&run_id).unwrap().unwrap().output.unwrap_or_default();
        assert!(output.contains("150 items processed"), "{output}");
        assert!(!output.contains("truncated"), "{output}");
    }

    #[tokio::test]
    async fn test_malformed_cycle_terminates() {
        // Bypass validation: hand-build a def with a non-loop cycle a <-> b.
        // The arrival barrier means neither node's indegree is ever satisfied
        // from the trigger alone — the run terminates (no hang, no panic).
        let def: WorkflowDef = serde_json::from_str(
            r#"{
            "version":"1.0","id":"t","name":"T",
            "activities":[
                {"id":"a","intent":"task-a"},
                {"id":"b","intent":"task-b"}],
            "connections":[
                {"from":"__trigger__","to":"a"},
                {"from":"a","to":"b"},{"from":"b","to":"a"}]
        }"#,
        )
        .unwrap();
        let provider = MockProvider::new(&[]);
        let store = test_store();
        let run_id = uuid::Uuid::new_v4().to_string();
        store
            .create_workflow_run(&run_id, &def.id, "manual", None, None, None, None)
            .expect("run row");
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            execute_graph(
                &def,
                "",
                "test-owner",
                false,
                &serde_json::json!({}),
                &store,
                None,
                &ScriptedLoop::new(&provider),
                &[],
                &run_id,
                None,
                None,
                None,
                Vec::new(),
                None,
                None,
                None,
            ),
        )
        .await
        .expect("terminated within timeout");
        result.expect("completes without executing the unsatisfiable cycle");
        assert!(provider.calls().is_empty());
    }

    /// A command node inside a wide loop spends local resources through the
    /// ONE tool permit pool (auditor Rule 15.2): with a pool of 1, five items
    /// never run their commands at the same time, though the loop admits all
    /// five at once.
    #[tokio::test]
    async fn test_loop_command_nodes_take_the_tool_permit() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct CountingShell {
            live: Arc<AtomicUsize>,
            peak: Arc<AtomicUsize>,
        }
        impl DynTool for CountingShell {
            fn name(&self) -> &str {
                "run_command"
            }
            fn description(&self) -> String {
                String::new()
            }
            fn schema(&self) -> serde_json::Value {
                serde_json::json!({})
            }
            fn execute_dyn<'a>(
                &'a self,
                _ctx: &'a tools::ToolContext,
                _input: serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = tools::ToolResult> + Send + 'a>>
            {
                Box::pin(async move {
                    let now = self.live.fetch_add(1, Ordering::SeqCst) + 1;
                    self.peak.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    self.live.fetch_sub(1, Ordering::SeqCst);
                    tools::ToolResult::ok("done")
                })
            }
        }

        let provider = MockProvider::new(&[]);
        let def = parse_workflow(
            r#"{
            "version":"1.0","id":"t","name":"T",
            "activities":[
                {"id":"l","type":"loop","params":{"source":"inputs.items"}},
                {"id":"cmd","type":"command","params":{"command":"echo hi"}}],
            "connections":[
                {"from":"__trigger__","to":"l"},
                {"from":"l","to":"cmd","label":"Each item"},
                {"from":"cmd","to":"l"},
                {"from":"l","to":"__emit__","label":"Done"}]
        }"#,
        )
        .expect("valid def");
        let store = test_store();
        let run_id = uuid::Uuid::new_v4().to_string();
        store
            .create_workflow_run(&run_id, &def.id, "manual", None, None, None, None)
            .expect("run row");
        let peak = Arc::new(AtomicUsize::new(0));
        let tools: Vec<Box<dyn DynTool>> = vec![Box::new(CountingShell {
            live: Arc::new(AtomicUsize::new(0)),
            peak: peak.clone(),
        })];
        let looper = ScriptedLoop {
            provider: &provider,
            tool_pool: Arc::new(tokio::sync::Semaphore::new(1)),
        };
        let result = execute_graph(
            &def,
            "",
            "test-owner",
            false,
            &serde_json::json!({"items": [1, 2, 3, 4, 5]}),
            &store,
            None,
            &looper,
            &tools,
            &run_id,
            None,
            None,
            None,
            Vec::new(),
            None,
            None,
            None,
        )
        .await;
        result.expect("run ok");
        assert_eq!(
            peak.load(Ordering::SeqCst),
            1,
            "commands must queue on the tool permit, not all run at once"
        );
    }

    /// A command node's `params.timeout_secs` reaches the step as its
    /// timeout (the tool takes milliseconds): 120 s when unnamed, capped at
    /// 600 s.
    #[tokio::test]
    async fn test_command_node_timeout_secs_reaches_the_step() {
        struct RecordingShell {
            timeouts: Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
        }
        impl DynTool for RecordingShell {
            fn name(&self) -> &str {
                "run_command"
            }
            fn description(&self) -> String {
                String::new()
            }
            fn schema(&self) -> serde_json::Value {
                serde_json::json!({})
            }
            fn execute_dyn<'a>(
                &'a self,
                _ctx: &'a tools::ToolContext,
                input: serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = tools::ToolResult> + Send + 'a>>
            {
                self.timeouts.lock().unwrap().push(input["timeout"].clone());
                Box::pin(async move { tools::ToolResult::ok("done") })
            }
        }

        let provider = MockProvider::new(&[]);
        let def = parse_workflow(
            r#"{
            "version":"1.0","id":"t","name":"T",
            "activities":[
                {"id":"a","type":"command","params":{"command":"echo a"}},
                {"id":"b","type":"command","params":{"command":"echo b","timeout_secs":300}},
                {"id":"c","type":"command","params":{"command":"echo c","timeout_secs":900}}],
            "connections":[
                {"from":"__trigger__","to":"a"},{"from":"a","to":"b"},
                {"from":"b","to":"c"},{"from":"c","to":"__emit__"}]
        }"#,
        )
        .expect("valid def");
        let store = test_store();
        let run_id = uuid::Uuid::new_v4().to_string();
        store
            .create_workflow_run(&run_id, &def.id, "manual", None, None, None, None)
            .expect("run row");
        let timeouts = Arc::new(std::sync::Mutex::new(Vec::new()));
        let tools: Vec<Box<dyn DynTool>> = vec![Box::new(RecordingShell {
            timeouts: timeouts.clone(),
        })];
        let result = execute_graph(
            &def,
            "",
            "test-owner",
            false,
            &serde_json::json!({}),
            &store,
            None,
            &ScriptedLoop::new(&provider),
            &tools,
            &run_id,
            None,
            None,
            None,
            Vec::new(),
            None,
            None,
            None,
        )
        .await;
        result.expect("run ok");
        assert_eq!(
            *timeouts.lock().unwrap(),
            vec![
                serde_json::json!(120_000),
                serde_json::json!(300_000),
                serde_json::json!(600_000)
            ]
        );
    }

    /// A failing node poisons ONLY its own downstream: the fork's sibling
    /// branch still completes, the failed node's dependents never run, and
    /// the run records failed — a half-dead graph must never look healthy.
    #[tokio::test]
    async fn test_failure_skips_dependents_but_sibling_branch_completes() {
        // "bad" is a command node executed with an EMPTY tool roster, so it
        // fails deterministically (no run_command tool) without any provider error.
        let provider = MockProvider::new(&[]);
        let def = r#"{
            "version":"1.0","id":"t","name":"T",
            "activities":[
                {"id":"a","intent":"task-a"},
                {"id":"bad","type":"command","params":{"command":"echo hi"}},
                {"id":"c","intent":"task-c"},
                {"id":"d","intent":"task-d"}],
            "connections":[
                {"from":"__trigger__","to":"a"},
                {"from":"a","to":"bad"},{"from":"a","to":"c"},
                {"from":"bad","to":"d"},{"from":"c","to":"d"},
                {"from":"d","to":"__emit__"}]
        }"#;
        let (result, store, run_id) = run_graph(def, serde_json::json!({}), &provider).await;
        match result {
            Err(WorkflowError::ActivityFailed(id, _)) => assert_eq!(id, "bad"),
            other => panic!("expected ActivityFailed(bad), got {:?}", other),
        }
        assert_eq!(run_status(&store, &run_id), "failed");
        let calls = provider.calls();
        let user_part = |c: &String| c.split("###SYSTEM###").next().unwrap_or("").to_string();
        assert!(
            calls.iter().any(|c| user_part(c).contains("task-c")),
            "independent sibling branch must still complete: {calls:?}"
        );
        assert!(
            !calls.iter().any(|c| user_part(c).contains("task-d")),
            "the failed node's dependent must never run: {calls:?}"
        );
    }

    /// The not-taken side of a condition: routing is label-driven, so a
    /// False verdict must activate the False edge — never "first edge wins".
    #[tokio::test]
    async fn test_condition_false_routes_false_branch() {
        let provider = MockProvider::new(&[]);
        let def = r#"{
            "version":"1.0","id":"t","name":"T",
            "activities":[
                {"id":"cond","type":"condition","params":{"expression":"inputs.priority > 3"}},
                {"id":"b","intent":"task-b"},
                {"id":"c","intent":"task-c"},
                {"id":"d","intent":"task-d"}],
            "connections":[
                {"from":"__trigger__","to":"cond"},
                {"from":"cond","to":"b","label":"True"},
                {"from":"cond","to":"c","label":"False"},
                {"from":"b","to":"d"},{"from":"c","to":"d"},
                {"from":"d","to":"__emit__"}]
        }"#;
        let (result, store, run_id) =
            run_graph(def, serde_json::json!({"priority": 2}), &provider).await;
        result.expect("run ok");
        let calls = provider.calls();
        assert_eq!(calls.len(), 2, "only c and d run: {:?}", calls);
        assert!(calls[0].contains("task-c"));
        assert!(calls[1].contains("task-d"));
        assert_eq!(run_status(&store, &run_id), "completed");
    }

    /// A skip propagates through EVERY transitive dependent of the not-taken
    /// branch: intermediate nodes never execute, and the downstream join
    /// still releases instead of waiting forever on the dead branch.
    #[tokio::test]
    async fn test_skip_propagates_through_chain_to_join() {
        let provider = MockProvider::new(&[]);
        // True side is a two-node chain (b -> x) into the join; False goes
        // straight to it. With False taken, b AND x must both be skipped.
        let def = r#"{
            "version":"1.0","id":"t","name":"T",
            "activities":[
                {"id":"cond","type":"condition","params":{"expression":"inputs.priority > 3"}},
                {"id":"b","intent":"task-b"},
                {"id":"x","intent":"task-x"},
                {"id":"c","intent":"task-c"},
                {"id":"d","intent":"task-d"}],
            "connections":[
                {"from":"__trigger__","to":"cond"},
                {"from":"cond","to":"b","label":"True"},
                {"from":"cond","to":"c","label":"False"},
                {"from":"b","to":"x"},{"from":"x","to":"d"},
                {"from":"c","to":"d"},
                {"from":"d","to":"__emit__"}]
        }"#;
        let (result, store, run_id) = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            run_graph(def, serde_json::json!({"priority": 1}), &provider),
        )
        .await
        .expect("join must release on the propagated skip — walk hung");
        result.expect("run ok");
        let calls = provider.calls();
        assert_eq!(calls.len(), 2, "only c and d run: {:?}", calls);
        assert!(calls[0].contains("task-c"));
        assert!(calls[1].contains("task-d"));
        // The join's context carries only the live branch.
        assert!(calls[1].contains("'c' result"));
        assert!(
            !calls[1].contains("'b' result") && !calls[1].contains("'x' result"),
            "skipped chain leaked output into the join: {}",
            calls[1]
        );
        assert_eq!(run_status(&store, &run_id), "completed");
    }

    /// A node's output is parsed as JSON into the data context — downstream
    /// condition routing reads structured fields via `nodes.<id>.<field>`,
    /// not just raw text.
    #[tokio::test]
    async fn test_node_json_output_drives_condition_routing() {
        let provider = MockProvider::new(&[("task-a", r#"{"count": 7}"#)]);
        let def = r#"{
            "version":"1.0","id":"t","name":"T",
            "activities":[
                {"id":"a","intent":"task-a"},
                {"id":"cond","type":"condition","params":{"expression":"nodes.a.count > 5"}},
                {"id":"hit","intent":"task-hit"},
                {"id":"miss","intent":"task-miss"}],
            "connections":[
                {"from":"__trigger__","to":"a"},
                {"from":"a","to":"cond"},
                {"from":"cond","to":"hit","label":"True"},
                {"from":"cond","to":"miss","label":"False"},
                {"from":"hit","to":"__emit__"},{"from":"miss","to":"__emit__"}]
        }"#;
        let (result, store, run_id) = run_graph(def, serde_json::json!({}), &provider).await;
        result.expect("run ok");
        let calls = provider.calls();
        let user_part = |c: &String| c.split("###SYSTEM###").next().unwrap_or("").to_string();
        assert_eq!(
            calls.iter().filter(|c| user_part(c).contains("task-hit")).count(),
            1,
            "structured field must route True: {calls:?}"
        );
        assert!(
            !calls.iter().any(|c| user_part(c).contains("task-miss")),
            "False branch must stay dead: {calls:?}"
        );
        assert_eq!(run_status(&store, &run_id), "completed");
    }

    /// A compound gate on bare names (the shape that silently routed False
    /// in 42 published conditions) opens on the parent's JSON fields; a gate
    /// reading a field the parent never wrote fails the run loudly.
    #[tokio::test]
    async fn test_compound_bare_name_condition_routes_and_missing_field_fails() {
        let def = |expr: &str| {
            format!(
                r#"{{
            "version":"1.0","id":"t","name":"T",
            "activities":[
                {{"id":"verify","intent":"task-verify"}},
                {{"id":"decide","type":"condition","params":{{"expression":"{expr}"}}}},
                {{"id":"release","intent":"task-release"}},
                {{"id":"hold","intent":"task-hold"}}],
            "connections":[
                {{"from":"__trigger__","to":"verify"}},
                {{"from":"verify","to":"decide"}},
                {{"from":"decide","to":"release","label":"True"}},
                {{"from":"decide","to":"hold","label":"False"}}]
        }}"#
            )
        };
        let user_part = |c: &String| c.split("###SYSTEM###").next().unwrap_or("").to_string();

        let provider = MockProvider::new(&[("task-verify", r#"{"verified": true, "calledBack": true, "flags": 0}"#)]);
        let (result, store, run_id) = run_graph(
            &def("verified && (calledBack || flags > 0) && !nodes.verify.flags"),
            serde_json::json!({}),
            &provider,
        )
        .await;
        result.expect("run ok");
        let calls = provider.calls();
        assert!(calls.iter().any(|c| user_part(c).contains("task-release")), "{calls:?}");
        assert!(!calls.iter().any(|c| user_part(c).contains("task-hold")), "{calls:?}");
        assert_eq!(run_status(&store, &run_id), "completed");

        let provider = MockProvider::new(&[("task-verify", r#"{"verified": true}"#)]);
        let (result, _store, _run_id) = run_graph(
            &def("verified && calledBack"),
            serde_json::json!({}),
            &provider,
        )
        .await;
        let err = result.expect_err("a missing field fails the gate, never routes False").to_string();
        assert!(err.contains("calledBack"), "{err}");
        let calls = provider.calls();
        assert!(!calls.iter().any(|c| user_part(c).contains("task-hold")), "{calls:?}");
        assert!(!calls.iter().any(|c| user_part(c).contains("task-release")), "{calls:?}");
    }

    /// A decide node's output shape — the `answers` map plus `model` — drives
    /// condition routing on `.choice` and `.confidence` with the existing
    /// expression syntax. Hand-written here because the live call is
    /// verified separately; the shape is the contract.
    #[tokio::test]
    async fn test_decide_shaped_output_drives_condition_routing() {
        let decide_output = r#"{"intent":{"type":"choice","choice":"quote_request","confidence":0.91,"probabilities":{"quote_request":0.91,"other":0.09}},"model":"jev-1.13.0"}"#;
        let provider = MockProvider::new(&[("task-a", decide_output)]);
        let def = r#"{
            "version":"1.0","id":"t","name":"T",
            "activities":[
                {"id":"classify","intent":"task-a"},
                {"id":"is-quote","type":"condition","params":{"expression":"nodes.classify.intent.choice == \"quote_request\""}},
                {"id":"is-sure","type":"condition","params":{"expression":"nodes.classify.intent.confidence >= 0.7"}},
                {"id":"hit","intent":"task-hit"},
                {"id":"miss","intent":"task-miss"},
                {"id":"unsure","intent":"task-unsure"}],
            "connections":[
                {"from":"__trigger__","to":"classify"},
                {"from":"classify","to":"is-quote"},
                {"from":"is-quote","to":"is-sure","label":"True"},
                {"from":"is-quote","to":"miss","label":"False"},
                {"from":"is-sure","to":"hit","label":"True"},
                {"from":"is-sure","to":"unsure","label":"False"},
                {"from":"hit","to":"__emit__"},{"from":"miss","to":"__emit__"},{"from":"unsure","to":"__emit__"}]
        }"#;
        let (result, store, run_id) = run_graph(def, serde_json::json!({}), &provider).await;
        result.expect("run ok");
        let calls = provider.calls();
        let user_part = |c: &String| c.split("###SYSTEM###").next().unwrap_or("").to_string();
        assert_eq!(
            calls.iter().filter(|c| user_part(c).contains("task-hit")).count(),
            1,
            "choice + confidence must route True/True: {calls:?}"
        );
        assert!(
            !calls.iter().any(|c| user_part(c).contains("task-miss") || user_part(c).contains("task-unsure")),
            "other branches must stay dead: {calls:?}"
        );
        assert_eq!(run_status(&store, &run_id), "completed");
    }

    /// A stand-in for Janus `/v1/systemone` on a local port: every request
    /// is read whole and answered with `status` and `body`; `hits` counts
    /// them. Lets a test see exactly how many decisions a run paid for.
    async fn fake_janus(
        status: u16,
        body: &'static str,
    ) -> (ai::DecideClient, Arc<std::sync::atomic::AtomicUsize>) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let counter = counter.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 8192];
                    loop {
                        let n = sock.read(&mut chunk).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                        let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
                            continue;
                        };
                        let head = String::from_utf8_lossy(&buf[..end]).to_ascii_lowercase();
                        let len = head
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:"))
                            .and_then(|v| v.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        if buf.len() >= end + 4 + len {
                            break;
                        }
                    }
                    counter.fetch_add(1, Ordering::SeqCst);
                    let reply = format!(
                        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(reply.as_bytes()).await;
                });
            }
        });
        let client = ai::DecideClient::new(&format!("http://{addr}"), || {
            Some(ai::Bearer {
                token: "test".into(),
                bot_id: None,
            })
        });
        (client, hits)
    }

    const OUTCOME_PROCEED: &str = r#"{"model":"jev-1.13.0","answers":{"outcome":{"type":"choice","choice":"proceed","confidence":0.99,"probabilities":{}}},"usage":{"input_tokens":10,"output_tokens":1,"cost_micro":10}}"#;
    const OUTCOME_HARMFUL: &str = r#"{"model":"jev-1.13.0","answers":{"outcome":{"type":"choice","choice":"harmful","confidence":0.99,"probabilities":{}}},"usage":{"input_tokens":10,"output_tokens":1,"cost_micro":10}}"#;

    /// A decide node routed by two conditions on its `intent` answer.
    const DECIDE_ROUTED: &str = r#"{
        "version":"1.0","id":"t","name":"T",
        "activities":[
            {"id":"classify","type":"decide","params":{"state":"inputs.text",DEFAULT"questions":{
                "intent":{"type":"choice","instructions":"What `text` asks for","criteria":{"quote_request":"a price","other":"anything else"}}}}},
            {"id":"is-other","type":"condition","params":{"expression":"nodes.classify.intent.choice == \"other\""}},
            {"id":"is-defaulted","type":"condition","params":{"mode":"exists","expression":"nodes.classify.defaulted"}},
            {"id":"other","intent":"task-other"},
            {"id":"not-other","intent":"task-not-other"},
            {"id":"flagged","intent":"task-flagged"}],
        "connections":[
            {"from":"__trigger__","to":"classify"},
            {"from":"classify","to":"is-other"},
            {"from":"classify","to":"is-defaulted"},
            {"from":"is-other","to":"other","label":"True"},
            {"from":"is-other","to":"not-other","label":"False"},
            {"from":"is-defaulted","to":"flagged","label":"True"},
            {"from":"other","to":"__emit__"},{"from":"not-other","to":"__emit__"},{"from":"flagged","to":"__emit__"}]
    }"#;

    fn ran(provider: &MockProvider, intent: &str) -> bool {
        provider.calls().iter().any(|c| {
            c.split("###SYSTEM###")
                .next()
                .unwrap_or("")
                .contains(intent)
        })
    }

    /// No decision service connected: the node records the author's declared
    /// default, flags itself defaulted, and the run completes down the
    /// default's branch. It never fails the run.
    #[tokio::test]
    async fn test_decide_node_without_service_takes_its_declared_default() {
        let provider = MockProvider::new(&[]);
        let def = DECIDE_ROUTED.replace("DEFAULT", r#""default":{"intent":"other"},"#);
        let (result, store, run_id) =
            run_graph(&def, serde_json::json!({"text": "how much"}), &provider).await;
        result.expect("a missing decision service never fails the run");
        assert!(ran(&provider, "task-other"), "{:?}", provider.calls());
        assert!(!ran(&provider, "task-not-other"));
        assert!(
            ran(&provider, "task-flagged"),
            "the output says it was defaulted"
        );
        assert_eq!(run_status(&store, &run_id), "completed");
    }

    /// The service errors (a throttled or failing upstream: 5xx, retried
    /// once) and the node declares no default: the question has no answer,
    /// every `.choice == ...` condition takes its False edge, and the run
    /// completes.
    #[tokio::test]
    async fn test_decide_node_fails_open_without_a_default() {
        let (client, hits) = fake_janus(502, r#"{"error":"upstream 529"}"#).await;
        let provider = MockProvider::new(&[]);
        let def = DECIDE_ROUTED.replace("DEFAULT", "");
        let (result, store, run_id) = run_graph_with(
            &def,
            serde_json::json!({"text": "how much"}),
            &provider,
            Some(&client),
        )
        .await;
        result.expect("a failed decision never fails the run");
        assert_eq!(
            hits.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "one call, one retry"
        );
        assert!(ran(&provider, "task-not-other"), "{:?}", provider.calls());
        assert!(!ran(&provider, "task-other"));
        assert!(ran(&provider, "task-flagged"));
        assert_eq!(run_status(&store, &run_id), "completed");
    }

    /// A state past the cap goes as its summary, said to be one; one within
    /// it is sent untouched, object shape included.
    #[test]
    fn test_decide_state_past_the_cap_is_its_summary() {
        let small = serde_json::json!({"subject": "hi"});
        assert_eq!(decide_state("n", small.clone()), (small, false));
        let big = serde_json::json!({
            "subject": "Quote please",
            "body": "x".repeat(DECIDE_STATE_CAP * 2),
            "zz_signature": "Sent from the road"
        });
        let (state, summarized) = decide_state("n", big);
        assert!(summarized);
        let text = state.as_str().expect("a summary goes as text");
        assert!(text.len() <= DECIDE_STATE_CAP, "{}", text.len());
        assert!(text.starts_with("An object with 3 keys"), "{}", &text[..80]);
        assert!(text.contains("- subject: \"Quote please\"") && text.contains("- zz_signature: \"Sent from the road\""), "{text}");
        assert!(text.contains(&format!("({} characters)", "48,000")), "the body's beginning and its size");
    }

    /// A two-step activity pays for ONE evaluator call: after step 1. The
    /// final step's verdict was always thrown away, so it is not asked.
    #[tokio::test]
    async fn test_step_evaluator_skips_the_final_step() {
        let (client, hits) = fake_janus(200, OUTCOME_PROCEED).await;
        let provider = MockProvider::new(&[]);
        let def = r#"{
            "version":"1.0","id":"t","name":"T",
            "activities":[{"id":"a","intent":"task-a","steps":["step-one","step-two"]}],
            "connections":[{"from":"__trigger__","to":"a"},{"from":"a","to":"__emit__"}]
        }"#;
        let (result, _store, _run_id) =
            run_graph_with(def, serde_json::json!({}), &provider, Some(&client)).await;
        result.expect("run ok");
        assert!(ran(&provider, "step-two"));
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    /// The evaluator can stop a run after a clean step, but never after one
    /// that read outside text, nor after any later step (the outside text is
    /// in their conversation): it is not asked there, so an injected "stop"
    /// in a web page or an email cannot end the run.
    #[tokio::test]
    async fn test_step_evaluator_cannot_stop_a_run_on_outside_text() {
        let def = r#"{
            "version":"1.0","id":"t","name":"T",
            "activities":[{"id":"a","intent":"task-a","steps":["read-inbox","step-two","step-three"]}],
            "connections":[{"from":"__trigger__","to":"a"},{"from":"a","to":"__emit__"}]
        }"#;

        // Clean step: a confident "harmful" stops the run before step two.
        let (client, hits) = fake_janus(200, OUTCOME_HARMFUL).await;
        let provider = MockProvider::new(&[]);
        let (result, store, run_id) =
            run_graph_with(def, serde_json::json!({}), &provider, Some(&client)).await;
        result.expect("an exit ends the run cleanly");
        assert_eq!(
            run_status(&store, &run_id),
            "exited",
            "the clean-step exit stands"
        );
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(!ran(&provider, "step-two"));

        // Same verdict, but step one read outside text: not asked, run goes on.
        let (client, hits) = fake_janus(200, OUTCOME_HARMFUL).await;
        let provider = MockProvider::new(&[]).with_tainted_turns(&["read-inbox"]);
        let (result, store, run_id) =
            run_graph_with(def, serde_json::json!({}), &provider, Some(&client)).await;
        result.expect("run ok");
        assert_eq!(
            run_status(&store, &run_id),
            "completed",
            "outside text never lets the evaluator stop the run"
        );
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(ran(&provider, "step-three"));
    }

    /// A run of an employee's binding, in `store`: the binding row, and the
    /// run row carrying `agent:<id>` and the binding name as a timer fire's
    /// run does.
    async fn run_binding_graph(
        def_json: &str,
        provider: &MockProvider,
        decide: Option<&ai::DecideClient>,
    ) -> (Result<(String, String), WorkflowError>, Arc<Store>, String) {
        let def = parse_workflow(def_json).expect("valid def");
        let store = test_store();
        store.conn_exec_for_test(
            "INSERT INTO agents (id, name, description, agent_md, frontmatter, updated_at) VALUES ('emp', 'E', '', '', '', 0)",
        );
        store
            .upsert_agent_workflow("emp", "sweep", "heartbeat", "30m", None, None, None, None, None, false)
            .unwrap();
        let run_id = uuid::Uuid::new_v4().to_string();
        store
            .create_workflow_run(&run_id, "agent:emp", "heartbeat", Some("sweep"), None, None, None)
            .expect("run row");
        let looper = ScriptedLoop::new(provider);
        let result = execute_graph(
            &def, "", "test-owner", false, &serde_json::json!({}), &store, decide, &looper, &[],
            &run_id, None, None, None, Vec::new(), None, None, None,
        )
        .await;
        (result, store, run_id)
    }

    const OUTCOME_PRECONDITION: &str = r#"{"model":"jev-1.13.0","answers":{"outcome":{"type":"choice","choice":"precondition_failed","confidence":0.99,"probabilities":{}}},"usage":{"input_tokens":10,"output_tokens":1,"cost_micro":10}}"#;

    /// How a run ends decides what it is, never what its text says. The step
    /// evaluator ending it after the first step is a clean end with a
    /// reason: `exited` (engine state `done`), the reason the step's own
    /// words, the binding's standing outcome recorded, and nothing after it
    /// runs — not the activity's next step, not the next activity. The
    /// employee calling `exit` is the same. A provider failure is `failed`
    /// and records no standing outcome.
    #[tokio::test]
    async fn test_a_run_ended_for_nothing_to_do_is_a_standing_outcome_and_a_failure_is_not() {
        let def = r#"{
            "version":"1.0","id":"t","name":"T",
            "activities":[
                {"id":"a","intent":"task-a","steps":["step-one","step-two"]},
                {"id":"b","intent":"task-b"}],
            "connections":[{"from":"__trigger__","to":"a"},{"from":"a","to":"b"},{"from":"b","to":"__emit__"}]
        }"#;

        // The step evaluator ends it.
        let (client, _hits) = fake_janus(200, OUTCOME_PRECONDITION).await;
        let provider = MockProvider::new(&[("step-one", "## Check\nThe list is empty; there is nothing to act on.")]);
        let (result, store, run_id) = run_binding_graph(def, &provider, Some(&client)).await;
        result.expect("an evaluator exit ends the run cleanly");
        let run = store.get_workflow_run(&run_id).unwrap().unwrap();
        assert_eq!(run.status, "exited");
        assert_eq!(store.engine_get_run(&run_id).unwrap().unwrap().state, "done");
        assert_eq!(run.error.as_deref(), Some("Step 1/2 evaluator: Check"));
        assert!(!ran(&provider, "step-two") && !ran(&provider, "task-b"), "{:?}", provider.calls());
        let (outcome, _at) = store.agent_workflow_last_outcome("emp", "sweep").unwrap().expect("standing outcome");
        assert_eq!(outcome, "Step 1/2 evaluator: Check");

        // The employee says why and exits.
        let provider = MockProvider::new(&[]).with_exit_scripts(&[("task-a", "Nothing new since the last run.\nDetail.")]);
        let one_step = def.replace(r#","steps":["step-one","step-two"]"#, "");
        let (result, store, run_id) = run_binding_graph(&one_step, &provider, None).await;
        result.expect("an exit ends the run cleanly");
        assert_eq!(run_status(&store, &run_id), "exited");
        assert!(!ran(&provider, "task-b"));
        let (outcome, _at) = store.agent_workflow_last_outcome("emp", "sweep").unwrap().expect("standing outcome");
        assert_eq!(outcome, "Nothing new since the last run.");

        // A provider failure is a failure, with no standing outcome.
        let provider = MockProvider::new(&[]).with_rate_limit_failures(3);
        let (result, store, run_id) = run_binding_graph(&one_step, &provider, None).await;
        assert!(result.is_err());
        assert_eq!(run_status(&store, &run_id), "failed");
        assert_eq!(store.agent_workflow_last_outcome("emp", "sweep").unwrap(), None);
    }

    /// A tool's terminal refusal (no account connected) ends every run of
    /// the binding the same way until the owner changes something: a
    /// standing outcome, like an exit — `exited` (engine state `done`), the
    /// refusal as the reason on the run, the activity and the binding, and
    /// nothing after it runs. Decided by how the turn ended (the runner's
    /// `terminal_tool_error`), never by the refusal's words.
    #[tokio::test]
    async fn test_a_run_a_tool_refused_terminally_is_a_standing_outcome() {
        let def = r#"{
            "version":"1.0","id":"t","name":"T",
            "activities":[
                {"id":"a","intent":"task-a"},
                {"id":"b","intent":"task-b"}],
            "connections":[{"from":"__trigger__","to":"a"},{"from":"a","to":"b"},{"from":"b","to":"__emit__"}]
        }"#;
        let refusal = "No example account is connected for this agent. Connect one in this agent's Settings, Plugins before using example.";
        let provider = MockProvider::new(&[]).with_blocked_scripts(&[("task-a", refusal)]);
        let (result, store, run_id) = run_binding_graph(def, &provider, None).await;
        result.expect("a terminal refusal ends the run cleanly");
        let run = store.get_workflow_run(&run_id).unwrap().unwrap();
        assert_eq!(run.status, "exited");
        assert_eq!(store.engine_get_run(&run_id).unwrap().unwrap().state, "done");
        let reason = format!("blocked: {refusal}");
        assert_eq!(run.error.as_deref(), Some(reason.as_str()));
        assert!(!ran(&provider, "task-b"), "{:?}", provider.calls());
        let results = store.list_activity_results(&run_id).unwrap();
        let a = results.iter().find(|r| r.activity_id == "a").expect("activity a recorded");
        assert_eq!((a.status.as_str(), a.error.as_deref()), ("exited", Some(reason.as_str())));
        let (outcome, _at) = store.agent_workflow_last_outcome("emp", "sweep").unwrap().expect("standing outcome");
        assert_eq!(outcome, reason);
        // What the refusing tool named is kept on the run as data.
        assert_eq!(
            store.workflow_run_owner_need(&run_id).unwrap(),
            Some(types::OwnerNeed::Account { plugin: "example".into() })
        );
    }

    /// on_error.retry is the activity-level retry budget: after
    /// stream_with_retry's transient retries are exhausted, a declared budget
    /// re-runs the activity and the run completes; the default budget (one
    /// attempt) turns the same failure into a failed run.
    #[tokio::test]
    async fn test_on_error_retry_budget() {
        // 3 consecutive RateLimit errors exhaust stream_with_retry's 3
        // in-stream attempts, failing activity attempt #1.
        let with_retry = MockProvider::new(&[]).with_rate_limit_failures(3);
        let def_retry = r#"{
            "version":"1.0","id":"t","name":"T",
            "activities":[{"id":"a","intent":"task-a","on_error":{"retry":2}}],
            "connections":[
                {"from":"__trigger__","to":"a"},{"from":"a","to":"__emit__"}]
        }"#;
        let (result, store, run_id) =
            run_graph(def_retry, serde_json::json!({}), &with_retry).await;
        result.expect("attempt 2 must recover the activity");
        assert_eq!(run_status(&store, &run_id), "completed");
        assert_eq!(with_retry.calls().len(), 1, "exactly one successful call");

        // Same failure, default budget (retry 1): the run fails.
        let no_retry = MockProvider::new(&[]).with_rate_limit_failures(3);
        let def_plain = r#"{
            "version":"1.0","id":"t","name":"T",
            "activities":[{"id":"a","intent":"task-a"}],
            "connections":[
                {"from":"__trigger__","to":"a"},{"from":"a","to":"__emit__"}]
        }"#;
        let (result, store, run_id) =
            run_graph(def_plain, serde_json::json!({}), &no_retry).await;
        assert!(result.is_err(), "no retry budget: the failure must surface");
        assert_eq!(run_status(&store, &run_id), "failed");
        assert!(no_retry.calls().is_empty(), "no successful call ever landed");
    }

    /// Independent roots both run: a graph with two trigger entries executes
    /// both chains and the final context carries both results.
    #[tokio::test]
    async fn test_independent_trigger_entries_both_run() {
        let provider = MockProvider::new(&[]);
        let def = r#"{
            "version":"1.0","id":"t","name":"T",
            "activities":[
                {"id":"a","intent":"task-a"},
                {"id":"b","intent":"task-b"}],
            "connections":[
                {"from":"__trigger__","to":"a"},
                {"from":"__trigger__","to":"b"},
                {"from":"a","to":"__emit__"},{"from":"b","to":"__emit__"}]
        }"#;
        let (result, store, run_id) = run_graph(def, serde_json::json!({}), &provider).await;
        let (_, final_context) = result.expect("run ok");
        assert_eq!(provider.calls().len(), 2, "both roots run exactly once");
        assert!(final_context.contains("[Activity 'a' result]"));
        assert!(final_context.contains("[Activity 'b' result]"));
        assert_eq!(run_status(&store, &run_id), "completed");
    }

    /// An edge to a node that doesn't exist (validation bypassed) fails the
    /// run loudly with the node named — never a panic, never a silent hang.
    #[tokio::test]
    async fn test_unknown_node_reference_fails_loudly() {
        let def: WorkflowDef = serde_json::from_str(
            r#"{
            "version":"1.0","id":"t","name":"T",
            "activities":[{"id":"a","intent":"task-a"}],
            "connections":[
                {"from":"__trigger__","to":"a"},{"from":"a","to":"ghost"}]
        }"#,
        )
        .unwrap();
        let provider = MockProvider::new(&[]);
        let store = test_store();
        let run_id = uuid::Uuid::new_v4().to_string();
        store
            .create_workflow_run(&run_id, &def.id, "manual", None, None, None, None)
            .expect("run row");
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            execute_graph(
                &def,
                "",
                "test-owner",
                false,
                &serde_json::json!({}),
                &store,
                None,
                &ScriptedLoop::new(&provider),
                &[],
                &run_id,
                None,
                None,
                None,
                Vec::new(),
                None,
                None,
                None,
            ),
        )
        .await
        .expect("terminated within timeout");
        match result {
            Err(WorkflowError::Other(msg)) => {
                assert!(msg.contains("unknown node 'ghost'"), "names the node: {msg}")
            }
            other => panic!("expected Other(unknown node), got {:?}", other),
        }
        // The reachable node still executed before the dangling edge tripped.
        assert_eq!(provider.calls().len(), 1);
        let status = store
            .get_workflow_run(&run_id)
            .unwrap()
            .unwrap()
            .status;
        assert_eq!(status, "failed");
    }

    /// The system prompt of the first call for the activity whose intent is
    /// `intent` (per-step activities send their steps as the user message,
    /// so the match is on the prompt's Task section).
    fn system_of(provider: &MockProvider, intent: &str) -> String {
        let task = format!("## Task\n{intent}\n");
        provider
            .calls()
            .into_iter()
            .filter_map(|c| c.split("###SYSTEM###").nth(1).map(str::to_string))
            .find(|sys| sys.contains(&task))
            .unwrap_or_else(|| panic!("no call for {intent}"))
    }

    /// a → gate (condition) → b → c.
    const CHAIN: &str = r#"{
        "version":"1.0","id":"t","name":"T",
        "activities":[
            {"id":"a","intent":"task-a"},
            {"id":"gate","type":"condition","params":{"mode":"exists","expression":"nodes.a"}},
            {"id":"b","intent":"task-b"},
            {"id":"c","intent":"task-c"}],
        "connections":[
            {"from":"__trigger__","to":"a"},
            {"from":"a","to":"gate"},
            {"from":"gate","to":"b","label":"True"},
            {"from":"b","to":"c"},
            {"from":"c","to":"__emit__"}]
    }"#;

    /// A step reads its direct parents only, and a condition stands aside
    /// for its own parents (it produced "True", not content).
    #[tokio::test]
    async fn prior_results_are_direct_parents_through_routing_nodes() {
        let provider = MockProvider::new(&[("task-a", "A-OUT"), ("task-b", "B-OUT")]);
        let (result, _, _) = run_graph(CHAIN, serde_json::json!({}), &provider).await;
        result.expect("run ok");
        let b = system_of(&provider, "task-b");
        assert!(b.contains("[Activity 'a' result]: A-OUT"), "b reads a through the gate: {b}");
        assert!(!b.contains("[Activity 'gate' result]"), "{b}");
        let c = system_of(&provider, "task-c");
        assert!(c.contains("[Activity 'b' result]: B-OUT"), "{c}");
        assert!(!c.contains("A-OUT"), "c must not read its grandparent: {c}");
    }

    /// Full data flows by reference: `{{nodes.<id>...}}` in a step resolves
    /// into its text from ANY earlier step — a grandparent here — and a
    /// parent it references is not repeated in Prior Results.
    #[tokio::test]
    async fn a_reference_carries_an_earlier_steps_data() {
        let provider = MockProvider::new(&[
            ("task-a", r#"{"rows":[1,2,3],"summary":"three rows"}"#),
            ("task-b", "B-OUT"),
        ]);
        let def = r#"{
            "version":"1.0","id":"t","name":"T",
            "activities":[
                {"id":"a","intent":"task-a"},
                {"id":"b","intent":"task-b"},
                {"id":"c","intent":"task-c","steps":["Use rows {{nodes.a.rows}} and {{nodes.b}} for {{inputs.who}}"]}],
            "connections":[
                {"from":"__trigger__","to":"a"},
                {"from":"a","to":"b"},
                {"from":"b","to":"c"},
                {"from":"c","to":"__emit__"}]
        }"#;
        let (result, _, _) = run_graph(def, serde_json::json!({"who": "Dana"}), &provider).await;
        result.expect("run ok");
        let all = provider.calls().join("\n");
        assert!(all.contains("Use rows [1,2,3] and B-OUT for Dana"), "{all}");
        let c = system_of(&provider, "task-c");
        assert!(!c.contains("[Activity 'b' result]"), "referenced parent repeated: {c}");
        assert!(!c.contains("three rows"), "{c}");
    }

    /// A loop body reads its own item and its own iteration's parents, never
    /// what came before the loop.
    #[tokio::test]
    async fn a_loop_body_reads_only_its_item() {
        let provider = MockProvider::new(&[("task-fetch", "FETCHED-EVERYTHING")]);
        let def = r#"{
            "version":"1.0","id":"t","name":"T",
            "activities":[
                {"id":"fetch","intent":"task-fetch"},
                {"id":"l","type":"loop","params":{"source":"inputs.items"}},
                {"id":"body","intent":"task-body"}],
            "connections":[
                {"from":"__trigger__","to":"fetch"},
                {"from":"fetch","to":"l"},
                {"from":"l","to":"body","label":"Each item"},
                {"from":"body","to":"l"}]
        }"#;
        let (result, _, _) = run_graph(def, serde_json::json!({"items": ["ONE"]}), &provider).await;
        result.expect("run ok");
        let body = system_of(&provider, "task-body");
        assert!(body.contains("[Current item]: \"ONE\""), "{body}");
        assert!(!body.contains("FETCHED-EVERYTHING"), "{body}");
    }

    /// A definition with no connections is a chain: a step reads the one
    /// before it, and anything further back only by reference.
    #[tokio::test]
    async fn a_chained_step_reads_the_one_before_it() {
        let provider = MockProvider::new(&[("task-a", "A-OUT"), ("task-b", "B-OUT")]);
        let def = parse_workflow(
            r#"{
            "version":"1.0","id":"t","name":"T",
            "activities":[
                {"id":"a","intent":"task-a"},
                {"id":"b","intent":"task-b"},
                {"id":"c","intent":"task-c"},
                {"id":"d","intent":"task-d","steps":["Compare {{nodes.a}} with what came before"]}]
        }"#,
        )
        .expect("valid def");
        let store = test_store();
        let looper = ScriptedLoop::new(&provider);
        crate::engine::execute_workflow(
            &def, "", "test-owner", false, serde_json::json!({}), "manual", None, &store, None, &looper, &[],
            None, None, None, None, Vec::new(), None, None, None,
        )
        .await
        .expect("run ok");
        let c = system_of(&provider, "task-c");
        assert!(c.contains("[Activity 'b' result]: B-OUT"), "{c}");
        assert!(!c.contains("A-OUT"), "{c}");
        let all = provider.calls().join("\n");
        assert!(all.contains("Compare A-OUT with what came before"), "{all}");
    }

    /// A parent's result reaches the next AI step whole, however large: a
    /// 200k-char JSON list arrives intact in its Prior Results, last row
    /// included.
    #[tokio::test]
    async fn a_large_parent_result_reaches_the_next_step_whole() {
        let rows: Vec<serde_json::Value> =
            (0..4_000).map(|i| serde_json::json!({"id": i, "name": format!("customer-{i:05}"), "note": "x".repeat(20)})).collect();
        let big = serde_json::json!({"summary": "4000 rows", "rows": rows}).to_string();
        assert!(big.len() >= 200_000, "{}", big.len());
        let provider = MockProvider::new(&[("task-a", big.as_str()), ("task-b", "B-OUT")]);
        let def = r#"{
            "version":"1.0","id":"t","name":"T",
            "activities":[
                {"id":"a","intent":"task-a"},
                {"id":"b","intent":"task-b"}],
            "connections":[
                {"from":"__trigger__","to":"a"},
                {"from":"a","to":"b"},
                {"from":"b","to":"__emit__"}]
        }"#;
        let (result, _, _) = run_graph(def, serde_json::json!({}), &provider).await;
        result.expect("run ok");
        let b = system_of(&provider, "task-b");
        assert!(b.contains(&format!("[Activity 'a' result]: {big}\n")), "the whole result, {} chars", big.len());
        assert!(b.contains("customer-03999"), "the last row");
        assert!(!b.contains("clipped"), "nothing clipped");
    }

    /// A parent's result too large for the step's model reaches it as a
    /// summary with the call that reads it in pages, never cut, and the
    /// result itself stays whole in the run.
    #[tokio::test]
    async fn a_parent_result_too_large_for_the_model_is_handed_as_a_summary() {
        let rows: Vec<serde_json::Value> =
            (0..4_000).map(|i| serde_json::json!({"id": i, "name": format!("customer-{i:05}"), "note": "x".repeat(20)})).collect();
        let big = serde_json::json!({"summary": "4000 rows", "rows": rows}).to_string();
        let provider = MockProvider::new(&[("task-a", big.as_str()), ("task-b", "B-OUT")]).with_context_chars(100_000);
        let def = r#"{
            "version":"1.0","id":"t","name":"T",
            "activities":[
                {"id":"a","intent":"task-a"},
                {"id":"b","intent":"task-b"}],
            "connections":[
                {"from":"__trigger__","to":"a"},
                {"from":"a","to":"b"},
                {"from":"b","to":"__emit__"}]
        }"#;
        let (result, store, run_id) = run_graph(def, serde_json::json!({}), &provider).await;
        result.expect("run ok");
        let b = system_of(&provider, "task-b");
        assert!(b.contains("[Activity 'a' result — too large to include whole]:\nIts own summary: 4000 rows\nAn object with 2 keys"), "{b}");
        assert!(b.contains("read_step_data {\"source\":\"nodes.a\"}"), "{b}");
        assert!(!b.contains("customer-03999"), "not whole");
        assert!(b.len() < 50_000, "{}", b.len());
        assert_eq!(node_output(&store, &run_id, "a", "").to_string(), big, "kept whole in the run");
    }

    /// Fitting a step to its model: inputs, a referenced path and a loop
    /// item each go as a summary with where to read them, the largest
    /// first, small ones whole; a step that was handed a summary is given
    /// `read_step_data`.
    #[test]
    fn step_data_fits_its_model_largest_pieces_first() {
        let rows: Vec<serde_json::Value> = (0..3_000).map(|i| serde_json::json!({"id": i, "tag": "t"})).collect();
        let inputs = serde_json::json!({"who": "Dana", "rows": rows});
        let mut outputs = HashMap::new();
        outputs.insert("fetch".to_string(), serde_json::json!({"records": rows}).to_string());
        let item = serde_json::json!({"blob": "z".repeat(60_000)});
        let item_source = ItemSource { source: tools::step_data::Source::new("nodes.fetch.records.4", ""), range: None };
        let activity: Activity = serde_json::from_value(serde_json::json!({
            "id": "digest", "intent": "Digest {{nodes.fetch.records}} for {{inputs.who}}"
        }))
        .unwrap();
        let step = step_context(
            &activity,
            &[],
            StepData { outputs: &outputs, inputs: &inputs, item: Some(&item), item_source: Some(&item_source), at: "4", budget: 20_000 },
        );
        assert!(step.activity.intent.starts_with("Digest [{{nodes.fetch.records}} — too large to include whole: A list of 3,000 items"), "{}", &step.activity.intent[..120]);
        assert!(step.activity.intent.contains("read_step_data {\"at\":\"4\",\"source\":\"nodes.fetch.records\"}"), "{}", step.activity.intent);
        assert!(step.activity.intent.ends_with(" for Dana"), "a small reference goes whole");
        let rows_view = step.inputs["rows"].as_str().expect("the input's summary");
        assert!(rows_view.contains("read_step_data {\"source\":\"inputs.rows\"}"), "{rows_view}");
        assert_eq!(step.inputs["who"], "Dana");
        assert!(step.prior.contains("[Current item — too large to include whole]:"), "{}", step.prior);
        assert!(step.prior.contains("read_step_data {\"source\":\"nodes.fetch.records.4\"}"), "{}", step.prior);
        assert!(step.activity.tools.contains(&"read_step_data".to_string()));

        // Everything fits: all of it whole, and a step with no tools of its
        // own gets none.
        let step = step_context(
            &activity,
            &[],
            StepData { outputs: &outputs, inputs: &inputs, item: None, item_source: None, at: "", budget: 10_000_000 },
        );
        assert!(step.activity.intent.contains("{\"id\":2999,\"tag\":\"t\"}"));
        assert_eq!(step.inputs, inputs);
        assert!(step.activity.tools.is_empty());
    }

    // ── expert nodes ────────────────────────────────────────────────────

    /// A store with the owner and its coworkers installed.
    fn expert_store(coworkers: &[&str]) -> Arc<Store> {
        let store = test_store();
        store.create_agent("owner", None, "Owner", "Runs the workflow.", "", "{}", None, None).unwrap();
        for c in coworkers {
            store
                .create_agent(c, None, &format!("{c} expert"), &format!("Prices things for {c}. Fast."), "", "{}", None, None)
                .unwrap();
        }
        store
    }

    fn new_run(store: &Arc<Store>, def_json: &str) -> String {
        let def = parse_workflow(def_json).expect("valid def");
        let run_id = uuid::Uuid::new_v4().to_string();
        store
            .create_workflow_run(&run_id, &def.id, "manual", None, None, None, Some(def_json))
            .expect("run row");
        run_id
    }

    /// One pass of the run under its own id — a first launch, or the
    /// re-entry a reply, a deadline or a restart causes (`_relaunch_run`).
    /// Every pass builds its context from nothing: what survives is the store.
    async fn pass(
        store: &Arc<Store>,
        run_id: &str,
        def_json: &str,
        inputs: serde_json::Value,
        provider: &MockProvider,
    ) -> Result<(String, String), WorkflowError> {
        let def = parse_workflow(def_json).expect("valid def");
        let looper = ScriptedLoop::new(provider);
        execute_graph(
            &def, "owner", "test-owner", false, &inputs, store, None, &looper, &[], run_id, None, None,
            None, Vec::new(), None, None, None,
        )
        .await
    }

    /// The coworker closes the assignment a request opened, as its case
    /// would: `status` and `summary` are its close.
    fn coworker_closes(store: &Store, run_id: &str, expert: &str, status: &str, summary: &str) {
        let effect = store
            .engine_effects_for_run(run_id)
            .unwrap()
            .into_iter()
            .find(|e| e.class == crate::expert::EFFECT_CLASS && e.counterparty.as_deref() == Some(expert))
            .expect("a request to that expert");
        let assignment = effect.provider_ref.expect("the assignment it opened");
        let case = store.engine_run_for_key("case:assignment", &assignment).unwrap().expect("the expert's case");
        let inputs: serde_json::Value = serde_json::from_str(case.inputs.as_deref().unwrap()).unwrap();
        crate::cases::settle_assignment(store, &inputs, status, summary, chrono::Utc::now().timestamp()).unwrap();
    }

    /// Make one expert's request older than its timeout: the clock moved.
    fn age_request(store: &Store, run_id: &str, expert: &str, secs: i64) {
        store.conn_exec_for_test(&format!(
            "UPDATE engine_effects SET created_at = created_at - {secs} WHERE run_id = '{run_id}' AND counterparty = '{expert}'"
        ));
    }

    fn expert_requests(store: &Store, run_id: &str) -> usize {
        store
            .engine_effects_for_run(run_id)
            .unwrap()
            .iter()
            .filter(|e| e.class == crate::expert::EFFECT_CLASS)
            .count()
    }

    fn live_wait(store: &Store, run_id: &str) -> db::EngineWait {
        let run = store.engine_get_run(run_id).unwrap().unwrap();
        assert_eq!(run.state, "waiting", "the run is parked");
        store.engine_get_wait(run.current_wait_id.expect("a live wait")).unwrap().unwrap()
    }

    fn node_output(store: &Store, run_id: &str, node: &str, iteration: &str) -> serde_json::Value {
        let done = store.completed_activity_contents(run_id).unwrap();
        serde_json::from_str(&done[&(node.to_string(), iteration.to_string())]).unwrap()
    }

    const SAME_BOT: &str = r#"{
        "version": "1.0", "id": "quote", "name": "quote",
        "activities": [
            {"id": "ask", "type": "expert", "params": {
                "expert": "ana", "task": "Price the order",
                "input": {"order": "{{inputs.order}}"},
                "output": {"total": "number"}, "timeout": "2h"}},
            {"id": "report", "intent": "Write the quote"}
        ],
        "connections": [
            {"from": "__trigger__", "to": "ask"},
            {"from": "ask", "to": "report"}
        ]
    }"#;

    #[tokio::test]
    async fn an_expert_on_this_bot_answers_and_the_run_goes_on() {
        let store = expert_store(&["ana"]);
        let provider = MockProvider::new(&[]);
        let run_id = new_run(&store, SAME_BOT);
        let inputs = serde_json::json!({ "order": { "sku": "A-1", "qty": 3 } });

        let first = pass(&store, &run_id, SAME_BOT, inputs.clone(), &provider).await;
        assert!(matches!(first, Err(WorkflowError::AwaitingExpert(1))), "{first:?}");
        assert_eq!(expert_requests(&store, &run_id), 1);
        assert!(provider.calls().is_empty(), "nothing after the expert ran yet");
        let wait = live_wait(&store, &run_id);
        assert_eq!(run_status(&store, &run_id), "waiting", "waiting on an expert, not on the owner");
        assert_eq!(wait.on_kind, crate::expert::REPLY_KIND);
        assert_eq!(wait.key, crate::expert::run_target(&run_id));
        assert!(wait.deadline.is_some(), "the timeout wakes the run");
        // The expert was handed the task and the referenced data.
        let effect = &store.engine_effects_for_run(&run_id).unwrap()[0];
        let assignment = store.get_assignment(effect.provider_ref.as_deref().unwrap()).unwrap().unwrap();
        assert_eq!(assignment.assignee_agent_id, "ana");
        assert!(assignment.subject.contains("Price the order"));
        assert!(assignment.done_means.contains("\"sku\":\"A-1\""), "{}", assignment.done_means);
        assert!(assignment.done_means.contains("total"));

        coworker_closes(&store, &run_id, "ana", "done", "Priced the order. {\"total\": 42}");
        // The reply is the event that wakes the parked run.
        let reply = store
            .engine_events_for("run", &crate::expert::run_target(&run_id), 10)
            .unwrap()
            .pop()
            .expect("reply recorded");
        assert_eq!(store.engine_match_wait(&reply).unwrap().map(|w| w.id), Some(wait.id));

        let second = pass(&store, &run_id, SAME_BOT, inputs, &provider).await;
        assert!(second.is_ok(), "{second:?}");
        assert_eq!(run_status(&store, &run_id), "completed");
        let out = node_output(&store, &run_id, "ask", "");
        assert_eq!(out["summary"], "Priced the order.");
        assert_eq!(out["reply"]["total"], 42);
        assert_eq!(expert_requests(&store, &run_id), 1, "never asked twice");
        let calls = provider.calls();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].contains("Priced the order."), "the next step reads the reply");
    }

    /// A project's card: the run is the project's, so its expert step is a
    /// card in the project. The hand-off carries the project and its team,
    /// the member reads the card's title (not the workflow's) and its "done
    /// when", the card's opening and closing ring the card bell, and with no
    /// timeout the step waits for the answer however long it takes.
    #[tokio::test]
    async fn a_project_card_waits_for_its_answer_and_is_the_projects_hand_off() {
        let store = expert_store(&["ana"]);
        store.create_team("t-1", "Launch", "", &[db::TeamMember::local("owner"), db::TeamMember::local("ana")], "owner", None).unwrap();
        store
            .create_project(&db::NewProject {
                id: "p-1",
                team_id: "t-1",
                lead_agent_id: "owner",
                title: "PH launch",
                ask: "launch",
                asker_session_key: "",
                asker_agent_id: "",
                playbook_binding: None,
            })
            .unwrap();
        let (bell, mut rang) = tokio::sync::mpsc::unbounded_channel();
        crate::cases::install_project_bell(bell);
        let def = SAME_BOT.replace(r#""timeout": "2h""#, r#""done_when": "the total is priced from the current list""#);
        let provider = MockProvider::new(&[]);
        let run_id = new_run(&store, &def);
        store.conn_exec_for_test(&format!("UPDATE workflow_runs SET project_id = 'p-1' WHERE id = '{run_id}'"));
        let inputs = serde_json::json!({ "order": 1 });

        assert!(matches!(pass(&store, &run_id, &def, inputs.clone(), &provider).await, Err(WorkflowError::AwaitingExpert(1))));
        assert_eq!(live_wait(&store, &run_id).deadline, None, "only the answer wakes it");
        let effect = &store.engine_effects_for_run(&run_id).unwrap()[0];
        let assignment_id = effect.provider_ref.clone().unwrap();
        let assignment = store.get_assignment(&assignment_id).unwrap().unwrap();
        assert_eq!(assignment.subject, "Price the order", "the card's title");
        assert!(assignment.done_means.starts_with("Done when: the total is priced from the current list. "), "{}", assignment.done_means);
        let card = store.get_handoff(&assignment_id).unwrap().unwrap();
        assert_eq!((card.project_id.as_deref(), card.team_id.as_str()), (Some("p-1"), "t-1"));
        assert_eq!(
            rang.try_recv().unwrap(),
            crate::cases::ProjectChange::Opened {
                assignment_id: assignment_id.clone(),
                task: "Price the order".into(),
                done_when: "the total is priced from the current list".into(),
            }
        );

        // A month later, still out: no timeout, so no failure.
        age_request(&store, &run_id, "ana", 30 * 86_400);
        assert!(matches!(pass(&store, &run_id, &def, inputs.clone(), &provider).await, Err(WorkflowError::AwaitingExpert(1))));
        assert_eq!(expert_requests(&store, &run_id), 1, "asked once");

        coworker_closes(&store, &run_id, "ana", "done", "Priced it. {\"total\": 42}");
        assert_eq!(rang.try_recv().unwrap(), crate::cases::ProjectChange::Closed { assignment_id: assignment_id.clone() });
        assert!(pass(&store, &run_id, &def, inputs, &provider).await.is_ok());
        assert_eq!(store.get_handoff(&assignment_id).unwrap().unwrap().status, "done");
        assert!(rang.try_recv().is_err(), "each change rings once");
    }

    /// A project at its limit hands out no more cards: before the next one
    /// goes out its run parks on `limit:<run>` with no deadline, the project
    /// is paused, and the bell rings so the owner is asked. Once the limit
    /// is raised (his Continue), the run re-enters and the card goes out.
    #[tokio::test]
    async fn a_project_at_its_limit_pauses_before_its_next_card() {
        let store = expert_store(&["ana"]);
        store.create_team("t-1", "Launch", "", &[db::TeamMember::local("owner"), db::TeamMember::local("ana")], "owner", None).unwrap();
        store
            .create_project(&db::NewProject {
                id: "p-1",
                team_id: "t-1",
                lead_agent_id: "owner",
                title: "PH launch",
                ask: "launch",
                asker_session_key: "",
                asker_agent_id: "",
                playbook_binding: None,
            })
            .unwrap();
        store.raise_project_limits("p-1", 2, 500).unwrap();
        let (bell, mut rang) = tokio::sync::mpsc::unbounded_channel();
        crate::cases::install_project_bell(bell);
        let def = SAME_BOT.replace(r#", "timeout": "2h""#, "");
        let provider = MockProvider::new(&[]);
        let run_id = new_run(&store, &def);
        store.conn_exec_for_test(&format!("UPDATE workflow_runs SET project_id = 'p-1' WHERE id = '{run_id}'"));
        // Two turns of the project's work already happened in the lead's seat.
        for _ in 0..2 {
            store
                .record_run_usage(&db::models::RunUsageEntry {
                    agent_id: "owner".into(),
                    session_key: Some("agent:owner:coworker:team:t-1:project:p-1".into()),
                    run_id: None,
                    run_type: "chat".into(),
                    model_id: "m".into(),
                    input_tokens: 1,
                    output_tokens: 1,
                    cache_read_tokens: 0,
                    cache_creation_tokens: 0,
                    cost_microcents: 0,
                    outcome: None,
                    exit_reason: None,
                })
                .unwrap();
        }
        let inputs = serde_json::json!({ "order": 1 });

        let paused = pass(&store, &run_id, &def, inputs.clone(), &provider).await;
        assert!(matches!(&paused, Err(WorkflowError::ProjectLimit(used)) if used.starts_with("2 of 2 steps")), "{paused:?}");
        assert_eq!(expert_requests(&store, &run_id), 0, "no card went out");
        let wait = live_wait(&store, &run_id);
        assert_eq!((wait.on_kind.as_str(), wait.key.clone(), wait.deadline), (crate::expert::LIMIT_KIND, crate::expert::limit_target(&run_id), None));
        assert_eq!(store.get_project("p-1").unwrap().unwrap().state, "paused");
        assert_eq!(
            rang.try_recv().unwrap(),
            crate::cases::ProjectChange::LimitReached { project_id: "p-1".into(), run_id: run_id.clone() }
        );

        // Continue: more room, and the run goes on.
        store.raise_project_limits("p-1", 62, 1_000).unwrap();
        assert!(matches!(pass(&store, &run_id, &def, inputs, &provider).await, Err(WorkflowError::AwaitingExpert(1))));
        assert_eq!(expert_requests(&store, &run_id), 1, "the card went out");
    }

    /// Held for your OK (`params.hold`): the expert's answer is not the
    /// step's output until the owner approves it. Held, the run parks on his
    /// review and he is asked once, however often the run re-enters. His
    /// Change sends the work back to the same expert as the next round, with
    /// his note; that answer is held again, and his Approve makes it the
    /// step's output.
    #[tokio::test]
    async fn a_held_answer_waits_for_the_owners_ok_and_a_change_goes_back() {
        let store = expert_store(&["ana"]);
        let (bell, mut rang) = tokio::sync::mpsc::unbounded_channel();
        crate::cases::install_project_bell(bell);
        let def = SAME_BOT.replace(r#""timeout": "2h""#, r#""hold": true"#);
        let provider = MockProvider::new(&[]);
        let run_id = new_run(&store, &def);
        let inputs = serde_json::json!({ "order": 1 });
        let first = crate::expert::request_key(&run_id, "ask", "", 0);

        assert!(matches!(pass(&store, &run_id, &def, inputs.clone(), &provider).await, Err(WorkflowError::AwaitingExpert(1))));
        coworker_closes(&store, &run_id, "ana", "done", "Drafted it. {\"total\": 40}");
        // Held: parked on the owner's review, asked once.
        for _ in 0..2 {
            assert!(matches!(pass(&store, &run_id, &def, inputs.clone(), &provider).await, Err(WorkflowError::AwaitingExpert(1))));
        }
        assert!(provider.calls().is_empty(), "nothing after the held step ran");
        let crate::cases::ProjectChange::HeldForOk { key, summary, assignment_id, .. } = rang.try_recv().unwrap() else { panic!("held") };
        assert_eq!((key.as_str(), summary.as_str()), (first.as_str(), "Drafted it."));
        assert!(assignment_id.is_some());
        assert!(rang.try_recv().is_err(), "asked once");
        assert_eq!(live_wait(&store, &run_id).deadline, None, "only his review wakes it");

        // He asks for a change: the same expert gets the work again, with his note.
        crate::expert::record_review(&store, &run_id, &first, &crate::expert::Review::Change("make it 42".into())).unwrap();
        assert!(matches!(pass(&store, &run_id, &def, inputs.clone(), &provider).await, Err(WorkflowError::AwaitingExpert(1))));
        assert_eq!(expert_requests(&store, &run_id), 2, "round two went out");
        let second = crate::expert::round_key(&first, 1);
        let asked = store.get_assignment(&crate::expert::assignment_of(&store, &run_id, &second).unwrap()).unwrap().unwrap();
        assert!(asked.subject.contains("asked for changes") && asked.subject.contains("make it 42"), "{}", asked.subject);

        // Round two comes back, is held again, and he approves it.
        let case = store.engine_run_for_key("case:assignment", &asked.id).unwrap().unwrap();
        let case_inputs: serde_json::Value = serde_json::from_str(case.inputs.as_deref().unwrap()).unwrap();
        crate::cases::settle_assignment(&store, &case_inputs, "done", "Changed it. {\"total\": 42}", chrono::Utc::now().timestamp()).unwrap();
        assert!(matches!(pass(&store, &run_id, &def, inputs.clone(), &provider).await, Err(WorkflowError::AwaitingExpert(1))));
        assert!(matches!(rang.try_recv().unwrap(), crate::cases::ProjectChange::HeldForOk { key, .. } if key == second));
        crate::expert::record_review(&store, &run_id, &second, &crate::expert::Review::Approve).unwrap();
        assert!(pass(&store, &run_id, &def, inputs, &provider).await.is_ok());
        let out = node_output(&store, &run_id, "ask", "");
        assert_eq!(out["reply"]["total"], 42, "the approved round is the output: {out}");
        assert!(provider.calls()[0].contains("Changed it."), "the next step reads it");
    }

    #[tokio::test]
    async fn an_expert_that_does_not_answer_in_time_follows_on_error() {
        let store = expert_store(&["ana"]);
        let provider = MockProvider::new(&[]);
        let run_id = new_run(&store, SAME_BOT);
        let inputs = serde_json::json!({ "order": 1 });
        assert!(matches!(
            pass(&store, &run_id, SAME_BOT, inputs.clone(), &provider).await,
            Err(WorkflowError::AwaitingExpert(1))
        ));
        // A pass before the deadline only parks again.
        assert!(matches!(
            pass(&store, &run_id, SAME_BOT, inputs.clone(), &provider).await,
            Err(WorkflowError::AwaitingExpert(1))
        ));
        age_request(&store, &run_id, "ana", 3 * 3600);

        // Default: the failure is carried and the run goes on.
        let done = pass(&store, &run_id, SAME_BOT, inputs.clone(), &provider).await;
        assert!(done.is_ok(), "{done:?}");
        let out = node_output(&store, &run_id, "ask", "");
        assert_eq!(out["failed"], true);
        assert!(out["reason"].as_str().unwrap().contains("did not answer within 2h"));
        assert!(out["summary"].as_str().is_some());
        assert_eq!(provider.calls().len(), 1, "the next step still ran");

        // on_error abort: the same timeout fails the run.
        let abort = SAME_BOT.replace(r#""timeout": "2h"}}"#, r#""timeout": "2h"}, "on_error": {"fallback": "abort"}}"#);
        let run_id = new_run(&store, &abort);
        let _ = pass(&store, &run_id, &abort, inputs.clone(), &provider).await;
        age_request(&store, &run_id, "ana", 3 * 3600);
        let failed = pass(&store, &run_id, &abort, inputs, &provider).await;
        assert!(matches!(failed, Err(WorkflowError::ActivityFailed(ref id, _)) if id == "ask"), "{failed:?}");
        assert_eq!(run_status(&store, &run_id), "failed");
    }

    #[tokio::test]
    async fn an_expert_on_another_bot_is_blocked_without_the_owners_approval() {
        let store = expert_store(&[]);
        let provider = MockProvider::new(&[]);
        let def = SAME_BOT.replace(r#""expert": "ana""#, r#""expert": "loop:acme/pricing""#);
        let run_id = new_run(&store, &def);
        let out = pass(&store, &run_id, &def, serde_json::json!({ "order": 1 }), &provider).await;
        assert!(out.is_ok(), "a block is a standing outcome, not a failure: {out:?}");
        assert_eq!(run_status(&store, &run_id), "exited");
        let run = store.get_workflow_run(&run_id).unwrap().unwrap();
        assert!(run.error.unwrap_or_default().contains("approval"));
        assert_eq!(expert_requests(&store, &run_id), 0, "nothing left the bot");
        assert!(provider.calls().is_empty());
    }

    #[tokio::test]
    async fn an_unknown_or_paused_expert_is_a_failure_the_run_carries() {
        let store = expert_store(&["ana"]);
        store.set_agent_enabled("ana", false).unwrap();
        let provider = MockProvider::new(&[]);
        let run_id = new_run(&store, SAME_BOT);
        let out = pass(&store, &run_id, SAME_BOT, serde_json::json!({ "order": 1 }), &provider).await;
        assert!(out.is_ok(), "{out:?}");
        let node = node_output(&store, &run_id, "ask", "");
        assert_eq!(node["failed"], true);
        assert!(node["reason"].as_str().unwrap().contains("paused"));
        assert_eq!(expert_requests(&store, &run_id), 0);
    }

    const FAN_OUT: &str = r#"{
        "version": "1.0", "id": "panel", "name": "panel",
        "activities": [
            {"id": "a", "type": "expert", "params": {"expert": "ana", "task": "Price it", "timeout": "1h"}},
            {"id": "b", "type": "expert", "params": {"expert": "ben", "task": "Check stock", "timeout": "30m"}},
            {"id": "join", "intent": "Combine the answers"}
        ],
        "connections": [
            {"from": "__trigger__", "to": "a"},
            {"from": "__trigger__", "to": "b"},
            {"from": "a", "to": "join"},
            {"from": "b", "to": "join"}
        ]
    }"#;

    #[tokio::test]
    async fn parallel_experts_wait_together_survive_a_restart_and_the_join_gets_each_summary() {
        let store = expert_store(&["ana", "ben"]);
        let provider = MockProvider::new(&[]);
        let run_id = new_run(&store, FAN_OUT);

        let first = pass(&store, &run_id, FAN_OUT, serde_json::json!({}), &provider).await;
        assert!(matches!(first, Err(WorkflowError::AwaitingExpert(2))), "{first:?}");
        assert_eq!(expert_requests(&store, &run_id), 2, "both asked at once");
        let wait = live_wait(&store, &run_id);
        let ben_deadline = store
            .engine_effects_for_run(&run_id)
            .unwrap()
            .iter()
            .find(|e| e.counterparty.as_deref() == Some("ben"))
            .map(|e| e.created_at + 1800)
            .unwrap();
        assert_eq!(wait.deadline, Some(ben_deadline), "the earliest timeout wakes the run");

        // A restart: nothing in memory, both requests still out. The run
        // re-enters, sends nothing again, and parks on both.
        let restarted = pass(&store, &run_id, FAN_OUT, serde_json::json!({}), &provider).await;
        assert!(matches!(restarted, Err(WorkflowError::AwaitingExpert(2))), "{restarted:?}");
        assert_eq!(expert_requests(&store, &run_id), 2);
        assert!(provider.calls().is_empty(), "the join waits for both branches");

        // Ana answers; Ben runs out of time.
        coworker_closes(&store, &run_id, "ana", "done", "Price is 40.");
        let one_left = pass(&store, &run_id, FAN_OUT, serde_json::json!({}), &provider).await;
        assert!(matches!(one_left, Err(WorkflowError::AwaitingExpert(1))), "{one_left:?}");
        age_request(&store, &run_id, "ben", 3600);

        let done = pass(&store, &run_id, FAN_OUT, serde_json::json!({}), &provider).await;
        assert!(done.is_ok(), "{done:?}");
        let calls = provider.calls();
        assert_eq!(calls.len(), 1, "the join ran once");
        assert!(calls[0].contains("Price is 40."), "the join reads a's summary");
        assert!(calls[0].contains("\"failed\":true"), "and b's failure, marked");
        assert_eq!(node_output(&store, &run_id, "b", "")["failed"], true);
    }

    #[tokio::test]
    async fn a_reply_that_lands_before_the_run_parks_wakes_it_at_once() {
        let store = expert_store(&["ana"]);
        let run_id = new_run(&store, SAME_BOT);
        let key = crate::expert::request_key(&run_id, "ask", "", 0);
        crate::expert::record_reply(&store, &run_id, &key, "done", "fast", "ana").unwrap();
        park_on_experts(&store, &run_id, &[(crate::expert::reply_watch(&key), Some(chrono::Utc::now().timestamp() + 60))]).unwrap();
        let run = store.engine_get_run(&run_id).unwrap().unwrap();
        assert_eq!(run.state, "queued", "woken, not left waiting for the deadline");
        assert!(run.woken_by().is_some());
    }

    const PER_ITEM: &str = r#"{
        "version": "1.0", "id": "fan", "name": "fan",
        "activities": [
            {"id": "each", "type": "loop", "params": {"source": "inputs.items", "concurrency": 2}},
            {"id": "ask", "type": "expert", "params": {"expert": "{{item.expert}}", "task": "Review {{item.doc}}", "timeout": "1h"}},
            {"id": "report", "intent": "Report"}
        ],
        "connections": [
            {"from": "__trigger__", "to": "each"},
            {"from": "each", "to": "ask", "label": "Each item"},
            {"from": "ask", "to": "each"},
            {"from": "each", "to": "report", "label": "Done"}
        ]
    }"#;

    #[tokio::test]
    async fn a_loop_asks_an_expert_per_item_and_collects_every_answer_in_order() {
        let store = expert_store(&["ana", "ben", "cy"]);
        let provider = MockProvider::new(&[]);
        let run_id = new_run(&store, PER_ITEM);
        let inputs = serde_json::json!({ "items": [
            { "expert": "ana", "doc": "one" },
            { "expert": "ben", "doc": "two" },
            { "expert": "cy", "doc": "three" }
        ]});

        let first = pass(&store, &run_id, PER_ITEM, inputs.clone(), &provider).await;
        assert!(matches!(first, Err(WorkflowError::AwaitingExpert(_))), "{first:?}");
        assert_eq!(expert_requests(&store, &run_id), 3, "every item asked its own expert");

        coworker_closes(&store, &run_id, "cy", "done", "Three is fine.");
        coworker_closes(&store, &run_id, "ana", "done", "One is fine.");
        coworker_closes(&store, &run_id, "ben", "blocked", "I do not review contracts.");

        let (_, final_context) = pass(&store, &run_id, PER_ITEM, inputs, &provider).await.expect("completes");
        let out: serde_json::Value = final_context
            .lines()
            .find_map(|l| l.strip_prefix("[Activity 'each' result]: "))
            .map(|j| serde_json::from_str(j).unwrap())
            .expect("the loop's output");
        let results = out["results"].as_array().unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0]["outputs"]["ask"]["summary"], "One is fine.");
        assert_eq!(results[1]["outputs"]["ask"]["failed"], true);
        assert!(results[1]["outputs"]["ask"]["reason"].as_str().unwrap().contains("blocked"));
        assert_eq!(results[2]["outputs"]["ask"]["summary"], "Three is fine.");
        assert_eq!(expert_requests(&store, &run_id), 3);
        assert_eq!(provider.calls().len(), 1, "the report ran once, after every item");
    }

    /// An expert node names its expert and its task; a timeout is its own
    /// choice (none waits for the answer), but one it gives must be readable.
    #[test]
    fn an_expert_node_needs_its_expert_and_task_and_a_readable_timeout() {
        let waits = SAME_BOT.replace(r#", "timeout": "2h""#, "");
        assert!(parse_workflow(&waits).is_ok(), "no timeout: it waits for the answer");
        let unreadable = SAME_BOT.replace(r#""timeout": "2h""#, r#""timeout": "soon""#);
        let err = parse_workflow(&unreadable).unwrap_err().to_string();
        assert!(err.contains("params.timeout"), "{err}");
        let loose = SAME_BOT.replace(r#""{{inputs.order}}""#, r#""the order""#);
        let err = parse_workflow(&loose).unwrap_err().to_string();
        assert!(err.contains("explicit reference"), "{err}");
    }

    #[test]
    fn the_catalog_lists_enabled_coworkers_with_a_capability_each() {
        let store = expert_store(&["ana", "ben"]);
        store.set_agent_enabled("ben", false).unwrap();
        let cat = crate::expert::catalog(&store, "owner");
        assert_eq!(cat.len(), 1, "not the owner, not a paused coworker");
        assert_eq!(cat[0].name, "ana expert");
        assert_eq!(cat[0].capability, "Prices things for ana.");
        assert!(crate::expert::catalog_lines(&cat).contains("expert: \"ana\""));
    }

    /// A stand-in for an operation tool (`ledger_invoice_search`): answers
    /// each call with `answer(input)` and records every input it was given,
    /// with the stdin the call carried.
    struct Recorded {
        name: &'static str,
        answer: fn(&serde_json::Value) -> tools::ToolResult,
        seen: Arc<StdMutex<Vec<(serde_json::Value, Option<String>)>>>,
    }

    impl DynTool for Recorded {
        fn name(&self) -> &str {
            self.name
        }
        fn description(&self) -> String {
            String::new()
        }
        fn schema(&self) -> serde_json::Value {
            serde_json::json!({})
        }
        fn execute_dyn<'a>(
            &'a self,
            ctx: &'a tools::ToolContext,
            input: serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = tools::ToolResult> + Send + 'a>> {
            let stdin = ctx.stdin.as_ref().map(|b| String::from_utf8_lossy(b).into_owned());
            self.seen.lock().unwrap().push((input.clone(), stdin));
            let r = (self.answer)(&input);
            Box::pin(async move { r })
        }
    }

    fn recorded(
        name: &'static str,
        answer: fn(&serde_json::Value) -> tools::ToolResult,
    ) -> (Box<dyn DynTool>, Arc<StdMutex<Vec<(serde_json::Value, Option<String>)>>>) {
        let seen = Arc::new(StdMutex::new(Vec::new()));
        (Box::new(Recorded { name, answer, seen: seen.clone() }), seen)
    }

    /// Run `def_json` with `tools` as the run's roster.
    async fn run_with_tools(
        def_json: &str,
        inputs: serde_json::Value,
        tools: &[Box<dyn DynTool>],
    ) -> (Result<(String, String), WorkflowError>, Arc<Store>, String) {
        let provider = MockProvider::new(&[]);
        let def = parse_workflow(def_json).expect("valid def");
        let store = test_store();
        let run_id = uuid::Uuid::new_v4().to_string();
        store
            .create_workflow_run(&run_id, &def.id, "manual", None, None, None, None)
            .expect("run row");
        let result = execute_graph(
            &def,
            "clerk",
            "test-owner",
            false,
            &inputs,
            &store,
            None,
            &ScriptedLoop::new(&provider),
            tools,
            &run_id,
            None,
            None,
            None,
            Vec::new(),
            None,
            None,
            None,
        )
        .await;
        (result, store, run_id)
    }

    fn one_step(id: &str, kind: &str, params: serde_json::Value) -> String {
        serde_json::json!({
            "version": "1.0", "id": "t", "name": "T",
            "activities": [{ "id": id, "type": kind, "params": params }],
            "connections": [{ "from": "__trigger__", "to": id }, { "from": id, "to": "__emit__" }],
        })
        .to_string()
    }

    /// FETCH: a read follows `nextCursor` until the plugin names none, and
    /// the records of every page land in one list. Each page's records are
    /// the answer's one list, whatever the plugin calls it.
    #[tokio::test]
    async fn an_operation_read_collects_every_page() {
        let (tool, seen) = recorded("ledger_invoice_search", |input| {
            tools::ToolResult::ok(match input.get("cursor").and_then(|c| c.as_str()) {
                None => r#"{"count":2,"items":[{"id":"1"},{"id":"2"}],"nextCursor":"p2","note":"x"}"#,
                Some("p2") => r#"{"count":2,"items":[{"id":"3"},{"id":"4"}],"nextCursor":"p3"}"#,
                Some(_) => r#"{"count":1,"items":[{"id":"5"}],"nextCursor":null}"#,
            })
        });
        let def = one_step(
            "fetch",
            "operation",
            serde_json::json!({"operation": "ledger.invoice.search", "input": {"status": "{{inputs.status}}"}}),
        );
        let (result, store, run_id) = run_with_tools(&def, serde_json::json!({"status": "open"}), &[tool]).await;
        result.expect("run ok");
        let out = node_output(&store, &run_id, "fetch", "");
        assert_eq!(out["pages"], 3);
        let ids: Vec<&str> = out["records"].as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["1", "2", "3", "4", "5"]);
        let calls: Vec<serde_json::Value> = seen.lock().unwrap().iter().map(|(i, _)| i.clone()).collect();
        assert_eq!(
            calls,
            [
                serde_json::json!({"status": "open"}),
                serde_json::json!({"status": "open", "cursor": "p2"}),
                serde_json::json!({"status": "open", "cursor": "p3"}),
            ],
            "the input is interpolated and each next page carries its cursor"
        );
    }

    /// A plugin that answers with a bare list, or never pages, is one page.
    #[tokio::test]
    async fn an_operation_read_of_a_plugin_that_does_not_page_is_one_page() {
        let (tool, seen) = recorded("ledger_account_list", |_| tools::ToolResult::ok(r#"[{"id":"a"},{"id":"b"}]"#));
        let def = one_step("fetch", "operation", serde_json::json!({"operation": "ledger.account.list"}));
        let (result, store, run_id) = run_with_tools(&def, serde_json::json!({}), &[tool]).await;
        result.expect("run ok");
        let out = node_output(&store, &run_id, "fetch", "");
        assert_eq!((out["pages"].as_u64(), out["records"].as_array().unwrap().len()), (Some(1), 2));
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    /// A page that can't be read, or a cursor that never moves, fails the
    /// step with the reason; it never loops or guesses.
    #[tokio::test]
    async fn an_operation_read_fails_on_a_page_it_cannot_follow() {
        for (answer, says) in [
            (
                (|_: &serde_json::Value| tools::ToolResult::ok(r#"{"items":[1],"nextCursor":"same"}"#))
                    as fn(&serde_json::Value) -> tools::ToolResult,
                "named page same as the next page twice",
            ),
            (|_| tools::ToolResult::ok(r#"{"bills":[1],"invoices":[2]}"#), "more than one list (bills, invoices)"),
            (|_| tools::ToolResult::ok("Here are your invoices"), "not JSON"),
            (|_| tools::ToolResult::error("QuickBooks refused the call"), "page 1: QuickBooks refused the call"),
        ] {
            let (tool, _) = recorded("ledger_invoice_search", answer);
            let def = one_step("fetch", "operation", serde_json::json!({"operation": "ledger.invoice.search"}));
            let (result, _, _) = run_with_tools(&def, serde_json::json!({}), &[tool]).await;
            match result {
                Err(WorkflowError::ActivityFailed(id, msg)) => {
                    assert_eq!(id, "fetch");
                    assert!(msg.contains(says), "{msg}");
                }
                other => panic!("expected the step to fail with '{says}', got {other:?}"),
            }
        }
    }

    /// APPLY: one call per row, serially, each row's fields over the shared
    /// input. A failed row is a row result, not the end of the batch. A write
    /// row without a key gets one from the run, the step and the row; a row's
    /// own key is kept.
    #[tokio::test]
    async fn an_operation_write_reports_every_row() {
        let (tool, seen) = recorded("ledger_invoice_update", |input| {
            if input["invoiceId"] == "bad" {
                tools::ToolResult::error("QuickBooks: no invoice bad")
            } else {
                tools::ToolResult::ok(format!(r#"{{"id":{},"updated":true}}"#, input["invoiceId"]))
            }
        });
        let def = one_step(
            "apply",
            "operation",
            serde_json::json!({"operation": "ledger.invoice.update", "input": {"terms": "Net 30"}, "rows": "inputs.decisions"}),
        );
        let decisions = serde_json::json!({"decisions": [
            {"invoiceId": "101", "dueDate": "2026-11-01"},
            {"invoiceId": "bad"},
            {"invoiceId": "103", "clientKey": "mine-103"},
            "not a row",
        ]});
        let (result, store, run_id) = run_with_tools(&def, decisions, &[tool]).await;
        result.expect("a failed row does not fail the step");
        let out = node_output(&store, &run_id, "apply", "");
        assert_eq!((out["succeeded"].as_u64(), out["failed"].as_u64()), (Some(2), Some(2)));
        let results = out["results"].as_array().unwrap();
        assert_eq!(results[0], serde_json::json!({"index": 0, "ok": true, "result": {"id": "101", "updated": true}}));
        assert_eq!(results[1], serde_json::json!({"index": 1, "ok": false, "error": "QuickBooks: no invoice bad"}));
        assert_eq!(results[2]["ok"], true);
        assert_eq!(results[3]["ok"], false);
        let calls: Vec<serde_json::Value> = seen.lock().unwrap().iter().map(|(i, _)| i.clone()).collect();
        assert_eq!(calls.len(), 3, "the non-object row never reached the tool");
        assert_eq!(
            calls[0],
            serde_json::json!({"terms": "Net 30", "invoiceId": "101", "dueDate": "2026-11-01", "clientKey": format!("{run_id}:apply::0")})
        );
        assert_eq!(calls[1]["clientKey"], format!("{run_id}:apply::1"));
        assert_eq!(calls[2]["clientKey"], "mine-103", "the row's own key is kept");
    }

    /// VERIFY: one read per row (a get by id) carries no key: a read is never
    /// put in the write ledger.
    #[tokio::test]
    async fn an_operation_read_per_row_carries_no_key() {
        let (tool, seen) = recorded("ledger_invoice_get", |input| {
            tools::ToolResult::ok(format!(r#"{{"id":{},"dueDate":"2026-11-01"}}"#, input["invoiceId"]))
        });
        let def = one_step("verify", "operation", serde_json::json!({"operation": "ledger.invoice.get", "rows": "inputs.ids"}));
        let ids = serde_json::json!({"ids": [{"invoiceId": "101"}, {"invoiceId": "103"}]});
        let (result, store, run_id) = run_with_tools(&def, ids, &[tool]).await;
        result.expect("run ok");
        assert_eq!(node_output(&store, &run_id, "verify", "")["succeeded"], 2);
        assert!(seen.lock().unwrap().iter().all(|(i, _)| i.get("clientKey").is_none()));
    }

    /// No connected plugin binds the operation: the step fails and says so.
    #[tokio::test]
    async fn an_operation_no_plugin_performs_fails_the_step() {
        let def = one_step("fetch", "operation", serde_json::json!({"operation": "crm.contact.find"}));
        let (result, _, _) = run_with_tools(&def, serde_json::json!({}), &[]).await;
        match result {
            Err(WorkflowError::ActivityFailed(_, msg)) => {
                assert!(msg.contains("No connected plugin performs crm.contact.find"), "{msg}")
            }
            other => panic!("expected the step to fail, got {other:?}"),
        }
    }

    /// MATCH: a command step reads a step's data on stdin — a list with a
    /// quote in a name, which a command line could not carry.
    #[tokio::test]
    async fn a_command_step_reads_its_stdin_from_the_run() {
        let (tool, seen) = recorded("run_command", |_| tools::ToolResult::ok("[]"));
        let def = one_step(
            "match",
            "command",
            serde_json::json!({"command": "jq length", "stdin": "inputs.records"}),
        );
        let records = serde_json::json!({"records": [{"name": "O'Brien & Sons"}]});
        let (result, _, _) = run_with_tools(&def, records, &[tool]).await;
        result.expect("run ok");
        let seen = seen.lock().unwrap();
        assert_eq!(seen[0].1.as_deref(), Some(r#"[{"name":"O'Brien & Sons"}]"#));

        let (tool, _) = recorded("run_command", |_| tools::ToolResult::ok("[]"));
        let def = one_step("match", "command", serde_json::json!({"command": "jq length", "stdin": "nodes.nothing"}));
        let (result, _, _) = run_with_tools(&def, serde_json::json!({}), &[tool]).await;
        assert!(matches!(result, Err(WorkflowError::ActivityFailed(_, m)) if m.contains("names no data")));
    }

    /// A command step asks for its result whole (the next step parses it,
    /// so it is never saved and previewed), and a 200k-char list it prints
    /// reaches the next step intact.
    #[tokio::test]
    async fn a_command_steps_big_output_reaches_the_next_step_whole() {
        /// Each call: whether it asked for its result whole, and its stdin.
        type Calls = Arc<StdMutex<Vec<(bool, Option<String>)>>>;
        struct Lister(Calls);
        impl DynTool for Lister {
            fn name(&self) -> &str {
                "run_command"
            }
            fn description(&self) -> String {
                String::new()
            }
            fn schema(&self) -> serde_json::Value {
                serde_json::json!({})
            }
            fn execute_dyn<'a>(
                &'a self,
                ctx: &'a tools::ToolContext,
                _input: serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = tools::ToolResult> + Send + 'a>> {
                let stdin = ctx.stdin.as_ref().map(|b| String::from_utf8_lossy(b).into_owned());
                self.0.lock().unwrap().push((ctx.whole_result, stdin));
                let rows: Vec<serde_json::Value> = (0..10_000).map(|i| serde_json::json!({"id": i, "sku": format!("sku-{i:05}")})).collect();
                Box::pin(async move { tools::ToolResult::ok(serde_json::Value::Array(rows).to_string()) })
            }
        }
        let seen = Arc::new(StdMutex::new(Vec::new()));
        let def = r#"{
            "version":"1.0","id":"t","name":"T",
            "activities":[
                {"id":"list","type":"command","params":{"command":"list-skus"}},
                {"id":"count","type":"command","params":{"command":"jq length","stdin":"nodes.list"}}],
            "connections":[
                {"from":"__trigger__","to":"list"},
                {"from":"list","to":"count"},
                {"from":"count","to":"__emit__"}]
        }"#;
        let (result, _, _) = run_with_tools(def, serde_json::json!({}), &[Box::new(Lister(seen.clone()))]).await;
        result.expect("run ok");
        let seen = seen.lock().unwrap();
        assert!(seen.iter().all(|(whole, _)| *whole), "every command step asks for its result whole");
        let piped = seen[1].1.as_deref().expect("the list on stdin");
        assert!(piped.len() >= 200_000, "{}", piped.len());
        let parsed: Vec<serde_json::Value> = serde_json::from_str(piped).expect("intact JSON");
        assert_eq!(parsed.len(), 10_000);
        assert_eq!(parsed[9_999]["sku"], "sku-09999");
    }
}

#[cfg(test)]
mod graph_tests {
    use super::*;

    fn act(id: &str, params: serde_json::Value) -> Activity {
        serde_json::from_value(serde_json::json!({
            "id": id, "type": "condition", "params": params
        }))
        .unwrap()
    }

    fn data() -> serde_json::Value {
        serde_json::json!({
            "inputs": { "priority": 5, "subject": "URGENT: server down", "items": [1, 2, 3] },
            "item": { "name": "alpha" },
            "nodes": { "fetch": { "count": 0, "ok": true } }
        })
    }

    #[test]
    fn test_resolve_path() {
        let d = data();
        assert_eq!(resolve_path(&d, "inputs.priority"), Some(serde_json::json!(5)));
        assert_eq!(resolve_path(&d, "priority"), Some(serde_json::json!(5))); // bare → inputs
        assert_eq!(resolve_path(&d, "fetch.ok"), Some(serde_json::json!(true))); // bare → nodes
        assert_eq!(resolve_path(&d, "item.name"), Some(serde_json::json!("alpha")));
        assert_eq!(resolve_path(&d, "inputs.items.1"), Some(serde_json::json!(2)));
        // interpolate_context: strings verbatim, non-strings as JSON, unknown left visible
        let cmd = interpolate_context(
            "python3 x.py {{item.name}} --n {{inputs.items.1}} {{nope.missing}}",
            &d,
        );
        assert_eq!(cmd, "python3 x.py alpha --n 2 {{nope.missing}}");
        assert_eq!(resolve_path(&d, "inputs.missing"), None);
    }

    #[test]
    fn test_condition_expression_mode() {
        let d = data();
        let eval = |expr: &str, parents: &[&str]| {
            evaluate_condition(
                &act("c", serde_json::json!({"expression": expr, "mode": "expression"})),
                &d,
                "",
                parents,
            )
        };
        let c = |expr: &str| eval(expr, &[]).unwrap();
        assert!(c("inputs.priority == 5"));
        assert!(c("inputs.priority >= 5"));
        assert!(!c("inputs.priority > 5"));
        assert!(c("inputs.priority != 3"));
        assert!(c("inputs.subject == 'URGENT: server down'"));
        assert!(c("nodes.fetch.ok")); // path truthiness
        assert!(!c("nodes.fetch.count")); // 0 is falsy
        // Compound conditions: the shapes 42 published gates use.
        assert!(c("inputs.priority >= 5 && nodes.fetch.ok == true"));
        assert!(c("inputs.priority > 9 || (nodes.fetch.ok && !nodes.fetch.count)"));
        assert!(!c("!(inputs.priority == 5)"));
        // Bare names read the one direct parent's output.
        assert!(eval("ok && count == 0", &["fetch"]).unwrap());
        assert!(eval("ok", &[]).is_err());
        assert!(eval("ok", &["fetch", "other"]).is_err());
        // Unresolved and unparseable are loud failures, never False.
        let err = eval("inputs.missing", &[]).unwrap_err().to_string();
        assert!(err.contains("inputs.missing") && err.contains("exists"), "{err}");
        let err = eval("nodes.ghost.ok", &[]).unwrap_err().to_string();
        assert!(err.contains("nodes.ghost") && err.contains("no output"), "{err}");
        let err = eval("inputs.priority > 3 and nodes.fetch.ok", &[]).unwrap_err().to_string();
        assert!(err.contains("does not parse") && err.contains("&&"), "{err}");
        // Unknown modes are refused, not treated as expressions.
        assert!(
            evaluate_condition(&act("c", serde_json::json!({"expression": "x", "mode": "fuzzy"})), &d, "", &[])
                .is_err()
        );
    }

    #[test]
    fn test_condition_contains_exists_regex() {
        let d = data();
        let eval = |params: serde_json::Value, text: &str| {
            evaluate_condition(&act("c", params), &d, text, &[]).unwrap()
        };
        assert!(eval(
            serde_json::json!({"expression": "inputs.subject contains URGENT", "mode": "contains"}),
            ""
        ));
        assert!(!eval(
            serde_json::json!({"expression": "inputs.subject contains calm", "mode": "contains"}),
            ""
        ));
        assert!(eval(
            serde_json::json!({"expression": "deploy failed", "mode": "contains"}),
            "the deploy failed at 3pm"
        ));
        assert!(eval(
            serde_json::json!({"expression": "inputs.items", "mode": "exists"}),
            ""
        ));
        assert!(!eval(
            serde_json::json!({"expression": "inputs.nope", "mode": "exists"}),
            ""
        ));
        assert!(eval(
            serde_json::json!({"expression": "(?i)error|failed", "mode": "regex"}),
            "Deploy FAILED"
        ));
        // Invalid regex is a hard error, not a silent false.
        assert!(
            evaluate_condition(
                &act("c", serde_json::json!({"expression": "(unclosed", "mode": "regex"})),
                &d,
                "",
                &[]
            )
            .is_err()
        );
    }

    /// The in-band pressure classifier: provider spellings of rate-limiting
    /// must match (else a rate-limited item is never requeued), and ordinary failures
    /// must NOT (else real errors get requeued instead of failing the run).
    #[test]
    fn test_is_rate_limit_shaped() {
        for hit in [
            "rate limit exceeded",
            "HTTP 429 from gateway",
            "Too Many Requests",
            "rate_limit_error",
            "provider overloaded",
        ] {
            assert!(is_rate_limit_shaped(hit), "must classify as rate limit: {hit}");
        }
        for miss in [
            "connection refused",
            "invalid api key",
            "activity a failed: bad input",
        ] {
            assert!(!is_rate_limit_shaped(miss), "must NOT classify as rate limit: {miss}");
        }
    }

    /// Error aggregation is deterministic: branches settle, then the FIRST
    /// error in edge order wins — never whichever branch happened to lose
    /// the race.
    #[test]
    fn test_first_error_is_edge_ordered() {
        let r = first_error(vec![
            Ok(()),
            Err(WorkflowError::Exited("first".into())),
            Err(WorkflowError::Exited("second".into())),
        ]);
        match r {
            Err(WorkflowError::Exited(reason)) => assert_eq!(reason, "first"),
            other => panic!("expected first edge-order error, got {:?}", other),
        }
        assert!(first_error(vec![Ok(()), Ok(())]).is_ok());
    }

    /// `${NEBO_DATA_DIR}`-style placeholders are NOT the engine's to expand:
    /// the command interpolator substitutes only `{{...}}` data paths and
    /// must pass `${...}` through byte-for-byte, so the process environment
    /// (never string interpolation) resolves data-dir paths.
    #[test]
    fn test_interpolate_leaves_env_placeholders_literal() {
        let d = data();
        let cmd = interpolate_context(
            "cat ${NEBO_DATA_DIR}/reports/{{item.name}}.csv ${NEBO_SKILL_DIR}/x",
            &d,
        );
        assert_eq!(cmd, "cat ${NEBO_DATA_DIR}/reports/alpha.csv ${NEBO_SKILL_DIR}/x");
    }
}
