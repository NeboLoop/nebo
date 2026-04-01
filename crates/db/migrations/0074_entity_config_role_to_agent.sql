-- Rename entity_type 'role' → 'agent' in entity_config table.
-- SQLite doesn't support ALTER CONSTRAINT, so recreate the table.

CREATE TABLE entity_config_new (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    entity_type TEXT NOT NULL CHECK(entity_type IN ('main', 'agent', 'channel')),
    entity_id   TEXT NOT NULL,
    heartbeat_enabled          INTEGER,
    heartbeat_interval_minutes INTEGER,
    heartbeat_content          TEXT,
    heartbeat_window_start     TEXT,
    heartbeat_window_end       TEXT,
    permissions     TEXT,
    resource_grants TEXT,
    model_preference    TEXT,
    personality_snippet TEXT,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    allowed_paths TEXT,
    UNIQUE(entity_type, entity_id)
);

INSERT INTO entity_config_new
    SELECT id,
           CASE entity_type WHEN 'role' THEN 'agent' ELSE entity_type END,
           entity_id, heartbeat_enabled, heartbeat_interval_minutes,
           heartbeat_content, heartbeat_window_start, heartbeat_window_end,
           permissions, resource_grants, model_preference, personality_snippet,
           created_at, updated_at, allowed_paths
    FROM entity_config;

DROP TABLE entity_config;
ALTER TABLE entity_config_new RENAME TO entity_config;
