-- Messages of an employee's web conversation (`agent:<id>:web`) stored
-- before every conversation had its own chats row get one, so the employee's
-- list shows them the way it shows every other conversation: titled
-- "Chat 1", from its first message to its last. A conversation that already
-- has its row is left as it is.
-- +goose Up
INSERT OR IGNORE INTO chats (id, session_name, title, created_at, updated_at)
SELECT m.chat_id, s.name, 'Chat 1', MIN(m.created_at), MAX(m.created_at)
FROM sessions s
JOIN chat_messages m ON m.chat_id = COALESCE(s.active_chat_id, s.name)
WHERE s.name LIKE 'agent:%:web'
  AND NOT EXISTS (SELECT 1 FROM chats c WHERE c.id = m.chat_id)
GROUP BY m.chat_id;

INSERT OR IGNORE INTO chats (id, session_name, title, created_at, updated_at)
SELECT m.chat_id, m.chat_id, 'Chat 1', MIN(m.created_at), MAX(m.created_at)
FROM chat_messages m
WHERE m.chat_id LIKE 'agent:%:web'
  AND NOT EXISTS (SELECT 1 FROM chats c WHERE c.id = m.chat_id)
GROUP BY m.chat_id;

-- +goose Down
