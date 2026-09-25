-- Every ask is a durable wait in the engine (owner, 09-25): the owner's
-- answer is the signal that wakes it, and silence never answers for the
-- owner. An ask no longer expires, so its expiry column and the index the
-- 72-hour sweep read go.
--
-- Asks the sweep already settled as a No stay settled: their calls were
-- refused and the employee was told. They read as the declines they were.
--
-- Every open ask becomes an engine run of kind `ask`, waiting on its answer
-- (`answer` on `ask:<id>`), with its first reminder due a day from now as
-- the wait's timer.
DROP INDEX IF EXISTS idx_permission_asks_status;
ALTER TABLE permission_asks DROP COLUMN expires_at;
CREATE INDEX IF NOT EXISTS idx_permission_asks_status ON permission_asks (status);

UPDATE permission_asks SET status = 'answered' WHERE status = 'expired';

INSERT INTO engine_runs (id, kind, state, session_key, agent_id, lane)
SELECT a.id, 'ask', 'waiting', a.session_key, a.agent_id, 'main'
FROM permission_asks a
WHERE a.status = 'open' AND NOT EXISTS (SELECT 1 FROM engine_runs r WHERE r.id = a.id);

INSERT INTO engine_waits (run_id, action, on_kind, key, deadline, reason)
SELECT r.id, 'resume', 'answer', 'ask:' || r.id, unixepoch() + 86400, a.sentence
FROM engine_runs r JOIN permission_asks a ON a.id = r.id
WHERE r.kind = 'ask' AND r.current_wait_id IS NULL;

UPDATE engine_runs
SET current_wait_id = (SELECT MAX(w.id) FROM engine_waits w WHERE w.run_id = engine_runs.id)
WHERE kind = 'ask' AND current_wait_id IS NULL;

INSERT INTO engine_events (kind, target_type, target_id, payload, idem_key, retention, due_at)
SELECT 'timer', 'wait', CAST(w.id AS TEXT), w.reason, 'wait:' || w.id || ':deadline', 'transient', w.deadline
FROM engine_waits w JOIN engine_runs r ON r.current_wait_id = w.id
WHERE r.kind = 'ask'
ON CONFLICT(idem_key) DO NOTHING;
