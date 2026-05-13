-- Backfill session_name for legacy agent chat rows.
-- These rows have id = 'agent:{slug}:web' (used as both chat ID and session key)
-- but session_name was never set because no sessions existed when migration 0075 ran.
-- Also fix titles that are raw session keys (e.g., "agent:chief-of-staff:web").
UPDATE chats SET
  session_name = id,
  title = CASE
    WHEN title = id THEN 'Chat'
    ELSE title
  END
WHERE id LIKE 'agent:%:web'
  AND (session_name IS NULL OR session_name = '');
