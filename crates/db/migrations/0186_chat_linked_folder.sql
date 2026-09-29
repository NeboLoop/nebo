-- The folder a linked coding employee's conversation works in.
--
-- A coding employee (Claude Code, Codex) works in a folder of its own, and
-- moves to another when the owner asks it to in chat ("work in
-- ~/workspaces/foo"): the host starts a new session there and the
-- conversation continues in it. The folder is the conversation's, so it is
-- kept on the chat, and the app shows it ("Works in ~/workspaces/foo").
-- The linked provider records it when the session is made and whenever the
-- host says the conversation moved. NULL = not a linked coding employee's
-- chat, or no turn yet.
-- +goose Up
ALTER TABLE chats ADD COLUMN linked_folder TEXT;

-- +goose Down
