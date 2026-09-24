-- Whether the session's objective is a multi-stage job, as the turn decision
-- answered it in the same call that set the objective. It decides how hard the
-- objective pushes: a multi-stage job gets "stay on it" and the progress
-- reminders, a conversation gets its objective as the topic only.
-- NULL = never answered (read as a conversation unless tracked work is open).
-- Set and cleared with active_task, never on its own.
-- +goose Up
ALTER TABLE sessions ADD COLUMN active_task_multi_stage INTEGER;

-- +goose Down
ALTER TABLE sessions DROP COLUMN active_task_multi_stage;
