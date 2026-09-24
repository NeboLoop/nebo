//! One round of tool calls: dispatch, gates, partition, run, caps, persist.
//! Moved here from `runner.rs` (WP1.2) without a change in behaviour; the
//! old gates move unchanged and the permission packages replace them.
//! `run_loop` calls [`run_tool_round`] until the turn driver does (WP2.3).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use futures::stream::{FuturesUnordered, StreamExt};
use tokio::sync::{RwLock, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use ai::{Provider, RequestTrace, StreamEvent, StreamEventType};
use db::Store;
use tools::{Origin, Registry, ToolContext, ToolResult};
use types::keyparser;

use crate::concurrency::ConcurrencyController;
use crate::harness::conversation::convert_messages;
use crate::harness::session_gate::RunProgress;
use crate::runner::{
    WorkflowMode, WorkflowPark, desktop_evidence, simple_hash, truncate_str,
};
use crate::session::SessionManager;

/// Timeout for individual tool execution.
const TOOL_EXECUTION_TIMEOUT: Duration = Duration::from_secs(300);
/// How often the tool clock checks whether the call is parked on the owner.
const PARKED_POLL: Duration = Duration::from_millis(250);

/// Absolute per-turn ceiling on repeats of ONE exact call (same tool, same
/// arguments), counted regardless of whether the result changed.
///
/// Every other repetition guard keys on the RESULT being identical
/// (`counts_toward_action_spiral` → `flagged_redundant` or an error). A polling
/// loop defeats all of them by construction: `docker compose logs`, `tail`, a
/// status endpoint — the bytes drift every call, so nothing is ever flagged
/// unproductive and no counter moves. Live-verified 2026-08-27: 16 identical
/// polls against a growing log produced ZERO guard firings, and the customer
/// incident it reproduces ran 12,093 requests in 24h.
///
/// This is the backstop for that class: it counts the CALL, not the answer.
/// The bound is set by EVIDENCE, not vibes: the incident's own legitimate
/// debugging repeated one `docker compose logs` 13 times in a single turn, so
/// the ceiling must clear 13 with margin — 12 would have cut that customer
/// off one call short of finishing real work. 16 sits above every legitimate
/// repeat we have observed and far below the iteration ceiling, and the abort
/// ends only the TURN (honest ControlNotice, resumable) — never the session.
///
/// The identical-call budget itself lives in `ai::call_budget` — ONE
/// implementation shared with the workflow activity loop (Rule 8). The
/// evidence-bound ceiling stays here with its incident history.
const IDENTICAL_CALL_ABORT: usize = 16;

/// The same ceiling for a call that only LOOKS: a search, a page read, a file
/// read, a screenshot. Those return the same thing every time (the web tool
/// even serves them from cache), so the third identical look is never work —
/// it is the loop. 16 was tuned for `docker compose logs`, which legitimately
/// changes between calls; the registry's concurrency-safety verdict is the
/// tool's own declaration that a call does not change anything (Nanna,
/// 2026-09-19: one search repeated 15 times in a turn, twice more the next).
const IDENTICAL_READONLY_CALL_ABORT: usize = 3;

/// Failed reads of one path before further reads of it are refused.
const READ_FAILURE_LIMIT: usize = 3;

/// Tool documentation results kept per run, and the bytes kept of each.
const MAX_TOOL_DOC_ENTRIES: usize = 5;
const MAX_TOOL_DOC_CONTENT: usize = 4_000;

/// What a run's tool calls carry: the one builder of their `ToolContext`,
/// for the round and for a CLI provider's calls over `/agent/mcp`.
#[derive(Clone, Copy)]
pub(crate) struct RunToolScope<'a> {
    pub sessions: &'a SessionManager,
    pub tx: &'a mpsc::Sender<StreamEvent>,
    pub session_id: &'a str,
    pub origin: Origin,
    pub cancel_token: &'a CancellationToken,
    pub progress: Option<&'a RunProgress>,
    pub ask_channels: Option<&'a tools::AskChannels>,
    pub full_access: bool,
    pub handoff_depth: u8,
    pub entity_permissions: Option<&'a HashMap<String, bool>>,
    pub operation_policy: Option<&'a tools::policy::OperationPolicy>,
    pub entity_resource_grants: Option<&'a HashMap<String, String>>,
    pub allowed_paths: &'a [String],
    pub run_cwd: Option<&'a str>,
    pub channel_ctx: Option<&'a tools::ChannelContext>,
    pub model_override: &'a str,
    pub memory_user_id: &'a String,
    pub memory_topics: &'a [napp::agent::MemoryTopic],
    pub memory_writes_disabled: bool,
    pub memory_write_bar: &'a Vec<types::provenance::ProvenanceClass>,
    pub audience_restricted: bool,
    pub memory_matter: &'a Option<String>,
    pub run_taint: &'a std::sync::Mutex<std::collections::BTreeSet<types::provenance::ProvenanceClass>>,
    pub review_fork: Option<&'a crate::review_fork::ReviewForkCtx>,
    pub tool_allowlist: Option<&'a HashSet<String>>,
    pub tool_denial_hint: &'a Option<String>,
    /// The tools whose definitions this step's request carried.
    pub declared_tools: &'a Arc<HashSet<String>>,
}

impl RunToolScope<'_> {
    /// The `ToolContext` every tool call of the run carries.
    pub(crate) fn tool_context(&self) -> ToolContext {
        let RunToolScope {
            sessions,
            session_id,
            origin,
            memory_user_id,
            handoff_depth,
            entity_permissions,
            operation_policy,
            entity_resource_grants,
            allowed_paths,
            run_cwd,
            cancel_token,
            tx,
            progress,
            ask_channels,
            channel_ctx,
            model_override,
            memory_topics,
            memory_writes_disabled,
            run_taint,
            memory_write_bar,
            audience_restricted,
            memory_matter,
            full_access,
            review_fork,
            tool_allowlist,
            tool_denial_hint,
            declared_tools,
        } = *self;
        let resolved_key = sessions
            .resolve_session_key(session_id)
            .unwrap_or_else(|_| session_id.to_string());
        ToolContext {
            origin,
            session_key: resolved_key,
            session_id: session_id.to_string(),
            user_id: memory_user_id.clone(),
            trusted_plugin_env: false,
            handoff_depth,
            entity_permissions: entity_permissions.cloned(),
            operation_policy: operation_policy.cloned(),
            resource_grants: entity_resource_grants.cloned(),
            allowed_paths: allowed_paths.to_vec(),
            cwd: run_cwd.map(str::to_string),
            cancel_token: cancel_token.clone(),
            stream_tx: Some(tx.clone()),
            run_id: progress.map(|p| p.run_id.clone()),
            ask_channels: ask_channels.cloned(),
            parked: Default::default(),
            channel: channel_ctx.cloned(),
            model_preference: (!model_override.is_empty()).then(|| model_override.to_string()),
            memory_topics: memory_topics.iter().map(|t| t.slug.clone()).collect(),
            memory_writes_disabled,
            run_taint: run_taint.lock().unwrap().iter().copied().collect(),
            memory_write_bar: memory_write_bar.clone(),
            audience_restricted,
            memory_matter: memory_matter.clone(),
            // Populated by the approval gate below, before tool execution.
            approved_categories: std::collections::HashSet::new(),
            full_access,
            // Restricted-run allowlist: the review fork's whitelist, or
            // the request's explicit allowlist (phone callers). None for
            // every normal run.
            tool_whitelist: review_fork
                .as_ref()
                .map(|r| r.whitelist.clone())
                .or_else(|| tool_allowlist.cloned()),
            whitelist_denial_hint: tool_denial_hint.clone(),
            learned_write_agent: review_fork.as_ref().map(|r| r.owner_agent_id.clone()),
            learned_write_staged: review_fork.as_ref().map(|r| r.staged).unwrap_or(false),
            // A fresh fork learning, not a re-apply — records its audit row.
            learned_write_reapply: false,
            skills_read: review_fork
                .as_ref()
                .map(|r| r.skills_read.clone())
                .unwrap_or_default(),
            declared_tools: Some(declared_tools.clone()),
            tool_call_id: String::new(),
        }
    }
}

/// What the round reads from the run: the seat, the channels and the run's
/// own settings.
pub(crate) struct RoundContext<'a> {
    pub scope: &'a RunToolScope<'a>,
    pub tools: &'a Arc<Registry>,
    pub store: &'a Arc<Store>,
    pub providers: &'a Arc<RwLock<Vec<Arc<dyn Provider>>>>,
    pub concurrency: &'a Arc<ConcurrencyController>,
    pub hooks: &'a napp::HookDispatcher,
    pub agent_id: &'a str,
    pub user_prompt: &'a str,
    pub iteration: usize,
    pub approval_channels: Option<&'a tools::ApprovalChannels>,
    pub approval_relay: bool,
    pub workflow_mode: Option<&'a WorkflowMode>,
    pub decide: Option<&'a Arc<ai::DecideClient>>,
    pub active_task: &'a String,
    pub guard_cfg: &'a crate::guardrails::GuardrailConfig,
    /// The trace a side call of this run carries: its purpose and the agent.
    pub side_trace: &'a (dyn Fn(&'static str) -> RequestTrace + Sync),
}

/// The run's state the round reads and updates: the loop guards, the read
/// ledger, the documentation cache and the done-gate bookkeeping.
pub(crate) struct RoundGuards<'a> {
    pub called_tools: &'a mut Vec<String>,
    pub recent_tool_result_hashes: &'a [(u64, u64, u64, bool)],
    pub identical_call_budget: &'a ai::call_budget::CallBudget,
    pub runaway_wrap_up: &'a mut Option<String>,
    pub runaway_wrap_up_issued: &'a mut bool,
    pub read_failures: &'a mut HashMap<String, usize>,
    pub action_call_counts: &'a mut HashMap<String, usize>,
    pub spiral_escalator: &'a mut crate::guardrails::Escalator,
    pub error_streak: &'a mut crate::guardrails::ErrorStreak,
    pub files_read_this_session: &'a mut HashSet<String>,
    pub recent_result_content_hashes: &'a mut Vec<u64>,
    pub readonly_result_hash_by_call: &'a mut HashMap<(u64, u64), u64>,
    pub read_ledger: &'a mut crate::read_ledger::ReadLedger,
    pub tool_doc_cache: &'a mut Vec<(String, String)>,
    pub plan_touch: &'a mut Option<(usize, String)>,
    pub edits_since_check: &'a mut usize,
    pub last_desktop_act: &'a mut Option<String>,
    pub ctx_spilled_results: &'a mut usize,
}

/// How a round ended.
pub(crate) enum RoundOutcome {
    /// Every call has its result saved; the loop takes another step.
    Ran(RoundResults),
    /// The round ended the turn for this reason; its notice is already sent.
    Ended(crate::guardrails::Exit),
    /// The run was cancelled while the round ran.
    Cancelled,
}

/// What a round that ran reports back to the loop.
pub(crate) struct RoundResults {
    pub all_errors_this_iteration: bool,
    pub had_results: bool,
    /// Per (name hash, args hash): whether that call made no progress.
    pub unproductive_this_iteration: HashMap<(u64, u64), bool>,
    /// The highest-signal rate-limit status seen this round (429/403).
    pub iteration_rate_limited: Option<u16>,
    /// Short snapshots of the calls and results for the tool-summary label.
    pub summary_tool_calls: Vec<ai::ToolCall>,
    pub summary_tool_results: Vec<ToolResult>,
}

