-- +goose Up
-- The workforce reporter's outbox cursor (accountability plan W2, bot half).
--
-- Terminal runs are pushed to the platform so an owner hears about a failure
-- in seconds instead of when they happen to open the console. A column on the
-- run itself — not a separate watermark — because completed_at has ties and a
-- cursor that skips or re-sends on a tie is a reporter that lies. The server
-- side dedups on (bot_id, run_id) anyway; this only keeps the outbox scan
-- from rereading history.
ALTER TABLE workflow_runs ADD COLUMN reported_at INTEGER;

-- +goose Down
ALTER TABLE workflow_runs DROP COLUMN reported_at;
