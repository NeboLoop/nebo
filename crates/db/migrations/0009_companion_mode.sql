-- +goose Up
-- Companion Mode: Single chat per user

-- Add user_id to chats for single-chat-per-user model
ALTER TABLE chats ADD COLUMN user_id TEXT;

-- Create unique index to enforce one chat per user
CREATE UNIQUE INDEX idx_chats_user_companion ON chats(user_id) WHERE user_id IS NOT NULL;

-- Add day markers for history browsing
ALTER TABLE chat_messages ADD COLUMN day_marker TEXT;

-- Index for efficient day-based queries
CREATE INDEX idx_chat_messages_day ON chat_messages(chat_id, day_marker);

-- Backfill day_marker for existing messages
UPDATE chat_messages SET day_marker = date(created_at, 'unixepoch') WHERE day_marker IS NULL;
