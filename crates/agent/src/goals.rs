//! Judge-gated auto-continuation ("persistent goals" v1).
//!
//! After a chat run completes normally, a typed-decision judge (Jev through
//! Janus, [`ai::DecideClient`]) decides whether the assistant's final response
//! left an explicit unfinished commitment (promised next steps, a partial
//! enumeration, work it said it would do but didn't show) and is not waiting
//! on the person. Two Noul questions, thresholds in code, milliseconds.
//! If so, the server re-dispatches a synthetic user message through the ONE
//! canonical chat pathway ([`run_chat`]) telling the agent to keep going.
//!
//! Loop safety, in order of defense:
//! - A real pending/queued run for the session preempts the loop (checked by
//!   the server hook against the RunRegistry before judging).
//! - Synthetic continuation messages carry an exact prefix
//!   ([`CONTINUATION_PREFIX`]) and NEVER reset the budget — only real user
//!   messages do (see [`GoalTracker::on_real_message`]).
//! - Budget: at most [`MAX_AUTO_CONTINUATIONS`] continuations per real user
//!   message, tracked in-memory per session key (deliberate v1 ceiling: a
//!   server restart drops the counters).
//! - The judge fails CLOSED: no decide client, any error, or a timeout is
//!   treated as `done` (we auto-continue on EVERY chat, not just explicit
//!   goals, so uncertainty must halt the loop — never fail-open).
//! - Subagent sessions, errored/cancelled runs, and empty responses are never
//!   judged (see [`eligible_for_judging`]).
//!
//! Off switch: `NEBO_AUTO_CONTINUE=0` (env kill-switch, default ON — see
//! [`enabled`]).

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;
use std::time::Duration;

use ai::{Answer, DecideClient, Question};
use tracing::debug;

use crate::runner::truncate_str;
use types::keyparser;

/// Max auto-continuations per real user message.
pub const MAX_AUTO_CONTINUATIONS: u32 = 5;

/// Char-boundary-safe caps for the judge prompt — keep the call cheap.
const JUDGE_USER_CAP: usize = 2_000;
const JUDGE_RESPONSE_CAP: usize = 4_000;
/// Cap on the stored last-real-prompt per session.
const STORED_PROMPT_CAP: usize = 4_000;
/// Judge call wall-clock ceiling — on timeout we fail CLOSED (done). A
/// decision answers in milliseconds; this only bounds a stalled connection.
const JUDGE_TIMEOUT_SECS: u64 = 5;
/// Floor on "the response commits to more work it can do now".
const COMMITMENT_FLOOR: f64 = 0.7;
/// Ceiling on "the response is waiting on the person"; at or above it the
/// ball is in their court and continuing cannot supply what is missing.
const WAITING_CEILING: f64 = 0.5;
/// The one reason a continuation carries; Jev decides, it does not write.
const CONTINUE_REASON: &str = "unfinished work in the previous response";

/// Exact prefix of every synthetic continuation message. This is the marker
/// the dispatch layer uses to tell continuations apart from real user
/// messages (the dispatch payload has no metadata channel for it).
pub const CONTINUATION_PREFIX: &str =
    "Continue — your previous response committed to more work that isn't done yet:";

/// Judge decision for a completed run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The response left an explicit unfinished commitment.
    Continue { reason: String },
    /// The response finished its work (or we can't tell — fail closed).
    Done,
}

/// True when a prompt is a synthetic continuation message (exact-prefix match).
pub fn is_continuation_prompt(prompt: &str) -> bool {
    prompt.trim_start().starts_with(CONTINUATION_PREFIX)
}

/// Build the synthetic continuation message dispatched as a normal user message.
pub fn continuation_prompt(reason: &str) -> String {
    let reason_line: String = reason.trim().replace(['\n', '\r'], " ");
    format!("{CONTINUATION_PREFIX} {reason_line}. Keep going and finish it.")
}

/// Whether a completed run should be judged at all.
///
/// Never judge: subagent sessions, runs that ended in error or cancellation,
/// or empty assistant responses. Continuation-triggered runs ARE judged (that
/// is how a chain forms, capped by the budget); the continuation *message* is
/// guarded at budget-reset time via [`is_continuation_prompt`].
pub fn eligible_for_judging(
    session_key: &str,
    assistant_response: &str,
    run_errored: bool,
    cancelled: bool,
) -> bool {
    if run_errored || cancelled {
        return false;
    }
    if keyparser::is_subagent_key(session_key) {
        return false;
    }
    if assistant_response.trim().is_empty() {
        return false;
    }
    true
}

/// Continue only when the response commits to more work AND is not waiting
/// on the person. Anything short of both thresholds is `Done`.
pub fn verdict_from(unfinished_commitment: f64, waiting_on_person: f64) -> Verdict {
    if unfinished_commitment >= COMMITMENT_FLOOR && waiting_on_person < WAITING_CEILING {
        Verdict::Continue {
            reason: CONTINUE_REASON.to_string(),
        }
    } else {
        Verdict::Done
    }
}

