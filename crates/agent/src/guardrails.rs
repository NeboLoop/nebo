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

#[cfg(test)]
mod tests {
    use super::*;

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
