-- +goose Up
ALTER TABLE agent_profile ADD COLUMN quiet_hours_start TEXT NOT NULL DEFAULT '';
ALTER TABLE agent_profile ADD COLUMN quiet_hours_end TEXT NOT NULL DEFAULT '';

-- +goose Down
-- SQLite doesn't support DROP COLUMN
