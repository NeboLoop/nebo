-- The approval ledger: the evidence that earns (and revokes) autonomy.
--
-- Approvals used to resolve and disappear — the suspension row is deleted on
-- approve/deny, leaving no record of how often an action class was approved
-- unchanged, denied, or later rolled back. Progressive autonomy ("remove
-- gates one class at a time once a workflow has proven itself") is a
-- measurement, and this table is where the measurement lives.
--
-- Append-only. A decision is a historical fact; corrections are new rows
-- (kind='rollback' against an earlier approval), never edits.
CREATE TABLE IF NOT EXISTS approval_ledger (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id    TEXT NOT NULL,
    -- The action class, as an operation suffix (e.g. cms.page.create).
    -- Ledger evidence is per (agent, operation) — the same shape the
    -- OperationPolicy gates on, so evidence maps 1:1 onto a gate.
    operation   TEXT NOT NULL,
    -- approved | denied | rollback.
    -- 'rollback' is recorded when a previously approved (or autonomous)
    -- action turned out bad and was reverted — it is the strongest negative
    -- signal and resets any graduation streak.
    kind        TEXT NOT NULL CHECK (kind IN ('approved','denied','rollback')),
    -- Whether the human materially edited the action before approving.
    -- The current approve API has no edit affordance, so this is 0 today;
    -- the column exists so the day it does, the evidence is already shaped.
    edited      INTEGER NOT NULL DEFAULT 0,
    -- Where it happened, for drill-down and dispute.
    run_id      TEXT NOT NULL DEFAULT '',
    binding     TEXT NOT NULL DEFAULT '',
    -- Which asset the action touched (extracted from run inputs when the
    -- workflow carries one). Graduation requires spread across assets —
    -- one friendly asset must not buy autonomy for the whole portfolio.
    asset       TEXT NOT NULL DEFAULT '',
    -- The human-readable sentence that was approved/denied, for the audit
    -- trail ("what exactly did I say yes to in March").
    display     TEXT NOT NULL DEFAULT '',
    decided_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_approval_ledger_agent_op
    ON approval_ledger(agent_id, operation, id DESC);
