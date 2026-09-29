-- How far a linked employee's agent session has gone, and when it last had
-- a turn.
--
-- Nebo keeps the conversation; the agent keeps one working stretch of it in
-- its session. A healthy session carries on from message to message, and the
-- conversation continues in a fresh session, with a summary Nebo writes,
-- once the session has used most of its context window (as the agent itself
-- reports it: ACP's `usage_update`) or has sat idle for a long time. The
-- linked provider records both at the end of every turn; they belong to the
-- session in `linked_chat_id` and are cleared when another session is
-- recorded. NULL = not known yet.
-- +goose Up
ALTER TABLE chats ADD COLUMN linked_used_tokens INTEGER;
ALTER TABLE chats ADD COLUMN linked_window_tokens INTEGER;
ALTER TABLE chats ADD COLUMN linked_turn_at INTEGER;

-- +goose Down
