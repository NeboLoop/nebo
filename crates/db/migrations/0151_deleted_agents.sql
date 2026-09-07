-- +goose Up
-- Deleting an employee removes what is the employee's own (its row,
-- configuration, memory, working state) and NOTHING that belongs to the
-- business: cases, turns, sent communications, approvals, the effect
-- ledger. Those rows keep their agent_id; this table keeps the name that
-- id had, so history can still say who did the work. Purging business
-- data is a separate, explicit, destructive operation.
CREATE TABLE IF NOT EXISTS deleted_agents (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    deleted_at INTEGER NOT NULL DEFAULT (unixepoch()),
    purged_at INTEGER
);

-- +goose Down
