-- name: ListChannels :many
SELECT * FROM channels ORDER BY created_at DESC;

-- name: ListEnabledChannels :many
SELECT * FROM channels WHERE is_enabled = 1 ORDER BY created_at DESC;

-- name: GetChannel :one
SELECT * FROM channels WHERE id = ?;

-- name: GetChannelByType :one
SELECT * FROM channels WHERE channel_type = ? AND is_enabled = 1 LIMIT 1;

-- name: CreateChannel :one
INSERT INTO channels (id, name, channel_type, is_enabled, created_at, updated_at)
VALUES (?, ?, ?, ?, unixepoch(), unixepoch())
RETURNING *;

-- name: UpdateChannel :one
UPDATE channels
SET name = ?, is_enabled = ?, updated_at = unixepoch()
WHERE id = ?
RETURNING *;

-- name: UpdateChannelStatus :exec
UPDATE channels
SET connection_status = ?, last_connected_at = ?, last_error = ?, updated_at = unixepoch()
WHERE id = ?;

-- name: IncrementChannelMessageCount :exec
UPDATE channels SET message_count = message_count + 1, updated_at = unixepoch() WHERE id = ?;

-- name: DeleteChannel :exec
DELETE FROM channels WHERE id = ?;

-- name: GetChannelCredential :one
SELECT * FROM channel_credentials WHERE channel_id = ? AND credential_key = ?;

-- name: ListChannelCredentials :many
SELECT * FROM channel_credentials WHERE channel_id = ?;

-- name: UpsertChannelCredential :one
INSERT INTO channel_credentials (id, channel_id, credential_key, credential_value, created_at, updated_at)
VALUES (?, ?, ?, ?, unixepoch(), unixepoch())
ON CONFLICT(channel_id, credential_key) DO UPDATE SET
    credential_value = excluded.credential_value,
    updated_at = unixepoch()
RETURNING *;

-- name: DeleteChannelCredential :exec
DELETE FROM channel_credentials WHERE channel_id = ? AND credential_key = ?;

-- name: DeleteChannelCredentials :exec
DELETE FROM channel_credentials WHERE channel_id = ?;

-- name: GetChannelConfig :one
SELECT * FROM channel_config WHERE channel_id = ? AND config_key = ?;

-- name: ListChannelConfig :many
SELECT * FROM channel_config WHERE channel_id = ?;

-- name: UpsertChannelConfig :one
INSERT INTO channel_config (id, channel_id, config_key, config_value, created_at, updated_at)
VALUES (?, ?, ?, ?, unixepoch(), unixepoch())
ON CONFLICT(channel_id, config_key) DO UPDATE SET
    config_value = excluded.config_value,
    updated_at = unixepoch()
RETURNING *;

-- name: DeleteChannelConfig :exec
DELETE FROM channel_config WHERE channel_id = ? AND config_key = ?;

-- name: ListChannelRegistry :many
SELECT * FROM channel_registry ORDER BY display_order ASC;

-- name: GetChannelRegistry :one
SELECT * FROM channel_registry WHERE id = ?;
