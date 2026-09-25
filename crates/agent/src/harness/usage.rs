//! Spend, cost, run classification and context accounting; the turn's
//! token ledger is declared here too.

use std::sync::Arc;

use ai::StreamEvent;
use db::Store;
use tokio::sync::mpsc;

use crate::read_ledger::LedgerStats;
use crate::selector::ModelSelector;

/// A turn's token and cost counters across its calls, and the local
/// estimate's calibration against what the provider reports.
#[derive(Default)]
pub(crate) struct RunState {
    /// System prompt + tool-schema tokens (display estimate).
    pub(crate) system_overhead_tokens: usize,
    /// Local estimate (chars/4) of the message tokens sent in the last request.
    /// Compared against API-reported usage to calibrate the checkpoint trigger.
    pub(crate) last_request_estimate: usize,
    /// Observed undercount of the local estimate vs API-reported usage.
    pub(crate) estimate_correction: usize,
    pub(crate) total_input_tokens: i32,
    pub(crate) total_output_tokens: i32,
    pub(crate) total_cache_read_tokens: i32,
    pub(crate) total_cache_creation_tokens: i32,
    /// Provider-reported cost this run, microdollars (Janus prices the model it
    /// routed to). 0 when no provider said; then the price table estimates.
    pub(crate) cost_microdollars: i64,
    /// Janus quota warning, set when session or weekly usage passes 80%.
    pub(crate) quota_warning: Option<String>,
    /// Whether the quota warning was already sent this run (once).
    pub(crate) quota_warning_sent: bool,
}

/// The tokens a turn has used across its calls.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenLedger {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl TokenLedger {
    /// Add one call's usage.
    pub fn add(&mut self, u: &ai::UsageInfo) {
        let n = |v: i32| u64::try_from(v).unwrap_or(0);
        self.input += n(u.input_tokens);
        self.output += n(u.output_tokens);
        self.cache_read += n(u.cache_read_input_tokens);
        self.cache_write += n(u.cache_creation_input_tokens);
    }
}

/// Persists the finished run's usage. Cost is computed from models.yaml
/// pricing at write time; a model with no pricing records zero rather than a
/// wrong number — a silently invented figure is worse than a visibly missing
/// one, because this number ends up on an invoice.
pub(crate) fn record_run_usage(
    store: &Arc<Store>,
    selector: &ModelSelector,
    agent_id: &str,
    session_id: &str,
    model_name: &str,
    state: &RunState,
    exit_reason: &str,
) {
    if state.total_input_tokens == 0 && state.total_output_tokens == 0 {
        // Nothing was spent — a run that never reached a provider (immediate
        // cancellation, empty prompt) has no cost to record.
        return;
    }

    let (run_type, run_id) = classify_run(session_id);
    let cost = turn_cost_microcents(selector, model_name, state);

    let entry = db::models::RunUsageEntry {
        agent_id: agent_id.to_string(),
        session_key: Some(session_id.to_string()),
        run_id,
        run_type: run_type.to_string(),
        model_id: model_name.to_string(),
        input_tokens: state.total_input_tokens as i64,
        output_tokens: state.total_output_tokens as i64,
        cache_read_tokens: state.total_cache_read_tokens as i64,
        cache_creation_tokens: state.total_cache_creation_tokens as i64,
        cost_microcents: cost,
        outcome: None,
        exit_reason: Some(exit_reason.to_string()),
    };
    if let Err(e) = store.record_run_usage(&entry) {
        // Loudly: this row is money. But the work is already done, and
        // failing a finished run over its receipt would be worse.
        tracing::error!(session_id, error = %e, "failed to record run usage");
    }
}

/// What one loop's turns cost, in microcents — the ONE pricing rule for the
/// ledger and the owner's limit. The provider's own figure when it reported
/// one (Janus prices the model it actually routed to); otherwise the local
/// price table, which for a routed alias such as nebo-1 knows nothing and
/// yields 0.
fn turn_cost_microcents(selector: &ModelSelector, model_name: &str, state: &RunState) -> i64 {
    if state.cost_microdollars > 0 {
        // microdollars → microcents
        return state.cost_microdollars * 100;
    }
    selector
        .get_model_info(model_name)
        .map(|info| {
            db::cost_microcents(
                state.total_input_tokens as i64,
                state.total_output_tokens as i64,
                state.total_cache_read_tokens as i64,
                state.total_cache_creation_tokens as i64,
                info.input_price,
                info.output_price,
                info.cached_input_price,
            )
        })
        .unwrap_or(0)
}

