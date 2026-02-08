-- +goose Up
-- Add compaction tracking fields to sessions table
-- Supports the memory flush pattern: one flush per compaction cycle

-- Track number of compactions (for memory flush deduplication)
ALTER TABLE sessions ADD COLUMN compaction_count INTEGER DEFAULT 0;

-- Track when memory flush last ran (timestamp)
ALTER TABLE sessions ADD COLUMN memory_flush_at INTEGER;

-- Track which compaction cycle the memory flush ran for
-- If memory_flush_compaction_count == compaction_count, skip flush
ALTER TABLE sessions ADD COLUMN memory_flush_compaction_count INTEGER;

-- +goose Down
-- SQLite doesn't support DROP COLUMN in older versions, so we recreate
-- For now, just leave the columns (they're nullable and won't break anything)
