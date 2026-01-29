-- Session queries for conversation persistence

-- name: CreateSession :one
INSERT INTO sessions (id, name, scope, scope_id, metadata, created_at, updated_at)
VALUES (?, ?, ?, ?, ?, unixepoch(), unixepoch())
RETURNING *;

-- name: GetSession :one
SELECT * FROM sessions WHERE id = ?;

-- name: GetSessionByName :one
SELECT * FROM sessions WHERE name = ?;

-- name: GetSessionByScope :one
SELECT * FROM sessions WHERE scope = ? AND scope_id = ?;

-- name: GetOrCreateScopedSession :one
INSERT INTO sessions (id, name, scope, scope_id, metadata, created_at, updated_at)
VALUES (?, ?, ?, ?, ?, unixepoch(), unixepoch())
ON CONFLICT(scope, scope_id) DO UPDATE SET updated_at = unixepoch()
RETURNING *;

-- name: ListSessions :many
SELECT * FROM sessions ORDER BY updated_at DESC LIMIT ? OFFSET ?;

-- name: UpdateSessionSummary :exec
UPDATE sessions
SET summary = ?, last_compacted_at = unixepoch(), updated_at = unixepoch()
WHERE id = ?;

-- name: UpdateSessionStats :exec
UPDATE sessions
SET token_count = ?, message_count = ?, updated_at = unixepoch()
WHERE id = ?;

-- name: DeleteSession :exec
DELETE FROM sessions WHERE id = ?;

-- Session messages

-- name: CreateSessionMessage :one
INSERT INTO session_messages (session_id, role, content, tool_calls, tool_results, token_estimate, created_at)
VALUES (?, ?, ?, ?, ?, ?, unixepoch())
RETURNING *;

-- name: GetSessionMessages :many
SELECT * FROM session_messages
WHERE session_id = ?
ORDER BY created_at ASC;

-- name: GetRecentSessionMessages :many
-- Get last N messages for context window
SELECT * FROM (
    SELECT * FROM session_messages
    WHERE session_id = ?
    ORDER BY created_at DESC
    LIMIT ?
) sub ORDER BY created_at ASC;

-- name: GetNonCompactedMessages :many
SELECT * FROM session_messages
WHERE session_id = ? AND is_compacted = 0
ORDER BY created_at ASC;

-- name: MarkMessagesCompacted :exec
UPDATE session_messages
SET is_compacted = 1
WHERE session_id = ? AND id <= ?;

-- name: DeleteCompactedMessages :exec
DELETE FROM session_messages
WHERE session_id = ? AND is_compacted = 1;

-- name: CountSessionMessages :one
SELECT COUNT(*) FROM session_messages WHERE session_id = ?;

-- name: GetSessionMessageStats :one
SELECT
    COUNT(*) as message_count,
    COALESCE(SUM(token_estimate), 0) as total_tokens
FROM session_messages
WHERE session_id = ? AND is_compacted = 0;
