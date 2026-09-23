//! Typed-decision gate in front of after-turn memory extraction.
//!
//! Extraction ([`crate::memory::extract_facts`]) asks the chat model (the aux
//! route, `max_tokens: 4096`) to read the last exchange and return durable
//! facts. Its own prompt says most conversations contain nothing durable, and
//! most calls come back with empty arrays. Before that call, ONE typed
//! decision (Jev through Janus, [`ai::DecideClient`]) reads the new turn —
//! the last user message, the assistant's reply, the objective — and answers
//! three Nouls in one round trip: is there a durable fact, is this a
//! correction, is this procedural. Extraction is SKIPPED only when there is
//! plausibly no durable fact and the turn is clearly neither a correction nor
//! a standing instruction; anything else runs extraction exactly as before.
//! The thresholds stay in code ([`verdict_from`]).
//!
//! The expensive mistake is skipping a real memory; the cheap one is running
//! extraction needlessly. So the gate fails OPEN: no decide client, any
//! error, a timeout, or an answer it cannot read all run extraction.
//!
//! Off switch: `NEBO_DECIDE_MEMORY_GATE=0` (env kill-switch, default ON — see
//! [`enabled`]).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use ai::{Answer, DecideClient, Decision, Question};
use db::models::ChatMessage;
use tracing::debug;

use crate::runner::truncate_str;

/// `has_durable_fact` at or under this skips (with the two below quiet).
/// Set from 71 after-turn gates on 2026-09-22 (two status heartbeats every
/// two minutes plus one chat): the old 0.15 was never met (min 0.13 once,
/// p25 0.26, median 0.31, p75 0.44), so the gate cost a call and saved none.
/// Heartbeat status replies sit at 0.21–0.69, and extraction from them
/// wrote restatements of the same status under new keys (0.88–0.95 similar
/// to keys already stored); the real memories in the chat (corrections and
/// "stop taking screenshots") read 0.65–0.83. At 0.35 the gate skips 40 of
/// the 71 (56%), none of them chat.
pub const DURABLE_SKIP_CEILING: f64 = 0.35;
/// `is_correction` and `is_procedural` must each be at or under this to
/// skip. Heartbeat turns read 0.02–0.07 on both; chat turns with nothing to
/// keep read up to 0.24, and the corrections 0.67–0.94. A Noul is a
/// probability, not an intensity: 0.1 means "almost certainly not".
pub const SIGNAL_SKIP_CEILING: f64 = 0.1;
/// Floor on an explicit confidence for `has_durable_fact`, when the answer
/// carries one. A Noul carries none on the wire (the binary distribution is
/// the whole answer, its certainty is `1 - noul`), and that certainty is
/// already bounded by [`DURABLE_SKIP_CEILING`].
pub const CONFIDENCE_FLOOR: f64 = 0.7;
/// Gate call ceiling. A decision answers in milliseconds; this only bounds
/// a stalled connection, and a trip runs extraction (fail open).
const GATE_TIMEOUT: Duration = Duration::from_secs(2);
/// Char-boundary-safe caps on the state (about 4k tokens in all): the last
/// user message, the assistant's reply, the objective line.
const USER_CAP: usize = 4_000;
const REPLY_CAP: usize = 10_000;
const OBJECTIVE_CAP: usize = 400;

/// Skips versus runs since boot, so the saving is visible in the log.
static SKIPPED: AtomicU64 = AtomicU64::new(0);
static RAN: AtomicU64 = AtomicU64::new(0);

/// What the gate decided for one extraction opportunity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    /// Nothing durable in the turn: skip the chat-model extraction.
    Skip,
    /// Run extraction exactly as before (also the fail-open answer).
    Run,
}

/// Env kill-switch: `NEBO_DECIDE_MEMORY_GATE=0` (or `false`/`off`/`no`)
/// disables the gate; extraction then runs as it always did. Default ON.
pub fn enabled() -> bool {
    match std::env::var("NEBO_DECIDE_MEMORY_GATE") {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "off" | "no"
        ),
        Err(_) => true,
    }
}

