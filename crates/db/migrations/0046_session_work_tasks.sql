-- +goose Up
ALTER TABLE sessions ADD COLUMN work_tasks TEXT;

-- +goose Down
