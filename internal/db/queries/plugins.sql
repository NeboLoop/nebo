-- Plugin Registry queries

-- name: ListPlugins :many
SELECT * FROM plugin_registry ORDER BY plugin_type, display_name;

-- name: ListPluginsByType :many
SELECT * FROM plugin_registry WHERE plugin_type = ? ORDER BY display_name;

-- name: ListEnabledPlugins :many
SELECT * FROM plugin_registry WHERE is_enabled = 1 ORDER BY plugin_type, display_name;

-- name: GetPlugin :one
SELECT * FROM plugin_registry WHERE id = ?;

-- name: GetPluginByName :one
SELECT * FROM plugin_registry WHERE name = ?;

-- name: CreatePlugin :one
INSERT INTO plugin_registry (id, name, plugin_type, display_name, description, icon, version, is_enabled, is_installed, settings_manifest, metadata, created_at, updated_at)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, unixepoch(), unixepoch())
RETURNING *;

-- name: UpdatePlugin :exec
UPDATE plugin_registry
SET display_name = ?, description = ?, icon = ?, version = ?,
    is_enabled = ?, settings_manifest = ?, metadata = ?,
    updated_at = unixepoch()
WHERE id = ?;

-- name: UpdatePluginStatus :exec
UPDATE plugin_registry
SET connection_status = ?, last_connected_at = ?, last_error = ?,
    updated_at = unixepoch()
WHERE id = ?;

-- name: TogglePlugin :exec
UPDATE plugin_registry SET is_enabled = ?, updated_at = unixepoch() WHERE id = ?;

-- name: DeletePlugin :exec
DELETE FROM plugin_registry WHERE id = ?;

-- Plugin Settings queries (UPSERT pattern matching channel_config)

-- name: GetPluginSetting :one
SELECT * FROM plugin_settings WHERE plugin_id = ? AND setting_key = ?;

-- name: ListPluginSettings :many
SELECT * FROM plugin_settings WHERE plugin_id = ? ORDER BY setting_key;

-- name: UpsertPluginSetting :one
INSERT INTO plugin_settings (id, plugin_id, setting_key, setting_value, is_secret, created_at, updated_at)
VALUES (?, ?, ?, ?, ?, unixepoch(), unixepoch())
ON CONFLICT(plugin_id, setting_key) DO UPDATE SET
    setting_value = excluded.setting_value,
    is_secret = excluded.is_secret,
    updated_at = unixepoch()
RETURNING *;

-- name: DeletePluginSetting :exec
DELETE FROM plugin_settings WHERE plugin_id = ? AND setting_key = ?;

-- name: DeletePluginSettings :exec
DELETE FROM plugin_settings WHERE plugin_id = ?;
