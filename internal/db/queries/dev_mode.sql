-- name: GetSettings :one
SELECT autonomous_mode, auto_approve_read, auto_approve_write, auto_approve_bash,
       heartbeat_interval_minutes, comm_enabled, comm_plugin, developer_mode, updated_at
FROM settings WHERE id = 1;

-- name: UpdateSettings :exec
UPDATE settings SET
    autonomous_mode = ?,
    auto_approve_read = ?,
    auto_approve_write = ?,
    auto_approve_bash = ?,
    heartbeat_interval_minutes = ?,
    comm_enabled = ?,
    comm_plugin = ?,
    developer_mode = ?,
    updated_at = strftime('%s', 'now')
WHERE id = 1;

-- name: ListDevSideloadedApps :many
SELECT app_id, path, loaded_at FROM dev_sideloaded_apps ORDER BY loaded_at DESC;

-- name: InsertDevSideloadedApp :exec
INSERT OR REPLACE INTO dev_sideloaded_apps (app_id, path) VALUES (?, ?);

-- name: DeleteDevSideloadedApp :exec
DELETE FROM dev_sideloaded_apps WHERE app_id = ?;

-- name: GetDevSideloadedApp :one
SELECT app_id, path, loaded_at FROM dev_sideloaded_apps WHERE app_id = ?;
