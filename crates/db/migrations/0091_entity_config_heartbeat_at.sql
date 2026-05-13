-- +goose Up
ALTER TABLE entity_config ADD COLUMN last_heartbeat_at TEXT;

-- +goose Down
-- SQLite does not support DROP COLUMN in older versions; safe to leave.
