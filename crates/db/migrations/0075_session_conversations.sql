-- +goose Up
-- Decouple session_key from chat_id: allow multiple conversations per session.

-- 1. Track which chat (conversation) a session is currently using.
ALTER TABLE sessions ADD COLUMN active_chat_id TEXT;

-- 2. Link chats back to their parent session.
ALTER TABLE chats ADD COLUMN session_name TEXT;

-- 3. Index for listing conversations by session.
CREATE INDEX idx_chats_session_name ON chats(session_name, updated_at DESC);

-- 4. Backfill: session.name WAS the chat_id, so set active_chat_id = name.
UPDATE sessions SET active_chat_id = name WHERE name IS NOT NULL;

-- 5. Backfill: chat.id WAS the session_key, so set session_name = id.
UPDATE chats SET session_name = id
WHERE id IN (SELECT name FROM sessions WHERE name IS NOT NULL);

-- +goose Down
DROP INDEX IF EXISTS idx_chats_session_name;
