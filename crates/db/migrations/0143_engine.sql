-- +goose Up
-- The ONE durable-work engine (design of record: "One Engine for Durable Work",
-- 2026-09-06). Cron jobs, session wakes, pending tasks, workflow runs,
-- suspensions, the heartbeat scheduler and the two dedupe tables are each a
-- slice of "something durable will cause a run, or a run is waiting for
-- something durable". These tables hold the whole of it. This migration
-- creates them dark — nothing is converted or dropped here; the conversion
-- migration lands once the engine has run under the existing traffic.

-- An event: a timer that becomes due, a signal that arrived, an approval, a
-- coworker reply, an owner note. Delivery is at-least-once under a lease;
-- processing is idempotent, so a redelivery changes nothing observable.
CREATE TABLE IF NOT EXISTS engine_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL,
    -- session | run | wait | binding | entity
    target_type TEXT NOT NULL,
    target_id TEXT NOT NULL,
    payload TEXT NOT NULL DEFAULT '',
    channel TEXT NOT NULL DEFAULT '',
    ref TEXT NOT NULL DEFAULT '',
    -- Unique across all time: replays, double-submits and hub reconnects
    -- are recorded as duplicates and never delivered.
    idem_key TEXT NOT NULL UNIQUE,
    provenance TEXT NOT NULL DEFAULT '[]',
    handoff_depth INTEGER NOT NULL DEFAULT 0,
    -- transient rows age out on the TTL; durable rows are history.
    retention TEXT NOT NULL DEFAULT 'transient',
    -- Timers: deliverable once due_at <= now. Recurring timers carry a
    -- schedule; delivering one emits the next occurrence.
    due_at INTEGER,
    schedule TEXT,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    claimed_at INTEGER,
    lease_until INTEGER,
    delivered_at INTEGER,
    attempts INTEGER NOT NULL DEFAULT 0,
    note TEXT
);
CREATE INDEX IF NOT EXISTS idx_engine_events_deliverable
    ON engine_events(due_at, id) WHERE delivered_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_engine_events_target
    ON engine_events(target_type, target_id, id);

-- A run: something executing, queued to execute, or waiting to be woken.
CREATE TABLE IF NOT EXISTS engine_runs (
    id TEXT PRIMARY KEY,
    -- chat | workflow | task | heartbeat | case | case_turn
    kind TEXT NOT NULL,
    -- queued | running | waiting | interrupted | done | failed | cancelled
    state TEXT NOT NULL DEFAULT 'queued',
    session_key TEXT NOT NULL,
    agent_id TEXT NOT NULL DEFAULT '',
    lane TEXT NOT NULL DEFAULT 'main',
    parent_run_id TEXT REFERENCES engine_runs(id) ON DELETE SET NULL,
    definition TEXT,
    inputs TEXT,
    current_wait_id INTEGER,
    attempts INTEGER NOT NULL DEFAULT 0,
    resume_attempted INTEGER NOT NULL DEFAULT 0,
    result TEXT,
    error TEXT,
    display TEXT,
    summary TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    started_at INTEGER,
    ended_at INTEGER
);
CREATE INDEX IF NOT EXISTS idx_engine_runs_state ON engine_runs(state, lane, created_at);
CREATE INDEX IF NOT EXISTS idx_engine_runs_session ON engine_runs(session_key, state);
CREATE INDEX IF NOT EXISTS idx_engine_runs_parent ON engine_runs(parent_run_id);

-- A wait: the ONE thing a waiting run will be woken by. `resume` continues
-- the same run mid-step with its parked messages; `trigger_child` leaves
-- the run waiting and starts a child. Every wait is a generation: a run's
-- current_wait_id names the live one, and an event aimed at an older wait
-- is superseded, never delivered.
CREATE TABLE IF NOT EXISTS engine_waits (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id TEXT NOT NULL REFERENCES engine_runs(id) ON DELETE CASCADE,
    -- resume | trigger_child
    action TEXT NOT NULL,
    -- event kind to match, or 'any'
    on_kind TEXT NOT NULL DEFAULT 'any',
    key TEXT NOT NULL DEFAULT '',
    deadline INTEGER,
    parked TEXT,
    reason TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    superseded_at INTEGER
);
CREATE INDEX IF NOT EXISTS idx_engine_waits_live
    ON engine_waits(on_kind, key) WHERE superseded_at IS NULL;

-- Keys that name the thing a run works: an email, a phone, an external id,
-- a hub conversation. One OPEN run per key, enforced here and not by
-- looking at the run's state: closing or merging releases the key in the
-- same transaction.
CREATE TABLE IF NOT EXISTS engine_run_keys (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id TEXT NOT NULL REFERENCES engine_runs(id) ON DELETE CASCADE,
    key_type TEXT NOT NULL,
    key_value TEXT NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    released_at INTEGER
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_engine_run_keys_active
    ON engine_run_keys(key_type, key_value) WHERE released_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_engine_run_keys_run ON engine_run_keys(run_id);

-- An effect: an outbound side effect (an email, a text, a calendar write, a
-- charge). Pending BEFORE it acts, completed after. Recovery reconciles or
-- retries pending effects under the same provider key; it never assumes
-- pending means done, and it never acts twice on a completed one.
CREATE TABLE IF NOT EXISTS engine_effects (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id TEXT NOT NULL REFERENCES engine_runs(id) ON DELETE CASCADE,
    -- messaging | financial
    class TEXT NOT NULL DEFAULT 'messaging',
    idem_key TEXT NOT NULL UNIQUE,
    provider TEXT NOT NULL DEFAULT '',
    provider_key TEXT NOT NULL DEFAULT '',
    -- pending | completed | failed
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
CREATE INDEX IF NOT EXISTS idx_engine_effects_pending
    ON engine_effects(run_id) WHERE state = 'pending';

-- +goose Down
DROP TABLE IF EXISTS engine_effects;
DROP TABLE IF EXISTS engine_run_keys;
DROP TABLE IF EXISTS engine_waits;
DROP TABLE IF EXISTS engine_runs;
DROP TABLE IF EXISTS engine_events;