/// Run the model's tool calls: hooks, the exit primitive, the loop guards,
/// the gates, then execution, result caps and persistence. A pre-execute
/// hook and input normalization may rewrite `tool_calls` in place.
pub(crate) async fn run_tool_round(
    cx: &RoundContext<'_>,
    st: RoundGuards<'_>,
    tool_calls: &mut [ai::ToolCall],
) -> RoundOutcome {
    let RoundContext {
        scope,
        tools,
        store,
        providers,
        concurrency,
        hooks,
        agent_id,
        user_prompt,
        iteration,
        approval_channels,
        approval_relay,
        workflow_mode,
        decide,
        active_task,
        guard_cfg,
        side_trace,
    } = *cx;
    let RunToolScope {
        sessions,
        tx,
        session_id,
        origin,
        cancel_token,
        progress,
        full_access,
        entity_permissions,
        operation_policy,
        run_cwd,
        review_fork,
        ..
    } = *scope;
    let RoundGuards {
        called_tools,
        recent_tool_result_hashes,
        identical_call_budget,
        runaway_wrap_up,
        runaway_wrap_up_issued,
        read_failures,
        action_call_counts,
        spiral_escalator,
        error_streak,
        files_read_this_session,
        recent_result_content_hashes,
        readonly_result_hash_by_call,
        read_ledger,
        tool_doc_cache,
        plan_touch,
        edits_since_check,
        last_desktop_act,
        ctx_spilled_results,
    } = st;
    let mut ctx = scope.tool_context();

    // Track tool names for context filtering
    for tc in tool_calls.iter() {
        called_tools.push(tc.name.clone());
    }

    // Update progress: count tools and set current tool name
    if let Some(p) = progress {
        p.tool_call_count.fetch_add(
            tool_calls.len() as u32,
            std::sync::atomic::Ordering::Relaxed,
        );
        if let Ok(mut ct) = p.current_tool.lock() {
            ct.clear();
            if tool_calls.len() == 1 {
                ct.push_str(&tool_calls[0].name);
            } else {
                ct.push_str(&format!("{} tools", tool_calls.len()));
            }
        }
    }
    // Apply tool.pre_execute filter hooks — may block individual tools.
    let mut blocked_results: Vec<Option<(ai::ToolCall, ToolResult)>> =
        vec![None; tool_calls.len()];
    // Workflow `exit` is a loop primitive, not a real tool: the first
    // exit call ends the turn before anything in the batch executes.
    let mut wf_break_reason: Option<String> = None;
    if workflow_mode.is_some()
        && let Some(tc) = tool_calls.iter().find(|tc| tc.name == "exit") {
        let reason = tc
            .input
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        wf_break_reason = Some(format!("workflow_exit:{reason}"));
    }
    // A pre hook that failed without blocking: its note rides on the
    // call's result, after the tool's own output, keyed by tool id.
    let mut pre_hook_notes: HashMap<String, String> = HashMap::new();
    let has_pre_hook = hooks.has_subscribers("tool.pre_execute");
    if has_pre_hook {
        for idx in 0..tool_calls.len() {
            let payload = serde_json::to_vec(&crate::hooks::ToolPreExecutePayload {
                tool_name: tool_calls[idx].name.clone(),
                input: tool_calls[idx].input.clone(),
                session_id: session_id.to_string(),
                tool_use_id: tool_calls[idx].id.clone(),
                cwd: run_cwd.map(str::to_string).unwrap_or_default(),
                agent_id: session_id.strip_prefix("subagent:").map(|_| session_id.to_string()),
            })
            .unwrap_or_default();
            let (result, _handled) = hooks.apply_filter("tool.pre_execute", payload).await;
            if let Ok(resp) =
                serde_json::from_slice::<crate::hooks::ToolPreExecuteResponse>(&result)
            {
                if resp.blocked {
                    let msg = resp
                        .blocked_message
                        .unwrap_or_else(|| "Blocked by plugin hook".into());
                    blocked_results[idx] =
                        Some((tool_calls[idx].clone(), ToolResult::error(msg)));
                } else {
                    if let Some(note) = resp.note {
                        pre_hook_notes.insert(tool_calls[idx].id.clone(), note);
                    }
                    if let Some(mutated_input) = resp.input {
                        tool_calls[idx].input = mutated_input;
                    }
                }
            }
        }
    }

    // Every gate below judges the call as it will run: the tool
    // settles an inferred action or resource here (after any hook
    // rewrote the input), so no call shape reaches execution past a
    // gate that read a different call.
    for tc in tool_calls.iter_mut() {
        let input = std::mem::take(&mut tc.input);
        tc.input = tools.normalize_input(&tc.name, input).await;
    }
    // What each settled call is, from its tool's spec: the guards below read
    // the job a call does (its rule key and field), never a tool name.
    let mut targets: Vec<Option<types::permissions::Target>> = Vec::with_capacity(tool_calls.len());
    for tc in tool_calls.iter() {
        targets.push(tools.target(&tc.name, &tc.input).await);
    }
    let targets = targets;

    // Hard guard: block tool calls that keep repeating identical args WITHOUT
    // making progress.
    //
    // Only UNPRODUCTIVE repeats count — a call that errored, or returned content
    // the model already had. This guard used to count every repeat including
    // successes, and "the result will not change" is simply false for a tool that
    // changes the world: re-running `pptx create` after editing its input spec
    // produces a different file every time. Blocking it stranded a model
    // mid-iteration and, because the block says to "use different parameters",
    // pushed it into renaming its own output to get past the guard — which is how
    // one deck became `deck.pptx`, `deck-v2.pptx`, `deck-final.pptx`.
    //
    // This is the same correction already applied to the spiral backstop (see
    // `counts_toward_action_spiral`): failures accrue, successes reset, and a tool
    // that mutates state is never held to a no-progress rule.
    for (idx, tc) in tool_calls.iter().enumerate() {
        if blocked_results[idx].is_some() {
            continue;
        }
        let name_hash = simple_hash(tc.name.as_bytes());
        let args_hash = simple_hash(tc.input.to_string().as_bytes());
        let unproductive_repeats = recent_tool_result_hashes
            .iter()
            .filter(|&&(nh, ah, _, unproductive)| {
                nh == name_hash && ah == args_hash && unproductive
            })
            .count();
        if unproductive_repeats >= guard_cfg.identical_args_block_after {
            blocked_results[idx] = Some((
                tc.clone(),
                ToolResult::error(format!(
                    "Blocked: {name} has been called {n} times with identical arguments \
                     and returned the same thing each time, or errored each time. \
                     Calling it the same way again, or the same way through a \
                     different command, returns the same thing. If you are waiting \
                     for a file or a process to change, wait in ONE bounded shell \
                     command instead of re-reading: os(action: \"exec\", command: \
                     \"for i in $(seq 1 12); do wc -l < FILE; test $(wc -l < FILE) -ge N && break; \
                     sleep 5; done; cat FILE\", timeout: 90), then report what you \
                     saw, changed or not, with the count and the time. Keep the loop \
                     shorter than the timeout you pass, or the command is killed and \
                     its output is discarded. If you need \
                     something different, change the arguments. Do NOT work around \
                     this by renaming an output file.",
                    name = tc.name,
                    n = unproductive_repeats + 1
                )),
            ));
        }
    }

    // ── Runaway backstop: the same exact call, over and over ──────────
    // Checked BEFORE execution and independent of every productivity
    // signal. This is the only guard that can see a drifting-output poll
    // loop, because it never looks at the result. It ENDS the turn rather
    // than nudging: the spiral backstop below resets its own budget when
    // it fires, which is precisely how a loop earns an unlimited number of
    // nudges and still runs to the iteration ceiling.
    let mut identical_call_abort: Option<(String, usize)> = None;
    for (idx, tc) in tool_calls.iter().enumerate() {
        // After the wrap-up turn a repeat ends the turn even when an
        // earlier guard already refused it: the 3-strike block kept
        // refusing the same search for ten more iterations until the
        // same-error guard finally ended the run (2026-09-19).
        if blocked_results[idx].is_some() && !*runaway_wrap_up_issued {
            continue;
        }
        let ceiling = if tools.concurrency_safe(&tc.name, &tc.input).await {
            IDENTICAL_READONLY_CALL_ABORT
        } else {
            IDENTICAL_CALL_ABORT
        };
        if let Some(repeats) = identical_call_budget.abort_due(&tc.name, &tc.input, ceiling) {
            identical_call_abort = Some((action_key(tc, targets[idx].as_ref()), repeats));
            break;
        }
    }
    if let Some((key, repeats)) = identical_call_abort.clone().filter(|_| !*runaway_wrap_up_issued) {
        // First trip: refuse the call, and make the next turn a
        // tool-less wrap-up so the user gets an answer, not a banner.
        *runaway_wrap_up_issued = true;
        warn!(session_id, action = %key, repeats, "runaway backstop: identical call refused — wrap-up turn next");
        for (idx, tc) in tool_calls.iter().enumerate() {
            if blocked_results[idx].is_none() && action_key(tc, targets[idx].as_ref()) == key {
                blocked_results[idx] = Some((
                    tc.clone(),
                    ToolResult::error(format!(
                        "Refused: this exact call has already been made {} times with identical \
                         arguments and returned the same thing each time. Do not call it again. \
                         Reply to the user now with what you have.",
                        repeats
                    )),
                ));
            }
        }
        *runaway_wrap_up = Some(format!(
            "You called '{}' {} times with identical arguments; repeating it will not change \
             the result. Tools are unavailable this turn: answer the user's latest message \
             now, in plain words, with what you already have. If something is missing, say \
             what it is and ask one question.",
            key, repeats
        ));
        identical_call_abort = None;
    }
    if let Some((key, repeats)) = identical_call_abort {
        warn!(
            session_id,
            action = %key,
            repeats,
            "runaway backstop: identical call repeated past ceiling — ending turn"
        );
        let _ = tx
            .send(StreamEvent::control_notice(
                format!(
                    "Stopped: '{}' was called {} times with identical arguments \
                     without resolving. Ending the run so it cannot continue \
                     indefinitely.",
                    key, repeats
                ),
                "runaway_tool_loop",
            ))
            .await;
        return RoundOutcome::Ended(crate::guardrails::Exit::RunawayToolLoop);
    }

    // Defense-in-depth: block repeated reads of the SAME target that keep
    // FAILING via different methods/args (which the identical-args guard above
    // misses — the #research read-loop). After READ_FAILURE_LIMIT failures of a
    // path, force the model to report instead of retrying. NOT a substitute for
    // the file-read fix.
    for (idx, tc) in tool_calls.iter().enumerate() {
        if blocked_results[idx].is_some() {
            continue;
        }
        if let Some(p) = targets[idx].as_ref().and_then(read_path)
            && read_failures.get(&p).copied().unwrap_or(0) >= READ_FAILURE_LIMIT {
            warn!(session_id, path = %p, "blocking read after repeated failures");
            blocked_results[idx] = Some((
                tc.clone(),
                ToolResult::error(format!(
                    "Blocked: reading {} has failed {} times via different methods. \
                     Stop retrying — tell the user the file could not be read and ask \
                     how they'd like to proceed.",
                    p, READ_FAILURE_LIMIT
                )),
            ));
        }
    }

    // Spiral backstop (see action_call_counts): once one (tool, action) has
    // racked up the same-action limit of UNPRODUCTIVE attempts this turn (errored or
    // returning content the model already had — glob-wander / browser re-read /
    // shell-retry), nudge the model off that action. Productive calls that
    // return novel results don't count, so legitimate bulk work (create N
    // todos, write N files) never trips this. (FRAMES Phase 2.)
    //
    // This is a NUDGE, not a stop. It used to end the run with a terminal
    // result, which turned every false positive into a dead turn the user saw
    // as a red "Stopped: … called 8 times without progress" banner — the model
    // had more to do and no way to say so. The offending call is refused with a
    // corrective error; every other tool, and the turn, carries on.
    //
    // The budget resets when it fires, so a model that changes approach isn't
    // locked out of the action for the rest of the turn — a genuine loop simply
    // earns another nudge after another limit's worth of unproductive calls.
    let mut spiral_hard_stop: Option<(String, usize)> = None;
    for (idx, tc) in tool_calls.iter().enumerate() {
        if blocked_results[idx].is_some() {
            continue;
        }
        let key = action_key(tc, targets[idx].as_ref());
        let observed = action_call_counts.get(&key).copied().unwrap_or(0);
        if observed >= guard_cfg.same_action_limit {
            warn!(
                session_id,
                action = %key,
                limit = guard_cfg.same_action_limit,
                hard_stop = guard_cfg.hard_stop,
                "spiral backstop: nudging model off repeated action"
            );
            // Hard-stop opt-in (Settings → Developer) ends the turn on the
            // first trip. Otherwise the first trip is a nudge and the
            // SECOND trip for the same action is the stop: a nudge that
            // did not help is never repeated (guardrails::Escalator).
            if guard_cfg.hard_stop
                || spiral_escalator.fire(&key) == crate::guardrails::Verdict::Stop
            {
                spiral_hard_stop = Some((key, observed));
                break;
            }
            action_call_counts.insert(key.clone(), 0);
            blocked_results[idx] = Some((
                tc.clone(),
                ToolResult::error(format!(
                    "'{}' has been called {} times this turn without resolving. \
                     Do not repeat it unchanged — change the arguments, use a \
                     different tool, or tell the user what you have so far and \
                     what is blocking you.",
                    key, guard_cfg.same_action_limit
                )),
            ));
        }
    }
    if let Some((key, observed)) = spiral_hard_stop {
        let notice = if guard_cfg.hard_stop {
            format!(
                "Stopped: '{key}' was repeated {observed} times without progress \
                 (hard stop is on)."
            )
        } else {
            format!(
                "Stopped: '{key}' was repeated {observed} times without progress, \
                 and again after being told to change approach."
            )
        };
        let _ = tx
            .send(StreamEvent::control_notice(notice, "repeated_tool_calls"))
            .await;
        return RoundOutcome::Ended(crate::guardrails::Exit::RepeatedToolCalls);
    }

    // ── Restricted-run allowlist ─────────────────────────────────────
    // Review fork (docs/design/SELF_IMPROVEMENT.md WS2) and phone-
    // caller runs: only allowlisted tools may EXECUTE; everything
    // else is denied here with a corrective error. Matching is
    // `tool:resource`-aware and shared with the registry choke point
    // (ToolContext::whitelist_allows) so the two fences can't drift.
    if ctx.tool_whitelist.is_some() {
        for (idx, tc) in tool_calls.iter().enumerate() {
            if blocked_results[idx].is_none() && !ctx.whitelist_allows(&tc.name, &tc.input)
            {
                let msg = if review_fork.is_some() {
                    format!(
                        "Background review denied non-whitelisted tool: {}. \
                         Only the skill tool is available in this review pass — \
                         save the learning with it or reply 'Nothing to save.'",
                        tc.name
                    )
                } else if let Some(ref hint) = ctx.whitelist_denial_hint {
                    format!("'{}' is not available in this run. {}", tc.name, hint)
                } else {
                    format!(
                        "'{}' is not available in this call. Use the tools you \
                         were given, or tell the caller plainly that you can't \
                         do that and offer to take a message.",
                        tc.name
                    )
                };
                blocked_results[idx] = Some((tc.clone(), ToolResult::error(msg)));
            }
        }
    }

    // ── Permission gate (PERMISSIONS_SME §11): see gate_tool_calls ──
    let gate = gate_tool_calls(
        &GateRun {
            tools,
            store,
            agent_id,
            session_id,
            session_key: &ctx.session_key,
            origin,
            full_access,
            entity_permissions,
            operation_policy,
            approval: approval_channels.map(|channels| ApprovalDoor {
                channels,
                tx,
                cancel_token,
            }),
            approval_relay,
            workflow_mode,
            sessions: Some(sessions),
        },
        tool_calls,
        &mut blocked_results,
    )
    .await;
    if gate.parked.is_some() {
        wf_break_reason = gate.parked;
    }
    let owner_answered = gate.owner_answered;
    ctx.approved_categories = gate.approved_categories;

    // ── Decide guardrail (crate::tool_guardrail) ──────────────────────
    // Last, after every gate above, and only for calls that are about
    // to run without the owner having answered a card for them: one
    // typed decision per side-effecting call judges risk and scope,
    // and the band is allow, ask (the SAME approval door as the gates
    // above, one card for the batch) or block. Off by default
    // (`NEBO_DECIDE_GUARDRAIL=1`; `=shadow` logs the band without
    // acting). Fail-open: no client, error or timeout leaves the
    // decision above unchanged.
    let guardrail_mode = crate::tool_guardrail::mode();
    if guardrail_mode != crate::tool_guardrail::Mode::Off && wf_break_reason.is_none() {
        // A workflow turn has no session objective and an empty
        // prompt; its task is the step it was given.
        let (objective, last_message, context) = match workflow_mode {
            Some(m) => (
                m.objective.as_str(),
                m.instruction.as_str(),
                crate::tool_guardrail::Context::WorkflowStep,
            ),
            None => (active_task.as_str(), user_prompt, crate::tool_guardrail::Context::Chat),
        };
        let mut judged = Vec::new();
        for (idx, tc) in tool_calls.iter().enumerate() {
            if blocked_results[idx].is_some() || owner_answered.contains(&idx) {
                continue;
            }
            if !tools.has_side_effects(&tc.name, &tc.input).await {
                continue;
            }
            let trace = side_trace("tool_guardrail");
            judged.push(async move {
                let judgment = crate::tool_guardrail::judge(
                    decide.map(|d| d.as_ref()),
                    &trace,
                    guardrail_mode,
                    &tc.name,
                    &tc.input,
                    objective,
                    last_message,
                    context,
                )
                .await;
                (idx, judgment)
            });
        }
        let mut guardrail_asks: Vec<usize> = Vec::new();
        for (idx, judgment) in futures::future::join_all(judged).await {
            let Some(judgment) = judgment else { continue };
            match crate::tool_guardrail::action_for(guardrail_mode, judgment.band) {
                crate::tool_guardrail::Band::Allow => {}
                crate::tool_guardrail::Band::Block => {
                    blocked_results[idx] = Some((
                        tool_calls[idx].clone(),
                        ToolResult::error(crate::tool_guardrail::BLOCKED_RESULT),
                    ));
                }
                crate::tool_guardrail::Band::Ask => guardrail_asks.push(idx),
            }
        }
        if !guardrail_asks.is_empty() {
            let attended = tools::ExecutionMode::from(origin)
                == tools::ExecutionMode::Interactive
                || approval_relay;
            match approval_channels {
                Some(chs) if attended => {
                    let calls: Vec<ai::ToolCall> =
                        guardrail_asks.iter().map(|&i| tool_calls[i].clone()).collect();
                    let decision = ask_tool_approval_batch(
                        chs, tx, cancel_token, &calls, session_id, "guardrail",
                    )
                    .await;
                    // "always" has nothing to persist here: the
                    // guardrail holds no per-tool grant, so it is an
                    // approval for this batch only.
                    if !matches!(
                        decision.as_str(),
                        "always" | "once" | "approve" | "approved" | "yes" | "true"
                    ) {
                        for idx in guardrail_asks {
                            blocked_results[idx] = Some((
                                tool_calls[idx].clone(),
                                ToolResult::error(crate::tool_guardrail::DECLINED_RESULT),
                            ));
                        }
                    }
                }
                // Unattended (cron/workflow/comm/subagent) or no
                // channel: nobody can answer, so the call does not
                // run, the same as the gates above.
                _ => {
                    for idx in guardrail_asks {
                        blocked_results[idx] = Some((
                            tool_calls[idx].clone(),
                            ToolResult::error(crate::tool_guardrail::UNATTENDED_RESULT),
                        ));
                    }
                }
            }
        }
    }

    // Workflow break (exit primitive / approval park): the turn ends
    // now — nothing in this batch executes.
    if let Some(reason) = wf_break_reason {
        return RoundOutcome::Ended(crate::guardrails::Exit::Workflow(reason));
    }

    // Claude Code's partitioning: consecutive concurrency-safe calls form
    // one batch that runs in parallel (at most MAX_PARALLEL_CALLS); every
    // other call runs alone; the calls' order is kept.
    let mut live: Vec<(usize, bool)> = Vec::new();
    for (idx, tc) in tool_calls.iter().enumerate() {
        if blocked_results[idx].is_none() {
            live.push((idx, tools.concurrency_safe(&tc.name, &tc.input).await));
        }
    }

    // Results as each completes; events sent immediately.
    let mut results: Vec<Option<(ai::ToolCall, ToolResult)>> = vec![None; tool_calls.len()];
    // The call's wall-clock time per tool id; persisted with the result.
    let mut durations: HashMap<String, u64> = HashMap::new();
    // Tool ids whose result a post-tool hook wrote into (the done gate's
    // "a check ran" signal).
    let mut hook_noted: HashSet<String> = HashSet::new();
    for batch in partition_tool_calls(&live) {
        let mut futures = FuturesUnordered::new();
        for idx in batch {
            let tools = tools.clone();
            let mut ctx = ctx.clone();
            let tc = tool_calls[idx].clone();
            let concurrency = concurrency.clone();
            futures.push(async move {
                let _permit = concurrency.acquire_tool_permit().await;
                let input_str = tc.input.to_string();
                let input_log = truncate_str(&input_str, 500);
                info!(tool = %tc.name, id = %tc.id, input = %input_log, "executing tool");
                let budget = tools
                    .execution_timeout(&tc.name, &tc.input)
                    .await
                    .unwrap_or(TOOL_EXECUTION_TIMEOUT);
                let started = std::time::Instant::now();
                ctx.tool_call_id = tc.id.clone();
                ctx.parked = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                let parked = ctx.parked.clone();
                let result = match run_within_budget(
                    budget,
                    parked,
                    tools.execute(&ctx, &tc.name, tc.input.clone()),
                )
                .await
                {
                    Some(r) => r,
                    None => ToolResult::error(tool_timeout_text(&tc.name, budget)),
                };
                let duration_ms = started.elapsed().as_millis() as u64;
                let result_log = truncate_str(&result.content, 300);
                info!(tool = %tc.name, id = %tc.id, is_error = result.is_error, result = %result_log, "tool result");
                (idx, tc, result, duration_ms)
            });
        }
        loop {
            let item = tokio::select! {
                _ = cancel_token.cancelled() => {
                    info!(session_id, "run cancelled during tool execution");
                    return RoundOutcome::Cancelled;
                }
                next = futures.next() => match next {
                    Some(v) => v,
                    None => break,
                }
            };
            let (idx, tc, mut result, duration_ms) = item;
            if let Some(note) = pre_hook_notes.remove(&tc.id) {
                result.content.push_str("\n\n");
                result.content.push_str(&note);
            }
            if apply_post_tool_hooks(hooks, &tc, &mut result, session_id, run_cwd).await {
                hook_noted.insert(tc.id.clone());
            }
            // Send tool result event immediately as each completes
            let _ = tx
                .send(StreamEvent { payload: result.payload.clone(),
                    provenance: None,
                    event_type: StreamEventType::ToolResult,
                    text: result.content.clone(),
                    tool_call: Some(ai::ToolCall {
                        id: tc.id.clone(),
                        name: tc.name.clone(),
                        input: tc.input.clone(),
                    }),
                    error: if result.is_error {
                        Some(result.content.clone())
                    } else {
                        None
                    },
                    usage: None,
                    rate_limit: None,
                    // The call's wall-clock time rides in the widgets slot so the
                    // live timeline and the reloaded one show the same duration.
                    widgets: Some(serde_json::json!({ "duration_ms": duration_ms })),
                    provider_metadata: None,
                    stop_reason: None,
                    image_url: result.image_url.clone(),
                })
                .await;
            durations.insert(tc.id.clone(), duration_ms);
            results[idx] = Some((tc, result));
        }
    }

    // Inject blocked tool results (from pre_execute hooks).
    for (idx, blocked) in blocked_results.into_iter().enumerate() {
        if let Some((tc, result)) = blocked {
            let _ = tx
                .send(StreamEvent { payload: None,
                    provenance: None,
                    event_type: StreamEventType::ToolResult,
                    text: result.content.clone(),
                    tool_call: Some(ai::ToolCall {
                        id: tc.id.clone(),
                        name: tc.name.clone(),
                        input: tc.input.clone(),
                    }),
                    error: Some(result.content.clone()),
                    usage: None,
                    rate_limit: None,
                    widgets: None,
                    provider_metadata: None,
                    stop_reason: None,
                    image_url: None,
                })
                .await;
            results[idx] = Some((tc, result));
        }
    }

    // Sidecar vision verification — only for providers that can't include
    // images directly in tool results. Vision-capable providers (Anthropic,
    // Gemini) get the raw image passed through instead.
    let mut had_image: Vec<usize> = Vec::new();
    {
        let main_supports_images = {
            let prov_lock = providers.read().await;
            prov_lock
                .first()
                .is_some_and(|p| p.supports_tool_result_images())
        };

        if !main_supports_images {
            let sidecar_provider = {
                let prov_lock = providers.read().await;
                prov_lock.first().cloned()
            };
            if let Some(provider) = sidecar_provider {
                let mut sidecar_futures = FuturesUnordered::new();

                for (idx, entry) in results.iter().enumerate() {
                    if let Some((tc, result)) = entry
                        && let Some(ref image_url) = result.image_url {
                        had_image.push(idx);
                        let image_url = image_url.clone();
                        let action_ctx = format!("{} — {}", tc.name, result.content);
                        let prov = provider.clone();
                        let trace = side_trace("screenshot_verify");
                        sidecar_futures.push(async move {
                            let verification = crate::sidecar::verify_screenshot(
                                trace,
                                prov.as_ref(),
                                &image_url,
                                &action_ctx,
                            )
                            .await;
                            (idx, verification)
                        });
                    }
                }

                while let Some((idx, verification)) = tokio::select! {
                    _ = cancel_token.cancelled() => {
                        info!(session_id, "run cancelled during sidecar verification");
                        return RoundOutcome::Cancelled;
                    }
                    next = sidecar_futures.next() => next
                } {
                    if let Some((_, ref mut result)) = results[idx] {
                        match verification {
                            Some(text) => result
                                .content
                                .push_str(&format!("\n\n[Screen Visual]\n{}", text)),
                            // Sidecar couldn't describe it — still tell the model an
                            // image exists, so it never claims "no image was returned."
                            None => result.content.push_str(
                                "\n\n[Screen Visual] A screenshot was captured (saved and \
                                 available to the user), but automatic description was \
                                 unavailable. Acknowledge the capture — do NOT say the tool \
                                 returned no image.",
                            ),
                        }
                        // The main model is non-vision, so the raw image is useless to it;
                        // always drop it (otherwise the provider silently strips it and the
                        // model is left blind with no signal).
                        result.image_url = None;
                    }
                }
            }
        }
    }

    // For comm-origin runs, tell the model that screenshots will be
    // delivered as attachments — otherwise it has no way to know.
    if origin == tools::Origin::Comm && !had_image.is_empty() {
        for idx in &had_image {
            if let Some((_, ref mut result)) = results[*idx] {
                result.content.push_str(
                    "\n\n✓ Screenshot captured and will be delivered as an attachment in your reply to the user."
                );
            }
        }
    }

    // Duplicate file read detection: if the model re-reads a file it
    // already read this session, append a note so it knows. A full
    // os file read gets the read-ledger note below instead (count,
    // read time, disk evidence), so it is only tracked here, never
    // double-noted.
    for (idx, entry) in results.iter_mut().enumerate() {
        let Some(entry) = entry else { continue };
        let target = targets[idx].as_ref();
        if let Some(path) = target.and_then(read_path) {
            let repeat = !files_read_this_session.insert(path.clone());
            if repeat && !target.is_some_and(|t| is_full_file_read(t, &entry.0.input)) {
                entry.1.content.push_str(
                    "\n\n(Note: a full read of this path was returned earlier this session.)",
                );
            }
        }
    }

    // Each result was shaped by the registry (its tool's threshold, the one
    // spill path); across the message, the largest go to disk first once
    // their total passes the per-message budget.
    apply_message_budget(&mut results, &tools::result_shape::results_dir(session_id));
    *ctx_spilled_results += results
        .iter()
        .flatten()
        .filter(|(_, r)| r.content.starts_with("<persisted-output>"))
        .count();

    // Save all tool results to session in deterministic order
    // and track whether ALL results in this iteration were errors.
    let mut all_errors_this_iteration = true;
    // Per-call productivity, indexed alongside the hash push below, so the
    // identical-args guard can count only the repeats that made no progress.
    let mut unproductive_this_iteration: std::collections::HashMap<(u64, u64), bool> =
        std::collections::HashMap::new();
    let mut had_results = false;
    // Terminal tool error (auth/permission/connection) → end the turn after
    // this batch and surface to the user, instead of feeding it back for the
    // model to retry/improvise (the death-spiral fix; FRAMES.md Phase 1).
    let mut terminal_error: Option<(String, Option<types::OwnerNeed>)> = None;
    let mut same_error_stop: Option<(String, String)> = None;
    // Highest-signal rate-limit status seen this iteration (429/403) — feeds the
    // RateLimit reminder so the model backs off instead of hammer-retrying a host.
    let mut iteration_rate_limited: Option<u16> = None;
    // Lightweight snapshots for the background tool summary generator.
    let mut summary_tool_calls: Vec<ai::ToolCall> = Vec::new();
    let mut summary_tool_results: Vec<ToolResult> = Vec::new();
    for (idx, entry) in results.into_iter().enumerate() {
        let Some((tc, mut result)) = entry else { continue };
        let target = targets[idx].as_ref();
        had_results = true;
        if let Some(t) = target
            && matches!(t.key.as_str(), "write_plan" | "check_plan")
            && let Some(p) = tc.input.get("path").and_then(|v| v.as_str()) {
            *plan_touch = Some((iteration, p.to_string()));
        }
        // Done gate bookkeeping: a landed write/edit counts; a hook
        // verdict on this result, or a check the model ran itself,
        // clears the count (in that order, so an edit whose own hook
        // ran ends at zero).
        if !result.is_error && target.is_some_and(is_file_change) {
            *edits_since_check += 1;
        }
        if hook_noted.contains(&tc.id) || target.is_some_and(is_check_run) {
            *edits_since_check = 0;
        }
        if target.is_some_and(is_desktop_act) && !result.is_error {
            *last_desktop_act = Some(desktop_evidence(&result.content));
        }
        // Terminal error (auth/permission/connection) — narrow, set only by
        // ToolResult::terminal(). End the run after this batch instead of
        // letting the model retry/improvise. Critical for autonomous
        // workflows: there's no human to ask or to hit stop, so a dead
        // account must fail the run cleanly, not spiral. (FRAMES Phase 1.)
        if result.terminal && terminal_error.is_none() {
            terminal_error = Some((result.content.clone(), result.need.clone()));
        }
        if matches!(result.http_status, Some(429) | Some(403)) {
            iteration_rate_limited = result.http_status;
        }
        // Capture pre-truncation snapshots for the summarizer (only name + short content)
        summary_tool_calls.push(tc.clone());
        summary_tool_results.push(ToolResult { payload: None, need: None,
            content: crate::runner::truncate_str(&result.content, 300).to_string(),
            is_error: result.is_error,
            image_url: None,
            http_status: None,
            terminal: result.terminal,
        });
        if !result.is_error {
            all_errors_this_iteration = false;
            // A successful read clears the failure count for that target.
            if let Some(p) = target.and_then(read_path) {
                read_failures.remove(&p);
            }
        } else if let Some(p) = target.and_then(read_path) {
            // A failed read of a path bumps its counter — even when interleaved
            // with successful discovery calls (which is why the all-errors
            // counter alone misses this).
            *read_failures.entry(p).or_insert(0) += 1;
        }

        // Tool-agnostic redundant-result dedup: if this result's content is
        // identical to one returned earlier this session — by ANY tool or args
        // (e.g. the same file read via os(read), then cat, then jq) — tell the
        // model it already has this instead of letting it re-fetch in a loop.
        // Hash is taken pre-truncation so it reflects the full original content.
        let mut flagged_redundant = false;
        if !result.is_error && result.content.len() > 200 {
            let content_hash = simple_hash(result.content.as_bytes());
            if recent_result_content_hashes.contains(&content_hash) {
                result.content.push_str(
                    "\n\n(Note: this is identical to a result you already received earlier in this session. You already have this content — use it instead of fetching it again.)",
                );
                flagged_redundant = true;
            } else {
                recent_result_content_hashes.push(content_hash);
                if recent_result_content_hashes.len() > 20 {
                    recent_result_content_hashes.remove(0);
                }
            }
        }

        // Idempotent-mutation dedup: os(write) carries its ENTIRE effect
        // (path + content) in its arguments, so a successful re-write with
        // byte-identical args leaves the file exactly as it was — no
        // progress by construction. Mutating tools are rightly exempt from
        // no-progress rules in general (re-running a build after editing
        // its input is legitimate), but that exemption let the
        // promise → write-same-JSON → rebuild-same-deck spiral run for an
        // hour: every cycle "succeeded", every counter stayed at zero.
        // Flagging it as redundant feeds the same downstream guards as a
        // redundant read (spiral counter, no-progress ledger, 3-strike
        // identical-args block).
        if !flagged_redundant
            && !result.is_error
            && target.is_some_and(|t| t.key == "write_file")
        {
            let nh = simple_hash(tc.name.as_bytes());
            let ah = simple_hash(tc.input.to_string().as_bytes());
            if recent_tool_result_hashes
                .iter()
                .any(|&(n, a, _, unproductive)| n == nh && a == ah && !unproductive)
            {
                result.content.push_str(
                    "\n\n(Note: this wrote byte-identical content to the same path as an earlier write this turn — the file is unchanged. If you meant to expand or modify it, actually change the content before writing again.)",
                );
                flagged_redundant = true;
            }
        }

        // Spiral backstop counter: only UNPRODUCTIVE attempts count. A call
        // that errored or returned content the model already had is a
        // wander-loop step (glob-wander / browser re-read / shell-retry); a
        // call that succeeded with a NOVEL result made progress. Counting
        // successes cut legitimate bulk work off at 8 (e.g. creating N
        // distinct todos, writing N files) — the false-trip this guard's own
        // comment warned about. File-read errors are excluded here — the
        // per-path read_failures map already stops same-target retry spirals;
        // counting cross-path failures toward os:read false-tripped codebase
        // exploration after 8 misses.
        record_action_spiral(
            action_call_counts,
            &tc,
            target,
            result.is_error,
            flagged_redundant,
        );

        // Remember whether THIS call made progress, keyed the same way the
        // identical-args guard looks calls up. A call that succeeded with a
        // novel result is progress and must never count toward a block.
        //
        // For a READ-ONLY call, "novel" is checked against its own previous
        // result under the same arguments: a browse that answers the same
        // "no resources found" for the twentieth time is a loop even though
        // every response was a success. (The general content-dedup above
        // ignores results under 200 chars, which is exactly the size of
        // such answers.) A mutating call is judged only by errors —
        // re-running a build after editing its input legitimately repeats
        // the same args AND the same "Created: <path>" result.
        let call_key = (
            simple_hash(tc.name.as_bytes()),
            simple_hash(tc.input.to_string().as_bytes()),
        );
        // Same-error streak: the identical-args block never sees a model
        // that varies its arguments against the same wall (2026-09-02:
        // "restore needs `checkpoint`" 49 times). Three identical error
        // texts nudge; three more after the nudge stop the turn.
        if result.is_error && !result.terminal {
            match error_streak.record(&tc.name, &result.content) {
                Some(crate::guardrails::Verdict::Nudge) => {
                    result.content.push_str(&format!(
                        "\n\n'{}' has returned this same error {} times this turn \
                         (with the same or different arguments). Read the error, use a \
                         different tool or approach, or tell the user what is blocking you.",
                        tc.name,
                        error_streak.count(&tc.name, &result.content)
                    ));
                }
                Some(crate::guardrails::Verdict::Stop) => {
                    same_error_stop.get_or_insert_with(|| {
                        (tc.name.clone(), tools::plan::first_line(&result.content, 120))
                    });
                }
                None => {}
            }
        }
        let mut no_progress = result.is_error || flagged_redundant;
        // Read-only per the registry's own classifier — the same verdict
        // the concurrency scheduler trusts, so there is exactly one
        // definition of "this call has no side effects".
        if !no_progress && tools.concurrency_safe(&tc.name, &tc.input).await {
            let own_hash = simple_hash(result.content.as_bytes());
            if readonly_result_hash_by_call.get(&call_key) == Some(&own_hash) {
                no_progress = true;
            }
            readonly_result_hash_by_call.insert(call_key, own_hash);
        }
        unproductive_this_iteration.insert(call_key, no_progress);

        // Arg-identity dedup (complements the content check above, which only
        // fires on byte-identical output): the model repeated a call it already
        // made this turn — same tool, identical arguments. Results that drift
        // slightly (mtime ordering, timestamps) slip past the content hash, so
        // flag the repeated CALL itself. The 3+ hard guard still blocks loops;
        // this annotates the second call so it never gets that far.
        // Only for read-only calls: a build or test re-run after an edit
        // repeats its arguments on purpose, and the fresh result is the one
        // that matters. Telling the model to reuse the old one there was
        // telling it to distrust a correct result.
        if !flagged_redundant && tools.concurrency_safe(&tc.name, &tc.input).await {
            let nh = simple_hash(tc.name.as_bytes());
            let ah = simple_hash(tc.input.to_string().as_bytes());
            if recent_tool_result_hashes
                .iter()
                .any(|&(n, a, _, _)| n == nh && a == ah)
            {
                result.content.push_str(
                    "\n\n(Note: this is the same read-only call, same arguments, as one earlier this turn. The result above is the fresh one; if it matches what you already had, nothing changed.)",
                );
            }
        }

        // Read ledger: note repeat observations of a file. Ranged reads
        // (offset/limit) are partial views and are deliberately not
        // fingerprinted. A file read is never persisted (it pages itself),
        // so its content here is what the tool returned.
        let ledger_note = match (result.is_error, target) {
            (false, Some(t)) if is_full_file_read(t, &tc.input) => tc
                .input
                .get("path")
                .and_then(|v| v.as_str())
                .and_then(|p| read_ledger.observe_read(p, &result.content)),
            // A command, or a search over a folder, fingerprinted by what it names.
            (false, Some(t)) if t.key == "run_command" => match &t.field {
                Some(types::permissions::RuleField::CommandPrefix(c)) => read_ledger.observe_command(c),
                Some(types::permissions::RuleField::Folder(p)) => {
                    read_ledger.observe_command(&p.to_string_lossy())
                }
                _ => None,
            },
            _ => None,
        };

        if let Some(note) = ledger_note {
            result.content.push_str(&note);
        }
        // Voluntary skill save: the model updated its library on its
        // own — push the self-improvement review backstop out (the
        // review only fires when organic learning has stalled).
        if !result.is_error && target.is_some_and(|t| t.key == "save_skill") {
            crate::review_fork::note_voluntary_save(session_id);
        }

        // Cache tool documentation results so they survive sliding window eviction.
        // Detect help/schema actions on skill and plugin tools.
        if !result.is_error && result.content.len() > 100
            && let Some(cache_key) = detect_tool_doc_call(&tc.name, &tc.input) {
            let content = if result.content.len() > MAX_TOOL_DOC_CONTENT {
                truncate_str(&result.content, MAX_TOOL_DOC_CONTENT).to_string()
            } else {
                result.content.clone()
            };
            // Remove existing entry with same key (LRU refresh)
            tool_doc_cache.retain(|(k, _)| k != &cache_key);
            // Evict oldest if at capacity
            if tool_doc_cache.len() >= MAX_TOOL_DOC_ENTRIES {
                tool_doc_cache.remove(0);
            }
            tool_doc_cache.push((cache_key.clone(), content));
            debug!(key = %cache_key, "cached tool documentation");
        }

        let row = ToolResultRow {
            tool_call_id: tc.id.clone(),
            outcome: Some(tools.labels(&tc.name, &tc.input).await.1),
            duration_ms: durations.get(&tc.id).copied(),
            content: result.content,
            is_error: result.is_error,
            image_url: result.image_url,
            payload: result.payload,
        };
        let tr_json = serde_json::json!([row]).to_string();

        if let Err(e) =
            sessions.append_message(session_id, "tool", "", None, Some(&tr_json), None)
        {
            warn!(session_id = %session_id, error = %e, "failed to save tool message to DB");
        }
    }

    // Terminal tool error → end the run now (FRAMES Phase 1). The failure is
    // unrecoverable (auth/permission/connection) — surface it and stop rather
    // than feed it back for the model to retry/improvise. This is the
    // death-spiral fix: in an autonomous workflow there is no human to ask or
    // to interrupt, so a dead account must stop the run cleanly. (Narrow: only
    // ToolResult::terminal sets this — healthy long-running tasks never trip it.)
    //
    // Typed termination: emitted as a ControlNotice status event, never Text —
    // the old emit-text-then-break made the notice indistinguishable from
    // assistant prose and it leaked verbatim into channel replies.
    if let Some((tool, first_line)) = same_error_stop.take() {
        warn!(session_id, iteration, tool = %tool, "same-error loop — ending run");
        let _ = tx
            .send(StreamEvent::control_notice(
                format!(
                    "Stopped: '{}' kept returning the same error ({}) after being told \
                     to change approach. Ending the run so it cannot continue indefinitely.",
                    tool, first_line
                ),
                "same_error_loop",
            ))
            .await;
        return RoundOutcome::Ended(crate::guardrails::Exit::SameErrorLoop);
    }
    if let Some((msg, need)) = terminal_error {
        warn!(session_id, iteration, "terminal tool error — ending run");
        let _ = tx
            .send(StreamEvent::control_notice(msg, "terminal_tool_error").with_owner_need(need))
            .await;
        return RoundOutcome::Ended(crate::guardrails::Exit::TerminalToolError);
    }

    RoundOutcome::Ran(RoundResults {
        all_errors_this_iteration,
        had_results,
        unproductive_this_iteration,
        iteration_rate_limited,
        summary_tool_calls,
        summary_tool_results,
    })
}

