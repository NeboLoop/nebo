-- +goose Up
-- Where Nebo opens: the assistant's latest thread (chat) or the Dashboard.
-- A user preference, not browser storage, so the phone and the desktop agree.
ALTER TABLE user_preferences ADD COLUMN start_page TEXT NOT NULL DEFAULT 'chat';

-- +goose Down
ALTER TABLE user_preferences DROP COLUMN start_page;
