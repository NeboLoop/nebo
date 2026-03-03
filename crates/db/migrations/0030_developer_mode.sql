-- +goose Up
-- Unified settings table — all user-facing toggleable settings in one place.
-- Replaces agent-settings.json file-based storage.
CREATE TABLE IF NOT EXISTS settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    autonomous_mode INTEGER NOT NULL DEFAULT 0,
    auto_approve_read INTEGER NOT NULL DEFAULT 1,
    auto_approve_write INTEGER NOT NULL DEFAULT 0,
    auto_approve_bash INTEGER NOT NULL DEFAULT 0,
    heartbeat_interval_minutes INTEGER NOT NULL DEFAULT 30,
    comm_enabled INTEGER NOT NULL DEFAULT 0,
    comm_plugin TEXT NOT NULL DEFAULT '',
    developer_mode INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
);
INSERT OR IGNORE INTO settings (id) VALUES (1);

-- Tracks sideloaded dev apps (symlinks to developer project directories)
CREATE TABLE IF NOT EXISTS dev_sideloaded_apps (
    app_id TEXT PRIMARY KEY,
    path TEXT NOT NULL,
    loaded_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
);

-- +goose Down
DROP TABLE IF EXISTS dev_sideloaded_apps;
DROP TABLE IF EXISTS settings;
