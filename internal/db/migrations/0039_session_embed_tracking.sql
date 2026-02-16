-- +goose Up
-- Track which messages have been embedded for session transcript indexing
ALTER TABLE sessions ADD COLUMN last_embedded_message_id INTEGER DEFAULT 0;

-- +goose Down
-- SQLite doesn't support DROP COLUMN before 3.35.0, but goose handles this
ALTER TABLE sessions DROP COLUMN last_embedded_message_id;
