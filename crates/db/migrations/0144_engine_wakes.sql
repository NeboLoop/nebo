-- +goose Up
-- First conversion onto the engine: session wakes. Every undelivered wake
-- becomes an engine event aimed at its session (target_type 'session'),
-- attempts and provenance carried, so a wake persisted before this upgrade
-- is delivered once by the rail after it — same write-ahead discipline,
-- one table. Delivered rows are history nobody reads; they are not kept.
INSERT INTO engine_events
    (kind, target_type, target_id, payload, idem_key, provenance, handoff_depth, retention, created_at, attempts, note)
SELECT kind, 'session', session_key, payload, 'wake:legacy:' || id, provenance, handoff_depth, 'transient', created_at, attempts, note
FROM session_wakes
WHERE delivered_at IS NULL;

DROP TABLE IF EXISTS session_wakes;

-- +goose Down
CREATE TABLE IF NOT EXISTS session_wakes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_key TEXT NOT NULL,
    kind TEXT NOT NULL,
    payload TEXT NOT NULL,
    provenance TEXT NOT NULL DEFAULT '[]',
    handoff_depth INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    delivered_at INTEGER,
    attempts INTEGER NOT NULL DEFAULT 0,
    note TEXT
);
CREATE INDEX IF NOT EXISTS idx_session_wakes_pending ON session_wakes(session_key, id) WHERE delivered_at IS NULL;
