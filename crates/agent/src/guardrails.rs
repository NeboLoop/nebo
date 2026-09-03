//! Loop-guardrail thresholds, configurable from Settings → Developer.
//!
//! Stored as a JSON object in the `settings.guardrails` column ('{}' =
//! defaults). Parsed once per run by the runner; unknown fields are ignored
//! and missing fields fall back to the built-in defaults, so a stale or
//! partial blob can never disable the guards entirely.

use serde::{Deserialize, Serialize};

/// Default: unproductive repeats of one (tool, action) before the spiral
/// backstop fires (nudge, or turn stop when `hard_stop` is on).
pub const DEFAULT_SAME_ACTION_LIMIT: usize = 8;
/// Default: unproductive identical-args repeats before the call is blocked.
pub const DEFAULT_IDENTICAL_ARGS_BLOCK_AFTER: usize = 3;
/// Default: auto-continuations per real user message (goals.rs judge).
pub const DEFAULT_MAX_AUTO_CONTINUATIONS: u32 = 5;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GuardrailConfig {
    /// Unproductive repeats of one (tool, action) per turn before the spiral
    /// backstop fires.
    pub same_action_limit: usize,
    /// Unproductive repeats of one exact (tool, args) call before it is
    /// hard-blocked.
    pub identical_args_block_after: usize,
    /// Auto-continuations the goals judge may chain per real user message.
    pub max_auto_continuations: u32,
    /// When true, a spiral-backstop trip ENDS the turn (ControlNotice) instead
    /// of nudging the model off the action. Off by default: interactive
    /// sessions get the gentle correction unless the developer opts in.
    pub hard_stop: bool,
}

impl Default for GuardrailConfig {
    fn default() -> Self {
        Self {
            same_action_limit: DEFAULT_SAME_ACTION_LIMIT,
            identical_args_block_after: DEFAULT_IDENTICAL_ARGS_BLOCK_AFTER,
            max_auto_continuations: DEFAULT_MAX_AUTO_CONTINUATIONS,
            hard_stop: false,
        }
    }
}

impl GuardrailConfig {
    /// Parse from the stored JSON blob; any parse failure or partial object
    /// falls back to defaults (per-field via serde(default)). Only JSON
    /// objects are accepted — serde would otherwise fill a struct
    /// positionally from an array like `[1,2]`.
    pub fn from_json(raw: &str) -> Self {
        match serde_json::from_str::<serde_json::Value>(raw) {
            Ok(v) if v.is_object() => serde_json::from_value(v).unwrap_or_default(),
            _ => Self::default(),
        }
    }

    /// Clamp to sane floors so a bad write can't set a guard to 0 and block
    /// every call (0 would trip the >= threshold immediately).
    pub fn sanitized(mut self) -> Self {
        self.same_action_limit = self.same_action_limit.max(2);
        self.identical_args_block_after = self.identical_args_block_after.max(1);
        self
    }
}

// ---------------------------------------------------------------------------
// Loop exits and guard escalation (coding parity, Stage 2).
//
// The reference labels every loop exit and every continuation, and gates a
// recovery on whether it already fired ("resetting to false here caused an
// infinite loop: compact → still too long → error → hook → compact → …").
// Ours: a closed `Exit` set so no site can invent a reason, and guards whose
// escalation lives WITH the detection — a nudge that did not help is a stop,
// never a second nudge (2026-08-27: 355 detections, 0 stops; 2026-09-02: the
// same tool error 49 times with varied arguments, never stopped).
// ---------------------------------------------------------------------------