/// Ask Jev whether the assistant's final response left work it can do now.
/// Fails CLOSED: no client, any error, or a timeout returns [`Verdict::Done`]
/// (logged at debug).
pub async fn judge(
    decide: Option<&DecideClient>,
    last_user_prompt: &str,
    assistant_response: &str,
) -> Verdict {
    let Some(client) = decide else {
        debug!("auto-continue judge: no decide client (Janus absent); treating as done");
        return Verdict::Done;
    };

    let state = serde_json::json!({
        "last_user_message": truncate_str(last_user_prompt, JUDGE_USER_CAP),
        "assistant_response": truncate_str(assistant_response, JUDGE_RESPONSE_CAP),
    });
    let questions = BTreeMap::from([
        (
            "unfinished_commitment",
            Question::noul(
                "`assistant_response` states work the assistant will do next and can do right now without anything more from the person: a promised next step ('I will now', 'next I will'), a list it said it would complete, or work it said it would do but did not show.",
            ),
        ),
        (
            "waiting_on_person",
            Question::noul(
                "`assistant_response` asks the person a question, or requests information, a decision, or access it needs before it can proceed. A promise conditioned on their answer ('once you give me X, I will') counts as waiting.",
            ),
        ),
    ]);

    let call = client.decide(&state, &questions);
    match tokio::time::timeout(Duration::from_secs(JUDGE_TIMEOUT_SECS), call).await {
        Ok(Ok(decision)) => {
            let unfinished = decision
                .answer("unfinished_commitment")
                .map(Answer::yes)
                .unwrap_or(0.0);
            let waiting = decision
                .answer("waiting_on_person")
                .map(Answer::yes)
                .unwrap_or(1.0);
            debug!(
                model = %decision.model,
                unfinished,
                waiting,
                "auto-continue judge decided"
            );
            verdict_from(unfinished, waiting)
        }
        Ok(Err(e)) => {
            debug!(error = %e, "auto-continue judge call failed; treating as done");
            Verdict::Done
        }
        Err(_) => {
            debug!("auto-continue judge timed out; treating as done");
            Verdict::Done
        }
    }
}

/// Env kill-switch: `NEBO_AUTO_CONTINUE=0` (or `false`/`off`/`no`) disables
/// auto-continuation. Default ON.
pub fn enabled() -> bool {
    match std::env::var("NEBO_AUTO_CONTINUE") {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "off" | "no"
        ),
        Err(_) => true,
    }
}

#[derive(Default)]
struct SessionGoalState {
    continuations: u32,
    last_real_prompt: String,
    /// Fingerprint of the response that triggered the last continuation.
    last_response: Option<u64>,
}

/// Whitespace- and case-insensitive fingerprint of an assistant response.
/// Two turns that say the same thing fingerprint the same.
fn response_fingerprint(response: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    response
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
        .hash(&mut hasher);
    hasher.finish()
}

/// In-memory per-session continuation budget + last real user prompt.
///
/// Deliberate v1 ceiling: state lives only in memory, so a server restart
/// drops the counters. Keyed by session key; entries are overwritten on each
/// real user message.
#[derive(Default)]
pub struct GoalTracker {
    inner: Mutex<HashMap<String, SessionGoalState>>,
}

impl GoalTracker {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, SessionGoalState>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Record a REAL user message: resets the continuation budget and stores
    /// the prompt (truncated) for the judge. Callers must NOT invoke this for
    /// synthetic continuations (guard with [`is_continuation_prompt`]).
    pub fn on_real_message(&self, session_key: &str, prompt: &str) {
        let mut map = self.lock();
        let entry = map.entry(session_key.to_string()).or_default();
        entry.continuations = 0;
        entry.last_real_prompt = truncate_str(prompt, STORED_PROMPT_CAP).to_string();
        entry.last_response = None;
    }

    /// The last real user prompt recorded for this session, if any.
    pub fn last_real_prompt(&self, session_key: &str) -> Option<String> {
        self.lock()
            .get(session_key)
            .map(|s| s.last_real_prompt.clone())
    }

    /// Cheap peek: true while budget remains (used to skip the judge call
    /// entirely once exhausted). `limit` comes from the guardrail settings
    /// (default [`MAX_AUTO_CONTINUATIONS`]); 0 disables auto-continuation.
    pub fn has_budget(&self, session_key: &str, limit: u32) -> bool {
        self.lock()
            .get(session_key)
            .map(|s| s.continuations < limit)
            .unwrap_or(limit > 0)
    }

