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
    active_task = NULL, updated_at = unixepoch()
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

-- Session transcript embedding tracking

-- name: GetSessionLastEmbeddedMessageID :one
SELECT COALESCE(last_embedded_message_id, 0) as last_embedded_message_id
FROM sessions WHERE id = ?;

-- name: UpdateSessionLastEmbeddedMessageID :exec
UPDATE sessions
SET last_embedded_message_id = ?, updated_at = unixepoch()
WHERE id = ?;


-- Active task tracking (survives compaction)

-- name: GetSessionActiveTask :one
SELECT COALESCE(active_task, '') as active_task
FROM sessions WHERE id = ?;

-- name: SetSessionActiveTask :exec
UPDATE sessions
SET active_task = ?, updated_at = unixepoch()
WHERE id = ?;

-- name: ClearSessionActiveTask :exec
UPDATE sessions
SET active_task = NULL, updated_at = unixepoch()
WHERE id = ?;
