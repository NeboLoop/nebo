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
use crate::runner::RunState;
use crate::selector::ModelSelector;
use crate::session::SessionManager;
use crate::steering;

/// Max transient error retries before giving up.
const MAX_TRANSIENT_RETRIES: usize = 10;
/// Max retryable (provider/rate_limit/billing) retries before giving up.
const MAX_RETRYABLE_RETRIES: usize = 5;
/// Max reactive-compaction attempts when the provider rejects for context
/// overflow despite the local estimate saying we fit (single-shot reactive
/// compact + give-up, a guard against an auto-compaction death-spiral).
const MAX_OVERFLOW_RETRIES: usize = 2;
/// Consecutive overloaded (529) errors before falling back to a cheaper model.
#[allow(dead_code)] // reserved for overload fallback logic
const MAX_OVERLOADS_BEFORE_FALLBACK: usize = 3;
/// Max gap between stream events before the stream is declared wedged
/// (connection open, no tokens). A 90s idle watchdog, classified transient so
/// the normal retry/failover path re-issues the request.
// Generous for the same reason as the HTTP client's read_timeout: buffered
// tool-call arguments make healthy streams go silent for minutes. TCP
// keepalive surfaces dead sockets as read errors long before this fires.
const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(300);
/// A first token this long in coming gets a status line, repeated at the
/// same interval until it arrives, so a slow model is never minutes of
/// silence (live 2026-09-03: 294 s with nothing shown).
const SLOW_FIRST_TOKEN: Duration = Duration::from_secs(30);
/// Max recovery attempts when output is truncated by token limit.
const MAX_OUTPUT_RECOVERY_ATTEMPTS: usize = 3;
/// Default output token cap for LLM requests.
const DEFAULT_MAX_OUTPUT_TOKENS: i32 = 16_384;
/// Escalated output token cap after a max_tokens truncation.
const ESCALATED_MAX_OUTPUT_TOKENS: i32 = 65_536;
/// Empty replies retried before the turn gives up with "(empty)".
pub(crate) const MAX_EMPTY_CONTENT_RETRIES: usize = 3;

/// Status for a retry the owner would otherwise never see. With partial
/// text on screen the retry resumes it; before any text it is a fresh try.
pub fn retry_notice(had_partial: bool, retry: usize) -> String {
    if had_partial {
        "Connection dropped mid-response, reconnecting to resume where it left off.".to_string()
    } else {
        format!("The model connection dropped before it replied; retry {retry} starting.")
    }
}

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

pub fn slow_first_token_notice(waited_secs: u64) -> String {
    format!("Still waiting on the model, {waited_secs} seconds with no reply yet.")
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
    let provider = providers.iter().find(|p| p.id() == provider_id)?.clone();
    Some((provider, model.to_string()))
}

/// What the model call carries from one step of a turn to the next: the
/// failover position, the retry counters and the output-cap escalation.
#[derive(Debug, Default)]
pub struct CallState {
    /// Round-robin offset into the providers once a call has failed over.
    pub provider_idx: usize,
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
}

/// What a call came back with.
pub(crate) enum CallOutcome {
    /// The model's reply. A stream error the retries could not clear has
    /// already been shown to the owner and rides in `stream_error`.
    Reply(ModelReply),
    /// Take the step again: reactive compaction, failover or a reconnect.
    Retry,
    /// Cancelled waiting for the permit or during the stream.
    Cancelled,
    /// Cancelled while backing off before a retry.
    CancelledInBackoff,
    /// Stream retries ran out; the owner has been told and the loop stops.
    Exhausted,
    /// The call failed for good; the turn ends with this error.
    Failed(String),
}

