-- +goose Up
ALTER TABLE sessions ADD COLUMN active_task TEXT;

-- +goose Down
ALTER TABLE sessions DROP COLUMN active_task;
