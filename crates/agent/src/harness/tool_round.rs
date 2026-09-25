//! One round of tool calls: dispatch, gates, partition, run, caps, persist.
//! The turn driver (`turn::drive_turn`) runs one round per reply that
//! called tools.

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
use crate::harness::workflow_turn::{WorkflowMode, WorkflowPark};
use tools::truncate_str;
use crate::session::SessionManager;

/// How often the tool clock checks whether the call is parked on the owner.
const PARKED_POLL: Duration = Duration::from_millis(250);

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
    /// The employee's own tools the run's tool scope leaves out.
    pub withheld_tools: &'a Arc<HashSet<String>>,
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
            withheld_tools,
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
            waiting: progress.map(|p| p.waiting.clone()).unwrap_or_default(),
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
                    "Only the skill tools are available in this review pass: save the learning \
                     with save_skill or reply 'Nothing to save.'"
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
            withheld_tools: (!withheld_tools.is_empty()).then(|| withheld_tools.clone()),
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
    /// The trace a side call of this run carries: its purpose and the agent.
    pub side_trace: &'a (dyn Fn(&'static str) -> RequestTrace + Sync),
}

/// The run's state the round reads and updates: the called tools and the
/// done-gate bookkeeping.
pub(crate) struct RoundState<'a> {
    pub called_tools: &'a mut Vec<String>,
    pub plan_touch: &'a mut Option<(usize, String)>,
    pub edits_since_check: &'a mut usize,
    pub last_desktop_act: &'a mut Option<String>,
}

/// How a round ended.
pub(crate) enum RoundOutcome {
    /// Every call has its result saved; the loop takes another step.
    Ran(RoundResults),
    /// A tool's terminal error ended the turn; its notice is already sent.
    Terminal,
    /// A workflow primitive ended the turn (`workflow_exit:…`,
    /// `awaiting_approval`, `suspension_failed:…`).
    Workflow(String),
    /// The run was cancelled while the round ran.
    Cancelled,
}

/// What a round that ran reports back to the loop.
pub(crate) struct RoundResults {
    /// Short snapshots of the calls and results for the tool-summary label.
    pub summary_tool_calls: Vec<ai::ToolCall>,
    pub summary_tool_results: Vec<ToolResult>,
}

