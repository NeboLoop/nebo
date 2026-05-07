-- +goose Up
-- Cleanup: drop task_items table if it was created by an earlier draft of 0086.
-- All tracking now lives in pending_tasks with task_type = 'tracking'.
DROP TABLE IF EXISTS task_items;

-- +goose Down
-- No-op: task_items was a draft table that should not be recreated.
