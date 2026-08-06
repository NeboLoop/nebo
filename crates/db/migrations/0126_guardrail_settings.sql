-- Loop-guardrail thresholds for the chat runner (spiral nudge limit,
-- identical-args block threshold, auto-continuation budget, hard-stop
-- opt-in). JSON blob so knobs can evolve without schema churn; '{}' means
-- built-in defaults.
ALTER TABLE settings ADD COLUMN guardrails TEXT NOT NULL DEFAULT '{}';
