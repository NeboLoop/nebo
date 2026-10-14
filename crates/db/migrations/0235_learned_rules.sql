-- Learned rules: the assistant offers to handle a kind of decision the
-- owner answered the same way three times; on his yes it is a permission
-- rule (`RuleSource::Learned`) like any "Allow always".
--
-- permission_rules.paused_at: the owner turned the rule off. It no longer
--   decides anything; it is kept, with its history, and can be turned on
--   again. Nothing is deleted.
-- pending_writes.resolved_via / resolved_tz: where the owner decided a
--   proposal (mobile, desktop, web, chat) and his time zone then. An offer
--   is a pending_writes row (kind 'rule', action 'offer'); every change to a
--   learned rule is one more applied row (kind 'rule', target the rule's
--   id): its history, kept for good.
-- +goose Up
ALTER TABLE permission_rules ADD COLUMN paused_at INTEGER;
ALTER TABLE pending_writes ADD COLUMN resolved_via TEXT;
ALTER TABLE pending_writes ADD COLUMN resolved_tz TEXT;
CREATE INDEX IF NOT EXISTS idx_pending_writes_kind_target ON pending_writes(kind, target);

-- +goose Down
DROP INDEX IF EXISTS idx_pending_writes_kind_target;
ALTER TABLE pending_writes DROP COLUMN resolved_tz;
ALTER TABLE pending_writes DROP COLUMN resolved_via;
ALTER TABLE permission_rules DROP COLUMN paused_at;
