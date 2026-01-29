-- +goose Up
-- Fix: ON CONFLICT requires a real UNIQUE constraint, not a partial index
-- SQLite treats NULL as distinct, so a regular UNIQUE index on user_id
-- still allows multiple rows with NULL user_id (for regular chats)

DROP INDEX IF EXISTS idx_chats_user_companion;
CREATE UNIQUE INDEX idx_chats_user_id ON chats(user_id);

-- +goose Down
DROP INDEX IF EXISTS idx_chats_user_id;
CREATE UNIQUE INDEX idx_chats_user_companion ON chats(user_id) WHERE user_id IS NOT NULL;
