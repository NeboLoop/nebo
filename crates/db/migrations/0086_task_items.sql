-- +goose Up
-- Extend pending_tasks for observability tracking (per-step workflow visibility + general task lists).
-- Tracking rows use task_type = 'tracking' and are grouped via list_id + ordered by seq.

ALTER TABLE pending_tasks ADD COLUMN list_id TEXT;
ALTER TABLE pending_tasks ADD COLUMN seq INTEGER DEFAULT 0;
ALTER TABLE pending_tasks ADD COLUMN tokens_input INTEGER DEFAULT 0;
ALTER TABLE pending_tasks ADD COLUMN tokens_output INTEGER DEFAULT 0;
ALTER TABLE pending_tasks ADD COLUMN metadata TEXT;

CREATE INDEX idx_pending_tasks_list ON pending_tasks(list_id, seq);

-- +goose Down
DROP INDEX IF EXISTS idx_pending_tasks_list;
