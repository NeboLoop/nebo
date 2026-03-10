-- +goose Up
-- Fix last_run values stored as Unix timestamps (integers) by strftime('%s', 'now').
-- Convert them to ISO datetime strings so rusqlite can read them as Option<String>.
UPDATE cron_jobs
SET last_run = datetime(last_run, 'unixepoch')
WHERE last_run IS NOT NULL AND typeof(last_run) = 'integer';

-- +goose Down
-- No-op: values are already valid datetime strings