    /// Consume one continuation slot for `assistant_response`. Returns false
    /// when the budget is exhausted, or when this response says the same thing
    /// as the one that triggered the previous continuation — a turn that
    /// repeats itself has stopped making progress, and nudging it again only
    /// prints the same answer into the owner's transcript. Either way the last
    /// assistant message simply stands.
    pub fn try_consume(&self, session_key: &str, limit: u32, assistant_response: &str) -> bool {
        let mut map = self.lock();
        let entry = map.entry(session_key.to_string()).or_default();
        if entry.continuations >= limit {
            return false;
        }
        let fingerprint = response_fingerprint(assistant_response);
        if entry.last_response == Some(fingerprint) {
            return false;
        }
        entry.last_response = Some(fingerprint);
        entry.continuations += 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_exhaustion() {
        let t = GoalTracker::new();
        t.on_real_message("agent:a:web", "do five things");
        for i in 0..MAX_AUTO_CONTINUATIONS {
            assert!(
                t.try_consume("agent:a:web", MAX_AUTO_CONTINUATIONS, &format!("step {i}")),
                "continuation {} should fit",
                i
            );
        }
        assert!(!t.has_budget("agent:a:web", MAX_AUTO_CONTINUATIONS));
        assert!(
            !t.try_consume("agent:a:web", MAX_AUTO_CONTINUATIONS, "step six"),
            "6th continuation must be refused"
        );
        // Other sessions are unaffected.
        assert!(t.try_consume("agent:b:web", MAX_AUTO_CONTINUATIONS, "step one"));
    }

    #[test]
    fn real_message_resets_budget() {
        let t = GoalTracker::new();
        t.on_real_message("agent:a:web", "first ask");
        for i in 0..MAX_AUTO_CONTINUATIONS {
            assert!(t.try_consume("agent:a:web", MAX_AUTO_CONTINUATIONS, &format!("step {i}")));
        }
        assert!(!t.try_consume("agent:a:web", MAX_AUTO_CONTINUATIONS, "step six"));

        t.on_real_message("agent:a:web", "second ask");
        assert!(t.has_budget("agent:a:web", MAX_AUTO_CONTINUATIONS));
        assert!(t.try_consume("agent:a:web", MAX_AUTO_CONTINUATIONS, "step one"));
        assert_eq!(
            t.last_real_prompt("agent:a:web").as_deref(),
            Some("second ask")
        );
    }

    #[test]
    fn a_repeated_answer_stops_the_loop() {
        let t = GoalTracker::new();
        t.on_real_message("agent:a:web", "run the audit");
        let asking = "I still need the 3 keywords. Once you give me those, I'll open Chrome.";
        assert!(t.try_consume("agent:a:web", MAX_AUTO_CONTINUATIONS, asking));
        // The nudge produced the same answer — continuing again cannot help,
        // even though four slots remain.
        assert!(
            !t.try_consume("agent:a:web", MAX_AUTO_CONTINUATIONS, asking),
            "an identical response must not be nudged again"
        );
        // Whitespace and casing don't make it a different answer.
        assert!(!t.try_consume(
            "agent:a:web",
            MAX_AUTO_CONTINUATIONS,
            "I STILL need the 3 keywords.   Once you give me those,\nI'll open Chrome."
        ));
        // Real progress continues to consume budget.
        assert!(t.try_consume("agent:a:web", MAX_AUTO_CONTINUATIONS, "Searched keyword 1 of 3."));
        // A real user message clears the fingerprint along with the budget.
        t.on_real_message("agent:a:web", "here are the keywords");
        assert!(t.try_consume("agent:a:web", MAX_AUTO_CONTINUATIONS, asking));
    }

    #[test]
    fn verdict_needs_a_commitment_and_no_open_question() {
        assert!(matches!(verdict_from(0.9, 0.1), Verdict::Continue { .. }));
        // Committed but the response is asking the person for something.
        assert_eq!(verdict_from(0.9, 0.6), Verdict::Done);
        // Nothing promised.
        assert_eq!(verdict_from(0.3, 0.1), Verdict::Done);
        // Both thresholds are edges: at the floor continues, at the ceiling stops.
        assert!(matches!(verdict_from(0.7, 0.49), Verdict::Continue { .. }));
        assert_eq!(verdict_from(0.69, 0.0), Verdict::Done);
        assert_eq!(verdict_from(1.0, 0.5), Verdict::Done);
    }

    #[test]
    fn continuation_message_detection_guard() {
        let synthetic = continuation_prompt("it promised step 3\nand step 4");
        assert!(is_continuation_prompt(&synthetic));
        assert!(is_continuation_prompt(&format!("  {synthetic}")));
        // Reason newlines are flattened so the prefix stays one detectable line.
        assert!(!synthetic.contains('\n'));
        assert!(!is_continuation_prompt("Continue with the deployment please"));
        assert!(!is_continuation_prompt("real user message"));
    }

    #[test]
    fn eligibility_guards() {
        assert!(eligible_for_judging("agent:a:web", "I'll do more", false, false));
        // Subagent sessions never judged.
        assert!(!eligible_for_judging(
            "subagent:parent:child",
            "I'll do more",
            false,
            false
        ));
        // Errored / cancelled runs never judged.
        assert!(!eligible_for_judging("agent:a:web", "I'll do more", true, false));
        assert!(!eligible_for_judging("agent:a:web", "I'll do more", false, true));
        // Empty responses never judged.
        assert!(!eligible_for_judging("agent:a:web", "   \n", false, false));
    }
}
