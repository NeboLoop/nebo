-- Ended helper, shell, hand-off and workflow work no longer posts to the
-- Inbox: the employee that started it hears how it ended in its own
-- conversation. The updates it already posted move to History (nothing is
-- deleted).
-- +goose Up
UPDATE notifications SET archived_at = strftime('%s', 'now')
 WHERE id LIKE 'finished:%' AND archived_at IS NULL;

-- +goose Down
SELECT 1;
