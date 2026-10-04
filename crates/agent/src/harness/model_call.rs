//! One provider call with its retry, overflow and output-cutoff ladders.
//! Moved here from `runner.rs` (WP1.1): `run_loop` builds each step's
//! request and hands it to `call_model`; the cutoff, empty-reply and
//! lost-tool-call retries it runs after a reply live here too.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{RwLock, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use ai::{ChatRequest, Provider, ProviderError, StreamEvent, StreamEventType};

use crate::concurrency::ConcurrencyController;
use crate::dedupe::{self, DedupeCache};
use super::usage::RunState;
use crate::selector::ModelSelector;
use crate::session::SessionManager;

/// Max transient error retries before giving up.
const MAX_TRANSIENT_RETRIES: usize = 10;
/// Max retryable (provider/rate_limit/billing) retries before giving up.
const MAX_RETRYABLE_RETRIES: usize = 5;
/// Max reactive-compaction attempts when the provider rejects for context
/// overflow despite the local estimate saying we fit (single-shot reactive
/// compact + give-up, a guard against an auto-compaction death-spiral).
const MAX_OVERFLOW_RETRIES: usize = 2;
/// Tokens added to the local estimate when the provider refuses a request
/// as over the window.
const OVERFLOW_ESTIMATE_BUMP: usize = 20_000;
/// Max gap between stream events before the stream is declared wedged
/// (connection open, no tokens). A 90s idle watchdog, classified transient so
/// the normal retry/failover path re-issues the request.
// Generous for the same reason as the HTTP client's read_timeout: buffered
// tool-call arguments make healthy streams go silent for minutes. TCP
// keepalive surfaces dead sockets as read errors long before this fires.
const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(300);
/// Max recovery attempts when output is truncated by token limit.
const MAX_OUTPUT_RECOVERY_ATTEMPTS: usize = 3;
/// Default output token cap for LLM requests.
const DEFAULT_MAX_OUTPUT_TOKENS: i32 = 16_384;
/// Escalated output token cap after a max_tokens truncation.
const ESCALATED_MAX_OUTPUT_TOKENS: i32 = 65_536;
/// Empty replies retried before the turn gives up with "(empty)".
pub(crate) const MAX_EMPTY_CONTENT_RETRIES: usize = 3;
/// Replies in a row that write a call out as text before the turn stops
/// asking for the call and ends on what the reply said.
const MAX_TEXT_CALL_RETRIES: usize = 2;
/// How long a cancel waits for a `cancel_is_async` provider's stream to
/// actually end (its own cancel handshake, e.g. the linked provider's ACP
/// `session/cancel` round trip) before giving up on it and freeing the slot
/// anyway. Longer than that provider's own cancel timeout, so its stream
/// ends on its own terms first.
const CANCEL_ACK_GRACE: Duration = Duration::from_secs(15);

/// What the owner reads when the model he picked refuses the request outright.
/// The raw upstream text ("Parameter 'temperature'=0.699… is not supported for
/// …") tells him nothing he can act on, so lead with the decision he can
/// change and keep the provider's words underneath for whoever needs them.
pub fn model_refusal_notice(model: &str, detail: &str) -> String {
    let name = model.rsplit('/').next().unwrap_or(model);
    let named = if name.is_empty() {
        "The model this employee is set to".to_string()
    } else {
        format!("The model this employee is set to ({name})")
    };
    format!(
        "{named} turned this request down, and would answer the same way every \
         time, so I stopped instead of retrying. Pick a different model under \
         Settings → General → Model and send this again.\n\nWhat the provider \
         said: {detail}"
    )
}

/// Where the owner reads why a request was blocked and what to do about it.
pub const BLOCKED_REQUESTS_URL: &str = "https://neboai.com/help/blocked-requests";

/// What the owner reads when the provider's answer is final: a request
/// every route's content filter refused says what to do about it, with the
/// page that explains it as a plain URL (a phone shows the text as is);
/// anything else reads as the gateway wrote it.
fn final_words(code: &str, message: &str) -> String {
    if code == ai::CONTENT_FILTERED {
        return format!(
            "This request couldn't be completed. Something earlier in this \
             conversation may be what's blocked. Running /compact often helps. \
             Learn more: {BLOCKED_REQUESTS_URL}"
        );
    }
    message.to_string()
}

/// Retry backoff: exponential 500ms × 2^(n−1) capped at 32s, plus 0–25% jitter.
/// An explicit provider Retry-After wins outright.
fn retry_backoff(attempt: usize, retry_after_secs: Option<u64>) -> Duration {
    if let Some(secs) = retry_after_secs {
        return Duration::from_secs(secs);
    }
    let exp = attempt.saturating_sub(1).min(6) as u32; // 500ms × 2^6 = 32s cap
    let base_ms = 500u64 << exp;
    // ponytail: clock-nanos jitter instead of a rand dependency
    let jitter_ms = base_ms
        * (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as u64 % 250)
            .unwrap_or(0))
        / 1000;
    Duration::from_millis(base_ms + jitter_ms)
}

/// What the owner reads when the connection to the model kept failing.
fn could_not_connect(selector: &ModelSelector, model: &str) -> String {
    let name = selector
        .get_model_info(model)
        .map(|m| m.display_name)
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| model.rsplit('/').next().unwrap_or(model).to_string());
    if name.is_empty() {
        "Could not connect. Try again.".to_string()
    } else {
        format!("Could not connect to {name}. Try again.")
    }
}

/// Pick a non-gateway provider when available.  Falls back to the default
/// provider (which may be Janus) only when no other option exists.  This
/// prevents background operations (memory extraction, compaction,
/// summarisation) from burning Janus credits when a CLI or direct-API
/// provider is loaded. Never the linked provider (`ai::default_provider`).
pub(crate) fn prefer_non_gateway(providers: &[Arc<dyn Provider>]) -> Option<Arc<dyn Provider>> {
    providers
        .iter()
        .find(|p| p.retryable() && p.id() != "janus")
        .cloned()
        .or_else(|| ai::default_provider(providers))
}

/// Resolve the (provider, model) for auxiliary side-tasks — chat title
/// generation, memory extraction, tool-batch summaries, and the auto-continue
/// judge.  Honors `task_routing.aux` ("provider/model") from models.yaml when
/// set AND that provider is loaded; returns `None` otherwise so each caller
/// silently keeps its existing selection.  Aux routing must never make a
/// side-task fail that would have succeeded before.
pub(crate) fn resolve_aux(
    cfg: &config::ModelsConfig,
    providers: &[Arc<dyn Provider>],
) -> Option<(Arc<dyn Provider>, String)> {
    let spec = cfg.task_routing.as_ref().map(|tr| tr.aux.as_str())?;
    if spec.is_empty() {
        return None;
    }
    let (provider_id, model) = spec.split_once('/')?;
    // Background work runs on a chat model only: an embedding or audio
    // model named as the aux route is passed over.
    let (capabilities, kind) = cfg
        .providers
        .get(provider_id)
        .and_then(|models| models.iter().find(|m| m.id == model))
        .map(|m| (m.capabilities.clone(), m.kind.clone()))
        .unwrap_or_default();
    if !crate::selector::is_chat_model(model, &capabilities, &kind) {
        return None;
    }
    let provider = providers.iter().find(|p| p.id() == provider_id)?.clone();
    Some((provider, model.to_string()))
}

/// What the model call carries from one step of a turn to the next: the
/// failover position, the retry counters and the output-cap escalation.
#[derive(Debug, Default)]
pub struct CallState {
    pub transient_retries: usize,
    pub retryable_retries: usize,
    pub overflow_retries: usize,
    pub output_recovery_attempts: usize,
    pub output_escalated: bool,
    /// Provider said the model stopped to call tools but the stream carried no
    /// parsed tool calls (payload lost between proxy and parser). Retried, not
    /// trusted — ending the turn silently strands the user mid-task.
    pub lost_toolcall_retries: usize,
    pub empty_content_retries: usize,
    /// Replies in a row, since the last real call, that wrote a call out as
    /// text instead of making it.
    pub text_call_retries: usize,
    /// Janus provider metadata for tool stickiness — echoed back in subsequent requests
    pub sticky_metadata: Option<HashMap<String, String>>,
}

