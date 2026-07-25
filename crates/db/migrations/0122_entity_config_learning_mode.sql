-- Per-employee self-improvement mode: 'auto' (review fork writes learned
-- skills directly), 'staged' (writes go to pending_writes for Inbox approval),
-- 'off' (no review fork). NULL = off until the customer opts in (staging
-- becomes the default once the Inbox review surface ships).
ALTER TABLE entity_config ADD COLUMN learning_mode TEXT;
