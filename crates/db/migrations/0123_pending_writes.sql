-- Staged self-improvement writes (docs/design/SELF_IMPROVEMENT.md WS3).
-- When an employee's learning_mode is 'staged', the review fork's learned-skill
-- writes land here instead of on disk; the owner approves or rejects from the
-- Inbox (notification id learn:<id>). target_hash = hash of the target
-- SKILL.md at stage time ('' for create) — re-checked at approve so a target
-- that changed meanwhile surfaces as a conflict instead of a blind replay.
CREATE TABLE pending_writes (
    id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    action TEXT NOT NULL,
    target TEXT NOT NULL,
    content TEXT,
    gist TEXT NOT NULL,
    target_hash TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'pending',
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    resolved_at INTEGER
);

CREATE INDEX idx_pending_writes_status ON pending_writes(status, created_at);
