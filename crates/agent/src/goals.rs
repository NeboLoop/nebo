//! Judge-gated auto-continuation ("persistent goals" v1).
//!
//! After a chat run completes normally, a cheap-model judge decides whether the
//! assistant's final response left an explicit unfinished commitment (promised
//! next steps, a partial enumeration, work it said it would do but didn't show).
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
//! - The judge fails CLOSED: any provider error, timeout, or unparseable
//!   verdict is treated as `done` (we auto-continue on EVERY chat, not just
//!   explicit goals, so uncertainty must halt the loop — the opposite of
//!   Hermes' fail-open choice for explicit goals).
//! - Subagent sessions, errored/cancelled runs, and empty responses are never
//!   judged (see [`eligible_for_judging`]).
//!
//! Off switch: `NEBO_AUTO_CONTINUE=0` (env kill-switch, default ON — see
//! [`enabled`]).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use ai::{ChatRequest, Message, Provider, StreamEventType};
use tracing::debug;

use crate::keyparser;
use crate::runner::{prefer_non_gateway, truncate_str};

/// Max auto-continuations per real user message.
pub const MAX_AUTO_CONTINUATIONS: u32 = 5;

/// Char-boundary-safe caps for the judge prompt — keep the call cheap.
const JUDGE_USER_CAP: usize = 2_000;
const JUDGE_RESPONSE_CAP: usize = 4_000;
/// Cap on the stored last-real-prompt per session.
const STORED_PROMPT_CAP: usize = 4_000;
/// Judge call wall-clock ceiling — on timeout we fail CLOSED (done).
const JUDGE_TIMEOUT_SECS: u64 = 20;

/// Exact prefix of every synthetic continuation message. This is the marker
/// the dispatch layer uses to tell continuations apart from real user
/// messages (the dispatch payload has no metadata channel for it).
pub const CONTINUATION_PREFIX: &str =
    "Continue — your previous response committed to more work that isn't done yet:";

const JUDGE_SYSTEM_PROMPT: &str = "\
You judge whether an AI assistant finished its turn or left an explicit unfinished commitment.\n\
Respond with STRICT JSON only: {\"verdict\": \"continue\" | \"done\", \"reason\": \"...\"}\n\
Verdict \"continue\" ONLY if the assistant's response contains an explicit unfinished commitment: \
promised next steps (\"I'll now…\", \"next I will…\"), a partial enumeration it said it would \
complete, or work it stated it would do but didn't show. If uncertain, the verdict is \"done\".";

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

