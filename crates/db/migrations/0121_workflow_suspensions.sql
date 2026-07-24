-- Headless workflow approval checkpoint: a run that hits a gated operation
-- whose per-employee policy says "Needs approval" suspends instead of failing.
-- The suspension row holds everything needed to resume: the conversation
-- messages, the pending (approved-or-denied) tool call, and its position.
-- One suspension per run; deleted on resume/deny.
CREATE TABLE IF NOT EXISTS workflow_run_suspensions (
    run_id       TEXT PRIMARY KEY,
    agent_id     TEXT NOT NULL,
    binding_name TEXT NOT NULL DEFAULT '',
    activity_id  TEXT NOT NULL,
    step_index   INTEGER,
    messages     TEXT NOT NULL,
    pending_tool TEXT NOT NULL,
    operation    TEXT NOT NULL,
    display      TEXT NOT NULL DEFAULT '',
    created_at   INTEGER NOT NULL DEFAULT (unixepoch())
);

-- Resume needs each completed activity's output to rebuild downstream context
-- (previously only status/tokens were recorded, so a run could not be
-- reconstructed after a restart).
ALTER TABLE workflow_activity_results ADD COLUMN result_content TEXT;
