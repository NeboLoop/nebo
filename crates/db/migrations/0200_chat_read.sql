-- Which reply the owner has seen in each conversation. `read_message_id` is
-- the id of the conversation's newest reply (an employee's visible message:
-- not hidden, not Nebo's own, not a failed run's error, not an automation's
-- notice) when the owner last had it open. The sidebar's "New reply" dot is
-- a newer reply than this one. Kept here, on the bot, so the desktop, the
-- web console and the phone agree. NULL = no reply read yet.
--
-- Every conversation that exists today starts read: the dot is for replies
-- that arrive from now on, not for the whole history.
-- +goose Up
ALTER TABLE chats ADD COLUMN read_message_id TEXT;

UPDATE chats SET read_message_id = (
    SELECT r.id FROM chat_messages r
    WHERE r.chat_id = chats.id
      AND r.role = 'assistant'
      AND r.content != ''
      AND (r.metadata IS NULL OR (
            r.metadata NOT LIKE '%"hidden":true%'
        AND r.metadata NOT LIKE '%"isMeta":true%'
        AND r.metadata NOT LIKE '%"runError":true%'
        AND r.metadata NOT LIKE '%"automation":true%'))
    ORDER BY r.created_at DESC, r.rowid DESC LIMIT 1
);

-- +goose Down
ALTER TABLE chats DROP COLUMN read_message_id;