/// Parse the judge's raw output defensively. Anything that isn't strict JSON
/// with `"verdict": "continue"` is `Done` (fail closed).
pub fn parse_verdict(raw: &str) -> Verdict {
    let trimmed = raw.trim();
    // Tolerate prose or code fences around the JSON: take the outermost braces.
    let candidate = match (trimmed.find('{'), trimmed.rfind('}')) {
        (Some(start), Some(end)) if end > start => &trimmed[start..=end],
        _ => return Verdict::Done,
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(candidate) else {
        return Verdict::Done;
    };
    match value.get("verdict").and_then(|v| v.as_str()) {
        Some("continue") => {
            let reason = value
                .get("reason")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .unwrap_or("unfinished work in the previous response")
                .to_string();
            Verdict::Continue { reason }
        }
        _ => Verdict::Done,
    }
}

/// Ask a cheap model whether the assistant's final response satisfied the
/// user's last real message. Fails CLOSED: any error, timeout, or garbage
/// output returns [`Verdict::Done`] (logged at debug).
pub async fn judge(
    providers: &[Arc<dyn Provider>],
    last_user_prompt: &str,
    assistant_response: &str,
) -> Verdict {
    let resolved = crate::runner::resolve_aux(&config::ModelsConfig::load(), providers)
        .or_else(|| prefer_non_gateway(providers).map(|p| (p, String::new())));
    let Some((provider, aux_model)) = resolved else {
        debug!("auto-continue judge: no provider available; treating as done");
        return Verdict::Done;
    };

    let content = format!(
        "Last user message:\n{}\n\nAssistant's final response:\n{}",
        truncate_str(last_user_prompt, JUDGE_USER_CAP),
        truncate_str(assistant_response, JUDGE_RESPONSE_CAP),
    );

    let req = ChatRequest {
        tool_choice: Default::default(),
        messages: vec![Message {
            role: "user".to_string(),
            content,
            ..Default::default()
        }],
        tools: vec![],
        max_tokens: 150,
        temperature: 0.0,
        system: JUDGE_SYSTEM_PROMPT.to_string(),
        static_system: String::new(),
        model: aux_model,
        enable_thinking: false,
        metadata: None,
        cache_breakpoints: vec![],
        cancel_token: None,
        trace: None,
    };

    let call = async {
        let mut rx = match provider.stream(&req).await {
            Ok(rx) => rx,
            Err(e) => {
                debug!(error = %e, "auto-continue judge call failed; treating as done");
                return None;
            }
        };
        let mut response = String::new();
        while let Some(event) = rx.recv().await {
            match event.event_type {
                StreamEventType::Text => response.push_str(&event.text),
                StreamEventType::Error => {
                    debug!(error = ?event.error, "auto-continue judge stream error; treating as done");
                    return None;
                }
                StreamEventType::Done => break,
                _ => {}
            }
        }
        Some(response)
    };

    match tokio::time::timeout(Duration::from_secs(JUDGE_TIMEOUT_SECS), call).await {
        Ok(Some(raw)) => parse_verdict(&raw),
        Ok(None) => Verdict::Done,
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
    }

    /// The last real user prompt recorded for this session, if any.
    pub fn last_real_prompt(&self, session_key: &str) -> Option<String> {
        self.lock()
            .get(session_key)
            .map(|s| s.last_real_prompt.clone())
    }

    /// Cheap peek: true while budget remains (used to skip the judge call
    /// entirely once exhausted).
    pub fn has_budget(&self, session_key: &str) -> bool {
        self.lock()
            .get(session_key)
            .map(|s| s.continuations < MAX_AUTO_CONTINUATIONS)
            .unwrap_or(true)
    }

    /// Consume one continuation slot. Returns false when the budget is
    /// exhausted (the last assistant message simply stands).
    pub fn try_consume(&self, session_key: &str) -> bool {
        let mut map = self.lock();
        let entry = map.entry(session_key.to_string()).or_default();
        if entry.continuations >= MAX_AUTO_CONTINUATIONS {
            return false;
        }
        entry.continuations += 1;
        true
    }
}

/// Process-wide tracker instance.
pub fn tracker() -> &'static GoalTracker {
    static TRACKER: OnceLock<GoalTracker> = OnceLock::new();
    TRACKER.get_or_init(GoalTracker::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_exhaustion() {
        let t = GoalTracker::new();
        t.on_real_message("agent:a:web", "do five things");
        for i in 0..MAX_AUTO_CONTINUATIONS {
            assert!(t.try_consume("agent:a:web"), "continuation {} should fit", i);
        }
        assert!(!t.has_budget("agent:a:web"));
        assert!(
            !t.try_consume("agent:a:web"),
            "6th continuation must be refused"
        );
        // Other sessions are unaffected.
        assert!(t.try_consume("agent:b:web"));
    }

    #[test]
    fn real_message_resets_budget() {
        let t = GoalTracker::new();
        t.on_real_message("agent:a:web", "first ask");
        for _ in 0..MAX_AUTO_CONTINUATIONS {
            assert!(t.try_consume("agent:a:web"));
        }
        assert!(!t.try_consume("agent:a:web"));

        t.on_real_message("agent:a:web", "second ask");
        assert!(t.has_budget("agent:a:web"));
        assert!(t.try_consume("agent:a:web"));
        assert_eq!(
            t.last_real_prompt("agent:a:web").as_deref(),
            Some("second ask")
        );
    }

    #[test]
    fn verdict_parsing_valid_continue() {
        let v = parse_verdict(r#"{"verdict": "continue", "reason": "promised to write tests next"}"#);
        assert_eq!(
            v,
            Verdict::Continue {
                reason: "promised to write tests next".to_string()
            }
        );
        // Fenced / prose-wrapped JSON still parses.
        let v = parse_verdict(
            "```json\n{\"verdict\": \"continue\", \"reason\": \"partial list\"}\n```",
        );
        assert!(matches!(v, Verdict::Continue { .. }));
    }

    #[test]
    fn verdict_parsing_valid_done() {
        assert_eq!(
            parse_verdict(r#"{"verdict": "done", "reason": "all work shown"}"#),
            Verdict::Done
        );
    }

    #[test]
    fn verdict_parsing_garbage_is_done() {
        assert_eq!(parse_verdict(""), Verdict::Done);
        assert_eq!(parse_verdict("the assistant should continue"), Verdict::Done);
        assert_eq!(parse_verdict("{not json at all"), Verdict::Done);
        assert_eq!(parse_verdict(r#"{"verdict": "CONTINUE"}"#), Verdict::Done);
        assert_eq!(parse_verdict(r#"{"reason": "no verdict field"}"#), Verdict::Done);
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
