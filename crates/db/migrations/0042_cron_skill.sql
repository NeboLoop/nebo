-- +goose Up
ALTER TABLE cron_jobs ADD COLUMN instructions TEXT DEFAULT '';

-- +goose Down
ALTER TABLE cron_jobs DROP COLUMN instructions;
