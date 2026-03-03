-- +goose Up

-- Step 1: Add new columns to chat_messages for tool data and token estimation
ALTER TABLE chat_messages ADD COLUMN tool_calls TEXT;
ALTER TABLE chat_messages ADD COLUMN tool_results TEXT;
ALTER TABLE chat_messages ADD COLUMN token_estimate INTEGER;

-- Step 2: Create chats rows for orphaned sessions (sub-agents, DMs, loop channels)
-- These sessions have session_messages but no corresponding chats row.
-- Use sessions.name as chats.id (same key the runner uses as sessionKey).
INSERT OR IGNORE INTO chats (id, title, created_at, updated_at)
SELECT s.name, 'Session: ' || s.name, s.created_at, s.updated_at
FROM sessions s
WHERE s.name IS NOT NULL
  AND s.name != ''
  AND s.name NOT IN (SELECT id FROM chats);

-- Step 3: Copy session_messages → chat_messages (deduplicate by content+role+timestamp)
-- Use hex(randomblob(16)) for UUID generation in SQLite.
-- Map: session_id → chat_id via sessions.name (the sessionKey the runner uses).
INSERT OR IGNORE INTO chat_messages (id, chat_id, role, content, metadata, tool_calls, tool_results, token_estimate, created_at, day_marker)
SELECT
    hex(randomblob(16)),
    s.name,
    sm.role,
    COALESCE(sm.content, ''),
    NULL,
    sm.tool_calls,
    sm.tool_results,
    sm.token_estimate,
    sm.created_at,
    date(sm.created_at, 'unixepoch')
FROM session_messages sm
JOIN sessions s ON sm.session_id = s.id
WHERE s.name IS NOT NULL AND s.name != ''
  AND NOT EXISTS (
    SELECT 1 FROM chat_messages cm
    WHERE cm.chat_id = s.name
      AND cm.role = sm.role
      AND cm.content = COALESCE(sm.content, '')
      AND cm.created_at = sm.created_at
  );

-- Step 4: Drop session_messages
DROP TABLE IF EXISTS session_messages;

-- +goose Down
-- Data migration is one-way — down recreates empty table
CREATE TABLE IF NOT EXISTS session_messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    role TEXT NOT NULL,
    content TEXT,
    tool_calls TEXT,
    tool_results TEXT,
    token_estimate INTEGER,
    is_compacted INTEGER DEFAULT 0,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_session_messages_session ON session_messages(session_id, created_at);
CREATE INDEX IF NOT EXISTS idx_session_messages_compacted ON session_messages(session_id, is_compacted);