impl CallState {
    /// The output cap for the next request.
    pub fn max_output_tokens(&self) -> i32 {
        if self.output_escalated {
            ESCALATED_MAX_OUTPUT_TOKENS
        } else {
            DEFAULT_MAX_OUTPUT_TOKENS
        }
    }
}

/// One request to the model, with what the call runs against.
pub(crate) struct ModelCall<'a> {
    pub request: ChatRequest,
    pub providers: &'a RwLock<Vec<Arc<dyn Provider>>>,
    pub selector: &'a ModelSelector,
    pub concurrency: &'a ConcurrencyController,
    /// Whose call this is: the owner's turn is served before queued work.
    pub priority: crate::concurrency::Priority,
    pub sessions: &'a SessionManager,
    pub cancel: &'a CancellationToken,
    pub tx: &'a mpsc::Sender<StreamEvent>,
    pub session_id: &'a str,
    pub step: usize,
    /// When the step began (telemetry).
    pub step_started: std::time::Instant,
    pub selected_provider_id: &'a str,
    pub selected_model: &'a str,
    /// The model the owner set, named in a refusal.
    pub model_override: &'a str,
    /// The compaction threshold, logged beside the context usage.
    pub context_limit: usize,
    /// Issues this run's tool credential, for a provider that runs tools
    /// itself over /agent/mcp (the CLI providers). Revoked when the call ends.
    pub tool_credential:
        Option<&'a (dyn Fn() -> crate::tool_credentials::CredentialGuard + Send + Sync)>,
    /// Each tool call as its input completes in the stream, for the tool
    /// executor to start while the reply streams. Dropped when the call
    /// returns, which tells the executor the stream is over.
    pub tool_calls_out: mpsc::UnboundedSender<ai::ToolCall>,
    /// The turn's text segments: a tool call closing one sends its verdict.
    pub folds: &'a mut super::text_fold::TurnFolds,
    /// The last row the request was built from, stamped on a partial reply
    /// a stop cuts short, as on every reply (`conversation::HEARD_THROUGH`):
    /// a message that landed during the call reads after it, unanswered.
    pub heard_through: Option<&'a str>,
    /// Every tool the run knows by name, declared or not yet loaded: a call
    /// to one written out as text is cut from the reply
    /// (`reminders::NoteFence`).
    pub tool_names: Vec<String>,
}

/// One content block of a reply, in stream order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Block {
    /// Coalesced text, with its verdict once a tool call in this reply
    /// closed it.
    Text(Option<super::text_fold::Fold>),
    /// The tool call at this index.
    Tool(usize),
}

/// What a call came back with.
pub(crate) enum CallOutcome {
    /// The model's reply. A stream error the retries could not clear has
    /// already been shown to the owner and rides in `stream_error`.
    Reply(ModelReply),
    /// Take the step again.
    Retry(RetryWhy),
    /// Cancelled waiting for the permit or during the stream.
    Cancelled,
    /// Cancelled while backing off before a retry.
    CancelledInBackoff,
    /// Stream retries ran out; the owner has been told and the loop stops.
    Exhausted,
    /// The call failed for good; the turn ends with this error.
    Failed(String),
}

/// Why a call is taken again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RetryWhy {
    /// The provider said the request is over the window: compact first.
    Overflow,
    /// A failover, a backoff or a reconnect: the same request again.
    Transient,
    /// The connection dropped mid-reply; the partial reply is stored and the
    /// next call resumes it.
    StreamCut,
}

/// The model's reply.
pub(crate) struct ModelReply {
    pub text: String,
    pub tool_calls: Vec<ai::ToolCall>,
    pub stop: Option<String>,
    pub stream_error: Option<String>,
    /// The order of content blocks.
    pub block_order: Vec<Block>,
    /// The provider that answered.
    pub provider: Arc<dyn Provider>,
    /// The reply's thinking blocks, in order, and the model that wrote them
    /// ("provider/model").
    pub thinking: Vec<ai::ThinkingBlock>,
    pub thinking_model: String,
    /// The tool the reply wrote a call to out as text, where the reply was
    /// cut (`reminders::NoteFence`): that call never ran.
    pub text_call: Option<String>,
}

/// After a cancel, waits (bounded by [`CANCEL_ACK_GRACE`]) for a
/// `cancel_is_async` provider's stream to end on its own — its cancel
/// handshake with the runtime it does not control — before the caller frees
/// the slot. The events themselves are not needed here: they were already
/// read up to the point the cancel fired, and nothing after it reaches the
/// owner. Returns once the stream ends (a `Done` event or the channel
/// closes) or the grace period runs out, whichever comes first.
async fn drain_after_cancel(rx: &mut mpsc::Receiver<StreamEvent>) {
    let deadline = tokio::time::Instant::now() + CANCEL_ACK_GRACE;
    loop {
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Some(event)) if event.event_type != StreamEventType::Done => continue,
            _ => return,
        }
    }
}

