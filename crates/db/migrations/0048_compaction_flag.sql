-- +goose Up
-- Add compaction flag to chat_messages for LLM compaction tracking.
-- Messages that have been compacted into a summary can be marked so they
-- are not re-compacted.
ALTER TABLE chat_messages ADD COLUMN is_compacted INTEGER DEFAULT 0;

-- +goose Down
-- SQLite does not support DROP COLUMN in older versions; this is a no-op.