/// The command a call would run in the shell, for the per-command
/// allowlist: its rule field when its rule key is `run_command`.
fn shell_command_of(target: &types::permissions::Target) -> Option<String> {
    match (&target.field, target.key.as_str()) {
        (Some(types::permissions::RuleField::CommandPrefix(c)), "run_command") => Some(c.clone()),
        _ => None,
    }
}

/// "Approve Always" on the ApprovalModal → grant the capability category for
/// next time (PERMISSIONS_SME §14). Flips the global `user_profiles.tool_permissions`
/// entry ON, the same store the Settings → Permissions toggles write.
fn persist_capability_grant(store: &Store, category: &str) -> Result<(), String> {
    let raw = store
        .get_user_profile()
        .map_err(|e| e.to_string())?
        .and_then(|p| p.tool_permissions)
        .unwrap_or_else(|| "{}".to_string());
    let mut map: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&raw).unwrap_or_default();
    map.insert(category.to_string(), serde_json::Value::Bool(true));
    let json = serde_json::to_string(&map).map_err(|e| e.to_string())?;
    store
        .update_tool_permissions(&json)
        .map_err(|e| e.to_string())
}

/// "Approve Always" on an MCP tool call → pin an explicit Always-allow override
/// in the server's tool-permission map — the SAME store Settings → MCP → Tool
/// permissions edits (tools::policy::McpServerPermissions on the integration row).
fn persist_mcp_tool_allow(store: &Store, integration_id: &str, tool: &str) -> Result<(), String> {
    let mut perms = tools::policy::McpServerPermissions::from_json(
        store
            .get_mcp_tool_permissions(integration_id)
            .map_err(|e| e.to_string())?
            .as_deref(),
    );
    perms
        .tools
        .insert(tool.to_string(), tools::policy::McpToolAccess::Allow);
    store
        .set_mcp_tool_permissions(integration_id, &perms.to_json())
        .map_err(|e| e.to_string())
}

