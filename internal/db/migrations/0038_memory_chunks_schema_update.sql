-- +goose Up
-- Recreate memory_chunks table:
--   1. Make memory_id nullable (for session transcript chunks)
--   2. Rename start_line/end_line to start_char/end_char

-- Drop FTS triggers first
DROP TRIGGER IF EXISTS memory_chunks_au;
DROP TRIGGER IF EXISTS memory_chunks_ad;
DROP TRIGGER IF EXISTS memory_chunks_ai;

-- Drop FTS virtual table
DROP TABLE IF EXISTS memory_chunks_fts;

-- Drop indexes
DROP INDEX IF EXISTS idx_memory_chunks_user_id;
DROP INDEX IF EXISTS idx_memory_chunks_source;
DROP INDEX IF EXISTS idx_memory_chunks_model;
DROP INDEX IF EXISTS idx_memory_chunks_memory_id;

-- Rename old table
ALTER TABLE memory_chunks RENAME TO memory_chunks_old;

-- Create new table with nullable memory_id and renamed columns
CREATE TABLE memory_chunks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    memory_id INTEGER REFERENCES memories(id) ON DELETE CASCADE,
    chunk_index INTEGER NOT NULL,
    text TEXT NOT NULL,
    source TEXT DEFAULT 'memory',
    path TEXT DEFAULT '',
    start_char INTEGER DEFAULT 0,
    end_char INTEGER DEFAULT 0,
    model TEXT DEFAULT '',
    user_id TEXT NOT NULL DEFAULT '',
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- Copy data (map old column names to new)
INSERT INTO memory_chunks (id, memory_id, chunk_index, text, source, path, start_char, end_char, model, user_id, created_at)
SELECT id, memory_id, chunk_index, text, source, path, start_line, end_line, model, user_id, created_at
FROM memory_chunks_old;

-- Drop old table
DROP TABLE memory_chunks_old;

-- Recreate indexes
CREATE INDEX idx_memory_chunks_memory_id ON memory_chunks(memory_id);
CREATE INDEX idx_memory_chunks_model ON memory_chunks(model);
CREATE INDEX idx_memory_chunks_source ON memory_chunks(source);
CREATE INDEX idx_memory_chunks_user_id ON memory_chunks(user_id);

-- Recreate FTS virtual table
CREATE VIRTUAL TABLE memory_chunks_fts USING fts5(
    text, source, path,
    content='memory_chunks',
    content_rowid='id'
);

-- Rebuild FTS index from existing data
INSERT INTO memory_chunks_fts(rowid, text, source, path)
SELECT id, text, source, path FROM memory_chunks;

-- +goose StatementBegin
CREATE TRIGGER memory_chunks_ai AFTER INSERT ON memory_chunks BEGIN
    INSERT INTO memory_chunks_fts(rowid, text, source, path)
    VALUES (new.id, new.text, new.source, new.path);
END;
-- +goose StatementEnd

-- +goose StatementBegin
CREATE TRIGGER memory_chunks_ad AFTER DELETE ON memory_chunks BEGIN
    INSERT INTO memory_chunks_fts(memory_chunks_fts, rowid, text, source, path)
    VALUES ('delete', old.id, old.text, old.source, old.path);
END;
-- +goose StatementEnd

-- +goose StatementBegin
CREATE TRIGGER memory_chunks_au AFTER UPDATE ON memory_chunks BEGIN
    INSERT INTO memory_chunks_fts(memory_chunks_fts, rowid, text, source, path)
    VALUES ('delete', old.id, old.text, old.source, old.path);
    INSERT INTO memory_chunks_fts(rowid, text, source, path)
    VALUES (new.id, new.text, new.source, new.path);
END;
-- +goose StatementEnd

-- +goose Down
-- Reverse: rename columns back, make memory_id NOT NULL again
-- For down migration, drop and recreate (no data preservation needed in dev)
DROP TRIGGER IF EXISTS memory_chunks_au;
DROP TRIGGER IF EXISTS memory_chunks_ad;
DROP TRIGGER IF EXISTS memory_chunks_ai;
DROP TABLE IF EXISTS memory_chunks_fts;
DROP INDEX IF EXISTS idx_memory_chunks_user_id;
DROP INDEX IF EXISTS idx_memory_chunks_source;
DROP INDEX IF EXISTS idx_memory_chunks_model;
DROP INDEX IF EXISTS idx_memory_chunks_memory_id;
DROP TABLE IF EXISTS memory_chunks;

CREATE TABLE memory_chunks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    memory_id INTEGER REFERENCES memories(id) ON DELETE CASCADE,
    chunk_index INTEGER NOT NULL,
    text TEXT NOT NULL,
    source TEXT DEFAULT 'memory',
    path TEXT DEFAULT '',
    start_line INTEGER DEFAULT 0,
    end_line INTEGER DEFAULT 0,
    model TEXT DEFAULT '',
    user_id TEXT NOT NULL DEFAULT '',
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_memory_chunks_memory_id ON memory_chunks(memory_id);
CREATE INDEX idx_memory_chunks_model ON memory_chunks(model);
CREATE INDEX idx_memory_chunks_source ON memory_chunks(source);
CREATE INDEX idx_memory_chunks_user_id ON memory_chunks(user_id);

CREATE VIRTUAL TABLE memory_chunks_fts USING fts5(
    text, source, path,
    content='memory_chunks',
    content_rowid='id'
);