/// Make one call, streaming its events on `call.tx`. `state` takes the
/// usage, the overflow correction and the quota warning.
pub(crate) async fn call_model(call: ModelCall<'_>, st: &mut CallState, state: &mut RunState) -> CallOutcome {
    let ModelCall {
        request: chat_req,
        providers,
        selector,
        concurrency,
        priority,
        sessions,
        cancel: cancel_token,
        tx,
        session_id,
        step: iteration,
        step_started: t_iter_start,
        selected_provider_id,
        selected_model,
        model_override,
        context_limit,
        tool_credential,
        tool_calls_out,
        folds,
        heard_through,
        tool_names,
    } = call;

    // Acquire LLM permit before provider call (blocks if at capacity)
    let t_permit_start = std::time::Instant::now();
    // `biased`: a stop that already happened wins over a free permit, a
    // stream that opened and an event already buffered, so a stopped turn
    // never sends another request or runs what it streamed.
    let llm_permit = tokio::select! {
        biased;
        _ = cancel_token.cancelled() => {
            info!(session_id, "run cancelled waiting for LLM permit");
            return CallOutcome::Cancelled;
        }
        permit = concurrency.acquire_llm_permit(priority) => permit,
    };
    // A 429 on this call reports the round its permit was granted in, so
    // one wave of rejections halves the pool once.
    let permit_round = llm_permit.round();
    let permit_wait_ms = t_permit_start.elapsed().as_millis() as u64;
    if permit_wait_ms > 5 {
        info!(
            ms = permit_wait_ms,
            iteration, session_id, "[telemetry] LLM permit wait"
        );
    }

    // Snapshot provider from lock, then release before I/O
    let provider = {
        let prov_lock = providers.read().await;
        if prov_lock.is_empty() {
            return CallOutcome::Failed("No AI providers available".to_string());
        }

        // The chosen model's provider, every attempt: a failed call is
        // retried on the same model, never handed to another provider.
        let Some(idx) = pick_provider(&prov_lock, selected_provider_id) else {
            return CallOutcome::Failed(no_provider(selected_provider_id));
        };

        info!(
            iteration,
            session_id,
            provider_idx = idx,
            provider_id = prov_lock[idx].id(),
            selected_provider_id,
            selected_model = %selected_model,
            provider_count = prov_lock.len(),
            message_count = chat_req.messages.len(),
            tool_count = chat_req.tools.len(),
            enable_thinking = chat_req.enable_thinking,
            "sending request to provider"
        );
        prov_lock[idx].clone()
    };

    // The chosen model's provider isn't loaded (a bot without the gateway):
    // the first provider serves with its own default model.
    let mut chat_req = chat_req;
    if provider.id() != selected_provider_id {
        chat_req.model = String::new();
    }

    // A CLI provider's tool calls come back over /agent/mcp carrying this
    // credential, and execute as this run until the call returns.
    let _tool_credential = match tool_credential {
        Some(issue) if provider.handles_tools() => {
            let guard = issue();
            chat_req.tool_credential = Some(guard.token().to_string());
            Some(guard)
        }
        _ => None,
    };

    let t_stream_start = std::time::Instant::now();
    let stream_result = tokio::select! {
        biased;
        _ = cancel_token.cancelled() => {
            info!(session_id, "run cancelled during provider.stream() call");
            return CallOutcome::Cancelled;
        }
        result = provider.stream(&chat_req) => result,
    };
    let stream_connect_ms = t_stream_start.elapsed().as_millis() as u64;

    let mut rx = match stream_result {
        Ok(rx) => {
            info!(
                iteration,
                session_id,
                connect_ms = stream_connect_ms,
                "[telemetry] provider stream connected"
            );
            rx
        }
        Err(e) => {
            // Every branch below retries after a wait or ends the turn:
            // a waiting call must not sit on a slot other calls could use.
            drop(llm_permit);
            // Deduplicate repeated errors to avoid log spam
            let err_str = format!("{}", e);
            let fingerprint = dedupe::fingerprint_error(&err_str);
            static ERROR_DEDUP: std::sync::OnceLock<DedupeCache> = std::sync::OnceLock::new();
            let dedup = ERROR_DEDUP.get_or_init(DedupeCache::default);
            if dedup.check(&fingerprint) {
                debug!(iteration, session_id, "deduplicated provider error");
            } else {
                warn!(iteration, session_id, error = %e, "provider error");
            }

            // A provider that will not take the call again (the linked
            // provider): its answer is final, in its own words.
            if !provider.retryable() {
                return CallOutcome::Failed(err_str);
            }

            if ai::is_context_overflow(&e) {
                st.overflow_retries += 1;
                if st.overflow_retries > MAX_OVERFLOW_RETRIES {
                    return CallOutcome::Failed(format!(
                        "Context overflow persisted after {} compaction attempts: {}",
                        MAX_OVERFLOW_RETRIES, e
                    ));
                }
                // The provider disagreed with our local estimate. Make the
                // retry state-changing: raise the estimate so the next step's
                // checkpoint trigger fires instead of re-sending the
                // identical request.
                state.estimate_correction += OVERFLOW_ESTIMATE_BUMP;
                warn!(
                    overflow_retries = st.overflow_retries,
                    correction = state.estimate_correction,
                    "context overflow: forcing reactive compaction"
                );
                return CallOutcome::Retry(RetryWhy::Overflow);
            }

            if ai::is_transient_error(&e) {
                st.transient_retries += 1;
                if st.transient_retries > MAX_TRANSIENT_RETRIES {
                    warn!(session_id, error = %e, "the connection kept failing; ending the turn");
                    return CallOutcome::Failed(could_not_connect(selector, selected_model));
                }
                // A dropped connection is nobody's model failing: the same
                // model is asked again after a backoff, never a different
                // model. A reconnect is
                // silent: the owner's screen still shows the turn working.
                tokio::select! {
                    _ = cancel_token.cancelled() => return CallOutcome::CancelledInBackoff,
                    _ = tokio::time::sleep(retry_backoff(st.transient_retries, None)) => {}
                }
                return CallOutcome::Retry(RetryWhy::Transient);
            }

            // A 429 slows the whole bot, not just this call: the pool
            // halves, and this call waits as long as the provider asked.
            let mut retry_after = None;
            if let ProviderError::RateLimit { retry_after_secs } = &e {
                concurrency.report_rate_limit(permit_round);
                retry_after = *retry_after_secs;
            }

            if e.is_retryable() {
                st.retryable_retries += 1;
                if st.retryable_retries > MAX_RETRYABLE_RETRIES {
                    warn!(
                        session_id,
                        retries = MAX_RETRYABLE_RETRIES,
                        error = %e,
                        "retries ran out; ending the turn"
                    );
                    return CallOutcome::Failed(could_not_connect(selector, selected_model));
                }
                // Busy or rate-limited: the same model again after the
                // wait the provider asked for.
                tokio::select! {
                    _ = cancel_token.cancelled() => return CallOutcome::CancelledInBackoff,
                    _ = tokio::time::sleep(retry_backoff(st.retryable_retries, retry_after)) => {}
                }
                return CallOutcome::Retry(RetryWhy::Transient);
            }

            // Final: in the gateway's words, written for the owner; the
            // details are already in the log above.
            return CallOutcome::Failed(match &e {
                ProviderError::Api { code, message, .. } => final_words(code, message),
                _ => e.to_string(),
            });
        }
    };

    // Process stream (retry counters are reset after stream produces content)
    let mut assistant_content = String::new();
    let mut tool_calls: Vec<ai::ToolCall> = Vec::new();
    let mut stream_error: Option<String> = None;
    // The provider marked the error final: a retry gets the same answer.
    // Its code ("" when it sent none) picks the words the owner reads.
    let mut stream_error_final = false;
    let mut stream_error_code = String::new();
    let mut last_retry_after: Option<u64> = None;
    let mut stop_reason: Option<String> = None;
    let mut t_first_token: Option<std::time::Instant> = None;
    // Track the order of content blocks (text vs tool) for correct rehydration.
    let mut block_order: Vec<Block> = Vec::new();
    let mut thinking: Vec<ai::ThinkingBlock> = Vec::new();
    // CLI providers run multi-turn tool loops — save each turn incrementally.
    let cli_incremental = provider.handles_tools();
    // The reply never carries a note in Nebo's own format, or a call
    // written out as text, past this point.
    let mut notes = super::reminders::NoteFence::new(tool_names.clone());

    loop {
        let mut event = tokio::select! {
            biased;
            _ = cancel_token.cancelled() => {
                info!(session_id, "run cancelled during LLM stream");
                // A provider whose stop is a round trip to a runtime it does
                // not control (the linked provider's ACP `session/cancel`)
                // is not done just because this token fired: waiting here
                // for its stream to actually end is what keeps the next
                // turn from sending a second prompt while the first is
                // still being told to stop (never two prompts in flight to
                // one linked session).
                if provider.cancel_is_async() {
                    drain_after_cancel(&mut rx).await;
                }
                // Best-effort: save whatever content we accumulated before cancellation
                if !assistant_content.is_empty() || !tool_calls.is_empty() {
                    let tc_json = if !tool_calls.is_empty() {
                        serde_json::to_string(&tool_calls).ok()
                    } else {
                        None
                    };
                    let heard = heard_through
                        .map(|id| serde_json::json!({ super::conversation::HEARD_THROUGH: id }).to_string());
                    if let Err(e) = sessions.append_message(
                        session_id, "assistant", &assistant_content,
                        tc_json.as_deref(), None, heard.as_deref(),
                    ) {
                        warn!(session_id = %session_id, error = %e, "failed to save partial assistant message on cancel");
                    } else {
                        info!(session_id, content_len = assistant_content.len(), tool_count = tool_calls.len(), "saved partial assistant message before cancel");
                    }
                }
                return CallOutcome::Cancelled;
            }
            ev = tokio::time::timeout(STREAM_IDLE_TIMEOUT, rx.recv()) => match ev {
                Ok(Some(e)) => e,
                Ok(None) => break,
                Err(_) => {
                    warn!(
                        iteration,
                        session_id,
                        "stream idle timeout: no events for {}s",
                        STREAM_IDLE_TIMEOUT.as_secs()
                    );
                    stream_error = Some(format!(
                        "stream idle timeout: no events for {}s",
                        STREAM_IDLE_TIMEOUT.as_secs()
                    ));
                    break;
                }
            }
        };
        if t_first_token.is_none() {
            t_first_token = Some(std::time::Instant::now());
            let ttft = t_stream_start.elapsed().as_millis() as u64;
            let iter_elapsed = t_iter_start.elapsed().as_millis() as u64;
            info!(
                ttft_ms = ttft,
                iter_total_ms = iter_elapsed,
                iteration,
                session_id,
                provider = %provider.id(),
                model = %chat_req.model,
                "[telemetry] first token received"
            );
        }
        match event.event_type {
            StreamEventType::Text => {
                // CLI incremental save: text after tool calls = new turn.
                // Flush the previous turn's content + tool calls to DB.
                if cli_incremental && !tool_calls.is_empty() {
                    let tc_json = serde_json::to_string(&tool_calls).ok();
                    if let Err(e) = sessions.append_message(
                        session_id,
                        "assistant",
                        &assistant_content,
                        tc_json.as_deref(),
                        None,
                        None,
                    ) {
                        warn!(session_id = %session_id, error = %e, "failed to save CLI turn to DB");
                    } else {
                        debug!(
                            session_id,
                            content_len = assistant_content.len(),
                            tool_count = tool_calls.len(),
                            "saved CLI turn incrementally"
                        );
                    }
                    assistant_content.clear();
                    tool_calls.clear();
                    block_order.clear();
                    notes = super::reminders::NoteFence::new(tool_names.clone());
                }
                event.text = notes.push(&event.text);
                if !event.text.is_empty() {
                    show_text(event, &mut assistant_content, folds, &mut block_order, tx).await;
                }
            }
            StreamEventType::Thinking => {
                let _ = tx.send(event).await;
            }
            StreamEventType::ThinkingBlock => {
                info!(session_id, "received thinking block");
                thinking.extend(event.block());
            }
            StreamEventType::ToolCall => {
                // The call ends the text before it: what the fence held
                // there never became a note.
                let held = notes.finish();
                if !held.is_empty() {
                    show_text(StreamEvent::text(held), &mut assistant_content, folds, &mut block_order, tx).await;
                }
                if let Some(ref tc) = event.tool_call {
                    info!(session_id, tool = %tc.name, tool_id = %tc.id, "tool call received");
                    tool_calls.push(tc.clone());
                    // The call closes the text before it: its verdict goes
                    // out first, so the clients fold it before the call row.
                    if let Some((segment, fold)) = folds.tool_call() {
                        if let Some(Block::Text(verdict)) = block_order.last_mut() {
                            *verdict = Some(fold);
                        }
                        let _ = tx.send(StreamEvent::text_verdict(segment, fold.as_str())).await;
                    }
                    block_order.push(Block::Tool(tool_calls.len() - 1));
                    // A CLI provider runs its own tools over /agent/mcp.
                    if !cli_incremental {
                        let _ = tool_calls_out.send(tc.clone());
                    }
                }
                let _ = tx.send(event).await;
            }
            StreamEventType::Error => {
                warn!(session_id, error = ?event.error, "stream error event");
                stream_error = event.error.clone();
                stream_error_final = event.is_non_retryable();
                stream_error_code = event.error_code().to_string();
                // Don't forward to user yet — classify after stream ends
            }
            StreamEventType::Usage => {
                if let Some(ref mut usage) = event.usage {
                    state.total_input_tokens += usage.input_tokens;
                    state.total_output_tokens += usage.output_tokens;
                    state.total_cache_read_tokens += usage.cache_read_input_tokens;
                    state.total_cache_creation_tokens += usage.cache_creation_input_tokens;
                    state.cost_microdollars += usage.cost_microdollars.unwrap_or(0);
                    usage.overhead_tokens = state.system_overhead_tokens as i32;

                    // Calibrate the local token estimate against ground truth.
                    // Context actually sent = input + cache tokens (with prompt
                    // caching, input_tokens alone excludes the cached bulk).
                    let context_actual = (usage.input_tokens
                        + usage.cache_creation_input_tokens
                        + usage.cache_read_input_tokens)
                        as usize;
                    if context_actual > 0 && state.last_request_estimate > 0 {
                        let conversation_actual =
                            context_actual.saturating_sub(state.system_overhead_tokens);
                        state.estimate_correction =
                            conversation_actual.saturating_sub(state.last_request_estimate);
                    }
                    info!(
                        session_id,
                        iteration,
                        context_tokens = context_actual,
                        cache_read_tokens = usage.cache_read_input_tokens,
                        estimated = state.last_request_estimate + state.system_overhead_tokens,
                        limit = context_limit,
                        "context usage"
                    );
                }
                let _ = tx.send(event).await;
            }
            StreamEventType::RateLimit => {
                if let Some(ref meta) = event.rate_limit {
                    last_retry_after = meta.retry_after_secs;

                    // Check Janus session/weekly usage and generate quota warning at >80%
                    let mut warnings = Vec::new();
                    if let (Some(limit), Some(remaining)) =
                        (meta.session_limit_credits, meta.session_remaining_credits)
                        && limit > 0
                    {
                        let used_pct =
                            ((limit.saturating_sub(remaining)) as f64 / limit as f64) * 100.0;
                        if used_pct >= 80.0 {
                            warnings.push(format!(
                                "Session usage at {:.0}% (resets at {})",
                                used_pct,
                                meta.session_reset_at.as_deref().unwrap_or("unknown"),
                            ));
                        }
                    }
                    if let (Some(limit), Some(remaining)) =
                        (meta.weekly_limit_credits, meta.weekly_remaining_credits)
                        && limit > 0
                    {
                        let used_pct =
                            ((limit.saturating_sub(remaining)) as f64 / limit as f64) * 100.0;
                        if used_pct >= 80.0 {
                            warnings.push(format!(
                                "Weekly usage at {:.0}% (resets at {})",
                                used_pct,
                                meta.weekly_reset_at.as_deref().unwrap_or("unknown"),
                            ));
                        }
                    }
                    if !warnings.is_empty() {
                        let warning_text = warnings.join(". ");
                        state.quota_warning = Some(warning_text.clone());

                        // Forward the rate limit event with warning text once per run
                        // so chat_dispatch can broadcast a quota_warning WS event.
                        if !state.quota_warning_sent {
                            state.quota_warning_sent = true;
                            let _ = tx
                                .send(StreamEvent {
                                    payload: None,
                                    provenance: None,
                                    event_type: StreamEventType::RateLimit,
                                    text: warning_text,
                                    tool_call: None,
                                    error: None,
                                    usage: None,
                                    rate_limit: event.rate_limit.clone(),
                                    widgets: None,
                                    provider_metadata: None,
                                    stop_reason: None,
                                    image_url: None,
                                    more_files: Vec::new(),
                                })
                                .await;
                        }
                    }
                }
            }
            StreamEventType::Done => {
                // Capture stop reason for max output recovery
                if event.stop_reason.is_some() {
                    stop_reason = event.stop_reason.clone();
                }
                // Capture provider metadata for Janus tool stickiness
                if let Some(meta) = event.provider_metadata {
                    st.sticky_metadata = Some(meta);
                }
            }
            StreamEventType::ToolResult => {
                // CLI providers (handles_tools) execute tools themselves via
                // MCP and stream the results back; relay so chat_dispatch can
                // broadcast tool_result. API providers never emit this event —
                // the runner synthesizes it after executing tools itself.
                let _ = tx.send(event).await;
            }
            StreamEventType::AskRequest => {
                // A provider that runs tools itself relays its runtime's own
                // question (the linked provider), registered on the run's ask
                // channels like the `ask` tool's; relay so chat_dispatch
                // parks and broadcasts ask_request. API providers never emit
                // this event.
                let _ = tx.send(event).await;
            }
            StreamEventType::ApprovalRequest
            | StreamEventType::ControlNotice
            | StreamEventType::TextVerdict
            | StreamEventType::TakenIn => {
                // Approval/ControlNotice: only sent by the runner, not
                // received from a provider.
            }
            StreamEventType::ToolSummary => {
                // Tool execution summary — relay to parent for display.
                let _ = tx.send(event).await;
            }
            StreamEventType::SubagentStart
            | StreamEventType::SubagentProgress
            | StreamEventType::SubagentComplete => {
                // Forwarded from sub-agent orchestrator via stream_tx; relay to parent.
                let _ = tx.send(event).await;
            }
        }
    }

    let held = notes.finish();
    if !held.is_empty() {
        show_text(StreamEvent::text(held), &mut assistant_content, folds, &mut block_order, tx).await;
    }
    let text_call = match notes.cut() {
        Some(super::reminders::Cut::Note) => {
            warn!(session_id, iteration, "the reply opened a note in Nebo's own format: it ends there, unshown and unstored");
            None
        }
        Some(super::reminders::Cut::Call(tool)) => {
            warn!(session_id, iteration, tool = %tool, "the reply wrote a tool call out as text: it ends there, unshown and unstored, and the call did not run");
            Some(tool.clone())
        }
        None => None,
    };

    // Drop LLM permit now that stream is complete
    drop(llm_permit);

    // Reset retry counters only when stream actually produced content
    if stream_error.is_none() && (!assistant_content.is_empty() || !tool_calls.is_empty()) {
        st.transient_retries = 0;
        st.retryable_retries = 0;
        // Note: estimate_correction is NOT reset — the compaction that
        // recovered from overflow must stay in effect for the rest of the run.
        st.overflow_retries = 0;
    }

    // Report success or rate limit to concurrency controller
    if stream_error.is_none() {
        concurrency.report_success();
    }

    // Handle stream errors — classify and retry (matches Go runner logic)
    if let Some(ref err_msg) = stream_error {
        warn!("stream error: {}", err_msg);
        let err = ProviderError::Stream(err_msg.clone());
        let reason = ai::classify_error_reason(&err);
        // A refusal the provider will repeat verbatim. Both ladders below
        // re-send the identical payload, so letting one through costs the
        // owner the whole retry budget and tells him nothing new.
        let deterministic = ai::is_deterministic_request_error(&err);
        // The provider said so itself (the gateway's `retryable: false`, a
        // content filter every route refused): neither ladder runs.
        let asks_again = provider.retryable() && !deterministic && !stream_error_final;

        // Mid-stream cutoff continuation: partial text the user already
        // watched stream would die with the retry below (which skips the
        // normal end-of-iteration save), so the retried call would
        // regenerate — repeating or restarting what was already delivered.
        // Persist the partial turn (same append pathway as the cancel save in
        // the stream loop) and steer the retry to resume in place. Partial
        // tool calls are NOT saved — an assistant tool_use with no tool
        // result is an invalid sequence for every provider.
        // Called only on the branches that actually retry; the
        // non-retryable fall-through persists via the normal save. The
        // retry itself is silent: the turn goes on and resumes in place.
        // Returns whether the stream was cut mid-reply: the retry is then a
        // resume, and the caller says so on the next call.
        let save_partial = || {
            if assistant_content.is_empty() && tool_calls.is_empty() {
                return RetryWhy::Transient;
            }
            if !assistant_content.is_empty()
                && let Err(e) = sessions.append_message(
                    session_id,
                    "assistant",
                    &assistant_content,
                    None,
                    None,
                    None,
                )
            {
                warn!(session_id = %session_id, error = %e, "failed to save partial assistant message before stream retry");
            }
            RetryWhy::StreamCut
        };

        // Layer 1: Transient errors (connection reset, timeout, EOF)
        if asks_again && ai::is_transient_error(&err) {
            st.transient_retries += 1;
            if st.transient_retries <= MAX_TRANSIENT_RETRIES {
                let why = save_partial();
                tokio::select! {
                    _ = cancel_token.cancelled() => return CallOutcome::CancelledInBackoff,
                    _ = tokio::time::sleep(retry_backoff(st.transient_retries, None)) => {}
                }
                return CallOutcome::Retry(why);
            }
        }

        // Report rate limit to concurrency controller
        if reason == "rate_limit" {
            concurrency.report_rate_limit(permit_round);
        }

        // Layer 2: Retryable errors (rate_limit, billing, provider errors)
        let is_retryable = asks_again
            && (err.is_retryable()
                || reason == "rate_limit"
                || reason == "billing"
                || reason == "provider"
                || reason == "timeout");
        if is_retryable {
            st.retryable_retries += 1;
            if st.retryable_retries > MAX_RETRYABLE_RETRIES {
                warn!(
                    session_id,
                    retries = MAX_RETRYABLE_RETRIES,
                    error = %err_msg,
                    "retries ran out; ending the turn"
                );
                let _ = tx
                    .send(StreamEvent::error(could_not_connect(selector, selected_model)))
                    .await;
                return CallOutcome::Exhausted;
            }
            warn!(
                reason,
                retryable_retries = st.retryable_retries,
                "retryable stream error, retrying the same model"
            );
            let why = save_partial();
            tokio::select! {
                _ = cancel_token.cancelled() => return CallOutcome::CancelledInBackoff,
                _ = tokio::time::sleep(retry_backoff(st.retryable_retries, last_retry_after)) => {}
            }
            return CallOutcome::Retry(why);
        }

        // Layer 3: Non-retryable — send error to user
        if deterministic {
            warn!(
                model = model_override,
                "provider refused the request outright; not retrying"
            );
        }
        let _ = tx
            .send(StreamEvent::error(if stream_error_final {
                final_words(&stream_error_code, err_msg)
            } else if deterministic {
                model_refusal_notice(model_override, err_msg)
            } else {
                err_msg.clone()
            }))
            .await;
    }

    let stream_total_ms = t_stream_start.elapsed().as_millis() as u64;
    let iter_total_ms = t_iter_start.elapsed().as_millis() as u64;
    info!(
        session_id,
        iteration,
        content_len = assistant_content.len(),
        tool_call_count = tool_calls.len(),
        has_error = stream_error.is_some(),
        stream_ms = stream_total_ms,
        iter_ms = iter_total_ms,
        "[telemetry] stream complete"
    );

    let thinking_model = format!("{}/{}", provider.id(), chat_req.model);
    CallOutcome::Reply(ModelReply {
        text: assistant_content,
        tool_calls,
        stop: stop_reason,
        stream_error,
        block_order,
        provider,
        thinking,
        thinking_model,
        text_call,
    })
}