/// The state the judge reads: the new turn only, as evidence. `messages` is
/// the last exchange (the last user message and everything after it). The
/// user message and the assistant's own text are kept, capped; tool results
/// are never included (what the reply quotes from them is already in the
/// reply). `objective` is the session's objective line, or `none`.
pub fn gate_state(messages: &[ChatMessage], objective: &str) -> serde_json::Value {
    let last_user = messages
        .iter()
        .rposition(|m| m.role == "user")
        .map(|i| &messages[i..])
        .unwrap_or(messages);
    let user_message = last_user
        .iter()
        .find(|m| m.role == "user")
        .map(|m| m.content.trim())
        .unwrap_or("");
    let reply = last_user
        .iter()
        .filter(|m| m.role == "assistant")
        .map(|m| m.content.trim())
        .filter(|c| !c.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    let objective = objective.trim();
    serde_json::json!({
        "objective": if objective.is_empty() { "none" } else { truncate_str(objective, OBJECTIVE_CAP) },
        "user_message": truncate_str(user_message, USER_CAP),
        "assistant_reply": truncate_str(&reply, REPLY_CAP),
    })
}

/// Skip only when `has_durable_fact` is at or under [`DURABLE_SKIP_CEILING`],
/// `is_correction` and `is_procedural` are at or under
/// [`SIGNAL_SKIP_CEILING`], and an explicit confidence on `has_durable_fact`,
/// if the answer carries one, clears [`CONFIDENCE_FLOOR`]. A missing answer
/// is a `Run`: the gate never skips on what it cannot read.
pub fn verdict_from(decision: &Decision) -> Gate {
    let (Some(durable), Some(correction), Some(procedural)) = (
        decision.answer("has_durable_fact"),
        decision.answer("is_correction"),
        decision.answer("is_procedural"),
    ) else {
        return Gate::Run;
    };
    let confident = durable.confidence.is_none_or(|c| c >= CONFIDENCE_FLOOR);
    let quiet = durable.yes() <= DURABLE_SKIP_CEILING
        && correction.yes() <= SIGNAL_SKIP_CEILING
        && procedural.yes() <= SIGNAL_SKIP_CEILING;
    if quiet && confident {
        Gate::Skip
    } else {
        Gate::Run
    }
}

/// Ask Jev whether the turn in `state` (from [`gate_state`]) holds anything
/// worth the chat-model extraction. Fails OPEN: the gate off, no client, any
/// error, or a timeout returns `true` (logged at debug).
pub async fn should_extract(
    decide: Option<&DecideClient>,
    trace: &ai::RequestTrace,
    state: &serde_json::Value,
) -> bool {
    if !enabled() {
        return true;
    }
    let Some(client) = decide else {
        debug!("memory gate: no decide client (Janus absent); running extraction");
        return true;
    };

    let questions = BTreeMap::from([
        (
            "has_durable_fact",
            Question::noul(
                "`user_message` or `assistant_reply` contains a preference, a fact about the person or their business, a decision, a commitment, or a correction that would still be true and useful weeks from now.",
            ),
        ),
        (
            "is_correction",
            Question::noul(
                "In `user_message` the person corrects something the assistant believed, said, or did: a wrong fact, a wrong assumption, or a wrong way of doing the work.",
            ),
        ),
        (
            "is_procedural",
            Question::noul(
                "`user_message` states how the person wants things done from now on: a rule, a format, a workflow, a standing instruction for `objective` or for work in general.",
            ),
        ),
    ]);

    let call = client.decide(trace, state, &questions);
    let gate = match tokio::time::timeout(GATE_TIMEOUT, call).await {
        Ok(Ok(decision)) => {
            let noul = |name: &str| decision.answer(name).map(Answer::yes).unwrap_or(-1.0);
            let gate = verdict_from(&decision);
            debug!(
                model = %decision.model,
                input_tokens = decision.usage.input_tokens,
                output_tokens = decision.usage.output_tokens,
                cost_micro = decision.usage.cost_micro,
                has_durable_fact = noul("has_durable_fact"),
                is_correction = noul("is_correction"),
                is_procedural = noul("is_procedural"),
                ?gate,
                "memory gate decided"
            );
            gate
        }
        Ok(Err(e)) => {
            debug!(error = %e, "memory gate call failed; running extraction");
            Gate::Run
        }
        Err(_) => {
            debug!(
                timeout_ms = GATE_TIMEOUT.as_millis() as u64,
                "memory gate timed out; running extraction"
            );
            Gate::Run
        }
    };

    let (skipped, ran) = match gate {
        Gate::Skip => (SKIPPED.fetch_add(1, Ordering::Relaxed) + 1, RAN.load(Ordering::Relaxed)),
        Gate::Run => (SKIPPED.load(Ordering::Relaxed), RAN.fetch_add(1, Ordering::Relaxed) + 1),
    };
    debug!(skipped, ran, "memory gate: extractions skipped vs run since boot");
    gate == Gate::Run
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn noul(p: f64) -> Answer {
        Answer {
            kind: "noul".into(),
            choice: None,
            score: None,
            noul: Some(p),
            confidence: None,
            probabilities: BTreeMap::new(),
        }
    }

    fn decision(durable: f64, correction: f64, procedural: f64) -> Decision {
        Decision {
            model: "jev-1.13.0".into(),
            answers: HashMap::from([
                ("has_durable_fact".to_string(), noul(durable)),
                ("is_correction".to_string(), noul(correction)),
                ("is_procedural".to_string(), noul(procedural)),
            ]),
            usage: Default::default(),
        }
    }

    fn msg(role: &str, content: &str) -> ChatMessage {
        ChatMessage {
            id: String::new(),
            chat_id: String::new(),
            role: role.into(),
            content: content.into(),
            metadata: None,
            created_at: 0,
            day_marker: None,
            tool_calls: None,
            tool_results: None,
            token_estimate: None,
            html: None,
        }
    }

    #[test]
    fn a_quiet_turn_is_skipped() {
        assert_eq!(verdict_from(&decision(0.02, 0.01, 0.0)), Gate::Skip);
        // The ceilings are inclusive.
        assert_eq!(
            verdict_from(&decision(DURABLE_SKIP_CEILING, SIGNAL_SKIP_CEILING, SIGNAL_SKIP_CEILING)),
            Gate::Skip
        );
    }

    #[test]
    fn any_noul_over_its_ceiling_runs_extraction() {
        assert_eq!(verdict_from(&decision(0.36, 0.0, 0.0)), Gate::Run);
        assert_eq!(verdict_from(&decision(0.0, 0.11, 0.0)), Gate::Run);
        assert_eq!(verdict_from(&decision(0.0, 0.0, 0.11)), Gate::Run);
        assert_eq!(verdict_from(&decision(0.9, 0.9, 0.9)), Gate::Run);
    }

    #[test]
    fn a_doubtful_no_runs_extraction() {
        // A Noul carries no confidence: the ceiling alone decides.
        assert_eq!(verdict_from(&decision(DURABLE_SKIP_CEILING, 0.0, 0.0)), Gate::Skip);
        // An explicit confidence on the wire is honoured when present.
        let mut low = decision(0.05, 0.0, 0.0);
        low.answers.get_mut("has_durable_fact").unwrap().confidence = Some(0.6);
        assert_eq!(verdict_from(&low), Gate::Run);
        let mut high = decision(0.05, 0.0, 0.0);
        high.answers.get_mut("has_durable_fact").unwrap().confidence = Some(CONFIDENCE_FLOOR);
        assert_eq!(verdict_from(&high), Gate::Skip);
    }

    /// Nouls copied from the 2026-09-22 gate log (the rule was set from it).
    #[test]
    fn logged_turns_skip_heartbeat_status_and_keep_corrections() {
        // Status heartbeat: extraction wrote a restatement of the status
        // under a new key (0.89 similar to one already stored).
        assert_eq!(verdict_from(&decision(0.31, 0.04, 0.05)), Gate::Skip);
        // The same heartbeat just over the cut still runs.
        assert_eq!(verdict_from(&decision(0.36, 0.03, 0.04)), Gate::Run);
        // Chat: the owner correcting what the employee did.
        assert_eq!(verdict_from(&decision(0.83, 0.89, 0.38)), Gate::Run);
        // Chat: "quit taking screenshots", a standing instruction.
        assert_eq!(verdict_from(&decision(0.82, 0.8, 0.94)), Gate::Run);
        // Chat: a low durable score with a procedural signal.
        assert_eq!(verdict_from(&decision(0.24, 0.15, 0.67)), Gate::Run);
        // Chat with nothing to keep, but not clearly quiet: runs (the cheap mistake).
        assert_eq!(verdict_from(&decision(0.23, 0.04, 0.12)), Gate::Run);
    }

    #[test]
    fn a_missing_answer_runs_extraction() {
        let mut d = decision(0.0, 0.0, 0.0);
        d.answers.remove("is_procedural");
        assert_eq!(verdict_from(&d), Gate::Run);
        let empty = Decision {
            model: "jev-1.13.0".into(),
            answers: HashMap::new(),
            usage: Default::default(),
        };
        assert_eq!(verdict_from(&empty), Gate::Run);
    }

    #[tokio::test]
    async fn no_client_or_a_failed_call_runs_extraction() {
        let state = gate_state(&[msg("user", "hi"), msg("assistant", "hello")], "");
        let trace = ai::RequestTrace::new("memory_gate");
        assert!(should_extract(None, &trace, &state).await);
        // A client with no bearer fails as Auth before any network call.
        let client = DecideClient::new("http://127.0.0.1:1", || None);
        assert!(should_extract(Some(&client), &trace, &state).await);
    }

    #[test]
    fn state_is_the_new_turn_without_tool_results() {
        let messages = [
            msg("user", "old question"),
            msg("assistant", "old answer"),
            msg("user", "  Always send invoices as PDF.  "),
            msg("assistant", "Checking your settings."),
            msg("tool", "{\"invoice_format\":\"docx\",\"secret\":\"do-not-leak\"}"),
            msg("assistant", ""),
            msg("assistant", "Done: invoices now go out as PDF."),
        ];
        let state = gate_state(&messages, "  Set up invoicing  ");
        assert_eq!(state["objective"], "Set up invoicing");
        assert_eq!(state["user_message"], "Always send invoices as PDF.");
        assert_eq!(
            state["assistant_reply"],
            "Checking your settings.\n\nDone: invoices now go out as PDF."
        );
        let text = state.to_string();
        assert!(!text.contains("do-not-leak"));
        assert!(!text.contains("old question"));
    }

    #[test]
    fn state_is_capped_and_an_empty_objective_is_none() {
        let long_user = "u".repeat(USER_CAP + 500);
        let long_reply = "r".repeat(REPLY_CAP + 500);
        let messages = [msg("user", &long_user), msg("assistant", &long_reply)];
        let state = gate_state(&messages, "");
        assert_eq!(state["objective"], "none");
        assert_eq!(state["user_message"].as_str().unwrap().len(), USER_CAP);
        assert_eq!(state["assistant_reply"].as_str().unwrap().len(), REPLY_CAP);
        // Caps land on a char boundary.
        let wide = "é".repeat(REPLY_CAP);
        let state = gate_state(&[msg("user", "x"), msg("assistant", &wide)], "");
        assert!(state["assistant_reply"].as_str().unwrap().len() <= REPLY_CAP);
    }
}