/// The model's reply.
pub(crate) struct ModelReply {
    pub text: String,
    pub tool_calls: Vec<ai::ToolCall>,
    pub stop: Option<String>,
    pub stream_error: Option<String>,
    /// The order of content blocks: "text" (coalesced) or a tool index.
    pub block_order: Vec<(&'static str, Option<usize>)>,
    /// The provider that answered.
    pub provider: Arc<dyn Provider>,
}

/// Make one call, streaming its events on `call.tx`. `state` takes the
/// usage, the overflow correction and the quota warning; `pending_stream_reminders`
/// takes a cutoff continuation and is emptied once the call lands.
pub(crate) async fn call_model(
    call: ModelCall<'_>,
    st: &mut CallState,
    state: &mut RunState,
    pending_stream_reminders: &mut Vec<String>,
) -> CallOutcome {
    let ModelCall {
        request: chat_req,
        providers,
        selector,
        concurrency,
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
    } = call;

    // Acquire LLM permit before provider call (blocks if at capacity)
    let t_permit_start = std::time::Instant::now();
    let llm_permit = tokio::select! {
        _ = cancel_token.cancelled() => {
            info!(session_id, "run cancelled waiting for LLM permit");
            return CallOutcome::Cancelled;
        }
        permit = concurrency.acquire_llm_permit() => permit,
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

        let Some(idx) = pick_provider(&prov_lock, st.provider_idx, selected_provider_id) else {
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

    // If we fell through to a different provider (e.g. CLI after Janus rate limit),
    // clear the model so the fallback provider uses its own default.
    let mut chat_req = chat_req;
    if provider.id() != selected_provider_id {
        chat_req.model = String::new();
    }

    // Providers that never put images on the wire get them as text instead.
    // Decided here, after the fallback above, because a run that started on a
    // vision provider can land on a blind one mid-retry.
    if !provider.supports_vision() && chat_req.messages.iter().any(|m| m.images.is_some()) {
        crate::sidecar::describe_attached_images(provider.as_ref(), &mut chat_req).await;
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
                // retry state-changing: tighten thresholds by 20% of the
                // compaction budget so next iteration's stages actually
                // evict, instead of re-sending the identical request.
                let bump = state
                    .thresholds
                    .as_ref()
                    .map(|t| t.auto_compact / 5)
                    .unwrap_or(20_000);
                state.estimate_correction += bump;
                warn!(
                    overflow_retries = st.overflow_retries,
                    correction = state.estimate_correction,
                    "context overflow: forcing reactive compaction"
                );
                return CallOutcome::Retry;
            }

            if ai::is_transient_error(&e) {
                st.transient_retries += 1;
                selector.mark_failed(selected_model);
                if st.transient_retries > MAX_TRANSIENT_RETRIES {
                    return CallOutcome::Failed(format!("Too many transient errors: {}", e));
                }
                // The owner sees the retry, not a silent gap (voice said
                // "on it" and went quiet for five minutes, 2026-09-03).
                if tx
                    .send(StreamEvent::control_notice(
                        retry_notice(false, st.transient_retries),
                        "stream_reconnecting",
                    ))
                    .await
                    .is_err()
                {
                    debug!(session_id, "retry notice: receiver gone");
                }
                // Try next provider on transient error — but never
                // silently fall from CLI to Janus (burns Nebo credits).
                let prov_lock = providers.read().await;
                let rotation = rotation(&prov_lock);
                let prov_count = rotation.len();
                if prov_count > 1 {
                    let next_idx = rotation[(st.provider_idx + 1) % prov_count];
                    if prov_lock[next_idx].id() == "janus" {
                        drop(prov_lock);
                        return CallOutcome::Failed(format!(
                            "Provider error (no fallback to Janus): {}",
                            e
                        ));
                    }
                    drop(prov_lock);
                    st.provider_idx += 1;
                } else {
                    drop(prov_lock);
                }
                tokio::select! {
                    _ = cancel_token.cancelled() => return CallOutcome::CancelledInBackoff,
                    _ = tokio::time::sleep(retry_backoff(st.transient_retries, None)) => {}
                }
                return CallOutcome::Retry;
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
                selector.mark_failed(selected_model);
                if st.retryable_retries > MAX_RETRYABLE_RETRIES {
                    return CallOutcome::Failed(format!(
                        "Service temporarily unavailable after {} retries: {}",
                        MAX_RETRYABLE_RETRIES, e
                    ));
                }
                let prov_lock = providers.read().await;
                let rotation = rotation(&prov_lock);
                let prov_count = rotation.len();
                if prov_count > 1 {
                    let next_idx = rotation[(st.provider_idx + 1) % prov_count];
                    if prov_lock[next_idx].id() == "janus" {
                        drop(prov_lock);
                        return CallOutcome::Failed(format!(
                            "Provider error (no fallback to Janus): {}",
                            e
                        ));
                    }
                    drop(prov_lock);
                    st.provider_idx += 1;
                } else {
                    drop(prov_lock);
                }
                tokio::select! {
                    _ = cancel_token.cancelled() => return CallOutcome::CancelledInBackoff,
                    _ = tokio::time::sleep(retry_backoff(st.retryable_retries, retry_after)) => {}
                }
                return CallOutcome::Retry;
            }

            return CallOutcome::Failed(format!("Provider error: {}", e));
        }
    };

    // Process stream (retry counters are reset after stream produces content)
    let mut assistant_content = String::new();
    let mut tool_calls: Vec<ai::ToolCall> = Vec::new();
    let mut stream_error: Option<String> = None;
    let mut last_retry_after: Option<u64> = None;
    let mut stop_reason: Option<String> = None;
    let mut t_first_token: Option<std::time::Instant> = None;
    let mut slow_notice_at = tokio::time::Instant::now() + SLOW_FIRST_TOKEN;
    // Track the order of content blocks (text vs tool) for correct rehydration.
    // Each entry is either "text" (coalesced) or a tool index.
    let mut block_order: Vec<(&'static str, Option<usize>)> = Vec::new();
    // CLI providers run multi-turn tool loops — save each turn incrementally.
    let cli_incremental = provider.handles_tools();

    loop {
        let mut event = tokio::select! {
            _ = cancel_token.cancelled() => {
                info!(session_id, "run cancelled during LLM stream");
                // Best-effort: save whatever content we accumulated before cancellation
                if !assistant_content.is_empty() || !tool_calls.is_empty() {
                    let tc_json = if !tool_calls.is_empty() {
                        serde_json::to_string(&tool_calls).ok()
                    } else {
                        None
                    };
                    if let Err(e) = sessions.append_message(
                        session_id, "assistant", &assistant_content,
                        tc_json.as_deref(), None, None,
                    ) {
                        warn!(session_id = %session_id, error = %e, "failed to save partial assistant message on cancel");
                    } else {
                        info!(session_id, content_len = assistant_content.len(), tool_count = tool_calls.len(), "saved partial assistant message before cancel");
                    }
                }
                return CallOutcome::Cancelled;
            }
            _ = tokio::time::sleep_until(slow_notice_at), if t_first_token.is_none() => {
                let waited = t_stream_start.elapsed().as_secs();
                if tx
                    .send(StreamEvent::control_notice(
                        slow_first_token_notice(waited),
                        "slow_first_token",
                    ))
                    .await
                    .is_err()
                {
                    debug!(session_id, "slow first token notice: receiver gone");
                }
                slow_notice_at += SLOW_FIRST_TOKEN;
                continue;
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
                }
                assistant_content.push_str(&event.text);
                // Coalesce consecutive text events into one block
                if block_order.last().is_none_or(|b| b.0 != "text") {
                    block_order.push(("text", None));
                }
                let _ = tx.send(event).await;
            }
            StreamEventType::Thinking => {
                info!(session_id, "received thinking block");
                let _ = tx.send(event).await;
            }
            StreamEventType::ToolCall => {
                if let Some(ref tc) = event.tool_call {
                    info!(session_id, tool = %tc.name, tool_id = %tc.id, "tool call received");
                    tool_calls.push(tc.clone());
                    block_order.push(("tool", Some(tool_calls.len() - 1)));
                }
                let _ = tx.send(event).await;
            }
            StreamEventType::Error => {
                warn!(session_id, error = ?event.error, "stream error event");
                stream_error = event.error.clone();
                // Don't forward to user yet — classify after stream ends
            }
            StreamEventType::Usage => {
                if let Some(ref mut usage) = event.usage {
                    state.last_input_tokens = usage.input_tokens as usize;
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
            StreamEventType::ApprovalRequest => {
                // A provider that runs tools itself relays its runtime's own
                // approval prompt (the linked provider), registered on the
                // run's approval channels like the runner's own gate; relay
                // so chat_dispatch broadcasts approval_request. API
                // providers never emit this event.
                let _ = tx.send(event).await;
            }
            StreamEventType::AskRequest
            | StreamEventType::PlanApproval
            | StreamEventType::ControlNotice
            | StreamEventType::ContextStats => {
                // Ask/Plan/ControlNotice: only sent by runner, not received
                // from provider.
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

        // Mid-stream cutoff continuation: partial text the user already
        // watched stream would die with the retry below (which skips the
        // normal end-of-iteration save), so the retried call would
        // regenerate — repeating or restarting what was already delivered.
        // Persist the partial turn (same append pathway as the cancel save in
        // the stream loop) and steer the retry to resume in place. Partial
        // tool calls are NOT saved — an assistant tool_use with no tool
        // result is an invalid sequence for every provider.
        // Called only on the branches that actually retry; the
        // non-retryable fall-through persists via the normal save.
        // The user watched this text stream and then freeze mid-sentence.
        // Without a status line the dead bubble reads as the model giving
        // up; with one, the retry reads as what it is — a reconnect.
        let had_partial = !assistant_content.is_empty();
        let reconnect_notice = |retry: usize| {
            tx.send(StreamEvent::control_notice(
                retry_notice(had_partial, retry),
                "stream_reconnecting",
            ))
        };
        let mut queue_cutoff_continuation = || {
            if assistant_content.is_empty() && tool_calls.is_empty() {
                return;
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
            // A stream that dies while a tool call is in flight is almost
            // always killed by the call itself — one enormous streamed
            // argument (a whole document inline). A generic "continue"
            // makes the model re-emit the same giant call and die the same
            // way; name the cause and steer it to chunk the work instead.
            let reminder = if tool_calls.is_empty() {
                "Your previous response was cut off mid-stream by a \
                 connection error. Continue EXACTLY where you left off — \
                 do not repeat or restart."
                    .to_string()
            } else {
                let names: Vec<&str> = tool_calls.iter().map(|tc| tc.name.as_str()).collect();
                format!(
                    "Your previous response was cut off mid-stream while \
                     emitting a tool call ({}) — the call was NOT delivered. \
                     Oversized tool arguments are the usual cause. Do NOT \
                     retry one giant call: break the work into several \
                     smaller tool calls (write large files in pieces, edit \
                     one section at a time), then continue from where you \
                     stopped.",
                    names.join(", ")
                )
            };
            pending_stream_reminders.push(steering::wrap_system_reminder(&reminder));
        };

        // Layer 1: Transient errors (connection reset, timeout, EOF)
        if provider.retryable() && !deterministic && ai::is_transient_error(&err) {
            st.transient_retries += 1;
            if st.transient_retries <= MAX_TRANSIENT_RETRIES {
                queue_cutoff_continuation();
                if reconnect_notice(st.transient_retries).await.is_err() {
                    debug!(session_id, "retry notice: receiver gone");
                }
                let prov_count = rotation(&providers.read().await).len();
                if prov_count > 1 {
                    st.provider_idx += 1;
                }
                tokio::select! {
                    _ = cancel_token.cancelled() => return CallOutcome::CancelledInBackoff,
                    _ = tokio::time::sleep(retry_backoff(st.transient_retries, None)) => {}
                }
                return CallOutcome::Retry;
            }
        }

        // Report rate limit to concurrency controller
        if reason == "rate_limit" {
            concurrency.report_rate_limit(permit_round);
        }

        // Layer 2: Retryable errors (rate_limit, billing, provider errors)
        let is_retryable = provider.retryable()
            && !deterministic
            && (err.is_retryable()
                || reason == "rate_limit"
                || reason == "billing"
                || reason == "provider"
                || reason == "timeout");
        if is_retryable {
            st.retryable_retries += 1;
            if st.retryable_retries > MAX_RETRYABLE_RETRIES {
                let _ = tx
                    .send(StreamEvent::error(format!(
                        "Service temporarily unavailable after {} retries: {}",
                        MAX_RETRYABLE_RETRIES, err_msg
                    )))
                    .await;
                return CallOutcome::Exhausted;
            }
            warn!(
                reason,
                retryable_retries = st.retryable_retries,
                "retryable stream error, trying next provider"
            );
            queue_cutoff_continuation();
            if reconnect_notice(st.retryable_retries).await.is_err() {
                debug!(session_id, "retry notice: receiver gone");
            }
            let prov_count = rotation(&providers.read().await).len();
            if prov_count > 1 {
                st.provider_idx += 1;
            }
            tokio::select! {
                _ = cancel_token.cancelled() => return CallOutcome::CancelledInBackoff,
                _ = tokio::time::sleep(retry_backoff(st.retryable_retries, last_retry_after)) => {}
            }
            return CallOutcome::Retry;
        }

        // Layer 3: Non-retryable — send error to user
        if deterministic {
            warn!(
                model = model_override,
                "provider refused the request outright; not retrying"
            );
        }
        let _ = tx
            .send(StreamEvent::error(if deterministic {
                model_refusal_notice(model_override, err_msg)
            } else {
                err_msg.clone()
            }))
            .await;
    }
    // The call landed: its stream reminders are spent.
    pending_stream_reminders.clear();

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

    CallOutcome::Reply(ModelReply {
        text: assistant_content,
        tool_calls,
        stop: stop_reason,
        stream_error,
        block_order,
        provider,
    })
}

/// The provider a call goes to, by index. The first attempt goes to the
/// provider the model names; after a failure (`provider_idx` > 0) the call
/// rotates so it actually falls through to the next provider (e.g. a CLI
/// agent). A named provider that is not registered falls back to the first
/// one that takes calls it did not build. A provider that answers only for
/// the agent it is addressed to (the linked one) is never that fallback,
/// never rotated onto, and never stood in for: a call named for it that it
/// cannot take, or a call with nowhere to go, is `None`.
fn pick_provider(providers: &[Arc<dyn Provider>], provider_idx: usize, selected: &str) -> Option<usize> {
    let rotation = rotation(providers);
    if provider_idx > 0 && !rotation.is_empty() {
        return Some(rotation[provider_idx % rotation.len()]);
    }
    if !selected.is_empty() {
        if let Some(idx) = providers.iter().position(|p| p.id() == selected) {
            return Some(idx);
        }
        if selected == ai::providers::linked::ID {
            return None;
        }
    }
    rotation.first().copied()
}

/// What the owner reads when no provider can take the call.
fn no_provider(selected: &str) -> String {
    if selected.is_empty() {
        "No AI provider is connected.".to_string()
    } else {
        format!("No AI provider is connected for {selected}.")
    }
}

/// The providers a call may be rotated onto after a failure, by index: every
/// one that takes a call it did not build (see `Provider::retryable`).
fn rotation(providers: &[Arc<dyn Provider>]) -> Vec<usize> {
    providers
        .iter()
        .enumerate()
        .filter(|(_, p)| p.retryable())
        .map(|(i, _)| i)
        .collect()
}

/// What a reply without tool calls asks of the next step.
pub(crate) enum StepRetry {
    /// Take the step again as it is.
    Same,
    /// Take the step again with this reminder on the call.
    WithReminder(String),
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
        // Continuation rides the next call as an ephemeral reminder
        // after the (already persisted) truncated turn.
        return Some(StepRetry::WithReminder(steering::wrap_system_reminder(
            "Your previous response was cut off by the output token limit. \
             Resume directly from where you stopped — no recap, no apology. \
             If you had pending tool calls, make them now.",
        )));
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

/// Contradictory stop: the provider says the model stopped TO CALL TOOLS,
/// but no tool calls were parsed from the stream — the payload was lost
/// in transit (observed live with Janus: stop_reason="tool_calls",
/// tool_call_count=0). Ending the turn there strands the user with only
/// the preamble text; the step is retried with this reminder instead.
pub(crate) fn lost_tool_calls(
    st: &mut CallState,
    stop_reason: Option<&str>,
    tool_calls: &[ai::ToolCall],
    iteration: usize,
    session_id: &str,
) -> Option<String> {
    let stop_says_tools = matches!(stop_reason, Some("tool_calls" | "tool_use"));
    if stop_says_tools && tool_calls.is_empty() && st.lost_toolcall_retries < 2 {
        st.lost_toolcall_retries += 1;
        warn!(
            iteration,
            session_id,
            attempt = st.lost_toolcall_retries,
            "stop_reason says tool_calls but none were parsed — retrying iteration"
        );
        return Some(steering::wrap_system_reminder(
            "Your previous response ended as if calling tools, but no tool \
             calls arrived. Make the tool calls now — do not re-introduce \
             the task.",
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_notice_says_which_kind_of_retry() {
        assert!(retry_notice(true, 1).contains("resume"));
        let fresh = retry_notice(false, 3);
        assert!(fresh.contains("retry 3") && !fresh.contains("resume"));
        assert!(slow_first_token_notice(60).contains("60 seconds"));
    }

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

    /// Another employee's failed call rotates over the addressed provider,
    /// and it is never the default when no provider is named.
    #[test]
    fn rotation_skips_a_provider_that_takes_no_call_it_did_not_build() {
        let providers: Vec<Arc<dyn Provider>> = vec![
            Arc::new(Addressed),
            Arc::new(StubProvider("anthropic")),
            Arc::new(Addressed),
            Arc::new(StubProvider("janus")),
        ];
        assert_eq!(rotation(&providers), vec![1, 3]);
        let none: Vec<Arc<dyn Provider>> = vec![Arc::new(Addressed)];
        assert!(rotation(&none).is_empty());
    }

    /// A call goes to the provider its model names. When that provider is
    /// not registered it goes to the first one that takes any call, never to
    /// the linked provider; with nowhere else to go it goes nowhere, and a
    /// linked call is never stood in for.
    #[test]
    fn a_missing_provider_never_lands_on_the_linked_one() {
        let only_linked: Vec<Arc<dyn Provider>> = vec![Arc::new(Addressed)];
        assert_eq!(pick_provider(&only_linked, 0, "janus"), None);
        assert_eq!(pick_provider(&only_linked, 0, ""), None);
        assert_eq!(pick_provider(&only_linked, 1, "janus"), None);
        assert_eq!(pick_provider(&only_linked, 0, "linked"), Some(0));

        let both: Vec<Arc<dyn Provider>> = vec![Arc::new(Addressed), Arc::new(StubProvider("anthropic"))];
        assert_eq!(pick_provider(&both, 0, "janus"), Some(1));
        assert_eq!(pick_provider(&both, 0, ""), Some(1));
        assert_eq!(pick_provider(&both, 0, "linked"), Some(0));

        let no_linked: Vec<Arc<dyn Provider>> = vec![Arc::new(StubProvider("anthropic"))];
        assert_eq!(pick_provider(&no_linked, 0, "linked"), None);
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
}
