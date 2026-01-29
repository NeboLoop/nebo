-- MCP Session queries for session persistence

-- name: GetMCPSession :one
SELECT * FROM mcp_sessions
WHERE session_id = ?;

-- name: GetMCPSessionByUser :one
-- Get most recent session for a user
SELECT * FROM mcp_sessions
WHERE user_id = ?
ORDER BY updated_at DESC
LIMIT 1;

-- name: UpsertMCPSession :exec
-- Persist session (upsert to handle both new and existing sessions)
INSERT INTO mcp_sessions (session_id, user_id, updated_at)
VALUES (?, ?, unixepoch())
ON CONFLICT (session_id) DO UPDATE SET
    updated_at = unixepoch();

-- name: DeleteMCPSession :exec
DELETE FROM mcp_sessions WHERE session_id = ?;

-- name: CleanupOldMCPSessions :exec
-- Run periodically to clean up stale sessions (older than 7 days)
DELETE FROM mcp_sessions
WHERE updated_at < unixepoch() - 604800;
