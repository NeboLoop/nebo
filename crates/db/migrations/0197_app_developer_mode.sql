-- +goose Up
-- App Developer mode: the developer pack for employees that build apps
-- (reload, status, console, publish). Off by default; a normal owner's
-- experience never changes.
ALTER TABLE settings ADD COLUMN app_developer_mode INTEGER NOT NULL DEFAULT 0;

-- +goose Down
ALTER TABLE settings DROP COLUMN app_developer_mode;
