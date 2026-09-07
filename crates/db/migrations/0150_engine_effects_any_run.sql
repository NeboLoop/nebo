-- +goose Up
-- An effect is any outbound side effect an employee performs — an email, a
-- text, a charge — from a case turn, a workflow, or a chat. Not every send
-- happens inside an engine run, so the ledger no longer requires one:
-- `run_id` is the run when there is one and the session otherwise. The
-- table is rebuilt without the foreign key; rows carry over.
CREATE TABLE engine_effects_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id TEXT NOT NULL,
    class TEXT NOT NULL DEFAULT 'messaging',
    idem_key TEXT NOT NULL UNIQUE,
    provider TEXT NOT NULL DEFAULT '',
    provider_key TEXT NOT NULL DEFAULT '',
    state TEXT NOT NULL DEFAULT 'pending',
    attempts INTEGER NOT NULL DEFAULT 0,
    amount TEXT,
    currency TEXT,
    counterparty TEXT,
    provider_ref TEXT,
    result TEXT,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    completed_at INTEGER
);
INSERT INTO engine_effects_new SELECT id, run_id, class, idem_key, provider, provider_key, state, attempts, amount, currency, counterparty, provider_ref, result, created_at, completed_at FROM engine_effects;
DROP TABLE engine_effects;
ALTER TABLE engine_effects_new RENAME TO engine_effects;
CREATE INDEX IF NOT EXISTS idx_engine_effects_pending ON engine_effects(run_id) WHERE state = 'pending';

-- +goose Down