/// One tool-approval round-trip (PERMISSIONS_SME §11): register a oneshot keyed
/// by the tool_call id, emit `approval_request`, and await the ApprovalModal
/// decision ("deny" / "once" / "always"). Run cancellation resolves to "deny".
/// The ONE ask pathway shared by the capability gate and the MCP tri-state gate.
async fn ask_tool_approval(
    channels: &tools::ApprovalChannels,
    tx: &mpsc::Sender<StreamEvent>,
    cancel_token: &CancellationToken,
    tool_call: &ai::ToolCall,
    session_id: &str,
    gate: &str,
) -> String {
    ask_tool_approval_batch(channels, tx, cancel_token, std::slice::from_ref(tool_call), session_id, gate).await
}

/// The batch form: one card, one decision, for every gated call in a batch
/// (a person answering five cards in a row for one parallel step was the
/// hazard). `calls` is never empty.
async fn ask_tool_approval_batch(
    channels: &tools::ApprovalChannels,
    tx: &mpsc::Sender<StreamEvent>,
    cancel_token: &CancellationToken,
    calls: &[ai::ToolCall],
    session_id: &str,
    gate: &str,
) -> String {
    let tool_call = &calls[0];
    let request_id = tool_call.id.clone();
    let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
    channels.lock().await.insert(request_id.clone(), resp_tx);
    let _ = tx
        .send(if calls.len() > 1 {
            StreamEvent::approval_request_batch(calls)
        } else {
            StreamEvent::approval_request(tool_call.clone())
        })
        .await;
    info!(
        session_id,
        request_id = %request_id,
        gate,
        tool = %tool_call.name,
        "tool approval: waiting for user decision"
    );
    tokio::select! {
        _ = cancel_token.cancelled() => {
            channels.lock().await.remove(&request_id);
            "deny".to_string()
        }
        result = resp_rx => result.unwrap_or_else(|_| "deny".to_string()),
    }
}

