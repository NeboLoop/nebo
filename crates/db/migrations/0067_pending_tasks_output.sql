-- +goose Up
ALTER TABLE pending_tasks ADD COLUMN output TEXT;

-- +goose Down
ALTER TABLE pending_tasks DROP COLUMN output;
