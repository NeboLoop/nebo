-- +goose Up
-- Add user_id column to memories table for multi-user support
-- Each user gets their own isolated set of memories

-- Add user_id column (default empty for existing rows, required for new ones)
ALTER TABLE memories ADD COLUMN user_id TEXT NOT NULL DEFAULT '';

-- Index for fast user-scoped queries
CREATE INDEX idx_memories_user_id ON memories(user_id);

-- Drop the existing unique constraint and create a new one that includes user_id
-- Note: SQLite doesn't support DROP CONSTRAINT, so we need to recreate the table
-- However, the existing UNIQUE(namespace, key) is defined inline, so we just need
-- to add a new index for the three-column uniqueness
DROP INDEX IF EXISTS memories_namespace_key;

-- Create composite unique index: (namespace, key) unique per user
CREATE UNIQUE INDEX idx_memories_namespace_key_user ON memories(namespace, key, user_id);

-- Also add user_id to memory_chunks for consistency
ALTER TABLE memory_chunks ADD COLUMN user_id TEXT NOT NULL DEFAULT '';
CREATE INDEX idx_memory_chunks_user_id ON memory_chunks(user_id);

-- +goose Down
DROP INDEX IF EXISTS idx_memory_chunks_user_id;
-- SQLite doesn't support DROP COLUMN, so we leave the column but remove indexes
DROP INDEX IF EXISTS idx_memories_namespace_key_user;
DROP INDEX IF EXISTS idx_memories_user_id;
-- Recreate original unique index
CREATE UNIQUE INDEX memories_namespace_key ON memories(namespace, key);
