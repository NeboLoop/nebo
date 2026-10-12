-- A conversation the owner pinned: it stays at the top of the employee's
-- list, above the recent ones, until he unpins it. `pinned_at` is when he
-- pinned it (unix seconds), which orders the pinned ones, newest pin first.
-- NULL = not pinned. Kept here, on the bot, so the desktop, the web console
-- and the phone agree.
-- +goose Up
ALTER TABLE chats ADD COLUMN pinned_at INTEGER;

-- +goose Down
ALTER TABLE chats DROP COLUMN pinned_at;
