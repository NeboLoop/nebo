CREATE TABLE IF NOT EXISTS entity_config (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    entity_type TEXT NOT NULL CHECK(entity_type IN ('main', 'role', 'channel')),
    entity_id   TEXT NOT NULL,
    -- Heartbeat
    heartbeat_enabled          INTEGER,  -- NULL=inherit, 0/1
    heartbeat_interval_minutes INTEGER,  -- NULL=inherit from settings
    heartbeat_content          TEXT,     -- NULL=inherit from HEARTBEAT.md
    heartbeat_window_start     TEXT,     -- HH:MM, NULL=no window
    heartbeat_window_end       TEXT,     -- HH:MM, NULL=no window
    -- Permissions (JSON: {"web": true, "desktop": false, ...})
    permissions     TEXT,
    -- Resource grants (JSON: {"screen": "allow"|"deny"|"inherit", "browser": ...})
    resource_grants TEXT,
    -- Model + personality
    model_preference    TEXT,
    personality_snippet TEXT,
    -- Timestamps
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE(entity_type, entity_id)
);

-- Seed the main entity config row
INSERT OR IGNORE INTO entity_config (entity_type, entity_id) VALUES ('main', 'main');
