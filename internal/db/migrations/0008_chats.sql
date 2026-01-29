-- Chat conversations and messages
-- +goose Up

-- Conversations/chats table
CREATE TABLE IF NOT EXISTS chats (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL DEFAULT 'New Chat',
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE INDEX idx_chats_updated_at ON chats(updated_at DESC);

-- Messages within a chat
CREATE TABLE IF NOT EXISTS chat_messages (
    id TEXT PRIMARY KEY,
    chat_id TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('user', 'assistant', 'system', 'tool')),
    content TEXT NOT NULL,
    metadata TEXT, -- JSON for tool calls, etc.
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    FOREIGN KEY (chat_id) REFERENCES chats(id) ON DELETE CASCADE
);

CREATE INDEX idx_chat_messages_chat_id ON chat_messages(chat_id);
CREATE INDEX idx_chat_messages_created_at ON chat_messages(created_at);

-- +goose Down
DROP INDEX IF EXISTS idx_chat_messages_created_at;
DROP INDEX IF EXISTS idx_chat_messages_chat_id;
DROP TABLE IF EXISTS chat_messages;
DROP INDEX IF EXISTS idx_chats_updated_at;
DROP TABLE IF EXISTS chats;