/// Hold one message's results to [`tools::result_shape::MESSAGE_RESULT_BUDGET`]
/// characters: while their total is over it, the largest result not yet
/// persisted (and carrying no image) is saved under `dir` and previewed.
fn apply_message_budget(results: &mut [Option<(ai::ToolCall, ToolResult)>], dir: &std::path::Path) {
    let persisted = |r: &ToolResult| r.content.starts_with("<persisted-output>");
    loop {
        let total: usize = results.iter().flatten().map(|(_, r)| r.content.chars().count()).sum();
        if total <= tools::result_shape::MESSAGE_RESULT_BUDGET {
            return;
        }
        let largest = results
            .iter_mut()
            .flatten()
            .filter(|(_, r)| !r.is_error && r.image_url.is_none() && !persisted(r))
            .max_by_key(|(_, r)| r.content.len());
        let Some((_, r)) = largest else { return };
        r.content = tools::result_shape::persist(dir, &r.content);
    }
}

/// The spiral counter's key for a call: its tool and action, or, for a
/// call that names no action, the job its rule key names when that differs
/// from the tool. A plugin is keyed on the PLUGIN, not the verb: a run that
/// fails "payment create", then "batch execute", then "journalentry create"
/// against the same plugin is one spiral, not three fresh starts (CFO,
/// 2026-09-06: 20 failed QuickBooks calls in one turn, no guard fired because
/// each verb stayed under the limit).
fn action_key(call: &ai::ToolCall, target: Option<&types::permissions::Target>) -> String {
    let action = call.input.get("action").and_then(|v| v.as_str()).unwrap_or("");
    if !action.is_empty() {
        return format!("{}:{}", call.name, action);
    }
    if let Some(t) = target.filter(|t| t.key != call.name) {
        return t.key.clone();
    }
    let verb = call
        .input
        .get("command")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ");
    format!("{}:{}", call.name, verb)
}

/// Whether an unproductive tool attempt should feed the coarse (tool, action)
/// spiral counter (the same-action limit from `guardrails::GuardrailConfig`).
///
/// File-read errors are excluded: per-path `read_failures` already caps retries
/// on the same target. Counting every failed read across different paths toward
/// the action-wide limit false-trips legitimate exploration. Redundant content
/// still counts — re-fetching bytes the model already has is the wander the
/// spiral is meant to catch for reads.
fn counts_toward_action_spiral(
    target: Option<&types::permissions::Target>,
    is_error: bool,
    flagged_redundant: bool,
) -> bool {
    flagged_redundant || (is_error && target.and_then(read_path).is_none())
}

/// Apply one spiral-counter update for a tool result.
fn record_action_spiral(
    counts: &mut std::collections::HashMap<String, usize>,
    call: &ai::ToolCall,
    target: Option<&types::permissions::Target>,
    is_error: bool,
    flagged_redundant: bool,
) {
    if counts_toward_action_spiral(target, is_error, flagged_redundant) {
        *counts.entry(action_key(call, target)).or_insert(0) += 1;
    }
}

/// The file a call reads: a `read_file` call's path, or the file a shell
/// command only dumps (cat/head/tail/jq…). `None` for anything else.
fn read_path(target: &types::permissions::Target) -> Option<String> {
    use types::permissions::RuleField;
    match (target.key.as_str(), &target.field) {
        ("read_file", Some(RuleField::Folder(p))) => Some(p.to_string_lossy().into_owned()),
        ("run_command", Some(RuleField::CommandPrefix(c))) => extract_shell_read_path(c),
        _ => None,
    }
}

/// An unranged `read_file` call: the shape the read ledger fingerprints and
/// notes itself, so the duplicate-read note must not stack on it.
fn is_full_file_read(target: &types::permissions::Target, input: &serde_json::Value) -> bool {
    target.key == "read_file" && input.get("offset").is_none() && input.get("limit").is_none()
}

/// A call that writes or edits a file (the done gate counts them).
fn is_file_change(target: &types::permissions::Target) -> bool {
    matches!(target.key.as_str(), "write_file" | "edit_file")
}

/// A shell command that is a project check (`is_check_command`).
fn is_check_run(target: &types::permissions::Target) -> bool {
    target.key == "run_command"
        && matches!(&target.field, Some(types::permissions::RuleField::CommandPrefix(c)) if crate::runner::is_check_command(c))
}

/// A desktop action whose result is the screen after it.
fn is_desktop_act(target: &types::permissions::Target) -> bool {
    matches!(
        target.key.as_str(),
        "desktop_click" | "desktop_type" | "desktop_key" | "desktop_scroll" | "desktop_drag"
    )
}

/// Detect a shell command whose sole purpose is dumping a file's contents and
/// return the target path, so the duplicate-read note can fire for shell reads.
/// Only matches read-only file-dump commands — not commands with side effects.
fn extract_shell_read_path(command: &str) -> Option<String> {
    let trimmed = command.trim();
    // Bail on anything that pipes, redirects, or chains — too ambiguous to
    // attribute to a single file read.
    if trimmed.contains('|') || trimmed.contains('>') || trimmed.contains("&&") {
        return None;
    }
    let tokens: Vec<&str> = trimmed.split_whitespace().collect();
    let cmd = *tokens.first()?;
    let base = cmd.rsplit('/').next().unwrap_or(cmd);
    let is_dump = matches!(
        base,
        "cat" | "head" | "tail" | "less" | "more" | "bat" | "nl" | "jq"
    );
    if !is_dump {
        return None;
    }
    // Take the last token that is not a flag or a jq filter expression.
    let path = tokens
        .iter()
        .skip(1)
        .rev()
        .find(|t| !t.starts_with('-') && **t != "." && !t.starts_with('\''))?;
    let cleaned = path.trim_matches(|c| c == '"' || c == '\'');
    if cleaned.is_empty() {
        return None;
    }
    Some(cleaned.to_string())
}

/// JSON shape for tool results stored in the DB. Includes optional image_url
/// so vision-capable providers can receive screenshots in tool result content.
#[derive(serde::Serialize)]
pub(crate) struct ToolResultRow {
    pub tool_call_id: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_error: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
    /// Structured rendering payload (ToolResult::payload) so reloaded history
    /// renders the same rich cards as the live stream.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
    /// The past-tense outcome the live stream showed ("Ran shell"), persisted
    /// so a reloaded thread reads the same as the live one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    /// Wall-clock milliseconds the call took, for the same reason.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

/// Runs a tool call under its budget, with the clock stopped while the call
/// is parked on the owner. `None` means the budget of working time ran out.
async fn run_within_budget<F>(
    budget: Duration,
    parked: std::sync::Arc<std::sync::atomic::AtomicBool>,
    fut: F,
) -> Option<ToolResult>
where
    F: std::future::Future<Output = ToolResult>,
{
    tokio::pin!(fut);
    let mut spent = Duration::ZERO;
    loop {
        let slice = PARKED_POLL.min(budget.saturating_sub(spent));
        match tokio::time::timeout(slice, &mut fut).await {
            Ok(result) => return Some(result),
            Err(_) => {
                if !parked.load(std::sync::atomic::Ordering::SeqCst) {
                    spent += slice;
                }
                if spent >= budget {
                    return None;
                }
            }
        }
    }
}

/// What the model reads when a tool's working time runs out. Names the
/// budget as working time, because parked time never counts toward it.
fn tool_timeout_text(tool: &str, budget: Duration) -> String {
    format!(
        "Tool '{}' did not finish within {}s of working time and was stopped. That is the tool's limit, not a verdict on the service behind it; if the same call is needed, say what it was waiting on.",
        tool,
        budget.as_secs()
    )
}

/// Post-tool hooks, applied to a result BEFORE it is streamed or persisted, so
/// the owner's transcript and the trace carry exactly what the model was
/// given (a formatter's note, a test runner's verdict). Plugins listen as
/// actions (fire-and-forget); shell hooks as filters whose response is the
/// result. Live 2026-09-02: the hook note reached the model but not the trace,
/// because the event went out first. Returns whether a hook attached
/// anything to the result (a note or a verdict), which the done gate takes
/// as "a check ran".
async fn apply_post_tool_hooks(
    hooks: &napp::HookDispatcher,
    tc: &ai::ToolCall,
    result: &mut ToolResult,
    session_id: &str,
    run_cwd: Option<&str>,
) -> bool {
    if !hooks.has_subscribers("tool.post_execute") {
        return false;
    }
    let payload = serde_json::to_vec(&crate::hooks::ToolPostExecutePayload {
        tool_name: tc.name.clone(),
        result: result.content.clone(),
        is_error: result.is_error,
        session_id: session_id.to_string(),
        tool_use_id: tc.id.clone(),
        tool_input: tc.input.clone(),
        cwd: run_cwd.map(str::to_string).unwrap_or_default(),
        agent_id: session_id.strip_prefix("subagent:").map(|_| session_id.to_string()),
    })
    .unwrap_or_default();
    hooks.do_action("tool.post_execute", payload.clone()).await;
    let (bytes, _) = hooks.apply_filter("tool.post_execute", payload).await;
    let mut attached = false;
    if let Ok(resp) = serde_json::from_slice::<crate::hooks::ToolPostExecuteResponse>(&bytes) {
        attached = resp.result != result.content;
        result.content = resp.result;
        result.is_error = resp.is_error;
    }
    attached
}

/// The seat that decides what this one may not: the line the owner drew
/// (`reports_to`), and failing that a seat the company has actually given the
/// authority to grant (`authority.grant.grant` — by a rule the owner or the
/// General Manager wrote, or by its own package declaring it).
///
/// Never a name compiled in here. A company may put a Chief Operating Officer
/// over its seats, a General Manager, a person, or nobody; who holds authority
/// is the owner's to say, and it is already in the data.
fn authority_seat(store: &Arc<Store>, asking_agent_id: &str) -> Option<db::models::Agent> {
    if asking_agent_id.is_empty() {
        return None;
    }
    // 1. The reporting line, as the owner set it.
    if let Ok(Some(me)) = store.get_agent(asking_agent_id)
        && let Some(above) = me
            .reports_to
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty() && *s != asking_agent_id)
            && let Ok(Some(seat)) = store.get_agent(above) {
        return Some(seat);
    }
    // 2. A seat that holds the authority to grant authority.
    let mut holders: Vec<db::models::Agent> = store
        .list_agents(10_000, 0)
        .unwrap_or_default()
        .into_iter()
        .filter(|a| {
            a.id != asking_agent_id
                && a.is_app.unwrap_or(0) == 0
                && a.is_enabled != 0
                && holds_granting_authority(store, a)
        })
        .collect();
    holders.sort_by(|a, b| a.name.cmp(&b.name));
    holders.into_iter().next()
}

/// The seat whose day counters a standing grant spends. A sub-agent carries
/// no agent id of its own and runs under its parent's operation policy, so it
/// spends its parent seat's counters — never a fresh allowance of its own.
fn grant_counter_seat(agent_id: &str, session_key: &str) -> String {
    if agent_id.is_empty() && session_key.starts_with("subagent:") {
        keyparser::extract_agent_id(session_key)
    } else {
        agent_id.to_string()
    }
}

/// Whether this seat may grant standing authority: the owner's own rule on
/// `authority.grant.grant`, or its package declaring that operation.
fn holds_granting_authority(store: &Arc<Store>, seat: &db::models::Agent) -> bool {
    const GRANT: &str = "authority.grant.grant";
    let stored = store
        .get_entity_config("agent", &seat.id)
        .ok()
        .flatten()
        .and_then(|c| c.operation_policy);
    if let Some(rule) = tools::policy::OperationPolicy::from_json(stored.as_deref())
        .operations
        .get(GRANT)
        && rule.access != tools::policy::OperationAccess::Blocked {
        return true;
    }
    !seat.frontmatter.is_empty()
        && napp::agent::parse_agent_config(&seat.frontmatter)
            .map(|c| {
                c.ceiling
                    .keys()
                    .any(|op| tools::plugin_tool::port_suffix(op).starts_with("authority."))
                    || c.requires.interfaces.iter().any(|i| i == "authority")
            })
            .unwrap_or(false)
}

