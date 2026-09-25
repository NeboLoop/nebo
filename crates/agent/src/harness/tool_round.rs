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
use tracing::{info, warn};

use ai::{Provider, RequestTrace, StreamEvent, StreamEventType};
use tools::{Origin, Registry, ToolContext, ToolResult};

use crate::concurrency::ConcurrencyController;
use crate::harness::conversation::convert_messages;
use crate::harness::session_gate::RunProgress;
use crate::runner::{
    WorkflowMode, WorkflowPark, desktop_evidence, simple_hash, truncate_str,
};
use crate::session::SessionManager;

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
    pub handoff_depth: u8,
    /// The run's permissions: every call is decided against them.
    pub grant: &'a Arc<types::permissions::Grant>,
    /// The entry the run came through.
    pub door: &'a types::permissions::Door,
    /// The run's input arrived from outside (a workflow started by an
    /// inbound payload).
    pub untrusted_input: bool,
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
            grant,
            door,
            untrusted_input,
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
            grant: Some(grant.clone()),
            door: door.clone(),
            answered_ask: None,
            untrusted_input,
            judgement: None,
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
            // Restricted-run allowlist: the review fork's whitelist, or
            // the request's explicit allowlist (phone callers). None for
            // every normal run.
            tool_whitelist: review_fork
                .as_ref()
                .map(|r| r.whitelist.clone())
                .or_else(|| tool_allowlist.cloned()),
            whitelist_denial_hint: match review_fork {
                Some(_) => Some(
                    "Only the skill tool is available in this review pass: save the learning \
                     with it or reply 'Nothing to save.'"
                        .to_string(),
                ),
                None => tool_denial_hint.clone(),
            },
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
    pub providers: &'a Arc<RwLock<Vec<Arc<dyn Provider>>>>,
    pub concurrency: &'a Arc<ConcurrencyController>,
    pub hooks: &'a napp::HookDispatcher,
    pub user_prompt: &'a str,
    pub iteration: usize,
    pub workflow_mode: Option<&'a WorkflowMode>,
    pub decide: Option<&'a Arc<ai::DecideClient>>,
    pub active_task: &'a String,
    /// The turn's mode: a helper's kind is an enforced tool set and its
    /// depth caps delegation (`delegation::permits`). `None` for `Runner`.
    pub turn_mode: Option<&'a crate::harness::TurnMode>,
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
        providers,
        concurrency,
        hooks,
        user_prompt,
        iteration,
        workflow_mode,
        decide,
        active_task,
        turn_mode,
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
        run_cwd,
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
        plan_touch,
        edits_since_check,
        last_desktop_act,
        ctx_spilled_results,
    } = st;
    let ctx = scope.tool_context();

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

    // A helper's kind and depth, whatever shape the call arrives in.
    if let Some(mode) = turn_mode {
        for (idx, tc) in tool_calls.iter().enumerate() {
            if let (None, Some(target)) = (&blocked_results[idx], &targets[idx])
                && let Err(refusal) = crate::harness::delegation::permits(mode, target)
            {
                blocked_results[idx] = Some((tc.clone(), ToolResult::error(refusal)));
            }
        }
    }

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
                     command instead of re-reading: run_command(command: \
                     \"for i in $(seq 1 12); do wc -l < FILE; test $(wc -l < FILE) -ge N && break; \
                     sleep 5; done; cat FILE\", timeout: 90000), then report what you \
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

    // ── The permission judgement (permissions::judgement) ─────────────
    // The calls about to run whose outward effect the code could not
    // decide (whether they publish, or speak for the owner outside) are
    // asked about once, together: Jev first, the aux classifier for what
    // it didn't settle. Each call carries its verdict to the permission
    // check, which records it (shadow) or acts on it (enforce).
    let mut judgements: Vec<Option<types::permissions::Verdict>> = vec![None; tool_calls.len()];
    if wf_break_reason.is_none() {
        let store = sessions.store();
        let mut asked: Vec<(usize, crate::harness::permissions::cases::Question)> = Vec::new();
        for (idx, tc) in tool_calls.iter().enumerate() {
            if blocked_results[idx].is_some() {
                continue;
            }
            let Some(target) = tools.target(&tc.name, &tc.input).await else { continue };
            let cx = crate::harness::permissions::CheckCx { ctx: &ctx, input: &tc.input, grant: scope.grant, store };
            if let Some(mut q) = crate::harness::permissions::question_for(&cx, &target) {
                q.activity = tools.labels(&tc.name, &tc.input).await.0;
                asked.push((idx, q));
            }
        }
        if !asked.is_empty() {
            // A workflow turn has no session objective and an empty
            // prompt; its task is the step it was given.
            let (objective, last_message) = match workflow_mode {
                Some(m) => (m.objective.as_str(), m.instruction.as_str()),
                None => (active_task.as_str(), user_prompt),
            };
            let providers = providers.read().await.clone();
            let judge = crate::harness::permissions::judgement::JudgeCx {
                decide: decide.map(|d| d.as_ref()),
                providers: &providers,
                trace: side_trace,
                objective,
                last_message,
            };
            let questions: Vec<_> = asked.iter().map(|(_, q)| q.clone()).collect();
            let verdicts = crate::harness::permissions::judgement::judge_round(&judge, &questions).await;
            for ((idx, _), verdict) in asked.into_iter().zip(verdicts) {
                judgements[idx] = Some(verdict);
            }
        }
    }

    // Workflow break (the exit primitive): the turn ends now — nothing in
    // this batch executes.
    if let Some(reason) = wf_break_reason {
        return RoundOutcome::Ended(crate::guardrails::Exit::Workflow(reason));
    }

    // Claude Code's partitioning: consecutive concurrency-safe calls form
    // one batch that runs in parallel, MAX_PARALLEL_CALLS at a time; every
    // other call runs alone; the calls' order is kept. Invalid input is not
    // safe (`Registry::concurrency_safe`).
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
    // A workflow activity that can park runs its calls one at a time, so a
    // call the permission check parks on the owner stops the step there.
    let workflow_park = workflow_mode.and_then(|m| m.park.as_ref());
    let batches = if workflow_park.is_some() {
        live.iter().map(|(idx, _)| vec![*idx]).collect()
    } else {
        partition_tool_calls(&live)
    };
    let mut parked_call: Option<usize> = None;
    for batch in batches {
        if parked_call.is_some() {
            break;
        }
        let mut futures = FuturesUnordered::new();
        // A batch of safe calls runs through a pool of MAX_PARALLEL_CALLS;
        // results still land as each call completes.
        let pool = Arc::new(tokio::sync::Semaphore::new(MAX_PARALLEL_CALLS));
        for idx in batch {
            let tools = tools.clone();
            let mut ctx = ctx.clone();
            ctx.judgement = judgements[idx].clone();
            let tc = tool_calls[idx].clone();
            let concurrency = concurrency.clone();
            let pool = pool.clone();
            futures.push(async move {
                let _slot = pool.acquire_owned().await;
                let _permit = concurrency.acquire_tool_permit().await;
                let input_str = tc.input.to_string();
                let input_log = truncate_str(&input_str, 500);
                info!(tool = %tc.name, id = %tc.id, input = %input_log, "executing tool");
                // Each tool's own timeout; the loop has none of its own.
                let budget = tools.execution_timeout(&tc.name, &tc.input).await;
                let started = std::time::Instant::now();
                ctx.tool_call_id = tc.id.clone();
                ctx.parked = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                let parked = ctx.parked.clone();
                let run = tools.execute(&ctx, &tc.name, tc.input.clone());
                let result = match budget {
                    None => run.await,
                    Some(budget) => match run_within_budget(budget, parked, run).await {
                        Some(r) => r,
                        None => ToolResult::error(tool_timeout_text(&tc.name, budget)),
                    },
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
            if workflow_park.is_some() && result.parked_ask.is_some() {
                parked_call = Some(idx);
            }
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

    // The parked call's result is not saved: the resumed run executes the
    // call itself once the owner answers.
    let parked = parked_call.and_then(|idx| results[idx].take().map(|(tc, r)| (idx, tc, r.parked_ask.unwrap_or_default())));

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
        summary_tool_results.push(ToolResult { payload: None, need: None, parked_ask: None,
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

    // A workflow step parked on the owner: the run suspends with the
    // conversation as it stands and the call that waits.
    if let (Some(park), Some((idx, tc, ask_id))) = (workflow_park, parked) {
        let snapshot = convert_messages(&sessions.get_messages(session_id).unwrap_or_default());
        let operation = targets[idx]
            .as_ref()
            .map(|t| t.operation.as_deref().map(tools::plugin_tool::port_suffix).unwrap_or_else(|| t.key.clone()))
            .unwrap_or_else(|| tc.name.clone());
        let display = tools.labels(&tc.name, &tc.input).await.0;
        let reason = match park(WorkflowPark { messages: snapshot, call: &tc, ask_id: &ask_id, operation, display }) {
            Ok(()) => "awaiting_approval".to_string(),
            // Can't persist the suspension: fail loud, never run the call.
            Err(e) => format!("suspension_failed:{e}"),
        };
        return RoundOutcome::Ended(crate::guardrails::Exit::Workflow(reason));
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
    // Every failed verb against one plugin is one spiral (the QuickBooks
    // thread, 2026-09-06: fifty-seven guesses, each a different verb).
    if tools::plugin_tools::plugin_slug(&call.name).is_some() {
        return call.name.clone();
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

/// Most calls of one parallel batch that run at once (Claude Code's pool).
const MAX_PARALLEL_CALLS: usize = 10;

/// Claude Code's partitioning. `calls` holds `(index, concurrency_safe)` in
/// call order; each run of consecutive concurrency-safe calls is one batch,
/// every other call is a batch of its own, and the batches keep the calls'
/// order.
fn partition_tool_calls(calls: &[(usize, bool)]) -> Vec<Vec<usize>> {
    let mut batches: Vec<Vec<usize>> = Vec::new();
    let mut open_safe = false;
    for &(idx, safe) in calls {
        match batches.last_mut() {
            Some(batch) if safe && open_safe => batch.push(idx),
            _ => batches.push(vec![idx]),
        }
        open_safe = safe;
    }
    batches
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
        target("read_file", "read_file", Some(RuleField::Folder(path.into())))
    }

    fn command(cmd: &str) -> Target {
        target("run_command", "run_command", Some(RuleField::CommandPrefix(cmd.into())))
    }

    /// Claude Code's partitioning: consecutive concurrency-safe calls batch,
    /// anything else runs alone, order is kept.
    #[test]
    fn safe_runs_batch_and_everything_else_runs_alone_in_order() {
        let calls = [(0, true), (1, true), (2, false), (3, true), (4, false), (5, false), (6, true)];
        assert_eq!(partition_tool_calls(&calls), vec![vec![0, 1], vec![2], vec![3], vec![4], vec![5], vec![6]]);
        assert!(partition_tool_calls(&[]).is_empty());
    }

    /// A run of safe calls is one batch however long; the pool, not the
    /// partition, keeps ten running at once.
    #[test]
    fn a_run_of_safe_calls_is_one_batch() {
        let calls: Vec<(usize, bool)> = (0..23).map(|i| (i, true)).collect();
        assert_eq!(partition_tool_calls(&calls), vec![(0..23).collect::<Vec<_>>()]);
    }

    #[test]
    fn spiral_exploration_read_errors_never_trip_limit() {
        // Failed reads across different paths are owned by the per-path
        // read_failures cap; the coarse spiral stays at 0.
        let mut counts = std::collections::HashMap::new();
        for i in 0..(SAME_ACTION_LIMIT + 4) {
            let path = format!("/tmp/miss-{i}.rs");
            let c = call("read_file", serde_json::json!({"path": path}));
            record_action_spiral(&mut counts, &c, Some(&read(&path)), true, false);
        }
        assert_eq!(counts.get("read_file:").copied().unwrap_or(0), 0);
    }

    #[test]
    fn spiral_redundant_reads_still_trip_limit() {
        let mut counts = std::collections::HashMap::new();
        let c = call("read_file", serde_json::json!({"path": "/tmp/same.rs"}));
        for _ in 0..SAME_ACTION_LIMIT {
            record_action_spiral(&mut counts, &c, Some(&read("/tmp/same.rs")), false, true);
        }
        assert_eq!(counts["read_file:"], SAME_ACTION_LIMIT);
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
        assert_eq!(read_path(&target("write_file", "write_file", Some(RuleField::Folder("/tmp/a".into())))), None);
        // A read with no path is not a tracked target.
        assert_eq!(read_path(&target("read_file", "read_file", None)), None);
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
        assert!(is_file_change(&target("edit_file", "edit_file", None)));
        assert!(is_file_change(&target("write_file", "write_file", None)));
        assert!(!is_file_change(&read("/a.rs")));
        assert!(is_check_run(&command("cargo test")));
        assert!(!is_check_run(&command("cargo build")));
        assert!(!is_check_run(&target("edit_file", "edit_file", Some(RuleField::Folder("cargo test".into())))));
        assert!(is_desktop_act(&target("os", "desktop_click", None)));
        assert!(!is_desktop_act(&target("os", "desktop_see", None)));
        assert!(!is_desktop_act(&command("ls")));
    }

    /// One message's results are held to the budget by persisting the
    /// largest first; errors and images stay inline.
    #[test]
    fn a_message_over_its_budget_persists_its_largest_results_first() {
        let dir = tempfile::tempdir().unwrap();
        let big = "x".repeat(150_000);
        let mid = "y".repeat(80_000);
        let mut results = vec![
            Some((call("read_file", serde_json::json!({})), ToolResult::ok(mid.clone()))),
            Some((call("web", serde_json::json!({})), ToolResult::ok(big))),
            Some((call("run_command", serde_json::json!({})), ToolResult::error("e".repeat(1_000)))),
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
            let name = format!("plugin__{slug}");
            (call(&name, serde_json::json!({"command": cmd})), target(&name, &name, None))
        };
        let (a, ta) = plugin("quickbooks", "payment create --line x");
        let (b, tb) = plugin("quickbooks", "batch execute --batch-item-request y");
        assert_eq!(action_key(&a, Some(&ta)), "plugin__quickbooks");
        assert_eq!(action_key(&b, Some(&tb)), "plugin__quickbooks");
        let (other, to) = plugin("gws", "gmail +send --to a@b.c");
        assert_ne!(action_key(&a, Some(&ta)), action_key(&other, Some(&to)));
        // A command keys on the tool and its first words.
        let ls = call("run_command", serde_json::json!({"command": "ls -la /tmp"}));
        assert_eq!(action_key(&ls, Some(&command("ls -la /tmp"))), "run_command:ls -la");
    }
}

#[cfg(test)]
mod runaway_backstop_tests {
    use super::*;

    fn call(name: &str, cmd: &str) -> ai::ToolCall {
        ai::ToolCall {
            id: "t".into(),
            name: name.into(),
            input: serde_json::json!({"command": cmd}),
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
        let c = call("run_command", "tail -30 /var/log/app.log");
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
            let c = call("run_command", &format!("echo {i}"));
            budget.record(&c.name, &c.input);
        }
        for i in 0..40 {
            let c = call("run_command", &format!("echo {i}"));
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
        let a = call("run_command", "ls");
        for _ in 0..IDENTICAL_CALL_ABORT {
            budget.record(&a.name, &a.input);
        }
        let b = call("execute", "ls");
        assert!(budget.abort_due(&a.name, &a.input, IDENTICAL_CALL_ABORT).is_some());
        assert!(budget.abort_due(&b.name, &b.input, IDENTICAL_CALL_ABORT).is_none());
    }
}
