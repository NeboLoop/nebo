-- +goose Up
-- Plugin Registry: Universal plugin catalog (comm, channel, tool, integration)
-- Follows the iPhone Settings.bundle model - plugins declare their settings schema,
-- the UI renders dynamically, and values are stored per-plugin in plugin_settings.

CREATE TABLE IF NOT EXISTS plugin_registry (
    id TEXT PRIMARY KEY,
    name TEXT UNIQUE NOT NULL,               -- Internal name (e.g., "neboloop", "discord")
    plugin_type TEXT NOT NULL DEFAULT 'comm', -- comm, channel, tool, integration
    display_name TEXT NOT NULL DEFAULT '',
    description TEXT NOT NULL DEFAULT '',
    icon TEXT NOT NULL DEFAULT '',
    version TEXT NOT NULL DEFAULT '0.0.0',
    is_enabled INTEGER NOT NULL DEFAULT 0,
    is_installed INTEGER NOT NULL DEFAULT 1,
    settings_manifest TEXT NOT NULL DEFAULT '{}', -- JSON SettingsManifest (schema for UI)
    connection_status TEXT NOT NULL DEFAULT 'disconnected', -- connected, connecting, disconnected, error
    last_connected_at INTEGER,
    last_error TEXT,
    metadata TEXT NOT NULL DEFAULT '{}',      -- JSON for extensible plugin-specific data
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE INDEX idx_plugin_registry_type ON plugin_registry(plugin_type);
CREATE INDEX idx_plugin_registry_enabled ON plugin_registry(is_enabled);

-- Plugin Settings: Key-value settings per plugin (like iOS NSUserDefaults per app)
CREATE TABLE IF NOT EXISTS plugin_settings (
    id TEXT PRIMARY KEY,
    plugin_id TEXT NOT NULL REFERENCES plugin_registry(id) ON DELETE CASCADE,
    setting_key TEXT NOT NULL,
    setting_value TEXT NOT NULL DEFAULT '',
    is_secret INTEGER NOT NULL DEFAULT 0,    -- Mark sensitive values (passwords, tokens)
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE(plugin_id, setting_key)
);

CREATE INDEX idx_plugin_settings_plugin ON plugin_settings(plugin_id);

-- Pre-populate known plugins (built-in comm plugins)
INSERT INTO plugin_registry (id, name, plugin_type, display_name, description, icon, is_installed) VALUES
('builtin-neboloop', 'neboloop', 'comm', 'NeboLoop', 'Connect to NeboLoop network for bot-to-bot communication via MQTT', '🔗', 1),
('builtin-loopback', 'loopback', 'comm', 'Loopback', 'Local testing - messages echo back to sender', '🔄', 1);

-- Pre-populate known channel plugins (migrate from channel_registry concept)
INSERT INTO plugin_registry (id, name, plugin_type, display_name, description, icon, is_installed) VALUES
('builtin-discord', 'discord', 'channel', 'Discord', 'Discord bot integration', '🎮', 1),
('builtin-telegram', 'telegram', 'channel', 'Telegram', 'Telegram bot integration', '📱', 1),
('builtin-slack', 'slack', 'channel', 'Slack', 'Slack bot integration (Socket Mode)', '💼', 1);

-- +goose Down
DROP INDEX IF EXISTS idx_plugin_settings_plugin;
DROP TABLE IF EXISTS plugin_settings;
DROP INDEX IF EXISTS idx_plugin_registry_enabled;
DROP INDEX IF EXISTS idx_plugin_registry_type;
DROP TABLE IF EXISTS plugin_registry;
