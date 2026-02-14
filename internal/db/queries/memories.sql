-- Memory queries

-- name: ListMemories :many
SELECT id, namespace, key, value, tags, metadata, created_at, updated_at, accessed_at, access_count
FROM memories
ORDER BY
    CASE WHEN namespace LIKE 'tacit/%' THEN 0
         WHEN namespace LIKE 'entity/%' THEN 1
         ELSE 2 END,
    access_count DESC
LIMIT sqlc.arg(limit) OFFSET sqlc.arg(offset);

-- name: ListMemoriesByNamespace :many
SELECT id, namespace, key, value, tags, metadata, created_at, updated_at, accessed_at, access_count
FROM memories
WHERE namespace LIKE sqlc.arg(namespace_prefix) || '%'
ORDER BY access_count DESC
LIMIT sqlc.arg(limit) OFFSET sqlc.arg(offset);

-- name: GetMemory :one
SELECT id, namespace, key, value, tags, metadata, created_at, updated_at, accessed_at, access_count
FROM memories
WHERE id = ?;

-- name: GetMemoryByKey :one
SELECT id, namespace, key, value, tags, metadata, created_at, updated_at, accessed_at, access_count
FROM memories
WHERE namespace = ? AND key = ?;

-- name: UpdateMemory :exec
UPDATE memories
SET value = COALESCE(sqlc.narg(value), value),
    tags = COALESCE(sqlc.narg(tags), tags),
    metadata = COALESCE(sqlc.narg(metadata), metadata),
    updated_at = CURRENT_TIMESTAMP
WHERE id = sqlc.arg(id);

-- name: DeleteMemory :exec
DELETE FROM memories WHERE id = ?;

-- name: SearchMemories :many
SELECT id, namespace, key, value, tags, metadata, created_at, updated_at, accessed_at, access_count
FROM memories
WHERE key LIKE '%' || sqlc.arg(query) || '%'
   OR value LIKE '%' || sqlc.arg(query) || '%'
   OR tags LIKE '%' || sqlc.arg(query) || '%'
ORDER BY access_count DESC
LIMIT sqlc.arg(limit) OFFSET sqlc.arg(offset);

-- name: CountMemories :one
SELECT COUNT(*) as total FROM memories;

-- name: CountMemoriesByNamespace :one
SELECT COUNT(*) as total FROM memories
WHERE namespace LIKE sqlc.arg(namespace_prefix) || '%';

-- name: GetMemoryStats :many
SELECT
    CASE
        WHEN namespace LIKE 'tacit/%' THEN 'tacit'
        WHEN namespace LIKE 'daily/%' THEN 'daily'
        WHEN namespace LIKE 'entity/%' THEN 'entity'
        ELSE 'other'
    END as layer,
    COUNT(*) as count
FROM memories
GROUP BY 1
ORDER BY 1;

-- name: IncrementMemoryAccess :exec
UPDATE memories
SET access_count = access_count + 1,
    accessed_at = CURRENT_TIMESTAMP
WHERE id = ?;

-- name: GetDistinctNamespaces :many
SELECT DISTINCT namespace FROM memories ORDER BY namespace;

-- User-scoped memory queries for agent tools

-- name: UpsertMemory :exec
INSERT INTO memories (namespace, key, value, tags, metadata, user_id, created_at, updated_at)
VALUES (?, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
ON CONFLICT(namespace, key, user_id) DO UPDATE SET
    value = excluded.value,
    tags = excluded.tags,
    metadata = excluded.metadata,
    updated_at = CURRENT_TIMESTAMP;

-- name: GetMemoryByKeyAndUser :one
SELECT id, namespace, key, value, tags, metadata, created_at, updated_at, accessed_at, access_count
FROM memories
WHERE namespace = ? AND key = ? AND user_id = ?;

-- name: SearchMemoriesByUser :many
SELECT id, namespace, key, value, tags, metadata, created_at, updated_at, accessed_at, access_count
FROM memories
WHERE user_id = sqlc.arg(user_id)
  AND (namespace LIKE '%' || sqlc.arg(query) || '%'
       OR key LIKE '%' || sqlc.arg(query) || '%'
       OR value LIKE '%' || sqlc.arg(query) || '%')
ORDER BY access_count DESC
LIMIT sqlc.arg(limit) OFFSET sqlc.arg(offset);

-- name: SearchMemoriesByUserAndNamespace :many
SELECT id, namespace, key, value, tags, metadata, created_at, updated_at, accessed_at, access_count
FROM memories
WHERE user_id = sqlc.arg(user_id) AND namespace LIKE sqlc.arg(namespace_prefix) || '%'
  AND (key LIKE '%' || sqlc.arg(query) || '%'
       OR value LIKE '%' || sqlc.arg(query) || '%')
ORDER BY access_count DESC
LIMIT sqlc.arg(limit) OFFSET sqlc.arg(offset);

-- name: ListMemoriesByUserAndNamespace :many
SELECT id, namespace, key, value, tags, metadata, created_at, updated_at, accessed_at, access_count
FROM memories
WHERE user_id = sqlc.arg(user_id) AND namespace LIKE sqlc.arg(namespace_prefix) || '%'
ORDER BY access_count DESC
LIMIT sqlc.arg(limit) OFFSET sqlc.arg(offset);

-- name: DeleteMemoryByKeyAndUser :execresult
DELETE FROM memories
WHERE namespace = ? AND key = ? AND user_id = ?;

-- name: DeleteMemoriesByNamespaceAndUser :execresult
DELETE FROM memories
WHERE namespace LIKE sqlc.arg(namespace_prefix) || '%' AND user_id = sqlc.arg(user_id);

-- name: IncrementMemoryAccessByKey :exec
UPDATE memories
SET access_count = access_count + 1,
    accessed_at = CURRENT_TIMESTAMP
WHERE namespace = ? AND key = ? AND user_id = ?;

-- name: GetMemoryByKeyAndUserAnyNamespace :one
SELECT id, namespace, key, value, tags, metadata, created_at, updated_at, accessed_at, access_count
FROM memories
WHERE key = ? AND user_id = ?
ORDER BY access_count DESC
LIMIT 1;

-- name: DeleteMemoryByKeyAndUserAnyNamespace :execresult
DELETE FROM memories
WHERE key = ? AND user_id = ?;

-- name: GetTacitMemoriesByUser :many
SELECT id, namespace, key, value, tags, metadata, created_at, updated_at, accessed_at, access_count
FROM memories
WHERE namespace LIKE 'tacit/%' AND user_id = ?
ORDER BY access_count DESC
LIMIT sqlc.arg(limit);
