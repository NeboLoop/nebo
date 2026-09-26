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
-- +goose Up
ALTER TABLE chats ADD COLUMN linked_chat_id TEXT;

-- +goose Down
