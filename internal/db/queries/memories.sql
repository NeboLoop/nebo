-- Memory queries

-- name: ListMemories :many
SELECT id, namespace, key, value, tags, metadata, created_at, updated_at, accessed_at, access_count
FROM memories
ORDER BY
    CASE WHEN namespace LIKE 'tacit.%' THEN 0
         WHEN namespace LIKE 'entity.%' THEN 1
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
        WHEN namespace LIKE 'tacit.%' THEN 'tacit'
        WHEN namespace LIKE 'daily.%' THEN 'daily'
        WHEN namespace LIKE 'entity.%' THEN 'entity'
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
