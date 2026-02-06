-- Chat queries

-- name: CreateChat :one
INSERT INTO chats (id, title, created_at, updated_at)
VALUES (?, ?, unixepoch(), unixepoch())
RETURNING *;

-- name: GetChat :one
SELECT * FROM chats WHERE id = ?;

-- name: ListChats :many
SELECT * FROM chats
ORDER BY updated_at DESC
LIMIT ? OFFSET ?;

-- name: CountChats :one
SELECT COUNT(*) FROM chats;

-- name: UpdateChatTitle :exec
UPDATE chats SET title = ?, updated_at = unixepoch()
WHERE id = ?;

-- name: UpdateChatTimestamp :exec
UPDATE chats SET updated_at = unixepoch()
WHERE id = ?;

-- name: DeleteChat :exec
DELETE FROM chats WHERE id = ?;

-- Chat message queries

-- name: CreateChatMessage :one
INSERT INTO chat_messages (id, chat_id, role, content, metadata, created_at)
VALUES (?, ?, ?, ?, ?, unixepoch())
RETURNING *;

-- name: GetChatMessages :many
SELECT * FROM chat_messages
WHERE chat_id = ?
ORDER BY created_at ASC;

-- name: GetChatMessage :one
SELECT * FROM chat_messages WHERE id = ?;

-- name: DeleteChatMessage :exec
DELETE FROM chat_messages WHERE id = ?;

-- name: DeleteChatMessagesAfter :exec
DELETE FROM chat_messages
WHERE chat_id = ? AND created_at > ?;

-- name: GetChatWithMessages :many
SELECT
    c.id as chat_id,
    c.title,
    c.created_at as chat_created_at,
    c.updated_at as chat_updated_at,
    m.id as message_id,
    m.role,
    m.content,
    m.metadata,
    m.created_at as message_created_at
FROM chats c
LEFT JOIN chat_messages m ON c.id = m.chat_id
WHERE c.id = ?
ORDER BY m.created_at ASC;

-- Companion Mode queries

-- name: GetOrCreateCompanionChat :one
INSERT INTO chats (id, user_id, title, created_at, updated_at)
VALUES (?, ?, 'Companion', unixepoch(), unixepoch())
ON CONFLICT(user_id) DO UPDATE SET updated_at = unixepoch()
RETURNING *;

-- name: GetCompanionChatByUser :one
SELECT * FROM chats WHERE user_id = ? LIMIT 1;

-- name: CreateChatMessageWithDay :one
INSERT INTO chat_messages (id, chat_id, role, content, metadata, day_marker, created_at)
VALUES (?, ?, ?, ?, ?, date('now', 'localtime'), unixepoch())
RETURNING *;

-- name: GetMessagesByDay :many
SELECT * FROM chat_messages
WHERE chat_id = ? AND day_marker = ?
ORDER BY created_at ASC;

-- name: GetDaysWithMessages :many
SELECT day_marker, COUNT(*) as message_count
FROM chat_messages
WHERE chat_id = ?
GROUP BY day_marker
ORDER BY day_marker DESC
LIMIT ? OFFSET ?;

-- name: SearchChatMessages :many
SELECT * FROM chat_messages
WHERE chat_id = ? AND content LIKE '%' || ? || '%'
ORDER BY created_at DESC
LIMIT ? OFFSET ?;

-- name: GetRecentChatMessages :many
-- Get last N messages for context window (most recent first, reversed for display)
SELECT * FROM (
    SELECT * FROM chat_messages
    WHERE chat_id = ?
    ORDER BY created_at DESC
    LIMIT ?
) sub ORDER BY created_at ASC;

-- name: UpdateChatMessageContent :exec
UPDATE chat_messages SET content = ?, metadata = ? WHERE id = ?;

-- name: CountChatMessages :one
SELECT COUNT(*) FROM chat_messages WHERE chat_id = ?;