/// Run the model's tool calls: the exit primitive, then every call through
/// the executor (hooks, gates, the tool), then result caps and persistence.
/// A pre-execute hook and input normalization may rewrite `tool_calls`.
pub(crate) async fn run_tool_round(
    cx: &RoundContext<'_>,
    st: RoundState<'_>,
    exec: ToolExecutor<'_>,
    tool_calls: &mut [ai::ToolCall],
) -> RoundOutcome {
    let RoundContext {
        scope,
        tools,
        providers,
        iteration,
        workflow_mode,
        side_trace,
        ..
    } = *cx;
    let RunToolScope {
        sessions,
        tx,
        session_id,
        origin,
        cancel_token,
        progress,
        ..
    } = *scope;
    let RoundState {
        called_tools,
        plan_touch,
        edits_since_check,
        last_desktop_act,
    } = st;

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

    // Workflow `exit` is a loop primitive, not a real tool: the first exit
    // call ends the turn and nothing else in the reply runs (a read-only
    // call the stream already started is dropped with the executor).
    if workflow_mode.is_some()
        && let Some(tc) = tool_calls.iter().find(|tc| tc.name == "exit")
    {
        let reason = tc.input.get("reason").and_then(|v| v.as_str()).unwrap_or("");
        return RoundOutcome::Workflow(format!("workflow_exit:{reason}"));
    }

    let Some(ran) = exec.finish(tool_calls).await else {
        info!(session_id, "run cancelled during tool execution");
        return RoundOutcome::Cancelled;
    };
    let workflow_park = workflow_mode.and_then(|m| m.park.as_ref());
    let mut results: Vec<Option<(ai::ToolCall, ToolResult)>> = vec![None; tool_calls.len()];
    let mut targets: Vec<Option<types::permissions::Target>> = vec![None; tool_calls.len()];
    // The call's wall-clock time per tool id; persisted with the result.
    let mut durations: HashMap<String, u64> = HashMap::new();
    // Tool ids whose result a post-tool hook wrote into (the done gate's
    // "a check ran" signal).
    let mut hook_noted: HashSet<String> = HashSet::new();
    let mut parked_call: Option<usize> = None;
    for r in ran.into_iter().flatten() {
        let Ran { idx, tc, target, result, duration_ms, hook_noted: noted } = r;
        if let Some(d) = duration_ms {
            durations.insert(tc.id.clone(), d);
        }
        if noted {
            hook_noted.insert(tc.id.clone());
        }
        if workflow_park.is_some() && result.parked_ask.is_some() {
            parked_call = Some(idx);
        }
        tool_calls[idx] = tc.clone();
        targets[idx] = target;
        results[idx] = Some((tc, result));
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

    // Each result was shaped by the registry (its tool's threshold, the one
    // spill path); across the message, the largest go to disk first once
    // their total passes the per-message budget.
    apply_message_budget(&mut results, &tools::result_shape::results_dir(session_id));

    // The parked call's result is not saved: the resumed run executes the
    // call itself once the owner answers.
    let parked = parked_call.and_then(|idx| results[idx].take().map(|(tc, r)| (idx, tc, r.parked_ask.unwrap_or_default())));

    // Save all tool results to session in deterministic order.
    // Terminal tool error (auth/permission/connection) → end the turn after
    // this batch and surface to the user, instead of feeding it back for the
    // model to retry/improvise (the death-spiral fix; FRAMES.md Phase 1).
    let mut terminal_error: Option<(String, Option<types::OwnerNeed>)> = None;
    // Lightweight snapshots for the background tool summary generator.
    let mut summary_tool_calls: Vec<ai::ToolCall> = Vec::new();
    let mut summary_tool_results: Vec<ToolResult> = Vec::new();
    for (idx, entry) in results.into_iter().enumerate() {
        let Some((tc, result)) = entry else { continue };
        let target = targets[idx].as_ref();
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
        // Capture pre-truncation snapshots for the summarizer (only name + short content)
        summary_tool_calls.push(tc.clone());
        summary_tool_results.push(ToolResult { payload: None, need: None, parked_ask: None, taint: Vec::new(),
            content: truncate_str(&result.content, 300).to_string(),
            is_error: result.is_error,
            image_url: None,
            http_status: None,
            terminal: result.terminal,
        });
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
        return RoundOutcome::Workflow(reason);
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
    if let Some((msg, need)) = terminal_error {
        warn!(session_id, iteration, "terminal tool error — ending run");
        let _ = tx
            .send(StreamEvent::control_notice(msg, "terminal_tool_error").with_owner_need(need))
            .await;
        return RoundOutcome::Terminal;
    }

    RoundOutcome::Ran(RoundResults {
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

/// A call that writes or edits a file (the done gate counts them).
fn is_file_change(target: &types::permissions::Target) -> bool {
    matches!(target.key.as_str(), "write_file" | "edit_file")
}

/// A shell command that is a project check (`is_check_command`).
fn is_check_run(target: &types::permissions::Target) -> bool {
    target.key == "run_command"
        && matches!(&target.field, Some(types::permissions::RuleField::CommandPrefix(c)) if is_check_command(c))
}

/// A desktop action whose result is the screen after it.
fn is_desktop_act(target: &types::permissions::Target) -> bool {
    matches!(
        target.key.as_str(),
        "desktop_click" | "desktop_type" | "desktop_key" | "desktop_scroll" | "desktop_drag"
    )
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

/// Most calls that run at once (Claude Code's pool).
const MAX_PARALLEL_CALLS: usize = 10;

/// Where a call of the reply stands in the executor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Queued,
    Running,
    Done,
}

/// One call run to its result.
pub(crate) struct Ran {
    idx: usize,
    /// The call as it ran (a pre-execute hook may rewrite it).
    tc: ai::ToolCall,
    target: Option<types::permissions::Target>,
    result: ToolResult,
    /// The tool's wall-clock time; `None` for a call refused before it ran.
    duration_ms: Option<u64>,
    /// A post-execute hook wrote into the result.
    hook_noted: bool,
}

/// A reply's tool calls, started as the stream hands them over (Claude
/// Code's streaming tool executor). A concurrency-safe call starts as soon
/// as its input is complete, while every running call is safe too; any
/// other call waits for the reply to end, then runs alone and in order.
/// Results come back in call order.
pub(crate) struct ToolExecutor<'a> {
    cx: &'a RoundContext<'a>,
    ctx: ToolContext,
    pool: Arc<tokio::sync::Semaphore>,
    calls: Vec<ai::ToolCall>,
    safe: Vec<bool>,
    status: Vec<Status>,
    done: Vec<Option<Ran>>,
    running: FuturesUnordered<futures::future::BoxFuture<'a, Ran>>,
    /// The reply is still streaming: only safe calls may start.
    streaming: bool,
    /// A workflow step parked on the owner: nothing more starts.
    halted: bool,
}

impl<'a> ToolExecutor<'a> {
    pub(crate) fn new(cx: &'a RoundContext<'a>) -> Self {
        Self {
            cx,
            ctx: cx.scope.tool_context(),
            pool: Arc::new(tokio::sync::Semaphore::new(MAX_PARALLEL_CALLS)),
            calls: Vec::new(),
            safe: Vec::new(),
            status: Vec::new(),
            done: Vec::new(),
            running: FuturesUnordered::new(),
            streaming: true,
            halted: false,
        }
    }

    /// A workflow activity that can park runs its calls one at a time, so a
    /// call the permission check parks on the owner stops the step there.
    fn parks(&self) -> bool {
        self.cx.workflow_mode.is_some_and(|m| m.park.is_some())
    }

    /// Take the calls as the stream hands them over, running what may run,
    /// until the stream ends.
    pub(crate) async fn stream(&mut self, mut calls: mpsc::UnboundedReceiver<ai::ToolCall>) {
        loop {
            tokio::select! {
                next = calls.recv() => match next {
                    Some(tc) => self.add(tc).await,
                    None => return,
                },
                Some(ran) = self.running.next(), if !self.running.is_empty() => self.complete(ran),
            }
        }
    }

    /// The reply is complete: queue any call the stream did not hand over,
    /// and run every call to its result, in call order. `None` when the run
    /// was cancelled; a call that never started (after a park) is `None`.
    async fn finish(mut self, calls: &[ai::ToolCall]) -> Option<Vec<Option<Ran>>> {
        for tc in calls.iter().skip(self.calls.len()) {
            self.add(tc.clone()).await;
        }
        self.streaming = false;
        self.pump();
        while !self.running.is_empty() {
            tokio::select! {
                _ = self.cx.scope.cancel_token.cancelled() => return None,
                Some(ran) = self.running.next() => self.complete(ran),
            }
        }
        Some(self.done)
    }

    /// Queue one call and start what may start. Invalid input is not safe
    /// (`Registry::concurrency_safe`).
    async fn add(&mut self, tc: ai::ToolCall) {
        let safe = !self.parks() && {
            let input = self.cx.tools.normalize_input(&tc.name, tc.input.clone()).await;
            self.cx.tools.concurrency_safe(&tc.name, &input).await
        };
        self.calls.push(tc);
        self.safe.push(safe);
        self.status.push(Status::Queued);
        self.done.push(None);
        self.pump();
    }

    fn complete(&mut self, ran: Ran) {
        let idx = ran.idx;
        self.status[idx] = Status::Done;
        if self.parks() && ran.result.parked_ask.is_some() {
            self.halted = true;
        }
        self.done[idx] = Some(ran);
        self.pump();
    }

    fn pump(&mut self) {
        if self.halted {
            return;
        }
        let view: Vec<(Status, bool)> = self.status.iter().copied().zip(self.safe.iter().copied()).collect();
        for idx in startable(&view, self.streaming) {
            self.status[idx] = Status::Running;
            let cx = self.cx;
            let run = run_call(cx, self.ctx.clone(), self.pool.clone(), idx, self.calls[idx].clone());
            self.running.push(Box::pin(run));
        }
    }
}

/// Claude Code's queue rule. Walking the calls in order, a queued call
/// starts when nothing runs, or when it and every running call are
/// concurrency-safe; a queued unsafe call that cannot start holds every
/// call after it. While the reply streams, an unsafe call cannot start.
fn startable(calls: &[(Status, bool)], streaming: bool) -> Vec<usize> {
    let running: Vec<bool> = calls.iter().filter(|(s, _)| *s == Status::Running).map(|(_, safe)| *safe).collect();
    let mut any_running = !running.is_empty();
    let mut all_safe = running.iter().all(|safe| *safe);
    let mut start = Vec::new();
    for (idx, &(status, safe)) in calls.iter().enumerate() {
        if status != Status::Queued {
            continue;
        }
        let can = (safe || !streaming) && (!any_running || (safe && all_safe));
        if can {
            start.push(idx);
            any_running = true;
            all_safe &= safe;
        } else if !safe {
            break;
        }
    }
    start
}

/// One call from its hooks to its result: the pre-execute hook (which may
/// refuse or rewrite it), the helper-kind check, the permission judgement
/// for a call whose outward effect the code could not decide, the tool
/// under its own timeout, then the post-execute hooks. Its result event
/// goes out as it completes.
async fn run_call(
    cx: &RoundContext<'_>,
    mut ctx: ToolContext,
    pool: Arc<tokio::sync::Semaphore>,
    idx: usize,
    mut tc: ai::ToolCall,
) -> Ran {
    let RoundContext {
        scope,
        tools,
        providers,
        concurrency,
        hooks,
        user_prompt,
        workflow_mode,
        decide,
        active_task,
        turn_mode,
        side_trace,
        ..
    } = *cx;
    let RunToolScope { sessions, tx, session_id, run_cwd, .. } = *scope;
    let _slot = pool.acquire_owned().await;

    // tool.pre_execute filter hooks: may refuse the call, rewrite its
    // input, or attach a note that rides after the tool's own output.
    let mut refusal: Option<String> = None;
    let mut pre_hook_note: Option<String> = None;
    if hooks.has_subscribers("tool.pre_execute") {
        let payload = serde_json::to_vec(&crate::hooks::ToolPreExecutePayload {
            tool_name: tc.name.clone(),
            input: tc.input.clone(),
            session_id: session_id.to_string(),
            tool_use_id: tc.id.clone(),
            cwd: run_cwd.map(str::to_string).unwrap_or_default(),
            agent_id: session_id.strip_prefix("subagent:").map(|_| session_id.to_string()),
        })
        .unwrap_or_default();
        let (result, _handled) = hooks.apply_filter("tool.pre_execute", payload).await;
        if let Ok(resp) = serde_json::from_slice::<crate::hooks::ToolPreExecuteResponse>(&result) {
            if resp.blocked {
                refusal = Some(resp.blocked_message.unwrap_or_else(|| "Blocked by plugin hook".into()));
            } else {
                pre_hook_note = resp.note;
                if let Some(mutated_input) = resp.input {
                    tc.input = mutated_input;
                }
            }
        }
    }

    // Every gate below judges the call as it will run: the tool settles an
    // inferred action or resource here (after any hook rewrote the input).
    tc.input = tools.normalize_input(&tc.name, std::mem::take(&mut tc.input)).await;
    let target = tools.target(&tc.name, &tc.input).await;

    // A helper's kind and depth, whatever shape the call arrives in.
    if refusal.is_none()
        && let (Some(mode), Some(t)) = (turn_mode, &target)
        && let Err(refused) = crate::harness::delegation::permits(mode, t)
    {
        refusal = Some(refused);
    }
    if let Some(msg) = refusal {
        let result = ToolResult::error(msg);
        let _ = tx
            .send(StreamEvent {
                payload: None,
                provenance: None,
                event_type: StreamEventType::ToolResult,
                text: result.content.clone(),
                tool_call: Some(tc.clone()),
                error: Some(result.content.clone()),
                usage: None,
                rate_limit: None,
                widgets: None,
                provider_metadata: None,
                stop_reason: None,
                image_url: None,
            })
            .await;
        return Ran { idx, tc, target, result, duration_ms: None, hook_noted: false };
    }

    // The permission judgement (permissions::judgement): a call whose
    // outward effect the code could not decide (whether it publishes, or
    // speaks for the owner outside) is asked about, Jev first, the aux
    // classifier for what it didn't settle. The verdict rides to the
    // permission check, which records it (shadow) or acts on it (enforce).
    if let Some(t) = &target {
        let question = {
            let check = crate::harness::permissions::CheckCx {
                ctx: &ctx,
                input: &tc.input,
                grant: scope.grant,
                store: sessions.store(),
            };
            crate::harness::permissions::question_for(&check, t)
        };
        if let Some(mut q) = question {
            q.activity = tools.labels(&tc.name, &tc.input).await.0;
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
            ctx.judgement = crate::harness::permissions::judgement::judge_round(&judge, std::slice::from_ref(&q))
                .await
                .into_iter()
                .next();
        }
    }

    let _permit = concurrency.acquire_tool_permit().await;
    let input_str = tc.input.to_string();
    info!(tool = %tc.name, id = %tc.id, input = %truncate_str(&input_str, 500), "executing tool");
    // Each tool's own timeout; the loop has none of its own.
    let budget = tools.execution_timeout(&tc.name, &tc.input).await;
    let started = std::time::Instant::now();
    ctx.tool_call_id = tc.id.clone();
    ctx.parked = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let parked = ctx.parked.clone();
    let run = tools.execute(&ctx, &tc.name, tc.input.clone());
    let mut result = match budget {
        None => run.await,
        Some(budget) => match run_within_budget(budget, parked, run).await {
            Some(r) => r,
            None => ToolResult::error(tool_timeout_text(&tc.name, budget)),
        },
    };
    let duration_ms = started.elapsed().as_millis() as u64;
    info!(tool = %tc.name, id = %tc.id, is_error = result.is_error, result = %truncate_str(&result.content, 300), "tool result");
    // What the result carries, the run has now read.
    if !result.taint.is_empty() {
        scope.run_taint.lock().unwrap().extend(result.taint.iter().copied());
    }
    if let Some(note) = pre_hook_note {
        result.content.push_str("\n\n");
        result.content.push_str(&note);
    }
    let hook_noted = apply_post_tool_hooks(hooks, &tc, &mut result, session_id, run_cwd).await;
    let _ = tx
        .send(StreamEvent {
            payload: result.payload.clone(),
            provenance: None,
            event_type: StreamEventType::ToolResult,
            text: result.content.clone(),
            tool_call: Some(tc.clone()),
            error: if result.is_error { Some(result.content.clone()) } else { None },
            usage: None,
            rate_limit: None,
            // The call's wall-clock time rides in the widgets slot so the
            // live timeline and the reloaded one show the same duration; a
            // call parked on the owner names its ask, so the conversation
            // the run came from can carry the card.
            widgets: Some(match &result.parked_ask {
                Some(ask) => serde_json::json!({ "duration_ms": duration_ms, "parked_ask": ask }),
                None => serde_json::json!({ "duration_ms": duration_ms }),
            }),
            provider_metadata: None,
            stop_reason: None,
            image_url: result.image_url.clone(),
        })
        .await;
    Ran { idx, tc, target, result, duration_ms: Some(duration_ms), hook_noted }
}

/// A shell command that IS a project check. Running one resets the edit
/// count the done gate watches, exactly as a post-tool hook verdict does.
/// Word-bounded so `rustc` is not `tsc`.
static CHECK_VERB_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(
        r"\b(?:cargo (?:test|check|clippy)|pytest|go (?:test|vet)|pnpm (?:check|test|build)|npm (?:test|run)|npx tsc|tsc|vitest|jest|ruff|make (?:test|check))\b",
    )
    .expect("CHECK_VERB_RE is a literal")
});

pub(crate) fn is_check_command(command: &str) -> bool {
    CHECK_VERB_RE.is_match(command)
}

/// What the last desktop act reported, cut to what a reply must agree with:
/// its first line (what was done), the screen header, and the first lines of
/// the element list.
pub(crate) fn desktop_evidence(result: &str) -> String {
    let mut lines = result.lines().filter(|l| !l.trim().is_empty());
    let mut out: Vec<&str> = lines.by_ref().take(2).collect();
    out.extend(lines.take_while(|l| !l.starts_with("Coordinates are")).take(12));
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use types::permissions::{CallEffects, RuleField, Target};

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
            subject: None,
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

    use Status::{Done, Queued, Running};

    /// While the reply streams, safe calls start as they arrive; an unsafe
    /// call waits and holds every call after it.
    #[test]
    fn while_streaming_only_safe_calls_start_and_an_unsafe_one_holds_the_rest() {
        assert_eq!(startable(&[(Queued, true), (Queued, true)], true), [0, 1]);
        assert_eq!(startable(&[(Running, true), (Queued, true)], true), [1]);
        assert!(startable(&[(Queued, false), (Queued, true)], true).is_empty());
        assert_eq!(startable(&[(Queued, true), (Queued, false), (Queued, true)], true), [0]);
    }

    /// After the reply: consecutive safe calls run together, anything else
    /// runs alone, and order is kept.
    #[test]
    fn after_the_reply_unsafe_calls_run_alone_and_in_order() {
        assert!(startable(&[(Running, true), (Queued, false), (Queued, true)], false).is_empty());
        assert_eq!(startable(&[(Done, true), (Queued, false), (Queued, true)], false), [1]);
        assert!(startable(&[(Done, true), (Running, false), (Queued, true)], false).is_empty());
        assert_eq!(startable(&[(Done, false), (Done, false), (Queued, true), (Queued, true), (Queued, false)], false), [2, 3]);
    }

    /// A run of safe calls starts at once however long; the pool, not the
    /// queue, keeps ten running.
    #[test]
    fn a_run_of_safe_calls_starts_at_once() {
        let calls: Vec<(Status, bool)> = (0..23).map(|_| (Queued, true)).collect();
        assert_eq!(startable(&calls, false), (0..23).collect::<Vec<_>>());
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

}