/// The provider a call goes to, by index: the provider the model names.
/// A named provider that is not registered, or no name (a CLI bot's empty
/// model), falls back to the first one that takes calls it did not build
/// (`ai::default_provider`). A provider that answers only for the agent it
/// is addressed to (the linked one) is never that fallback and never stood
/// in for: a call named for it that it cannot take, or a call with nowhere
/// to go, is `None`.
fn pick_provider(providers: &[Arc<dyn Provider>], selected: &str) -> Option<usize> {
    if let Some(idx) = providers.iter().position(|p| p.id() == selected) {
        return Some(idx);
    }
    if selected == ai::providers::linked::ID {
        return None;
    }
    providers.iter().position(|p| p.retryable())
}

/// What the owner reads when no provider can take the call.
fn no_provider(selected: &str) -> String {
    if selected.is_empty() {
        "No AI provider is connected.".to_string()
    } else {
        format!("No AI provider is connected for {selected}.")
    }
}

/// What a reply without tool calls asks of the next step.
pub(crate) enum StepRetry {
    /// Take the step again as it is.
    Same,
    /// Continue in place: the reply stands and the next call resumes it
    /// (`TurnEvent::CutoffResume`).
    Resume,
}

/// Hand a piece of the reply's text the note fence let through to the
/// owner's stream, the reply being built and the turn's fold tracker.
async fn show_text(
    event: StreamEvent,
    assistant_content: &mut String,
    folds: &mut super::text_fold::TurnFolds,
    block_order: &mut Vec<Block>,
    tx: &mpsc::Sender<StreamEvent>,
) {
    assistant_content.push_str(&event.text);
    folds.text(&event.text);
    // Coalesce consecutive text events into one block
    if !matches!(block_order.last(), Some(Block::Text(_))) {
        block_order.push(Block::Text(None));
    }
    let _ = tx.send(event).await;
}

