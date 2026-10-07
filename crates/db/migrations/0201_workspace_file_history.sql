-- Nothing in the owner's workspace (<data_dir>/files) is lost without
-- history. Nebo can't see a shell's or a script's writes, so it looks at the
-- workspace around every turn (`tools::workspace_history`): every file's
-- current bytes are kept in the content-addressed blob store
-- (files/work/blobs/<hash>.<ext>), and a file found changed or gone since the
-- last look keeps what it was here.
--
-- 2026-10-06: an employee overwrote the owner's 36 KB, 9-sheet workbook with
-- a 7.9 KB partial through a command; the earlier content was simply gone.
-- +goose Up

-- What each workspace file was at the last look: its size and mtime decide
-- whether it needs hashing again, its hash names its kept blob.
CREATE TABLE IF NOT EXISTS workspace_index (
    path        TEXT PRIMARY KEY,   -- relative to <data_dir>/files, '/'-separated
    size_bytes  INTEGER NOT NULL,
    mtime_ns    INTEGER NOT NULL,
    hash        TEXT NOT NULL,      -- SHA-256 of the content, its blob's name
    ext         TEXT NOT NULL
);

-- A file's earlier content: what it was before it changed or went away.
-- One row per (path, content): the same bytes overwritten twice are one
-- restorable entry, seen last at `captured_at`. Purgeable by age or size with
-- one DELETE on captured_at / size_bytes.
CREATE TABLE IF NOT EXISTS file_history (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    path        TEXT NOT NULL,
    hash        TEXT NOT NULL,
    ext         TEXT NOT NULL,
    size_bytes  INTEGER NOT NULL,
    reason      TEXT NOT NULL,      -- modified | deleted
    chat_id     TEXT,               -- the conversation whose turn saw the change
    captured_at INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE (path, hash)
);
CREATE INDEX IF NOT EXISTS idx_file_history_path ON file_history(path, captured_at);
CREATE INDEX IF NOT EXISTS idx_file_history_captured ON file_history(captured_at);

-- +goose Down
DROP TABLE IF EXISTS file_history;
DROP TABLE IF EXISTS workspace_index;