/// Out-of-bounds work is somebody's job, not an impossibility.
///
/// When a gated operation decides Approval in a run with nobody to ask, the
/// work is handed to the seat that holds authority as that seat's OWN
/// assignment — the one hand-over pathway (`open_assignment`, through the
/// opener the server installs at boot, the same one the `agent` tool uses) —
/// carrying the operation, the seat that wanted it, the bound it fell outside,
/// and what is now stopped. Returns the sentence the stopped run tells the
/// model, or `None` when nobody here holds that authority, in which case the
/// caller's own fallback stands: park for the owner, or refuse.
fn hand_off_out_of_bounds(
    store: &Arc<Store>,
    agent_id: &str,
    session_key: &str,
    operation: &str,
    display: &str,
    reason: &str,
) -> Option<String> {
    let opener = tools::assignments::assignment_opener()?;
    let seat = authority_seat(store, agent_id)?;
    let my_name = store
        .get_agent(agent_id)
        .ok()
        .flatten()
        .map(|a| a.name)
        .unwrap_or_else(|| agent_id.to_string());
    let req = tools::assignments::AssignmentRequest {
        assigner_agent_id: agent_id.to_string(),
        assigner_name: my_name.clone(),
        assigner_session_key: session_key.to_string(),
        parent_run_id: None,
        assignee_agent_id: seat.id.clone(),
        subject: format!("{my_name} is stopped on {operation}: {display}"),
        done_means: format!(
            "Decide {operation} for {my_name}. It fell outside what {my_name} may do unattended: \
             {reason}. The work that is stopped: {display}. Either grant {my_name} standing \
             authority for {operation} within bounds you can stand behind, do it yourself if it is \
             yours to do, or close this saying it will not happen and why."
        ),
        due: None,
    };
    match opener.open(&req) {
        Ok(id) => {
            tracing::info!(
                agent = %agent_id, op = %operation, assignee = %seat.id, assignment = %id,
                "out of bounds: handed to the seat that holds authority"
            );
            Some(format!(
                "'{operation}' is outside what you may do unattended ({reason}), so it was handed to \
                 {} as an assignment: the operation, the bound it fell outside, and the work that is \
                 stopped. Do not retry it and do not work around it — say plainly that it is now {}'s \
                 to decide, and carry on with anything else you can finish.",
                seat.name, seat.name
            ))
        }
        Err(e) => {
            warn!(agent = %agent_id, op = %operation, error = %e, "handing out-of-bounds work on failed");
            None
        }
    }
}

/// Who can answer an approval card for a batch: the channels the answer
/// comes back on, the stream the card is shown in, and the run's cancel token.
pub struct ApprovalDoor<'a> {
    pub channels: &'a tools::ApprovalChannels,
    pub tx: &'a mpsc::Sender<StreamEvent>,
    pub cancel_token: &'a CancellationToken,
}

/// The run a batch of tool calls belongs to, as the permission gate reads it.
/// The runner fills it for every batch; `/agent/mcp` fills it for a single
/// call from an MCP client (no door, `Origin::Mcp`).
pub struct GateRun<'a> {
    pub tools: &'a Registry,
    pub store: &'a Arc<Store>,
    pub agent_id: &'a str,
    pub session_id: &'a str,
    pub session_key: &'a str,
    pub origin: Origin,
    pub full_access: bool,
    pub entity_permissions: Option<&'a HashMap<String, bool>>,
    pub operation_policy: Option<&'a tools::policy::OperationPolicy>,
    /// None: nobody can be asked, so what would ask is refused.
    pub approval: Option<ApprovalDoor<'a>>,
    pub approval_relay: bool,
    pub workflow_mode: Option<&'a WorkflowMode>,
    /// For a workflow park's conversation snapshot.
    pub sessions: Option<&'a SessionManager>,
}

/// What the gate decided for a batch, beyond the refusals it wrote.
pub struct GateOutcome {
    /// Capabilities cleared for this batch — set on the `ToolContext` so the
    /// registry's capability check lets them through.
    pub approved_categories: HashSet<String>,
    /// Calls the owner answered on a card.
    pub owner_answered: HashSet<usize>,
    /// A workflow parked an Approval-gated call: the loop's break reason.
    pub parked: Option<String>,
}

/// The permission gate every tool call passes before the registry runs it:
/// MCP server tool permissions, the per-employee operation policy, and the
/// capability toggles — asking the owner where someone can be asked, and
/// refusing (or handing off) where nobody can. Refusals are written into
/// `blocked_results`; the registry's own checks (safeguard, path scope,
/// origin deny list, capability, resource grants) still run after this.
pub async fn gate_tool_calls(
    run: &GateRun<'_>,
    tool_calls: &[ai::ToolCall],
    blocked_results: &mut [Option<(ai::ToolCall, ToolResult)>],
) -> GateOutcome {
    let GateRun {
        tools,
        store,
        agent_id,
        session_id,
        session_key,
        origin,
        full_access,
        entity_permissions,
        operation_policy,
        approval: _,
        approval_relay,
        workflow_mode,
        sessions,
    } = *run;
    let mut wf_break_reason: Option<String> = None;
    // ── Per-tool approval gate (PERMISSIONS_SME §11) ──────────────────
    // A capability that's OFF means ASK the user, not hard-fail. We wire
    // the previously-dangling producer: emit `approval_request` and await
    // the ApprovalModal decision via the shared `approval_channels`
    // round-trip (the SAME pathway plan-mode uses). Autonomous mode and
    // pre-granted (ON) categories proceed without asking; Deny returns a
    // clean declined result; "Always" flips the capability ON for next
    // time. Categories cleared here are recorded on the ToolContext so the
    // registry permission gate (Phase 1c) treats them as allowed.
    let mut approved_cats: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    // Per-command allowlist: prefixes the user chose "Approve Always" for.
    // Loaded once; appended on an "always" decision for a shell command.
    let mut approved_cmds: Vec<String> = store.get_approved_commands().unwrap_or_default();
    // Gated calls in one batch get ONE approval card (Stage 7): the first
    // pass collects them, one ask covers the lot, the second pass applies
    // the decision through the same grant code as a single ask.
    #[derive(Clone, Copy, PartialEq)]
    enum GatePass { Collect, Apply }
    let mut to_ask: Vec<usize> = Vec::new();
    let mut batch_decision: Option<String> = None;
    // Calls the owner answered on a card in this batch: the decide
    // guardrail below never asks about them a second time.
    let mut owner_answered: std::collections::HashSet<usize> =
        std::collections::HashSet::new();
    for gate_pass in [GatePass::Collect, GatePass::Apply] {
    if gate_pass == GatePass::Apply {
        if to_ask.is_empty() { break; }
        let calls: Vec<ai::ToolCall> = to_ask.iter().map(|&i| tool_calls[i].clone()).collect();
        let door = run.approval.as_ref().expect("collect pass only records calls with a door");
        batch_decision = Some(ask_tool_approval_batch(door.channels, door.tx, door.cancel_token, &calls, session_id, "capability").await);
        owner_answered.extend(to_ask.iter().copied());
    }
    for idx in 0..tool_calls.len() {
        if blocked_results[idx].is_some() {
            continue;
        }
        // ── MCP tri-state gate (Settings → MCP → Tool permissions) ────
        // Per-server default + per-tool override, decided by
        // tools::policy::McpServerPermissions: Allow auto-approves,
        // Ask runs the SAME ApprovalGate round-trip as capabilities,
        // Deny refuses with an error naming the setting. This gate is
        // the ONE enforcement site for MCP tool permissions; MCP
        // proxies carry no ambient capability, so the loop `continue`s
        // here and never reaches the capability gate below.
        if tool_calls[idx].name.starts_with("mcp__")
            && let Some((integration_id, original)) =
            tools.mcp_proxy_info(&tool_calls[idx].name).await
        {
            // Company Memory (the platform-authenticated server,
            // auth_type "neboai") is governed on the KB page —
            // the owner grants or revokes each Nebo there and the
            // shard enforces it with a 401. Asking again here
            // would leave every unattended run (a shopper on a
            // code, a workflow) with no one to answer. One gate.
            let platform_memory = store
                .get_mcp_integration(&integration_id)
                .ok()
                .flatten()
                .map(|i| i.auth_type == "neboai")
                .unwrap_or(false);
            if platform_memory {
                continue;
            }
            let perms = tools::policy::McpServerPermissions::from_json(
                store
                    .get_mcp_tool_permissions(&integration_id)
                    .unwrap_or_default()
                    .as_deref(),
            );
            // mcp__<server>__<tool> — the server slug, for messages.
            let server = tool_calls[idx]
                .name
                .split("__")
                .nth(1)
                .unwrap_or("server")
                .to_string();
            match perms.decide(&original) {
                tools::policy::McpToolAccess::Allow => {}
                tools::policy::McpToolAccess::Deny => {
                    blocked_results[idx] = Some((
                        tool_calls[idx].clone(),
                        ToolResult::error(format!(
                            "Blocked: the MCP tool '{original}' on server \
                             '{server}' is set to Blocked in Settings → MCP → \
                             Tool permissions. Tell the user this tool is \
                             blocked by that setting and stop — do not retry \
                             or work around it."
                        )),
                    ));
                }
                tools::policy::McpToolAccess::Ask if full_access => {
                    // Full Access bypasses the ask, same as the
                    // capability gate. Blocked above still blocks.
                }
                tools::policy::McpToolAccess::Ask => {
                    match &run.approval {
                        Some(door)
                            if tools::ExecutionMode::from(origin)
                                == tools::ExecutionMode::Interactive
                                || approval_relay =>
                        {
                            let decision = ask_tool_approval(
                                door.channels,
                                door.tx,
                                door.cancel_token,
                                &tool_calls[idx],
                                session_id,
                                "mcp",
                            )
                            .await;
                            owner_answered.insert(idx);
                            match decision.as_str() {
                                "always" => {
                                    if let Err(e) = persist_mcp_tool_allow(
                                        store,
                                        &integration_id,
                                        &original,
                                    ) {
                                        warn!(session_id, tool = %original, error = %e, "failed to persist MCP tool grant");
                                    }
                                }
                                "once" | "approve" | "approved" | "yes" | "true" => {}
                                _ => {
                                    blocked_results[idx] = Some((
                                        tool_calls[idx].clone(),
                                        ToolResult::error(format!(
                                            "The user declined to allow the MCP \
                                             tool '{original}' on server \
                                             '{server}'. Tell the user it needs \
                                             their approval and stop — do not \
                                             retry or work around it."
                                        )),
                                    ));
                                }
                            }
                        }
                        // Unattended (cron/workflow/comm/subagent) or no
                        // channel: nobody can answer — refuse instead of
                        // hanging on a prompt nobody sees. Unlike the
                        // capability gate there is no registry backstop
                        // for MCP tools, so the refusal happens here.
                        _ => {
                            blocked_results[idx] = Some((
                                tool_calls[idx].clone(),
                                ToolResult::error(format!(
                                    "The MCP tool '{original}' on server \
                                     '{server}' needs the user's approval \
                                     (Settings → MCP → Tool permissions) and no \
                                     one is available to approve it in this \
                                     run. Report this and stop."
                                )),
                            ));
                        }
                    }
                }
            }
            continue;
        }
        // ── Per-operation approval gate (per-employee three-state policy) ──
        // A gated interface operation is decided by the employee's
        // OperationPolicy: Always runs, Approval asks the owner
        // (interactive) / refuses when unattended, Blocked is refused (the
        // toolset also omits it — this is the hard backstop). Origin-aware
        // (WS2): an untrusted origin floors gated Always to Approval, and
        // with NO policy set a trusted origin keeps "installation is the
        // grant" (except a critical operation, which always asks) while an
        // untrusted one falls back to the safe default — the decision lives
        // in decide/decide_optional, shared with the workflow checkpoint
        // (Rule 8.1).
        //
        // WHICH operation a call performs is the TOOL's to declare
        // (`DynTool::operation_performed`), never this gate's to infer from
        // a tool name: the `plugin` tool answers with its typed
        // `operation`, the `pack` tool with the layer write or removal it
        // performs, and any tool that grows a gated operation is decided
        // here without touching this code. A call that performs no typed
        // operation (plugin list/discover/exec-by-slug, pack list/show)
        // falls through ungated.
        if let Some(op) = tools
            .operation_performed(&tool_calls[idx].name, &tool_calls[idx].input)
            .await
        {
            // The operation's parameters, as far as the call states
            // them: a standing grant is checked against amount,
            // counterparty, and today's counters (R16). A call that
            // states no amount is checked against count and freshness
            // only.
            let params = tools::policy::OperationParams {
                amount_cents: tool_calls[idx]
                    .input
                    .get("amount_cents")
                    .and_then(|v| v.as_i64()),
                counterparty: tool_calls[idx]
                    .input
                    .get("counterparty")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                counterparty_has_source_id: tool_calls[idx]
                    .input
                    .get("counterparty_id")
                    .and_then(|v| v.as_str())
                    .is_some_and(|s| !s.is_empty()),
                irreversible: tools::interface_catalog::is_critical(&op),
            };
            let company_policy = store
                .get_company_policy()
                .ok()
                .flatten()
                .map(|j| tools::policy::CompanyPolicy::from_json(Some(&j)));
            let today = chrono::Local::now().format("%Y-%m-%d").to_string();
            let rule_key = format!(
                "{}:{}",
                grant_counter_seat(agent_id, session_key),
                tools::plugin_tool::port_suffix(&op)
            );
            let counters = {
                let cp = params.counterparty.clone().unwrap_or_default();
                let mine = store.day_counters(&rule_key, &today, &cp).ok();
                let company = store.day_counters(db::COMPANY_COUNTER_KEY, &today, "").ok();
                mine.map(|m| tools::policy::DayCounters {
                    count: m.count,
                    cents: m.cents,
                    counterparty_cents: m.counterparty_cents,
                    company_count: company.as_ref().map(|c| c.count).unwrap_or(0),
                    company_cents: company.as_ref().map(|c| c.cents).unwrap_or(0),
                })
            };
            let decision = tools::policy::OperationPolicy::decide_optional(
                operation_policy,
                &op,
                // Tainted workflow inputs decide as Comm: a gated
                // Always floors to Approval (WS2-R7), the same
                // rule the engine checkpoint applied.
                if workflow_mode.is_some_and(|m| m.tainted) {
                    tools::Origin::Comm
                } else {
                    origin
                },
                &params,
                company_policy.as_ref(),
                counters.as_ref(),
                // The projection is proven current once the cache
                // exists (Playbook PRD 6.4); until then local policy
                // is the only copy and is current by definition.
                true,
            );
            if let Some(decision) = decision {
                match decision.access {
                    tools::policy::OperationAccess::Always => {
                        // A standing grant spent: count it against the
                        // day before the call runs, so a crash between
                        // decision and execution can never under-count.
                        if decision.layer == tools::policy::PolicyLayer::StandingAuthority {
                            let cp = params.counterparty.clone().unwrap_or_default();
                            let cents = params.amount_cents.unwrap_or(0);
                            let _ = store.bump_counters(&rule_key, &today, &cp, cents);
                            let _ = store.bump_counters(db::COMPANY_COUNTER_KEY, &today, "", cents);
                            tracing::info!(
                                agent = %agent_id, op = %op, rule = %rule_key, reason = %decision.reason,
                                "operation approved by standing authority"
                            );
                        }
                    }
                    tools::policy::OperationAccess::Blocked => {
                        blocked_results[idx] = Some((
                            tool_calls[idx].clone(),
                            ToolResult::error(format!(
                                "The operation '{op}' is Blocked for this AI employee \
                                 ({layer}: {reason}). Tell the user it's blocked and \
                                 stop — do not retry or work around it.",
                                layer = decision.layer.as_str(),
                                reason = decision.reason,
                            )),
                        ));
                    }
                    tools::policy::OperationAccess::Approval => {
                        // NOTE: deliberately NO full_access bypass here. The
                        // per-employee operation policy is an explicit setting;
                        // the whole point is that a global convenience (Full
                        // Access) never overrides a per-employee gate on money/
                        // outbound/irreversible operations. decide() rules.
                        //
                        // The approval prompt must be comprehensible to a
                        // non-technical owner: require the `display` sentence
                        // (real names + formatted amounts, not ids/cents).
                        // Missing → corrective retry, never a raw-JSON prompt.
                        let display = tool_calls[idx]
                            .input
                            .get("display")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        if display.is_empty() {
                            blocked_results[idx] = Some((
                                tool_calls[idx].clone(),
                                ToolResult::error(format!(
                                    "The operation '{op}' needs the owner's approval, and \
                                     the approval prompt requires a `display` sentence. \
                                     Retry the SAME call adding display: one plain-language \
                                     sentence a non-technical person understands — real \
                                     names and formatted amounts (e.g. \"Pay Acme Supplies \
                                     $2,500.00 for bill #1042\"), never raw ids or cents."
                                )),
                            ));
                        } else if tools::ExecutionMode::from(origin)
                            == tools::ExecutionMode::Interactive
                            || approval_relay
                        {
                            match &run.approval {
                                Some(door) => {
                                    let decision = ask_tool_approval(
                                        door.channels,
                                        door.tx,
                                        door.cancel_token,
                                        &tool_calls[idx],
                                        session_id,
                                        "operation",
                                    )
                                    .await;
                                    owner_answered.insert(idx);
                                    match decision.as_str() {
                                        "always" => {
                                            // Approve Always → persist this op as
                                            // Always in the employee's policy so the
                                            // button does what it says.
                                            if !agent_id.is_empty() {
                                                let mut policy = operation_policy
                                                    .cloned()
                                                    .unwrap_or_default();
                                                // A locked entry (the seat's
                                                // ceiling or a law) refuses the
                                                // edit: the button cannot loosen it.
                                                if let Err(e) = policy.apply_edit(
                                                    &tools::plugin_tool::port_suffix(&op),
                                                    tools::policy::OperationRule::access(
                                                        tools::policy::OperationAccess::Always,
                                                    ),
                                                ) {
                                                    tracing::warn!(op = %op, error = %e, "Approve Always refused by the policy");
                                                }
                                                let patch = serde_json::json!({
                                                    "operationPolicy": policy.to_json()
                                                });
                                                if let Err(e) = store
                                                    .upsert_entity_config(
                                                        "agent", agent_id, &patch,
                                                    )
                                                {
                                                    warn!(session_id, op, error = %e, "failed to persist operation Always grant");
                                                }
                                            }
                                        }
                                        "once" | "approve" | "approved" | "yes"
                                        | "true" => {}
                                        _ => {
                                            blocked_results[idx] = Some((
                                                tool_calls[idx].clone(),
                                                ToolResult::error(format!(
                                                    "The user declined to approve the \
                                                     operation '{op}'. Tell the user it \
                                                     needs their approval and stop — do \
                                                     not retry or work around it."
                                                )),
                                            ));
                                        }
                                    }
                                }
                                None => {
                                    // Nobody to ask on this surface: the
                                    // work goes to whoever holds the
                                    // authority for it, or waits for the
                                    // owner if nobody does.
                                    blocked_results[idx] = Some((
                                        tool_calls[idx].clone(),
                                        match hand_off_out_of_bounds(
                                            store,
                                            agent_id,
                                            session_id,
                                            op.as_str(),
                                            &display,
                                            &decision.reason,
                                        ) {
                                            Some(handed) => ToolResult::ok(handed),
                                            None => ToolResult::error(format!(
                                                "The operation '{op}' needs approval and no \
                                                 one is available to approve it in this run. \
                                                 Report this and stop."
                                            )),
                                        },
                                    ));
                                }
                            }
                        } else if let Some(park) =
                            workflow_mode.and_then(|m| m.park.as_ref())
                        {
                            // Workflow suspend/resume: park the run for the
                            // owner instead of refusing — the closure persists
                            // the suspension row; the loop exits parked.
                            let snapshot = convert_messages(
                                &sessions
                                .map(|s| s.get_messages(session_id).unwrap_or_default())
                                .unwrap_or_default(),
                            );
                            match park(WorkflowPark {
                                messages: snapshot,
                                call: &tool_calls[idx],
                                operation: tools::plugin_tool::port_suffix(&op),
                                display: display.clone(),
                            }) {
                                Ok(()) => {
                                    wf_break_reason =
                                        Some("awaiting_approval".to_string());
                                }
                                Err(e) => {
                                    // Can't persist the suspension → fail loud,
                                    // never silent-run the gated call.
                                    wf_break_reason =
                                        Some(format!("suspension_failed:{e}"));
                                }
                            }
                            break;
                        } else {
                            // Unattended chat origin (cron/comm/subagent): the chat
                            // gate can't pause, and the workflow path already parks
                            // at its checkpoint. Work that fell outside this seat's
                            // bounds is not impossible work — it is somebody's to
                            // decide, so it is handed to the seat that holds that
                            // authority and this run stops cleanly. With nobody
                            // holding it, it waits for the owner as before.
                            blocked_results[idx] = Some((
                                tool_calls[idx].clone(),
                                match hand_off_out_of_bounds(
                                    store,
                                    agent_id,
                                    session_id,
                                    op.as_str(),
                                    &display,
                                    &decision.reason,
                                ) {
                                    Some(handed) => ToolResult::ok(handed),
                                    None => ToolResult::error(format!(
                                        "The operation '{op}' needs your approval and this is \
                                         an unattended run. It was not performed."
                                    )),
                                },
                            ));
                        }
                    }
                }
            }
            // The operation gate is the decision for a declared operation:
            // `plugin` and `pack` belong to no capability, so there is
            // nothing further to ask here.
            continue;
        }
        let target = tools.target(&tool_calls[idx].name, &tool_calls[idx].input).await;
        let category = match target.as_ref().and_then(|t| t.capability.clone()) {
            Some(c) => c,
            None => continue, // ungated (installed extension / non-ambient tool)
        };
        let category = category.as_str();
        let cap_off = entity_permissions
            .map(|p| p.get(category) == Some(&false))
            .unwrap_or(false);
        // The shell command this call would run, if any (for the per-command
        // allowlist). None for non-shell tools.
        let shell_cmd = target.as_ref().and_then(shell_command_of);
        if !cap_off || full_access {
            // Pre-granted (capability ON), no permission map, or Full Access
            // → proceed without asking.
            approved_cats.insert(category.to_string());
            continue;
        }
        // Capability OFF, but this exact shell command was "approved always"
        // (matched by prefix; compound/interpreter commands never match) →
        // run without asking. Hard safeguards still apply unconditionally.
        if let Some(ref c) = shell_cmd
            && tools::policy::command_matches(&approved_cmds, c) {
            approved_cats.insert(category.to_string());
            continue;
        }
        // Capability OFF + not Full Access + not pre-approved → ask, but ONLY
        // when a human is present. Unattended runs (cron/heartbeat/workflow/
        // comm/subagent) have no one to answer, so it's denied (left ungranted
        // → Phase 1c blocks) rather than hanging on a prompt nobody sees.
        if tools::ExecutionMode::from(origin) != tools::ExecutionMode::Interactive
            && !approval_relay
        {
            continue;
        }
        if run.approval.is_none() {
            // No channel to ask through: leave the category ungranted so
            // registry Phase 1c hard-blocks (safe).
            continue;
        }
        // Collect pass: remember the call and move on; the batch is
        // asked once below and the decision applied in the apply pass.
        let decision = match gate_pass {
            GatePass::Collect => {
                to_ask.push(idx);
                continue;
            }
            GatePass::Apply => batch_decision.clone().unwrap_or_else(|| "deny".to_string()),
        };
        match decision.as_str() {
            "always" => {
                approved_cats.insert(category.to_string());
                match &shell_cmd {
                    // Shell command → remember just this command's PREFIX
                    // (not all of Shell). Interpreters/compound commands
                    // yield None → no durable grant (approved once only).
                    Some(c) => {
                        if let Some(prefix) = tools::policy::command_prefix(c)
                            && !approved_cmds.iter().any(|p| p == &prefix) {
                            approved_cmds.push(prefix.clone());
                            if let Err(e) = store.set_approved_commands(&approved_cmds)
                            {
                                warn!(session_id, error = %e, "failed to persist approved command");
                            }
                        }
                    }
                    // Non-shell capability → grant the whole capability for
                    // next time (per-item grants aren't meaningful there).
                    None => {
                        if let Err(e) = persist_capability_grant(store, category) {
                            warn!(session_id, category, error = %e, "failed to persist capability grant");
                        }
                    }
                }
            }
            "once" | "approve" | "approved" | "yes" | "true" => {
                approved_cats.insert(category.to_string());
            }
            _ => {
                // Deny → skip execution with a clean, non-spiraling result.
                blocked_results[idx] = Some((
                    tool_calls[idx].clone(),
                    ToolResult::error(format!(
                        "The user declined to allow this action (the \"{}\" capability \
                         is off). Tell the user it needs their approval and stop — do \
                         not retry or work around it.",
                        tools::capabilities::capability_label(category)
                    )),
                ));
            }
        }
    }
    }
    GateOutcome {
        approved_categories: approved_cats,
        owner_answered,
        parked: wf_break_reason,
    }
}