/// Why the agentic loop ended. `label()` is the string contract read by
/// `workflow_loop.rs` and the logs; the variants are the closed set.
#[derive(Debug, Clone, PartialEq)]
pub enum Exit {
    Unknown,
    AdaptiveLimitNoProgress,
    UserRequestedStop,
    OutputBudgetExceeded,
    /// The same exact call repeated past `IDENTICAL_CALL_ABORT`.
    RunawayToolLoop,
    /// The spiral backstop fired twice for one action (or hard-stop is on).
    RepeatedToolCalls,
    /// The same tool error came back `SAME_ERROR_NUDGE_AFTER` times after a nudge.
    SameErrorLoop,
    TerminalToolError,
    EmptyResponseExhausted,
    /// Normal end: the model answered with text. Carries the provider's stop reason.
    TextResponse(String),
    MaxIterations { done: usize, max: usize },
    /// A workflow primitive ended the turn (`workflow_exit:…`, `suspension_failed:…`).
    Workflow(String),
    /// Nothing moved for [`RUN_IDLE_LIMIT`]: no event reached the dispatcher.
    /// Raised outside the loop, by the dispatcher, so it also covers a run
    /// whose loop has already returned but whose channel never closed.
    Stalled,
}

/// How long a run may go without a single stream event before the dispatcher
/// ends it as [`Exit::Stalled`]. Must outlast the longest thing that is
/// silent while it works: a blocking child (`SUBAGENT_INACTIVITY_TIMEOUT`)
/// and a shell command at its own timeout. Live 2026-09-02: a run whose turn
/// had ended sat "active" for 38 minutes with nothing bounding it.
pub const RUN_IDLE_LIMIT: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// The owner-facing line for a stalled run. States what was observed and what
/// was done; never guesses at a cause.
pub fn stall_notice() -> String {
    format!(
        "Stopped: nothing happened for {} minutes (no reply text and no tool activity), \
         so the run was ended instead of left hanging. Ask again to continue.",
        RUN_IDLE_LIMIT.as_secs() / 60
    )
}

/// The next event of a run's stream, or `Stalled` once nothing has arrived
/// for [`RUN_IDLE_LIMIT`] since `last_event`. The ONE idle bound for every
/// loop that drains a run (both chat dispatchers and voice), so a third copy
/// of the select arm cannot drift. Cancel-safe: both arms are.
pub enum Next<T> {
    Event(T),
    Closed,
    Stalled,
}

pub async fn next_event<T>(
    rx: &mut tokio::sync::mpsc::Receiver<T>,
    last_event: tokio::time::Instant,
) -> Next<T> {
    tokio::select! {
        _ = tokio::time::sleep_until(last_event + RUN_IDLE_LIMIT) => Next::Stalled,
        ev = rx.recv() => match ev {
            Some(e) => Next::Event(e),
            None => Next::Closed,
        },
    }
}

#[cfg(test)]
mod next_event_tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn stalls_only_when_nothing_arrives() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<u8>(1);
        tx.send(7).await.expect("channel open");
        assert!(matches!(next_event(&mut rx, tokio::time::Instant::now()).await, Next::Event(7)));
        // Sender alive, nothing sent: paused time jumps straight to the limit.
        assert!(matches!(next_event(&mut rx, tokio::time::Instant::now()).await, Next::Stalled));
        drop(tx);
        assert!(matches!(next_event(&mut rx, tokio::time::Instant::now()).await, Next::Closed));
    }
}

impl Exit {
    pub fn label(&self) -> String {
        match self {
            Exit::Unknown => "unknown".into(),
            Exit::AdaptiveLimitNoProgress => "adaptive_limit_no_progress".into(),
            Exit::UserRequestedStop => "user_requested_stop".into(),
            Exit::OutputBudgetExceeded => "output_budget_exceeded".into(),
            Exit::RunawayToolLoop => "runaway_tool_loop".into(),
            Exit::RepeatedToolCalls => "repeated_tool_calls".into(),
            Exit::SameErrorLoop => "same_error_loop".into(),
            Exit::TerminalToolError => "terminal_tool_error".into(),
            Exit::EmptyResponseExhausted => "empty_response_exhausted".into(),
            Exit::TextResponse(stop) => format!("text_response(stop_reason={stop})"),
            Exit::MaxIterations { done, max } => format!("max_iterations_reached({done}/{max})"),
            Exit::Workflow(reason) => reason.clone(),
            Exit::Stalled => "stalled".into(),
        }
    }
    pub fn is_text_response(&self) -> bool {
        matches!(self, Exit::TextResponse(_))
    }
}

