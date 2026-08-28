//! Per-turn identical-call budget — the ONE implementation shared by every
//! agentic loop (chat runner AND workflow activities).
//!
//! Why it counts the CALL, not the answer: every result-keyed repetition
//! guard is defeated by polling — `docker compose logs`, `tail`, a status
//! endpoint — because the bytes drift each call, so nothing is ever flagged
//! unproductive. Live-verified 2026-08-27: 16 identical polls against a
//! growing log produced ZERO guard firings, and the customer incident this
//! reproduces ran 12,093 requests in 24h.
//!
//! Why the ceiling is a parameter set by EVIDENCE: the incident's own
//! legitimate debugging repeated one command 13 times in a single turn, so
//! callers must pass a bound that clears their observed legitimate maximum
//! with margin (the chat runner passes 16). An abort ends the TURN or the
//! ACTIVITY with an honest message — never a session, never silently.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

/// Key for one exact call: tool name + exact serialized arguments.
fn key(name: &str, input: &serde_json::Value) -> (u64, u64) {
    let mut h1 = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut h1);
    let mut h2 = std::collections::hash_map::DefaultHasher::new();
    input.to_string().hash(&mut h2);
    (h1.finish(), h2.finish())
}

/// Per-turn budget over exact (tool, arguments) pairs. Create one per
/// turn/activity; never share across turns (files and world state
/// legitimately change between them).
#[derive(Debug, Default)]
pub struct CallBudget {
    counts: HashMap<(u64, u64), usize>,
}

impl CallBudget {
    pub fn new() -> Self {
        Self::default()
    }

    /// Count this call against the budget.
    pub fn record(&mut self, name: &str, input: &serde_json::Value) {
        *self.counts.entry(key(name, input)).or_insert(0) += 1;
    }

    /// Whether this exact call has already been made `ceiling` times —
    /// returns the repeat count when the caller must END the turn/activity.
    /// Deliberately ignores the result: that is the whole point.
    pub fn abort_due(
        &self,
        name: &str,
        input: &serde_json::Value,
        ceiling: usize,
    ) -> Option<usize> {
        let repeats = self.counts.get(&key(name, input)).copied().unwrap_or(0);
        (repeats >= ceiling).then_some(repeats)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The budget counts the CALL, not the answer — recording the same call
    /// `ceiling` times trips the abort even though no result was ever seen.
    #[test]
    fn identical_calls_trip_the_ceiling_regardless_of_results() {
        let mut b = CallBudget::new();
        let input = serde_json::json!({"action":"exec","command":"docker compose logs"});
        for _ in 0..3 {
            b.record("os", &input);
        }
        assert!(b.abort_due("os", &input, 4).is_none(), "under the ceiling");
        b.record("os", &input);
        assert_eq!(b.abort_due("os", &input, 4), Some(4), "at the ceiling");
    }

    /// Different arguments never share a budget — varying the call is the
    /// legitimate escape hatch and must always work.
    #[test]
    fn different_arguments_have_independent_budgets() {
        let mut b = CallBudget::new();
        for i in 0..10 {
            b.record("os", &serde_json::json!({"cmd": format!("step {i}")}));
        }
        assert!(b
            .abort_due("os", &serde_json::json!({"cmd": "step 0"}), 2)
            .is_none());
    }

    /// The tool name is part of the key — two tools with identical arguments
    /// accrue separately.
    #[test]
    fn tool_name_is_part_of_the_key() {
        let mut b = CallBudget::new();
        let input = serde_json::json!({"path": "/a.py"});
        b.record("os", &input);
        b.record("web", &input);
        assert!(b.abort_due("os", &input, 2).is_none());
    }
}
