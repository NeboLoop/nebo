-- Drop FK constraint on workflow_runs.workflow_id to allow inline workflow runs
-- (role heartbeat/event/schedule bindings use "role:{role_id}" as workflow_id
-- which has no corresponding row in the workflows table).

PRAGMA foreign_keys = OFF;

CREATE TABLE workflow_runs_new (
    id TEXT PRIMARY KEY,
    workflow_id TEXT NOT NULL,
    trigger_type TEXT NOT NULL,
    trigger_detail TEXT,
    status TEXT NOT NULL DEFAULT 'running',
    inputs TEXT,
    current_activity TEXT,
    total_tokens_used INTEGER DEFAULT 0,
    error TEXT,
    error_activity TEXT,
    session_key TEXT,
    started_at INTEGER NOT NULL DEFAULT (unixepoch()),
    completed_at INTEGER
);

INSERT INTO workflow_runs_new SELECT * FROM workflow_runs;
DROP TABLE workflow_runs;
ALTER TABLE workflow_runs_new RENAME TO workflow_runs;

PRAGMA foreign_keys = ON;