impl std::fmt::Display for Exit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label())
    }
}

/// What a guard decided on this firing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Refuse the call with a corrective error; the turn continues.
    Nudge,
    /// End the turn with a ControlNotice.
    Stop,
}

/// A nudge is spent once per key per turn. Firing again for the same key
/// means the nudge did not help, and the answer is a stop, not another nudge.
#[derive(Debug, Default)]
pub struct Escalator {
    nudged: std::collections::HashSet<String>,
}

impl Escalator {
    pub fn fire(&mut self, key: &str) -> Verdict {
        if self.nudged.insert(key.to_string()) {
            Verdict::Nudge
        } else {
            Verdict::Stop
        }
    }
}

/// Identical error texts from one tool before the streak nudges; the same
/// count again after the nudge is a stop. Arguments are ignored on purpose:
/// the identical-args block never sees a model that varies its arguments
/// against the same wall.
pub const SAME_ERROR_NUDGE_AFTER: usize = 3;

/// Per-turn streaks of one tool returning one error text.
#[derive(Debug, Default)]
pub struct ErrorStreak {
    counts: std::collections::HashMap<(String, u64), usize>,
    escalator: Escalator,
}

impl ErrorStreak {
    /// Stable fingerprint of an error: first 160 chars, lowercased, digits
    /// dropped so "attempt 3" and "attempt 4" are the same wall.
    pub fn fingerprint(error: &str) -> u64 {
        let norm: String = error
            .chars()
            .take(160)
            .filter(|c| !c.is_ascii_digit())
            .flat_map(char::to_lowercase)
            .collect();
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in norm.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        h
    }

    /// Record one error result. Returns the verdict when the streak reaches
    /// the threshold (every `SAME_ERROR_NUDGE_AFTER` errors), else None.
    pub fn record(&mut self, tool: &str, error: &str) -> Option<Verdict> {
        let key = (tool.to_string(), Self::fingerprint(error));
        let n = self.counts.entry(key.clone()).or_insert(0);
        *n += 1;
        if *n % SAME_ERROR_NUDGE_AFTER == 0 {
            Some(self.escalator.fire(&format!("{}:{}", key.0, key.1)))
        } else {
            None
        }
    }