/// What this run has cost so far: every turn already recorded against its
/// run id, plus the current turn priced the same way record_run_usage will.
pub(crate) fn run_spend_so_far(
    store: &Arc<Store>,
    selector: &ModelSelector,
    session_key: &str,
    model_name: &str,
    state: &RunState,
) -> i64 {
    let (_, run_id) = classify_run(session_key);
    let recorded = run_id
        .as_deref()
        .and_then(|id| store.run_spend_microcents(id).ok())
        .unwrap_or(0);
    recorded + turn_cost_microcents(selector, model_name, state)
}

/// classify_run derives what kind of run a session key names, and for the
/// canonical workflow form, which workflow run it was — the join that lets
/// "what did this workflow cost" be answered at all.
fn classify_run(session_id: &str) -> (&'static str, Option<String>) {
    if session_id.starts_with("heartbeat-") {
        return ("heartbeat", None);
    }
    if let Some(idx) = session_id.find(":workflow:") {
        // `agent:<id>:workflow:<run>:<activity>::<n>` — the run id is the
        // segment, not the rest of the key; the cost join is on the run.
        let rest = &session_id[idx + ":workflow:".len()..];
        let run_id = rest.split(':').next().unwrap_or("");
        if !run_id.is_empty() {
            return ("workflow", Some(run_id.to_string()));
        }
        return ("workflow", None);
    }
    // The engine's legacy key ("workflow-{def}-{run}") is ambiguous — both
    // segments may contain hyphens — so it classifies without a join rather
    // than guessing a wrong id into a money table.
    if session_id.starts_with("workflow-") {
        return ("workflow", None);
    }
    ("chat", None)
}

/// Context accounting for the owner: one event per turn, rendered as a
/// quiet line under the reply (Stage 8), never as reply text.
pub(crate) async fn send_context_stats(
    tx: &mpsc::Sender<StreamEvent>,
    ledger: LedgerStats,
    compaction_passes: usize,
    evictions: usize,
    spilled_results: usize,
    state: &RunState,
) {
    let _ = tx
        .send(StreamEvent::context_stats(serde_json::json!({
            "files": ledger.files,
            "files_reread": ledger.files_reread,
            "redundant_reads": ledger.redundant_observations,
            "compaction_passes": compaction_passes,
            "evictions": evictions,
            "spilled_results": spilled_results,
            "input_tokens": state.total_input_tokens,
            "cache_read_tokens": state.total_cache_read_tokens,
        })))
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ledger_sums_calls_and_ignores_negative_counts() {
        let mut ledger = TokenLedger::default();
        let call = ai::UsageInfo {
            input_tokens: 100,
            output_tokens: 20,
            cache_read_input_tokens: 80,
            cache_creation_input_tokens: 5,
            ..Default::default()
        };
        ledger.add(&call);
        ledger.add(&call);
        ledger.add(&ai::UsageInfo {
            input_tokens: -1,
            ..Default::default()
        });
        assert_eq!(
            ledger,
            TokenLedger {
                input: 200,
                output: 40,
                cache_read: 160,
                cache_write: 10
            }
        );
    }

    // classify_run feeds a money table: a wrong run_id joins someone's cost
    // to the wrong workflow, so the parse gets a check rather than a comment.
    #[test]
    fn classify_run_reads_every_session_key_shape() {
        // Canonical workflow key carries the run id for the cost join.
        assert_eq!(
            classify_run("agent:abc:workflow:run-123-xyz"),
            ("workflow", Some("run-123-xyz".to_string()))
        );
        // The real key carries the activity and loop index after the run id.
        assert_eq!(
            classify_run("agent:abc:workflow:run-123-xyz:store-snapshot::2"),
            ("workflow", Some("run-123-xyz".to_string()))
        );
        // The legacy engine key is ambiguous (both segments may contain
        // hyphens) — classified, but never a guessed id in a money table.
        assert_eq!(classify_run("workflow-def-1-run-2"), ("workflow", None));
        assert_eq!(classify_run("heartbeat-agent-42"), ("heartbeat", None));
        assert_eq!(classify_run("agent:abc:desktop"), ("chat", None));
        assert_eq!(classify_run("subagent:parent:child"), ("chat", None));
        // A truncated workflow key must not record an empty-string id.
        assert_eq!(classify_run("agent:abc:workflow:"), ("workflow", None));
    }
}
