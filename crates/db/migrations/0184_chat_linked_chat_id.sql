-- The linked runtime's conversation behind one Nebo chat.
--
-- An employee hired from a linked bot (OpenClaw, Hermes) is driven through
-- the link's chat contract, where the runtime keeps the transcript. One Nebo
-- chat is one runtime session: the linked provider creates the runtime's chat
-- on the thread's first turn, records its id here, and sends every later turn
-- to the same session. NULL = not a linked employee's chat, or no turn yet.
--
-- Same shape as chats.model (0161): one nullable TEXT column on the chat,
-- because the conversation is the thing being mapped.
--
-- First shipped as 0166 while a parallel branch had already taken 0166–0183,
-- so databases migrated by that branch never ran it. Renumbered after that
-- branch's last; a database that applied it as 0166 keeps it applied (the
-- migrator matches a renumbered file by name, `reconcile_renumbered`).
-- +goose Up
ALTER TABLE chats ADD COLUMN linked_chat_id TEXT;

-- +goose Down
