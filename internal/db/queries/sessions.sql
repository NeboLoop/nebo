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

-- name: GetSessionByNameAndScope :one
SELECT * FROM sessions
WHERE name = ? AND scope = ? AND scope_id = ?;

-- name: GetSessionByNameAndScopeNullID :one
SELECT * FROM sessions
WHERE name = ? AND scope = ? AND (scope_id IS NULL OR scope_id = '');

-- name: GetOrCreateScopedSession :one
INSERT INTO sessions (id, name, scope, scope_id, metadata, created_at, updated_at)
VALUES (?, ?, ?, ?, ?, unixepoch(), unixepoch())
ON CONFLICT(name, scope, scope_id) DO UPDATE SET updated_at = unixepoch()
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

-- name: IncrementSessionMessageCount :exec
UPDATE sessions
SET message_count = COALESCE(message_count, 0) + 1, updated_at = unixepoch()
WHERE id = ?;

-- name: ResetSession :exec
UPDATE sessions
SET message_count = 0, token_count = 0, summary = NULL, last_compacted_at = NULL,
    compaction_count = 0, memory_flush_at = NULL, memory_flush_compaction_count = NULL,
    updated_at = unixepoch()
WHERE id = ?;

-- name: CompactSession :exec
UPDATE sessions
SET summary = ?, last_compacted_at = unixepoch(),
    compaction_count = COALESCE(compaction_count, 0) + 1, updated_at = unixepoch()
WHERE id = ?;

-- name: RecordMemoryFlush :exec
UPDATE sessions
SET memory_flush_at = unixepoch(),
    memory_flush_compaction_count = COALESCE(compaction_count, 0),
    updated_at = unixepoch()
WHERE id = ?;

-- name: ListSessionsByScope :many
SELECT * FROM sessions
WHERE scope = ?
ORDER BY updated_at DESC;

-- name: ListSessionsByScopeAndScopeID :many
SELECT * FROM sessions
WHERE scope = ? AND scope_id = ?
ORDER BY updated_at DESC;

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
WHERE session_id = ? AND (is_compacted IS NULL OR is_compacted = 0)
ORDER BY id ASC;

-- name: GetRecentNonCompactedMessages :many
-- Get last N non-compacted messages, ordered by id (insertion order)
SELECT * FROM (
    SELECT * FROM session_messages
    WHERE session_id = ? AND (is_compacted IS NULL OR is_compacted = 0)
    ORDER BY id DESC
    LIMIT ?
) sub ORDER BY id ASC;

-- name: MarkMessagesCompacted :exec
UPDATE session_messages
SET is_compacted = 1
WHERE session_id = ? AND id <= ?;

-- name: GetMaxMessageIDToKeep :one
-- Get the minimum ID of the N most recent messages to keep
SELECT COALESCE(MIN(id), 0) FROM (
    SELECT id FROM session_messages
    WHERE session_id = ?
    ORDER BY id DESC
    LIMIT ?
);

-- name: MarkMessagesCompactedBeforeID :exec
-- Mark all messages with ID less than the given threshold as compacted
UPDATE session_messages
SET is_compacted = 1
WHERE session_id = ? AND id < ?;

-- name: DeleteCompactedMessages :exec
DELETE FROM session_messages
WHERE session_id = ? AND is_compacted = 1;

-- name: DeleteSessionMessages :exec
DELETE FROM session_messages WHERE session_id = ?;

-- name: CountSessionMessages :one
SELECT COUNT(*) FROM session_messages WHERE session_id = ?;

-- name: GetSessionMessageStats :one
SELECT
    COUNT(*) as message_count,
    COALESCE(SUM(token_estimate), 0) as total_tokens
FROM session_messages
WHERE session_id = ? AND is_compacted = 0;

-- Session policy queries

-- name: GetSessionPolicy :one
SELECT send_policy, model_override, provider_override, auth_profile_override,
       auth_profile_override_source, verbose_level, custom_label
FROM sessions
WHERE id = ?;

-- name: UpdateSessionPolicy :exec
UPDATE sessions
SET send_policy = COALESCE(sqlc.narg(send_policy), send_policy),
    model_override = COALESCE(sqlc.narg(model_override), model_override),
    provider_override = COALESCE(sqlc.narg(provider_override), provider_override),
    auth_profile_override = COALESCE(sqlc.narg(auth_profile_override), auth_profile_override),
    auth_profile_override_source = COALESCE(sqlc.narg(auth_profile_override_source), auth_profile_override_source),
    verbose_level = COALESCE(sqlc.narg(verbose_level), verbose_level),
    custom_label = COALESCE(sqlc.narg(custom_label), custom_label),
    updated_at = unixepoch()
WHERE id = sqlc.arg(id);

-- name: SetSessionModelOverride :exec
UPDATE sessions
SET model_override = ?, provider_override = ?, updated_at = unixepoch()
WHERE id = ?;

-- name: SetSessionAuthProfileOverride :exec
UPDATE sessions
SET auth_profile_override = ?,
    auth_profile_override_source = ?,
    updated_at = unixepoch()
WHERE id = ?;

-- name: ClearSessionOverrides :exec
UPDATE sessions
SET model_override = NULL,
    provider_override = NULL,
    auth_profile_override = NULL,
    auth_profile_override_source = NULL,
    verbose_level = NULL,
    updated_at = unixepoch()
WHERE id = ?;

-- name: SetSessionSendPolicy :exec
UPDATE sessions
SET send_policy = ?, updated_at = unixepoch()
WHERE id = ?;

-- name: SetSessionLabel :exec
UPDATE sessions
SET custom_label = ?, updated_at = unixepoch()
WHERE id = ?;
