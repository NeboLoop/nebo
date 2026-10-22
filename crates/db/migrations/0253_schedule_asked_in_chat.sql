-- The owner's own conversation (its chat id) a schedule was asked for in,
-- when an employee made it in one: where a one-shot's answer goes when it
-- fires. NULL for a schedule made anywhere else (a helper, a colleague, a
-- call, an outside app, a workflow, the owner's settings), and for every
-- schedule made before this was kept.
-- +goose Up
ALTER TABLE cron_jobs ADD COLUMN created_in_chat TEXT;

-- +goose Down
ALTER TABLE cron_jobs DROP COLUMN created_in_chat;