/// The output-cap ladder for a reply the output cap cut off: first a retry
/// at the escalated cap, then up to `MAX_OUTPUT_RECOVERY_ATTEMPTS`
/// continuations. A reply that was not cut off resets both.
pub(crate) fn output_cutoff(
    st: &mut CallState,
    stop_reason: Option<&str>,
    iteration: usize,
    session_id: &str,
) -> Option<StepRetry> {
    // Output token escalation: on first truncation, retry with a higher cap
    // before falling through to the multi-attempt continuation recovery.
    if (stop_reason == Some("length") || stop_reason == Some("max_tokens")) && !st.output_escalated
    {
        info!(
            iteration,
            session_id,
            "output truncated at {}K tokens, retrying with {}K",
            DEFAULT_MAX_OUTPUT_TOKENS / 1024,
            ESCALATED_MAX_OUTPUT_TOKENS / 1024,
        );
        st.output_escalated = true;
        return Some(StepRetry::Same);
    }

    // Max output tokens recovery: if response was truncated, force continuation
    if (stop_reason == Some("length") || stop_reason == Some("max_tokens"))
        && st.output_recovery_attempts < MAX_OUTPUT_RECOVERY_ATTEMPTS
    {
        st.output_recovery_attempts += 1;
        info!(
            iteration,
            session_id,
            attempt = st.output_recovery_attempts,
            "max output tokens recovery"
        );
        return Some(StepRetry::Resume);
    }
    // Reset recovery counter and escalation flag on successful non-truncated completion
    if stop_reason != Some("length") && stop_reason != Some("max_tokens") {
        st.output_recovery_attempts = 0;
        st.output_escalated = false;
    }
    None
}

