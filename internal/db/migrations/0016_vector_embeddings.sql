-- +goose Up
-- Vector embeddings for hybrid search in memory tool

-- Memory chunks for chunked text storage
CREATE TABLE IF NOT EXISTS memory_chunks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    memory_id INTEGER REFERENCES memories(id) ON DELETE CASCADE,
    chunk_index INTEGER NOT NULL,
    text TEXT NOT NULL,
    source TEXT DEFAULT 'memory',
    path TEXT DEFAULT '',
    start_line INTEGER DEFAULT 0,
    end_line INTEGER DEFAULT 0,
    model TEXT DEFAULT '',
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_memory_chunks_memory_id ON memory_chunks(memory_id);
CREATE INDEX IF NOT EXISTS idx_memory_chunks_model ON memory_chunks(model);
CREATE INDEX IF NOT EXISTS idx_memory_chunks_source ON memory_chunks(source);

-- FTS for chunks (extends existing FTS pattern)
CREATE VIRTUAL TABLE IF NOT EXISTS memory_chunks_fts USING fts5(
    text, source, path,
    content='memory_chunks',
    content_rowid='id'
);

-- Triggers to keep FTS in sync
-- +goose StatementBegin
CREATE TRIGGER IF NOT EXISTS memory_chunks_ai AFTER INSERT ON memory_chunks BEGIN
    INSERT INTO memory_chunks_fts(rowid, text, source, path)
    VALUES (new.id, new.text, new.source, new.path);
END;
-- +goose StatementEnd

-- +goose StatementBegin
CREATE TRIGGER IF NOT EXISTS memory_chunks_ad AFTER DELETE ON memory_chunks BEGIN
    INSERT INTO memory_chunks_fts(memory_chunks_fts, rowid, text, source, path)
    VALUES ('delete', old.id, old.text, old.source, old.path);
END;
-- +goose StatementEnd

-- +goose StatementBegin
CREATE TRIGGER IF NOT EXISTS memory_chunks_au AFTER UPDATE ON memory_chunks BEGIN
    INSERT INTO memory_chunks_fts(memory_chunks_fts, rowid, text, source, path)
    VALUES ('delete', old.id, old.text, old.source, old.path);
    INSERT INTO memory_chunks_fts(rowid, text, source, path)
    VALUES (new.id, new.text, new.source, new.path);
END;
-- +goose StatementEnd

-- Embedding cache for deduplication
CREATE TABLE IF NOT EXISTS embedding_cache (
    content_hash TEXT PRIMARY KEY,
    embedding BLOB NOT NULL,
    model TEXT NOT NULL,
    dimensions INTEGER NOT NULL,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- Vector embeddings table (requires sqlite-vec extension)
-- Note: This table will only work if sqlite-vec is loaded
-- The application should check for extension availability
CREATE TABLE IF NOT EXISTS memory_embeddings (
    id INTEGER PRIMARY KEY,
    chunk_id INTEGER REFERENCES memory_chunks(id) ON DELETE CASCADE,
    model TEXT NOT NULL,
    dimensions INTEGER NOT NULL,
    embedding BLOB NOT NULL,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_memory_embeddings_chunk_id ON memory_embeddings(chunk_id);
CREATE INDEX IF NOT EXISTS idx_memory_embeddings_model ON memory_embeddings(model);

-- +goose Down
DROP INDEX IF EXISTS idx_memory_embeddings_model;
DROP INDEX IF EXISTS idx_memory_embeddings_chunk_id;
DROP TABLE IF EXISTS memory_embeddings;
DROP TABLE IF EXISTS embedding_cache;
DROP TRIGGER IF EXISTS memory_chunks_au;
DROP TRIGGER IF EXISTS memory_chunks_ad;
DROP TRIGGER IF EXISTS memory_chunks_ai;
DROP TABLE IF EXISTS memory_chunks_fts;
DROP INDEX IF EXISTS idx_memory_chunks_source;
DROP INDEX IF EXISTS idx_memory_chunks_model;
DROP INDEX IF EXISTS idx_memory_chunks_memory_id;
DROP TABLE IF EXISTS memory_chunks;
