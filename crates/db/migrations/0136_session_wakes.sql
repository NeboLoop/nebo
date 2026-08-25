-- Session wake rail (R1): the durable write-ahead queue that makes "their
-- reply wakes you automatically" a true sentence. A producer persists the
-- wake FIRST, then attempts delivery; `delivered_at` is stamped only after
-- the woken run completes. Crash between persist and delivery -> the boot
-- sweep redelivers. `attempts` is poison protection: a wake that fails
-- delivery 3 times is stamped with an error note, never boot-looped.
CREATE TABLE session_wakes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_key TEXT NOT NULL,
    kind TEXT NOT NULL,
    payload TEXT NOT NULL,
    -- JSON array of provenance classes; taint rides the wake (WS2).
    provenance TEXT NOT NULL DEFAULT '[]',
    handoff_depth INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    delivered_at INTEGER,
    attempts INTEGER NOT NULL DEFAULT 0,
    note TEXT
);
CREATE INDEX idx_session_wakes_pending ON session_wakes(session_key, id) WHERE delivered_at IS NULL;
