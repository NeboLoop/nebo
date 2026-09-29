-- The one-time upgrade conversions: the old permission settings into rules,
-- the stored tool names onto the current tool set, the approvals parked
-- before the upgrade into asks. Each is done once per install and records
-- what it converted.
-- +goose Up
ALTER TABLE permission_migrations RENAME TO upgrade_conversions;

-- +goose Down
ALTER TABLE upgrade_conversions RENAME TO permission_migrations;
