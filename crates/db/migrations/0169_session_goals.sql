-- A session's agreed goal: the end state its work continues toward until a
-- separate done check confirms it from the conversation. One goal per
-- session. `declined` (JSON array) keeps the conditions the owner turned down
-- so they are never suggested again; it outlives a cleared goal.
CREATE TABLE IF NOT EXISTS session_goals (
    session_id  TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
    condition   TEXT NOT NULL DEFAULT '',
    source      TEXT NOT NULL DEFAULT '',
    status      TEXT NOT NULL DEFAULT 'cleared',
    turns       INTEGER NOT NULL DEFAULT 0,
    last_reason TEXT,
    declined    TEXT NOT NULL DEFAULT '[]',
    created_at  INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at  INTEGER NOT NULL DEFAULT (unixepoch())
);
