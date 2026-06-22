-- Fix memory_embeddings dangling foreign key (regression from migration 0038).
--
-- 0038 ran `ALTER TABLE memory_chunks RENAME TO memory_chunks_old`. SQLite
-- propagates a table rename into the FK definitions of dependent tables, so
-- memory_embeddings.chunk_id — which referenced memory_chunks — was silently
-- rewritten to reference memory_chunks_old. 0038 then dropped memory_chunks_old,
-- leaving memory_embeddings with a foreign key to a table that no longer exists.
-- With FK enforcement on, every embedding insert fails ("no such table:
-- memory_chunks_old"), silently disabling semantic/vector memory recall (FTS
-- keyword recall still works, which is why it went unnoticed).
--
-- memory_embeddings has had 0 rows ever since (no embedding could insert), so we
-- simply drop and recreate it with the FK pointing at the live memory_chunks.
DROP TABLE IF EXISTS memory_embeddings;
CREATE TABLE memory_embeddings (
    id INTEGER PRIMARY KEY,
    chunk_id INTEGER REFERENCES memory_chunks(id) ON DELETE CASCADE,
    model TEXT NOT NULL,
    dimensions INTEGER NOT NULL,
    embedding BLOB NOT NULL,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_memory_embeddings_chunk ON memory_embeddings(chunk_id);
