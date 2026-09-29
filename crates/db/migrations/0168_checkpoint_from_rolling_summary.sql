-- The rolling summary becomes a checkpoint boundary row.
--
-- A session's rolling summary (sessions.summary) covered the messages the
-- sliding window had evicted from its active conversation. The harness keeps
-- that state in the conversation instead: a boundary row (role user,
-- metadata {"checkpoint":true}) that the model's conversation loads from
-- (harness/compact/checkpoint.rs). Each non-empty summary is written once as
-- such a row in the session's active chat, and the summary is cleared.
--
-- Placement: the window never held more than its last 80 messages, so the
-- boundary goes just before the 80th-from-last visible row (before the
-- first visible row when there are fewer). Everything still in the window
-- stays after the boundary. The text is `boundary_text(summary, false,
-- OwnerAsked)`: the owner speaks next. The column is dropped at cutover,
-- with its last readers.
-- +goose Up
CREATE TEMP TABLE checkpoint_from_summary AS
SELECT
    s.id AS session_id,
    COALESCE(s.active_chat_id, s.name, 'chat-' || s.id) AS chat_id,
    s.summary AS summary
FROM sessions s
WHERE s.summary IS NOT NULL AND trim(s.summary) != '';

CREATE TEMP TABLE checkpoint_placed AS
SELECT
    c.chat_id AS chat_id,
    c.summary AS summary,
    COALESCE(
        (SELECT m.created_at FROM chat_messages m
         WHERE m.chat_id = c.chat_id
           AND m.rowid > COALESCE((SELECT compacted_below_rowid FROM chats WHERE id = c.chat_id), 0)
         ORDER BY m.created_at DESC, m.rowid DESC LIMIT 1 OFFSET 79),
        (SELECT MIN(m.created_at) FROM chat_messages m
         WHERE m.chat_id = c.chat_id
           AND m.rowid > COALESCE((SELECT compacted_below_rowid FROM chats WHERE id = c.chat_id), 0)),
        unixepoch() + 1
    ) - 1 AS at
FROM checkpoint_from_summary c;

INSERT OR IGNORE INTO chats (id, title, created_at, updated_at)
SELECT chat_id, chat_id, unixepoch(), unixepoch() FROM checkpoint_placed;

INSERT INTO chat_messages (id, chat_id, role, content, metadata, created_at, day_marker)
SELECT
    lower(hex(randomblob(16))),
    chat_id,
    'user',
    'This conversation continues from an earlier part that was summarized:' || char(10) || char(10) || trim(summary)
        || char(10) || char(10) || 'If you need a specific detail from before this summary (an exact snippet, an error message, something you wrote), the earlier conversation is still stored: search it with search_history(query: "...").',
    '{"checkpoint":true,"reason":"migrated","headCut":false,"loadedTools":[]}',
    at,
    date(at, 'unixepoch', 'localtime')
FROM checkpoint_placed;

UPDATE sessions SET summary = NULL
WHERE id IN (SELECT session_id FROM checkpoint_from_summary);

DROP TABLE checkpoint_placed;
DROP TABLE checkpoint_from_summary;
