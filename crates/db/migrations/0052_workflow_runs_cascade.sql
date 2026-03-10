-- Add ON DELETE CASCADE to workflow_runs.workflow_id foreign key.
-- SQLite doesn't support ALTER TABLE ... ADD CONSTRAINT, so recreate.

CREATE TABLE workflow_runs_new (
    id TEXT PRIMARY KEY,
    workflow_id TEXT NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
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
