-- +goose Up
-- One row per completed run. Token counts already flow through the runner and
-- get broadcast to the UI, where they are displayed for five seconds and then
-- discarded — so today we cannot answer what any employee costs, or what it
-- did, once the window closes.
--
-- The two columns that make this a work record rather than a usage log are
-- run_id and outcome. They are here from the start deliberately: nothing else
-- records what a run achieved, so an outcome cannot be backfilled onto history
-- later. See docs/plans/per-run-cost-tracking.md §7.
CREATE TABLE IF NOT EXISTS run_usage (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id TEXT NOT NULL,
    session_key TEXT,
    -- Ties a workflow run to its cost. NULL for a bare chat turn, which has no
    -- workflow_runs row; without it, cost and result are two tables that
    -- cannot be joined and "what did chasing this invoice cost" has no answer.
    run_id TEXT,
    run_type TEXT NOT NULL DEFAULT 'chat',   -- chat | workflow | heartbeat
    model_id TEXT NOT NULL DEFAULT '',
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
    cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
    -- Pre-computed at write time from models.yaml pricing. Integer microcents
    -- (1 = $0.000001) so a month of sub-cent runs cannot accumulate float drift.
    cost_microcents INTEGER NOT NULL DEFAULT 0,
    -- What the run achieved: invoice_chased, appointment_booked, escalated,
    -- no_action, failed... 'no_action' is a real outcome, not a missing row —
    -- a run that checked and correctly found nothing is a healthy employee,
    -- and must never read as silence to a health check.
    outcome TEXT,
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE INDEX IF NOT EXISTS idx_run_usage_agent_created ON run_usage(agent_id, created_at);
CREATE INDEX IF NOT EXISTS idx_run_usage_created ON run_usage(created_at);
-- Billing counts outcomes per agent per period; the partial index keeps that
-- scan off the chat turns, which are the bulk of the rows and carry no outcome.
CREATE INDEX IF NOT EXISTS idx_run_usage_outcome ON run_usage(agent_id, outcome, created_at)
    WHERE outcome IS NOT NULL;

-- +goose Down
DROP TABLE IF EXISTS run_usage;
