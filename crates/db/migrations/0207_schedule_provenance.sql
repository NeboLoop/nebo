-- +goose Up
-- Every schedule says who made it and why. A run created 23 retry
-- schedules on a customer's bot that nobody noticed (2026-10): a schedule
-- with no maker and no reason can't be told apart from the owner's own.
--   created_by  owner (the app or API), chat (an employee in a
--               conversation), workflow (a workflow binding's trigger),
--               schedule (a scheduled turn), system (Nebo itself), unknown
--               (made before this was kept)
--   created_by_run  the run that made it, when a run did
--   created_in      the conversation (session key) it was made in
--   reason          why it exists, in a few words
ALTER TABLE cron_jobs ADD COLUMN created_by TEXT NOT NULL DEFAULT 'unknown'
    CHECK (created_by IN ('owner', 'chat', 'workflow', 'schedule', 'system', 'unknown'));
ALTER TABLE cron_jobs ADD COLUMN created_by_run TEXT;
ALTER TABLE cron_jobs ADD COLUMN created_in TEXT;
ALTER TABLE cron_jobs ADD COLUMN reason TEXT NOT NULL DEFAULT '';
CREATE INDEX IF NOT EXISTS idx_cron_jobs_created_by_run ON cron_jobs(created_by_run) WHERE created_by_run IS NOT NULL;

-- +goose Down
DROP INDEX IF EXISTS idx_cron_jobs_created_by_run;
ALTER TABLE cron_jobs DROP COLUMN reason;
ALTER TABLE cron_jobs DROP COLUMN created_in;
ALTER TABLE cron_jobs DROP COLUMN created_by_run;
ALTER TABLE cron_jobs DROP COLUMN created_by;