    pub fn count(&self, tool: &str, error: &str) -> usize {
        self.counts
            .get(&(tool.to_string(), Self::fingerprint(error)))
            .copied()
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod escalation_tests {
    use super::*;

    #[test]
    fn a_nudge_that_did_not_help_is_followed_by_a_stop_never_a_second_nudge() {
        let mut e = Escalator::default();
        assert_eq!(e.fire("os:glob"), Verdict::Nudge);
        assert_eq!(e.fire("os:glob"), Verdict::Stop);
        assert_eq!(e.fire("os:glob"), Verdict::Stop, "never back to a nudge");
        assert_eq!(e.fire("web:fetch"), Verdict::Nudge, "another key gets its own one nudge");
    }

    #[test]
    fn same_error_with_varied_args_stops_after_n() {
        let mut s = ErrorStreak::default();
        let err = "restore needs `checkpoint`: the id from checkpoint/checkpoints";
        for _ in 0..SAME_ERROR_NUDGE_AFTER - 1 {
            assert_eq!(s.record("os", err), None);
        }
        assert_eq!(s.record("os", err), Some(Verdict::Nudge), "third identical error nudges");
        for _ in 0..SAME_ERROR_NUDGE_AFTER - 1 {
            assert_eq!(s.record("os", err), None);
        }
        assert_eq!(s.record("os", err), Some(Verdict::Stop), "the nudge did not help: stop");
        // Numbers inside the text do not make it a different wall.
        assert_eq!(ErrorStreak::fingerprint("attempt 3 failed"), ErrorStreak::fingerprint("attempt 4 failed"));
        // A different error, or the same error from another tool, is its own streak.
        assert_eq!(s.record("os", "path not found: /x"), None);
        assert_eq!(s.record("web", err), None);
        assert_eq!(s.count("os", err), 2 * SAME_ERROR_NUDGE_AFTER);
    }

    /// The exit set is closed: no site in the runner may invent a reason
    /// string, and every label a consumer matches on is produced by a variant.
    #[test]
    fn terminal_reasons_are_a_closed_enum() {
        let runner = include_str!("runner.rs");
        let offenders: Vec<&str> = runner
            .lines()
            .filter(|l| l.contains("turn_exit_reason = \"") || l.contains("turn_exit_reason = format!("))
            .collect();
        assert!(offenders.is_empty(), "free-form exit reasons: {offenders:?}");
        // The strings workflow_loop.rs matches on.
        for (exit, expected) in [
            (Exit::TerminalToolError, "terminal_tool_error"),
            (Exit::RunawayToolLoop, "runaway_tool_loop"),
            (Exit::OutputBudgetExceeded, "output_budget_exceeded"),
            (Exit::UserRequestedStop, "user_requested_stop"),
        ] {
            assert_eq!(exit.label(), expected);
        }
        assert!(Exit::MaxIterations { done: 50, max: 50 }.label().starts_with("max_iterations"));
        assert!(Exit::Workflow("workflow_exit:done".into()).label().starts_with("workflow_exit:"));
        assert_eq!(Exit::Stalled.label(), "stalled");
        assert!(Exit::TextResponse("Some(\"stop\")".into()).is_text_response());
    }

    /// The overflow-compaction retry counter is reset only when the model
    /// actually produced content, never by another recovery's continue: a
    /// compaction that could not recover must not be retried by a nudge.
    #[test]
    fn has_attempted_compact_survives_a_continuation() {
        let runner = include_str!("runner.rs");
        let resets: Vec<usize> = runner
            .lines()
            .enumerate()
            .filter(|(_, l)| l.trim() == "overflow_retries = 0;")
            .map(|(i, _)| i)
            .collect();
        assert_eq!(resets.len(), 1, "exactly one reset site");
        let window: String = runner.lines().skip(resets[0].saturating_sub(6)).take(6).collect::<Vec<_>>().join("\n");
        assert!(
            window.contains("stream_error.is_none() && (!assistant_content.is_empty() || !tool_calls.is_empty())"),
            "the reset is gated on real content:\n{window}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A blocking child's stall must fire before its parent's: otherwise the
    /// parent is ended as stalled while the child is about to report.
    #[test]
    fn run_idle_limit_outlasts_a_child_stall_and_the_notice_states_the_window() {
        assert!(RUN_IDLE_LIMIT > crate::orchestrator::SUBAGENT_INACTIVITY_TIMEOUT);
        let n = stall_notice();
        assert!(n.contains("15 minutes"), "{n}");
        assert!(!n.contains('\u{2014}'), "no em dash in owner copy");
    }

    #[test]
    fn defaults_when_empty_or_invalid() {
        let d = GuardrailConfig::default();
        for raw in ["{}", "", "not json", "[1,2]"] {
            let c = GuardrailConfig::from_json(raw);
            assert_eq!(c.same_action_limit, d.same_action_limit, "raw={raw}");
            assert_eq!(c.max_auto_continuations, d.max_auto_continuations);
            assert!(!c.hard_stop);
        }
    }

    #[test]
    fn partial_json_keeps_other_defaults_and_zero_is_clamped() {
        let c = GuardrailConfig::from_json(r#"{"sameActionLimit": 0, "hardStop": true}"#)
            .sanitized();
        assert_eq!(c.same_action_limit, 2);
        assert!(c.hard_stop);
        assert_eq!(
            c.identical_args_block_after,
            DEFAULT_IDENTICAL_ARGS_BLOCK_AFTER
        );
    }
}
