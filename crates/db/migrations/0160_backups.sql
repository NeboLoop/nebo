-- The snapshot ring: every copy this Nebo has taken of its own database.
--
-- A row is only written after the copy has passed PRAGMA integrity_check, so
-- a row is a promise: this file opens and is whole. The migrator's
-- pre-migration copies are adopted into the ring on the next start, so there
-- is one list of what can be restored, not a table and a folder.
CREATE TABLE IF NOT EXISTS backups (
    id         TEXT PRIMARY KEY,
    path       TEXT NOT NULL UNIQUE,
    taken_at   INTEGER NOT NULL,
    reason     TEXT NOT NULL,           -- nightly | pre-migration | manual
    bytes      INTEGER NOT NULL,
    integrity  TEXT NOT NULL,           -- what PRAGMA integrity_check said ("ok")
    shipped_at INTEGER,                 -- when a copy reached the hub, if it has
    file_id    TEXT                     -- the hub's handle for that copy
);
CREATE INDEX IF NOT EXISTS idx_backups_taken_at ON backups(taken_at DESC);
