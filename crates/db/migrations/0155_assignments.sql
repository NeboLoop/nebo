-- +goose Up
-- An assignment: work handed from one seat to another as the assignee's OWN
-- work, tracked to done. The assignee works it as a case keyed on the
-- assignment id; the assigner hears `assignment.done | blocked | failed`
-- when it closes. Implementation PRD R5.
CREATE TABLE IF NOT EXISTS assignments (
    id TEXT PRIMARY KEY,
    assigner_agent_id TEXT NOT NULL,
    assigner_session_key TEXT NOT NULL DEFAULT '',
    assignee_agent_id TEXT NOT NULL,
    subject TEXT NOT NULL,
    done_means TEXT NOT NULL DEFAULT '',
    due TEXT,
    state TEXT NOT NULL DEFAULT 'open' CHECK (state IN ('open', 'done', 'blocked', 'failed')),
    outcome TEXT,
    parent_run_id TEXT,
    case_key TEXT NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    closed_at INTEGER
);
CREATE INDEX IF NOT EXISTS idx_assignments_assignee ON assignments(assignee_agent_id, state);
CREATE INDEX IF NOT EXISTS idx_assignments_assigner ON assignments(assigner_agent_id, state);

-- A binding whose trigger names a capability with no Connection to serve it
-- is kept, not dropped, and says why it is not running. R7.
ALTER TABLE agent_workflows ADD COLUMN degraded_reason TEXT;

-- +goose Down