/// Empty response retry: true while retries remain (the step is taken
/// again), false once `MAX_EMPTY_CONTENT_RETRIES` are spent.
pub(crate) fn retry_empty_reply(st: &mut CallState, iteration: usize, session_id: &str) -> bool {
    if st.empty_content_retries < MAX_EMPTY_CONTENT_RETRIES {
        st.empty_content_retries += 1;
        warn!(
            iteration,
            session_id,
            retry = st.empty_content_retries,
            "empty response — retrying"
        );
        return true;
    }
    false
}

/// A reply that wrote a call out as text: true while the turn goes on for
/// the call (the next step is told it didn't run), false once
/// `MAX_TEXT_CALL_RETRIES` replies in a row have done it.
pub(crate) fn retry_text_call(st: &mut CallState, iteration: usize, session_id: &str) -> bool {
    if st.text_call_retries < MAX_TEXT_CALL_RETRIES {
        st.text_call_retries += 1;
        warn!(iteration, session_id, retry = st.text_call_retries, "the reply wrote a call out as text; the turn goes on for the call");
        return true;
    }
    false
}

/// Contradictory stop: the provider says the model stopped TO CALL TOOLS,
/// but no tool calls were parsed from the stream — the payload was lost
/// in transit (observed live with Janus: stop_reason="tool_calls",
/// tool_call_count=0). Ending the turn there strands the user with only
/// the preamble text; the step is taken again instead.
pub(crate) fn lost_tool_calls(
    st: &mut CallState,
    stop_reason: Option<&str>,
    tool_calls: &[ai::ToolCall],
    iteration: usize,
    session_id: &str,
) -> bool {
    let stop_says_tools = matches!(stop_reason, Some("tool_calls" | "tool_use"));
    if stop_says_tools && tool_calls.is_empty() && st.lost_toolcall_retries < 2 {
        st.lost_toolcall_retries += 1;
        warn!(
            iteration,
            session_id,
            attempt = st.lost_toolcall_retries,
            "stop_reason says tool_calls but none were parsed — retrying iteration"
        );
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The live failure: "Nebo 1 Pro" resolved to a model that rejects the
    /// temperature every Nebo chat turn sends, so the owner's employee died
    /// on a 400 the retry ladder could never clear. What he reads has to name
    /// the setting he can change, not the parameter he has never heard of.
    #[test]
    fn a_refused_model_names_the_setting_the_owner_can_change() {
        let notice = model_refusal_notice(
            "janus/nebo-1-pro",
            "Provider dashscope error: OpenAI API error (HTTP 400): \
             [invalid_parameter_error] Parameter 'temperature'=0.7 is not \
             supported for kimi-k3 model.",
        );
        assert!(notice.contains("nebo-1-pro"), "names the model: {notice}");
        assert!(!notice.contains("janus/"), "not the wire id: {notice}");
        assert!(
            notice.contains("Settings → General → Model"),
            "says where to fix it: {notice}"
        );
        // The provider's words stay available underneath, never the headline.
        assert!(notice.contains("kimi-k3"));
        assert!(notice.find("turned this request down").unwrap() < notice.find("kimi-k3").unwrap());
        // No model set at all still reads as a sentence.
        assert!(
            model_refusal_notice("", "boom")
                .starts_with("The model this employee is set to turned")
        );
    }

    /// The overflow-compaction retry counter is reset only when the model
    /// actually produced content, never by another recovery's continue: a
    /// compaction that could not recover must not be retried by a nudge.
    #[test]
    fn has_attempted_compact_survives_a_continuation() {
        let source = include_str!("model_call.rs");
        let resets: Vec<usize> = source
            .lines()
            .enumerate()
            .filter(|(_, l)| l.trim() == "st.overflow_retries = 0;")
            .map(|(i, _)| i)
            .collect();
        assert_eq!(resets.len(), 1, "exactly one reset site");
        let window: String = source
            .lines()
            .skip(resets[0].saturating_sub(6))
            .take(6)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            window.contains("stream_error.is_none() && (!assistant_content.is_empty() || !tool_calls.is_empty())"),
            "the reset is gated on real content:\n{window}"
        );
    }

    /// Minimal provider stub for resolve_aux tests (only id() matters).
    struct StubProvider(&'static str);

    #[async_trait::async_trait]
    impl Provider for StubProvider {
        fn id(&self) -> &str {
            self.0
        }
        async fn stream(&self, _req: &ChatRequest) -> Result<ai::EventReceiver, ProviderError> {
            Err(ProviderError::Request("stub".into()))
        }
    }

    /// A provider that answers only for the agent it is addressed to.
    struct Addressed;

    #[async_trait::async_trait]
    impl Provider for Addressed {
        fn id(&self) -> &str {
            "linked"
        }
        fn retryable(&self) -> bool {
            false
        }
        async fn stream(&self, _req: &ChatRequest) -> Result<ai::EventReceiver, ProviderError> {
            Err(ProviderError::Request("stub".into()))
        }
    }

    /// A call goes to the provider its model names. When that provider is
    /// not registered it goes to the first one that takes any call, never to
    /// the linked provider; with nowhere else to go it goes nowhere, and a
    /// linked call is never stood in for.
    #[test]
    fn a_missing_provider_never_lands_on_the_linked_one() {
        let only_linked: Vec<Arc<dyn Provider>> = vec![Arc::new(Addressed)];
        assert_eq!(pick_provider(&only_linked, "janus"), None);
        assert_eq!(pick_provider(&only_linked, ""), None);
        assert_eq!(pick_provider(&only_linked, "linked"), Some(0));

        let both: Vec<Arc<dyn Provider>> = vec![Arc::new(Addressed), Arc::new(StubProvider("anthropic"))];
        assert_eq!(pick_provider(&both, "janus"), Some(1));
        assert_eq!(pick_provider(&both, ""), Some(1));
        assert_eq!(pick_provider(&both, "linked"), Some(0));

        let no_linked: Vec<Arc<dyn Provider>> = vec![Arc::new(StubProvider("anthropic"))];
        assert_eq!(pick_provider(&no_linked, "linked"), None);
        assert_eq!(no_provider("janus"), "No AI provider is connected for janus.");
    }

    /// A background call (summary, compaction, review) on a NeboAI-only bot
    /// goes to Janus, never to the linked provider listed after it; with
    /// only the linked provider there is nowhere to send it.
    #[test]
    fn a_background_call_never_goes_to_the_linked_provider() {
        let janus_only: Vec<Arc<dyn Provider>> = vec![Arc::new(StubProvider("janus")), Arc::new(Addressed)];
        assert_eq!(prefer_non_gateway(&janus_only).unwrap().id(), "janus");
        let with_key: Vec<Arc<dyn Provider>> =
            vec![Arc::new(Addressed), Arc::new(StubProvider("anthropic")), Arc::new(StubProvider("janus"))];
        assert_eq!(prefer_non_gateway(&with_key).unwrap().id(), "anthropic");
        assert_eq!(ai::default_provider(&with_key).unwrap().id(), "anthropic");
        let only_linked: Vec<Arc<dyn Provider>> = vec![Arc::new(Addressed)];
        assert!(prefer_non_gateway(&only_linked).is_none());
        assert!(ai::default_provider(&only_linked).is_none());
    }

    fn aux_config(aux: &str) -> config::ModelsConfig {
        config::ModelsConfig {
            version: "1.0".into(),
            defaults: None,
            task_routing: Some(config::models::TaskRouting {
                vision: String::new(),
                audio: String::new(),
                reasoning: String::new(),
                code: String::new(),
                general: String::new(),
                aux: aux.to_string(),
                fallbacks: std::collections::HashMap::new(),
            }),
            lane_routing: None,
            aliases: vec![],
            providers: std::collections::HashMap::new(),
            cli_providers: vec![],
        }
    }

    #[test]
    fn test_resolve_aux_unset_returns_none() {
        let providers: Vec<Arc<dyn Provider>> = vec![Arc::new(StubProvider("anthropic"))];
        // Empty aux field → fallback.
        assert!(resolve_aux(&aux_config(""), &providers).is_none());
        // No task_routing at all → fallback.
        let mut cfg = aux_config("");
        cfg.task_routing = None;
        assert!(resolve_aux(&cfg, &providers).is_none());
    }

    #[test]
    fn test_resolve_aux_provider_missing_returns_none() {
        let providers: Vec<Arc<dyn Provider>> = vec![Arc::new(StubProvider("anthropic"))];
        let cfg = aux_config("openai/gpt-4o-mini");
        assert!(resolve_aux(&cfg, &providers).is_none());
    }

    #[test]
    fn test_resolve_aux_malformed_spec_returns_none() {
        // Bare model id without a provider prefix cannot be routed → fallback.
        let providers: Vec<Arc<dyn Provider>> = vec![Arc::new(StubProvider("anthropic"))];
        assert!(resolve_aux(&aux_config("gpt-4o-mini"), &providers).is_none());
    }

    /// An embedding model named as the aux route is never used.
    #[test]
    fn test_resolve_aux_never_an_embedding_model() {
        let providers: Vec<Arc<dyn Provider>> = vec![Arc::new(StubProvider("janus"))];
        assert!(resolve_aux(&aux_config("janus/nebo-embed-small"), &providers).is_none());
    }

    #[test]
    fn test_resolve_aux_set_and_available_routes() {
        let providers: Vec<Arc<dyn Provider>> = vec![
            Arc::new(StubProvider("anthropic")),
            Arc::new(StubProvider("openai")),
        ];
        let cfg = aux_config("openai/gpt-4o-mini");
        let (provider, model) = resolve_aux(&cfg, &providers).expect("aux route should resolve");
        assert_eq!(provider.id(), "openai");
        assert_eq!(model, "gpt-4o-mini");
    }

    /// A provider whose cancel is a round trip to a runtime it does not
    /// control (`cancel_is_async`): it keeps streaming until it has heard
    /// the cancel token, then only ends its stream after `ack_lag` — the
    /// stand-in for an ACP `session/cancel` round trip.
    struct SlowToAck {
        ack_lag: Duration,
        cancel_is_async: bool,
    }

    #[async_trait::async_trait]
    impl Provider for SlowToAck {
        fn id(&self) -> &str {
            "slow-to-ack"
        }
        fn cancel_is_async(&self) -> bool {
            self.cancel_is_async
        }
        async fn stream(&self, req: &ChatRequest) -> Result<ai::EventReceiver, ProviderError> {
            let (tx, rx) = mpsc::channel(4);
            let cancel = req.cancel_token.clone().unwrap_or_default();
            let ack_lag = self.ack_lag;
            tokio::spawn(async move {
                let _ = tx.send(StreamEvent::text("Working")).await;
                cancel.cancelled().await;
                tokio::time::sleep(ack_lag).await;
                let _ = tx.send(StreamEvent::error("Cancelled".to_owned())).await;
                let _ = tx.send(StreamEvent::done()).await;
            });
            Ok(rx)
        }
    }

    /// Everything `call_model` needs besides the provider, built fresh so
    /// each test owns its store file.
    struct Rig {
        _dir: tempfile::TempDir,
        store: Arc<db::Store>,
        selector: ModelSelector,
        concurrency: ConcurrencyController,
    }

    impl Rig {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let store = Arc::new(db::Store::new(dir.path().join("t.db").to_str().unwrap()).expect("store"));
            Self {
                _dir: dir,
                store,
                selector: ModelSelector::new(Default::default()),
                concurrency: ConcurrencyController::new(Some(4)),
            }
        }
    }

    /// A stop on a provider that needs an ack round trip to actually end
    /// (the linked provider's stand-in here) must not let `call_model`
    /// declare the slot free before that round trip lands: the bug this
    /// closes let the runner start a second prompt on the still-live ACP
    /// session the moment the local cancel token fired, before the agent
    /// had even been told to stop.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancel_waits_for_a_cancel_is_async_provider_to_actually_end() {
        for (cancel_is_async, min_elapsed) in [(true, Duration::from_millis(150)), (false, Duration::ZERO)] {
            let rig = Rig::new();
            let sessions = SessionManager::new(rig.store.clone());
            let providers: RwLock<Vec<Arc<dyn Provider>>> = RwLock::new(vec![Arc::new(SlowToAck {
                ack_lag: Duration::from_millis(200),
                cancel_is_async,
            })]);
            let token = CancellationToken::new();
            let request = ChatRequest {
                model: "slow-to-ack".to_string(),
                cancel_token: Some(token.clone()),
                ..ChatRequest::new(ai::RequestTrace::new("agent_turn"))
            };
            let (tx, _rx_out) = mpsc::channel(16);
            let (tool_calls_out, _tc_rx) = mpsc::unbounded_channel();
            let mut folds = crate::harness::text_fold::TurnFolds::default();
            let mut st = CallState::default();
            let mut state = RunState::default();

            let cancel_after = token.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(20)).await;
                cancel_after.cancel();
            });

            let started = std::time::Instant::now();
            let outcome = call_model(
                ModelCall {
                    request,
                    providers: &providers,
                    selector: &rig.selector,
                    concurrency: &rig.concurrency,
                    priority: crate::concurrency::Priority::Owner,
                    sessions: &sessions,
                    cancel: &token,
                    tx: &tx,
                    session_id: "s1",
                    step: 0,
                    step_started: std::time::Instant::now(),
                    selected_provider_id: "slow-to-ack",
                    selected_model: "slow-to-ack",
                    model_override: "slow-to-ack",
                    context_limit: 100_000,
                    tool_credential: None,
                    tool_calls_out,
                    folds: &mut folds,
                    heard_through: None,
                    tool_names: vec![],
                },
                &mut st,
                &mut state,
            )
            .await;
            let elapsed = started.elapsed();

            assert!(matches!(outcome, CallOutcome::Cancelled), "cancel_is_async={cancel_is_async}");
            assert!(
                elapsed >= min_elapsed,
                "cancel_is_async={cancel_is_async}: returned after {elapsed:?}, wanted at least {min_elapsed:?}"
            );
            if cancel_is_async {
                assert!(
                    elapsed < CANCEL_ACK_GRACE,
                    "the provider's own ack ended the stream well inside the grace period: {elapsed:?}"
                );
            }
        }
    }

    /// The gateway's answer when every route's content filter refused.
    const FILTERED: &str = "This request couldn't be completed. Try rephrasing it.";

    /// A provider whose every call ends the same way: one stream event, or
    /// a refusal before the stream opens.
    enum Answers {
        Streams(StreamEvent),
        Refuses(ProviderError),
    }

    #[async_trait::async_trait]
    impl Provider for Answers {
        fn id(&self) -> &str {
            "janus"
        }
        async fn stream(&self, _req: &ChatRequest) -> Result<ai::EventReceiver, ProviderError> {
            let event = match self {
                Answers::Streams(event) => event.clone(),
                Answers::Refuses(e) => return Err(e.clone()),
            };
            let (tx, rx) = mpsc::channel(4);
            tokio::spawn(async move {
                let _ = tx.send(event).await;
                let _ = tx.send(StreamEvent::done()).await;
            });
            Ok(rx)
        }
    }

    /// One call to `provider` from state `st`: what it returned, and the
    /// error text the owner was shown.
    async fn call_once(provider: Answers, st: &mut CallState) -> (CallOutcome, Vec<String>) {
        let rig = Rig::new();
        let sessions = SessionManager::new(rig.store.clone());
        let providers: RwLock<Vec<Arc<dyn Provider>>> = RwLock::new(vec![Arc::new(provider)]);
        let token = CancellationToken::new();
        let request = ChatRequest {
            model: "nebo-1-flash".to_string(),
            cancel_token: Some(token.clone()),
            ..ChatRequest::new(ai::RequestTrace::new("agent_turn"))
        };
        let (tx, mut rx_out) = mpsc::channel(16);
        let (tool_calls_out, _tc_rx) = mpsc::unbounded_channel();
        let mut folds = crate::harness::text_fold::TurnFolds::default();
        let mut state = RunState::default();
        let outcome = call_model(
            ModelCall {
                request,
                providers: &providers,
                selector: &rig.selector,
                concurrency: &rig.concurrency,
                priority: crate::concurrency::Priority::Owner,
                sessions: &sessions,
                cancel: &token,
                tx: &tx,
                session_id: "s1",
                step: 0,
                step_started: std::time::Instant::now(),
                selected_provider_id: "janus",
                selected_model: "janus/nebo-1-flash",
                model_override: "janus/nebo-1-flash",
                context_limit: 100_000,
                tool_credential: None,
                tool_calls_out,
                folds: &mut folds,
                heard_through: None,
                tool_names: vec![],
            },
            st,
            &mut state,
        )
        .await;
        drop(tx);
        let mut shown = Vec::new();
        while let Ok(event) = rx_out.try_recv() {
            if let Some(error) = event.error {
                shown.push(error);
            }
        }
        (outcome, shown)
    }

    /// What the owner reads when every route's content filter refused the
    /// request: what to do about it, and the page that explains it as a
    /// plain URL, so a phone showing the text as is can open it.
    const BLOCKED: &str = "This request couldn't be completed. Something earlier in this \
        conversation may be what's blocked. Running /compact often helps. \
        Learn more: https://neboai.com/help/blocked-requests";

    /// The live failure (09-30): the gateway said a content filter refused
    /// the request and that a retry gets the same answer, and the runner
    /// asked five more times, then told the owner the service was
    /// "temporarily unavailable". The gateway's answer is final: it is
    /// shown once, and nothing is asked again.
    #[tokio::test]
    async fn a_content_filtered_stream_is_shown_once_and_never_retried() {
        let mut st = CallState::default();
        let refused = StreamEvent::error(FILTERED).non_retryable(ai::CONTENT_FILTERED);
        let (outcome, shown) = call_once(Answers::Streams(refused), &mut st).await;
        match outcome {
            CallOutcome::Reply(reply) => assert_eq!(reply.stream_error.as_deref(), Some(FILTERED)),
            _ => panic!("a final error ends the call with the reply, not a retry"),
        }
        assert_eq!((st.retryable_retries, st.transient_retries), (0, 0), "nothing was asked again");
        assert_eq!(shown, vec![BLOCKED.to_string()], "shown once, saying what to do");
    }

    /// Any other final answer is shown once, as the gateway wrote it.
    #[tokio::test]
    async fn a_final_stream_error_reads_as_the_gateway_wrote_it() {
        let mut st = CallState::default();
        let refused = StreamEvent::error("That model isn't available.").non_retryable("MODEL_NOT_SUPPORTED");
        let (outcome, shown) = call_once(Answers::Streams(refused), &mut st).await;
        assert!(matches!(outcome, CallOutcome::Reply(_)));
        assert_eq!(st.retryable_retries, 0);
        assert_eq!(shown, vec!["That model isn't available.".to_string()]);
    }

    /// The same final answers before the stream opens (a plain HTTP error):
    /// no retry, no prefix of ours.
    #[tokio::test]
    async fn a_final_refusal_before_the_stream_reads_plainly() {
        let mut st = CallState::default();
        let refused = ProviderError::Api {
            code: ai::CONTENT_FILTERED.into(),
            message: FILTERED.into(),
            retryable: false,
        };
        let (outcome, _) = call_once(Answers::Refuses(refused), &mut st).await;
        match outcome {
            CallOutcome::Failed(text) => assert_eq!(text, BLOCKED),
            _ => panic!("a final refusal fails the call"),
        }
        assert_eq!(st.retryable_retries, 0);

        let refused = ProviderError::Api {
            code: "MODEL_NOT_SUPPORTED".into(),
            message: "That model isn't available.".into(),
            retryable: false,
        };
        let (outcome, _) = call_once(Answers::Refuses(refused), &mut st).await;
        match outcome {
            CallOutcome::Failed(text) => assert_eq!(text, "That model isn't available."),
            _ => panic!("a final refusal fails the call"),
        }
    }

    /// Retries that run out end the turn in plain words — what happened and
    /// what to do — never "temporarily unavailable", never the upstream's
    /// own text (that goes to the log).
    #[tokio::test]
    async fn exhausted_retries_say_plainly_what_happened() {
        let could_not = "Could not connect to nebo-1-flash. Try again.";

        let mut st = CallState { retryable_retries: MAX_RETRYABLE_RETRIES, ..CallState::default() };
        let busy = StreamEvent::error("vendor-x said: upstream busy");
        let (outcome, shown) = call_once(Answers::Streams(busy), &mut st).await;
        assert!(matches!(outcome, CallOutcome::Exhausted));
        assert_eq!(shown, vec![could_not.to_string()]);

        let mut st = CallState { retryable_retries: MAX_RETRYABLE_RETRIES, ..CallState::default() };
        let busy = ProviderError::Api {
            code: "PROVIDER_UNAVAILABLE".into(),
            message: "vendor-x said: upstream busy".into(),
            retryable: true,
        };
        let (outcome, _) = call_once(Answers::Refuses(busy), &mut st).await;
        match outcome {
            CallOutcome::Failed(text) => assert_eq!(text, could_not),
            _ => panic!("retries that ran out fail the call"),
        }
    }
}
