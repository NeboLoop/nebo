-- +goose Up
ALTER TABLE sessions ADD COLUMN last_summarized_count INTEGER DEFAULT 0;

-- +goose Down
-- SQLite doesn't support DROP COLUMN before 3.35.0, so this is a no-op
