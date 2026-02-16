-- Embedding cache queries

-- name: GetEmbeddingCache :one
SELECT content_hash, embedding, model, dimensions, created_at
FROM embedding_cache
WHERE content_hash = ? AND model = ?;

-- name: UpsertEmbeddingCache :exec
INSERT INTO embedding_cache (content_hash, embedding, model, dimensions, created_at)
VALUES (?, ?, ?, ?, CURRENT_TIMESTAMP)
ON CONFLICT(content_hash) DO UPDATE SET
    embedding = excluded.embedding,
    model = excluded.model,
    dimensions = excluded.dimensions,
    created_at = excluded.created_at;

-- name: DeleteEmbeddingCache :exec
DELETE FROM embedding_cache WHERE content_hash = ?;

-- name: CleanOldEmbeddingCache :exec
DELETE FROM embedding_cache WHERE created_at < ?;

-- Memory chunks queries

-- name: CreateMemoryChunk :one
INSERT INTO memory_chunks (memory_id, chunk_index, text, source, path, start_char, end_char, model, user_id)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
RETURNING id, memory_id, chunk_index, text, source, path, start_char, end_char, model, user_id, created_at;

-- name: GetMemoryChunk :one
SELECT id, memory_id, chunk_index, text, source, path, start_char, end_char, model, created_at
FROM memory_chunks
WHERE id = ?;

-- name: ListMemoryChunks :many
SELECT id, memory_id, chunk_index, text, source, path, start_char, end_char, model, created_at
FROM memory_chunks
WHERE memory_id = ?
ORDER BY chunk_index;

-- name: DeleteMemoryChunks :exec
DELETE FROM memory_chunks WHERE memory_id = ?;

-- Note: FTS queries use raw SQL in hybrid.go because sqlc doesn't support virtual tables

-- Memory embeddings queries

-- name: CreateMemoryEmbedding :one
INSERT INTO memory_embeddings (chunk_id, model, dimensions, embedding)
VALUES (?, ?, ?, ?)
RETURNING id, chunk_id, model, dimensions, embedding, created_at;

-- name: GetMemoryEmbedding :one
SELECT id, chunk_id, model, dimensions, embedding, created_at
FROM memory_embeddings
WHERE chunk_id = ? AND model = ?;

-- name: DeleteMemoryEmbeddings :exec
DELETE FROM memory_embeddings WHERE chunk_id = ?;

-- name: DeleteMemoryEmbeddingsByModel :exec
DELETE FROM memory_embeddings WHERE model = ?;

-- name: ListMemoryEmbeddingsByModel :many
SELECT me.id, me.chunk_id, me.model, me.dimensions, me.embedding, me.created_at,
       mc.text, mc.source, mc.path, mc.memory_id
FROM memory_embeddings me
JOIN memory_chunks mc ON mc.id = me.chunk_id
WHERE me.model = ?
ORDER BY me.id
LIMIT ? OFFSET ?;

-- name: CountMemoryEmbeddings :one
SELECT COUNT(*) as total FROM memory_embeddings WHERE model = ?;
