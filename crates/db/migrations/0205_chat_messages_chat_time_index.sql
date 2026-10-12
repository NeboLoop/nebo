-- A chat's messages newest first: the unread dots (each conversation's
-- newest reply), the roster's latest thread (each chat's last activity) and
-- its status line (the newest rows) all read one chat `ORDER BY created_at
-- DESC, rowid DESC`. The only index on chat_id was chat_id alone, so each
-- read fetched every message of the chat from the table and sorted it: the
-- roster and the unread dots, which the apps poll, read all of chat_messages
-- every time. With (chat_id, created_at) — and the rowid every index ends
-- with — the order is the index's, and each read stops at the row it needs.
--
-- 0085 meant to add this (as chat_id, created_at DESC, id DESC) but carried
-- no goose markers, so its whole file ran as the Up script, its "Down" DROP
-- included: the index never existed.
-- +goose Up
CREATE INDEX IF NOT EXISTS idx_chat_messages_chat_time ON chat_messages(chat_id, created_at);

-- +goose Down
DROP INDEX IF EXISTS idx_chat_messages_chat_time;
