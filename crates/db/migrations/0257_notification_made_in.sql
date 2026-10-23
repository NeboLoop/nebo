-- made_in: the session an employee's run posted the item from
--   (`push_notification`, `message_owner`); NULL for Nebo's own items. A
--   schedule's answer reads it to tell whether the run that answered already
--   brought the owner an item of its own, whatever that item links to.
-- +goose Up
ALTER TABLE notifications ADD COLUMN made_in TEXT;

-- +goose Down
ALTER TABLE notifications DROP COLUMN made_in;
