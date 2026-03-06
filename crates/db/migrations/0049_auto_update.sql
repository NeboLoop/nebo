-- +goose Up
ALTER TABLE settings ADD COLUMN auto_update INTEGER NOT NULL DEFAULT 1;
-- +goose Down
ALTER TABLE settings DROP COLUMN auto_update;