/// Detect if a tool call is requesting documentation (help/schema).
/// Returns a cache key like "skill:gws-sheets" or "plugin:sheets:help" if so.
fn detect_tool_doc_call(tool_name: &str, input: &serde_json::Value) -> Option<String> {
    let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");
    let resource = input.get("resource").and_then(|v| v.as_str()).unwrap_or("");

    match tool_name {
        "skill" => {
            if action == "help" || action == "list" || action == "docs" {
                let skill_name = input
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                Some(format!("skill:{}", skill_name))
            } else {
                None
            }
        }
        "plugin" => {
            if action == "help" || action == "schema" || action == "services" {
                let name = if !resource.is_empty() {
                    resource
                } else {
                    input
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                };
                Some(format!("plugin:{}:{}", name, action))
            } else {
                None
            }
        }
        // MCP tool documentation
        "mcp" => {
            if action == "help" || action == "list" || action == "schema" {
                let server = input
                    .get("server")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                Some(format!("mcp:{}:{}", server, action))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Most calls one parallel batch runs at once.
const MAX_PARALLEL_CALLS: usize = 10;

/// Claude Code's partitioning. `calls` holds `(index, concurrency_safe)` in
/// call order; consecutive concurrency-safe calls form one batch (at most
/// [`MAX_PARALLEL_CALLS`]), every other call is a batch of its own, and the
/// batches keep the calls' order.
fn partition_tool_calls(calls: &[(usize, bool)]) -> Vec<Vec<usize>> {
    let mut batches: Vec<Vec<usize>> = Vec::new();
    let mut open_safe = false;
    for &(idx, safe) in calls {
        match batches.last_mut() {
            Some(batch) if safe && open_safe && batch.len() < MAX_PARALLEL_CALLS => batch.push(idx),
            _ => batches.push(vec![idx]),
        }
        open_safe = safe;
    }
    batches
}

#[cfg(test)]
mod grant_counter_tests {
    /// A sub-agent runs under its parent's operation policy, so a standing
    /// grant it spends counts against the parent seat's day — it gets no
    /// fresh allowance of its own. Other runs are keyed as before.
    #[test]
    fn a_sub_agent_spends_its_parent_seats_counters() {
        assert_eq!(super::grant_counter_seat("", "subagent:agent:bk:web:sa-1"), "bk");
        assert_eq!(super::grant_counter_seat("", "subagent:subagent:agent:bk:web:sa-1:sa-2"), "bk");
        assert_eq!(super::grant_counter_seat("bk", "agent:bk:web"), "bk");
        assert_eq!(super::grant_counter_seat("", "agent:assistant:web"), "");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use types::permissions::{CallEffects, RuleField, Target};

    /// Spiral tests exercise the counting mechanics at the shipped default.
    const SAME_ACTION_LIMIT: usize = crate::guardrails::DEFAULT_SAME_ACTION_LIMIT;

    fn call(name: &str, input: serde_json::Value) -> ai::ToolCall {
        ai::ToolCall { id: "c1".into(), name: name.into(), input }
    }

    fn target(tool: &str, key: &str, field: Option<RuleField>) -> Target {
        Target {
            tool: tool.into(),
            key: key.into(),
            operation: None,
            capability: None,
            field,
            read_only: false,
            effects: CallEffects::unknown(),
        }
    }

    fn read(path: &str) -> Target {
        target("os", "read_file", Some(RuleField::Folder(path.into())))
    }

    fn command(cmd: &str) -> Target {
        target("os", "run_command", Some(RuleField::CommandPrefix(cmd.into())))
    }

    /// Claude Code's partitioning: consecutive concurrency-safe calls batch,
    /// anything else runs alone, order is kept.
    #[test]
    fn safe_runs_batch_and_everything_else_runs_alone_in_order() {
        let calls = [(0, true), (1, true), (2, false), (3, true), (4, false), (5, false), (6, true)];
        assert_eq!(partition_tool_calls(&calls), vec![vec![0, 1], vec![2], vec![3], vec![4], vec![5], vec![6]]);
        assert!(partition_tool_calls(&[]).is_empty());
    }

    #[test]
    fn a_safe_batch_holds_at_most_ten() {
        let calls: Vec<(usize, bool)> = (0..23).map(|i| (i, true)).collect();
        let batches = partition_tool_calls(&calls);
        assert_eq!(batches.iter().map(Vec::len).collect::<Vec<_>>(), [10, 10, 3]);
        assert_eq!(batches.concat(), (0..23).collect::<Vec<_>>(), "order kept");
    }

    #[test]
    fn spiral_exploration_read_errors_never_trip_limit() {
        // Failed reads across different paths are owned by the per-path
        // read_failures cap; the coarse spiral stays at 0.
        let mut counts = std::collections::HashMap::new();
        for i in 0..(SAME_ACTION_LIMIT + 4) {
            let path = format!("/tmp/miss-{i}.rs");
            let c = call("os", serde_json::json!({"action": "read", "path": path}));
            record_action_spiral(&mut counts, &c, Some(&read(&path)), true, false);
        }
        assert_eq!(counts.get("os:read").copied().unwrap_or(0), 0);
    }

    #[test]
    fn spiral_redundant_reads_still_trip_limit() {
        let mut counts = std::collections::HashMap::new();
        let c = call("os", serde_json::json!({"action": "read", "path": "/tmp/same.rs"}));
        for _ in 0..SAME_ACTION_LIMIT {
            record_action_spiral(&mut counts, &c, Some(&read("/tmp/same.rs")), false, true);
        }
        assert_eq!(counts["os:read"], SAME_ACTION_LIMIT);
    }

    #[test]
    fn spiral_counts_what_made_no_progress_except_read_misses() {
        let r = read("/tmp/a.rs");
        assert!(!counts_toward_action_spiral(Some(&r), true, false), "a missed read is read_failures' job");
        assert!(counts_toward_action_spiral(Some(&r), false, true), "a re-fetch is the wander");
        assert!(!counts_toward_action_spiral(Some(&r), false, false), "a novel read is progress");
        // A shell dump of a file is a read too.
        assert!(!counts_toward_action_spiral(Some(&command("cat /tmp/missing.rs")), true, false));
        // Other failed commands, and unknown tools, count.
        assert!(counts_toward_action_spiral(Some(&command("ls /nope")), true, false));
        assert!(counts_toward_action_spiral(None, true, false));
    }

    #[test]
    fn a_read_is_named_by_its_rule_key_and_field() {
        assert_eq!(read_path(&read("/tmp/a.rs")).as_deref(), Some("/tmp/a.rs"));
        assert_eq!(read_path(&command("cat /tmp/a.rs")).as_deref(), Some("/tmp/a.rs"));
        assert_eq!(read_path(&command("ls /tmp")), None);
        assert_eq!(read_path(&target("os", "write_file", Some(RuleField::Folder("/tmp/a".into())))), None);
        // A read with no path is not a tracked target.
        assert_eq!(read_path(&target("os", "read_file", None)), None);
    }

    #[test]
    fn full_file_read_is_left_to_the_ledger() {
        let r = read("/tmp/a.rs");
        assert!(is_full_file_read(&r, &serde_json::json!({"path": "/tmp/a.rs"})));
        assert!(!is_full_file_read(&r, &serde_json::json!({"path": "/tmp/a.rs", "offset": 10, "limit": 20})));
        assert!(!is_full_file_read(&command("cat /tmp/a.rs"), &serde_json::json!({})));
    }

    #[test]
    fn the_done_gate_reads_the_job_not_the_tool() {
        assert!(is_file_change(&target("os", "edit_file", None)));
        assert!(is_file_change(&target("os", "write_file", None)));
        assert!(!is_file_change(&read("/a.rs")));
        assert!(is_check_run(&command("cargo test")));
        assert!(!is_check_run(&command("cargo build")));
        assert!(!is_check_run(&target("os", "edit_file", Some(RuleField::Folder("cargo test".into())))));
        assert!(is_desktop_act(&target("os", "desktop_click", None)));
        assert!(!is_desktop_act(&target("os", "desktop_see", None)));
        assert!(!is_desktop_act(&command("ls")));
        assert_eq!(shell_command_of(&command("ls -la")).as_deref(), Some("ls -la"));
        assert_eq!(shell_command_of(&read("/a")), None);
    }

    /// One message's results are held to the budget by persisting the
    /// largest first; errors and images stay inline.
    #[test]
    fn a_message_over_its_budget_persists_its_largest_results_first() {
        let dir = tempfile::tempdir().unwrap();
        let big = "x".repeat(150_000);
        let mid = "y".repeat(80_000);
        let mut results = vec![
            Some((call("os", serde_json::json!({})), ToolResult::ok(mid.clone()))),
            Some((call("web", serde_json::json!({})), ToolResult::ok(big))),
            Some((call("os", serde_json::json!({})), ToolResult::error("e".repeat(1_000)))),
            None,
        ];
        apply_message_budget(&mut results, dir.path());
        let content = |i: usize| results[i].as_ref().unwrap().1.content.clone();
        assert!(content(1).starts_with("<persisted-output>"), "the largest goes first");
        assert_eq!(content(0), mid, "under budget after one: the rest stay inline");
        assert!(content(2).starts_with("eee"));
    }

    /// The repeated-action backstop is a nudge: it refuses the offending call and
    /// lets the turn continue. Making it terminal again would resurrect the dead
    /// turns users saw as "Stopped: … called 8 times without progress".
    #[test]
    fn spiral_backstop_is_a_nudge_not_a_stop() {
        let src = include_str!("tool_round.rs");
        let block = src
            .split("racked up the same-action limit of UNPRODUCTIVE attempts")
            .nth(1)
            .expect("spiral backstop block");
        let block = &block[..block.find("\n    // ──").unwrap_or(block.len())];
        assert!(
            !block.contains("ToolResult::terminal"),
            "spiral backstop must not end the run — use ToolResult::error"
        );
        assert!(
            block.contains("action_call_counts.insert(key.clone(), 0)"),
            "the budget must reset when the nudge fires, or the action is locked out for the turn"
        );
    }

    /// A plugin call with no `action` is keyed on the plugin (its rule key),
    /// so every failed verb against one plugin lands on one counter.
    #[test]
    fn action_key_keys_plugin_calls_on_the_plugin() {
        let plugin = |slug: &str, cmd: &str| {
            (
                call("plugin", serde_json::json!({"resource": slug, "command": cmd})),
                target("plugin", &format!("plugin__{slug}"), None),
            )
        };
        let (a, ta) = plugin("quickbooks", "payment create --line x");
        let (b, tb) = plugin("quickbooks", "batch execute --batch-item-request y");
        assert_eq!(action_key(&a, Some(&ta)), "plugin__quickbooks");
        assert_eq!(action_key(&b, Some(&tb)), "plugin__quickbooks");
        let (other, to) = plugin("gws", "gmail +send --to a@b.c");
        assert_ne!(action_key(&a, Some(&ta)), action_key(&other, Some(&to)));
        // An explicit action keys on the tool and action.
        let with_action = call("plugin", serde_json::json!({"resource": "quickbooks", "action": "exec", "command": "q"}));
        assert_eq!(action_key(&with_action, Some(&ta)), "plugin:exec");
        let glob = call("os", serde_json::json!({"action": "glob", "path": "/tmp"}));
        assert_eq!(action_key(&glob, Some(&command("x"))), "os:glob");
    }
}

#[cfg(test)]
mod runaway_backstop_tests {
    use super::*;

    fn call(name: &str, cmd: &str) -> ai::ToolCall {
        ai::ToolCall {
            id: "t".into(),
            name: name.into(),
            input: serde_json::json!({"action": "exec", "command": cmd}),
        }
    }

    /// The runaway backstop counts the CALL, not the answer. This is the case
    /// every other guard misses: a poll whose output drifts every time
    /// (`docker compose logs`, `tail`, a status endpoint) is never flagged
    /// unproductive, so `counts_toward_action_spiral` never fires and the
    /// 3-strike identical-args block never accrues. Live-verified 2026-08-27:
    /// 16 such polls produced ZERO guard firings.
    #[test]
    fn identical_call_aborts_even_when_every_result_differs() {
        let mut budget = ai::call_budget::CallBudget::new();
        let c = call("os", "tail -30 /var/log/app.log");
        for i in 0..IDENTICAL_CALL_ABORT {
            assert!(
                budget.abort_due(&c.name, &c.input, IDENTICAL_CALL_ABORT).is_none(),
                "must not abort at {i} repeats"
            );
            budget.record(&c.name, &c.input);
        }
        assert_eq!(
            budget.abort_due(&c.name, &c.input, IDENTICAL_CALL_ABORT),
            Some(IDENTICAL_CALL_ABORT),
            "the {IDENTICAL_CALL_ABORT}th repeat ends the turn"
        );
    }

    /// Distinct work never accrues toward one budget — the guard must not
    /// punish a model doing many different things with the same tool.
    #[test]
    fn different_arguments_do_not_share_a_budget() {
        let mut budget = ai::call_budget::CallBudget::new();
        for i in 0..40 {
            let c = call("os", &format!("echo {i}"));
            budget.record(&c.name, &c.input);
        }
        for i in 0..40 {
            let c = call("os", &format!("echo {i}"));
            assert!(
                budget.abort_due(&c.name, &c.input, IDENTICAL_CALL_ABORT).is_none(),
                "40 distinct commands must never trip the backstop"
            );
        }
    }

    /// Same command, different tool = different budget.
    #[test]
    fn tool_name_is_part_of_the_key() {
        let mut budget = ai::call_budget::CallBudget::new();
        let a = call("os", "ls");
        for _ in 0..IDENTICAL_CALL_ABORT {
            budget.record(&a.name, &a.input);
        }
        let b = call("execute", "ls");
        assert!(budget.abort_due(&a.name, &a.input, IDENTICAL_CALL_ABORT).is_some());
        assert!(budget.abort_due(&b.name, &b.input, IDENTICAL_CALL_ABORT).is_none());
    }
}

#[cfg(test)]
mod out_of_bounds_tests {
    use super::*;

    /// Stands in for the case opener the server installs at boot, so the join
    /// can be proven without the engine: what matters here is that ONE
    /// hand-over pathway is used and what is put on the assignment.
    struct Capture {
        seen: std::sync::Mutex<Vec<tools::assignments::AssignmentRequest>>,
    }

    impl tools::assignments::AssignmentOpener for Capture {
        fn open(
            &self,
            req: &tools::assignments::AssignmentRequest,
        ) -> Result<String, String> {
            self.seen.lock().unwrap().push(req.clone());
            Ok("assignment-1".to_string())
        }
    }

    fn store() -> Arc<Store> {
        let path = std::env::temp_dir().join(format!("nebo-oob-{}.db", uuid::Uuid::new_v4()));
        Arc::new(Store::new(&path.to_string_lossy()).expect("store"))
    }

    fn seat(store: &Arc<Store>, id: &str, name: &str, frontmatter: &str) {
        store
            .create_agent(id, None, name, "", "", frontmatter, None, None)
            .expect("seat");
    }

    /// Out-of-bounds work in an unattended run is somebody's assignment, not a
    /// refusal — and who that somebody is comes out of the data, never out of a
    /// name compiled into Rust.
    #[test]
    fn out_of_bounds_work_becomes_the_authority_seats_assignment() {
        let store = store();
        let captured = Arc::new(Capture { seen: std::sync::Mutex::new(Vec::new()) });
        tools::assignments::install_assignment_opener(captured.clone());

        // A seat the owner has given the authority to grant, and a worker.
        seat(&store, "coo", "Operations Lead", "");
        let mut policy = tools::policy::OperationPolicy::default();
        policy
            .apply_edit(
                "authority.grant.grant",
                tools::policy::OperationRule::access(tools::policy::OperationAccess::Approval),
            )
            .unwrap();
        store
            .upsert_entity_config(
                "agent",
                "coo",
                &serde_json::json!({ "operationPolicy": policy.to_json() }),
            )
            .expect("policy");
        seat(&store, "bk", "Bookkeeper", "");

        // Found from the data: the seat that holds the authority to grant.
        assert_eq!(
            authority_seat(&store, "bk").map(|a| a.id).as_deref(),
            Some("coo")
        );

        const OP: &str = "ledger.billpayment.create";
        const DISPLAY: &str = "Pay Acme Supplies $3,000.00 for bill #1042";
        const REASON: &str = "amount 300000 exceeds the grant's 250000 per operation";
        let handed = hand_off_out_of_bounds(&store, "bk", "agent:bk:cron", OP, DISPLAY, REASON)
            .expect("the work is handed on, not refused");
        assert!(handed.contains("Operations Lead"), "{handed}");
        assert!(handed.contains("Do not retry"), "{handed}");

        let reqs = captured.seen.lock().unwrap();
        assert_eq!(reqs.len(), 1, "ONE hand-over, through open_assignment");
        let req = &reqs[0];
        assert_eq!(req.assignee_agent_id, "coo");
        assert_eq!(req.assigner_agent_id, "bk");
        assert_eq!(req.assigner_session_key, "agent:bk:cron");
        // The assignment carries the operation, who wanted it, the bound it fell
        // outside, and what is now stopped.
        assert!(req.subject.contains("Bookkeeper") && req.subject.contains(OP), "{}", req.subject);
        assert!(req.subject.contains(DISPLAY), "{}", req.subject);
        assert!(req.done_means.contains(REASON), "{}", req.done_means);
        assert!(req.done_means.contains(DISPLAY), "{}", req.done_means);
        assert!(req.done_means.contains(OP), "{}", req.done_means);
        drop(reqs);

        // The line the owner drew wins over the search: an Office Manager who
        // holds no authority of its own still gets the decision when the owner
        // put it above this seat.
        seat(&store, "om", "Office Manager", "");
        store
            .update_agent(
                "bk", "", "", "", "", None, None, None, None, None, None, None, None, None,
                Some("om"),
            )
            .expect("reporting line");
        assert_eq!(
            authority_seat(&store, "bk").map(|a| a.id).as_deref(),
            Some("om"),
        );

        // Nobody above it and nobody holding that authority: the work waits for
        // the owner, which is what the caller's own fallback does.
        assert!(
            hand_off_out_of_bounds(&store, "coo", "agent:coo:cron", OP, DISPLAY, REASON).is_none(),
            "with no authority seat the owner decides",
        );
    }
}
