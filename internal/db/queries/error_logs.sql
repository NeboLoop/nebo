-- Error log queries

-- name: InsertErrorLog :exec
INSERT INTO error_logs (level, module, message, stacktrace, context)
VALUES (?, ?, ?, ?, ?);

-- name: ListErrorLogs :many
SELECT id, timestamp, level, module, message, stacktrace, context, resolved
FROM error_logs
ORDER BY timestamp DESC
LIMIT ? OFFSET ?;

-- name: ListErrorLogsByLevel :many
SELECT id, timestamp, level, module, message, stacktrace, context, resolved
FROM error_logs
WHERE level = ?
ORDER BY timestamp DESC
LIMIT ? OFFSET ?;

-- name: CountErrorLogs :one
SELECT COUNT(*) FROM error_logs WHERE resolved = 0;

-- name: ResolveErrorLog :exec
UPDATE error_logs SET resolved = 1 WHERE id = ?;

-- name: ClearOldErrorLogs :exec
DELETE FROM error_logs WHERE timestamp < unixepoch() - ?;
