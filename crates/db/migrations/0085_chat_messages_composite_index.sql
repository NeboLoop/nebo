-- +migrate Up
-- Composite index for efficient chat message queries: count, ordering, and preview lookup.
CREATE INDEX IF NOT EXISTS idx_chat_messages_chat_created
    ON chat_messages(chat_id, created_at DESC, id DESC);

-- +migrate Down
DROP INDEX IF EXISTS idx_chat_messages_chat_created;
