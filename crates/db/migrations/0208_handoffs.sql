-- Hand-offs: the trace of work one employee passes to another, kept after
-- it ends so the owner can see who sent what to whom, whether it is still
-- going, and what came back.
--
-- One row per delivery: a message between employees (a team post asks
-- each member it addresses in a row of its own) or an assignment. parent_id
-- is the hand-off the sending conversation was itself working on, so a
-- chain A -> B -> C reads as one tree. sender_session is the conversation
-- that handed the work on; receiver_session the one the receiving employee
-- works it in; receiver_run_id the engine run for an assignment's case.
-- status: queued -> running -> done | failed | stopped.
-- +goose Up
CREATE TABLE handoffs (
    id TEXT PRIMARY KEY,
    parent_id TEXT,
    kind TEXT NOT NULL DEFAULT 'message' CHECK (kind IN ('message', 'assignment')),
    from_agent_id TEXT NOT NULL DEFAULT '',
    to_agent_id TEXT NOT NULL,
    team_id TEXT NOT NULL DEFAULT '',
    sender_session TEXT NOT NULL DEFAULT '',
    sender_run_id TEXT,
    receiver_session TEXT NOT NULL DEFAULT '',
    receiver_run_id TEXT,
    ask TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'queued' CHECK (status IN ('queued', 'running', 'done', 'failed', 'stopped')),
    result TEXT NOT NULL DEFAULT '',
    error TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL,
    started_at INTEGER,
    finished_at INTEGER
);
CREATE INDEX idx_handoffs_sender ON handoffs(sender_session, created_at);
CREATE INDEX idx_handoffs_receiver ON handoffs(receiver_session, status);
CREATE INDEX idx_handoffs_parent ON handoffs(parent_id);
CREATE INDEX idx_handoffs_live ON handoffs(status) WHERE status IN ('queued', 'running');

-- +goose Down
DROP INDEX idx_handoffs_live;
DROP INDEX idx_handoffs_parent;
DROP INDEX idx_handoffs_receiver;
DROP INDEX idx_handoffs_sender;
DROP TABLE handoffs;
