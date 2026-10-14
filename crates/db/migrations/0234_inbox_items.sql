-- The Inbox's two kinds of item, on the one table it already is.
--
-- needs_you: 1 for a decision or question the owner answers on the card
--   (an ask, an approval, a proposal, an update to apply, an employee's own
--   question); 0 for an update (a title, one sentence, a link).
-- options: the answers a needs_you item takes, JSON [{"label","value"}];
--   NULL when it is answered on its own screen (the link).
-- link_label: the words on the item's link ("Orders sheet", "Chat").
-- reply_to: the session an employee's own question is answered into.
-- answered_at / answer / answered_via: how a needs_you item was settled, by
--   whichever surface answered it first.
-- archived_at: the owner put it away. Answered and archived items are
--   History. Nothing is deleted.
-- +goose Up
ALTER TABLE notifications ADD COLUMN needs_you INTEGER NOT NULL DEFAULT 0;
ALTER TABLE notifications ADD COLUMN options TEXT;
ALTER TABLE notifications ADD COLUMN link_label TEXT;
ALTER TABLE notifications ADD COLUMN reply_to TEXT;
ALTER TABLE notifications ADD COLUMN answered_at INTEGER;
ALTER TABLE notifications ADD COLUMN answer TEXT;
ALTER TABLE notifications ADD COLUMN answered_via TEXT;
ALTER TABLE notifications ADD COLUMN archived_at INTEGER;
CREATE INDEX IF NOT EXISTS idx_notifications_needs_you ON notifications(user_id, needs_you, answered_at);

-- What was already a decision is a needs_you item.
UPDATE notifications SET needs_you = 1
 WHERE id LIKE 'permission-ask:%' OR id LIKE 'artifact-update:%' OR id LIKE 'coworker-ask:%'
    OR type IN ('approval', 'permission_ask', 'question_ask');

-- Asks already settled are answered, with the answer and where it came from.
UPDATE notifications
   SET answered_at = (SELECT COALESCE(a.answered_at, a.created_at) FROM permission_asks a
                       WHERE 'permission-ask:' || a.id = notifications.id),
       answer = (SELECT CASE COALESCE(a.answer, a.status)
                           WHEN 'allow_always' THEN 'Allow always'
                           WHEN 'this_once' THEN 'This once'
                           WHEN 'no' THEN 'No'
                           WHEN 'sent' THEN 'It went out'
                           WHEN 'not_sent' THEN 'It didn''t go out'
                           ELSE 'Withdrawn' END
                  FROM permission_asks a WHERE 'permission-ask:' || a.id = notifications.id),
       answered_via = (SELECT a.answered_via FROM permission_asks a
                        WHERE 'permission-ask:' || a.id = notifications.id)
 WHERE id LIKE 'permission-ask:%'
   AND EXISTS (SELECT 1 FROM permission_asks a
                WHERE 'permission-ask:' || a.id = notifications.id AND a.status != 'open');

-- Proposals already decided are answered with their outcome.
UPDATE notifications
   SET answered_at = (SELECT COALESCE(p.resolved_at, p.created_at) FROM pending_writes p
                       WHERE 'learn:' || p.id = notifications.id),
       answer = (SELECT CASE p.status
                           WHEN 'approved' THEN 'Approved'
                           WHEN 'rejected' THEN 'Declined'
                           WHEN 'reverted' THEN 'Undone'
                           WHEN 'conflict' THEN 'Changed meanwhile'
                           ELSE 'Expired' END
                  FROM pending_writes p WHERE 'learn:' || p.id = notifications.id)
 WHERE id LIKE 'learn:%' AND needs_you = 1
   AND EXISTS (SELECT 1 FROM pending_writes p
                WHERE 'learn:' || p.id = notifications.id AND p.status != 'pending');

-- An update notice whose version is no longer the one on offer is settled.
UPDATE notifications
   SET answered_at = created_at, answer = 'Updated'
 WHERE id LIKE 'artifact-update:%'
   AND NOT EXISTS (SELECT 1 FROM artifact_update_prefs c
                    WHERE notifications.id = 'artifact-update:' || c.artifact_type || ':' || c.artifact_id || ':' || c.remote_version
                      AND c.update_available = 1);

-- Coworker cards from before this were read once their run moved on: a read
-- one is no longer waiting.
UPDATE notifications
   SET answered_at = COALESCE(read_at, created_at), answer = 'Done'
 WHERE (id LIKE 'coworker-approval:%' OR id LIKE 'coworker-ask:%' OR id LIKE 'wf-approval:%')
   AND read_at IS NOT NULL;

-- +goose Down
DROP INDEX IF EXISTS idx_notifications_needs_you;
ALTER TABLE notifications DROP COLUMN archived_at;
ALTER TABLE notifications DROP COLUMN answered_via;
ALTER TABLE notifications DROP COLUMN answer;
ALTER TABLE notifications DROP COLUMN answered_at;
ALTER TABLE notifications DROP COLUMN reply_to;
ALTER TABLE notifications DROP COLUMN link_label;
ALTER TABLE notifications DROP COLUMN options;
ALTER TABLE notifications DROP COLUMN needs_you;
