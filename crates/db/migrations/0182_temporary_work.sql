-- Temporary work (owner, 09-25): a workflow or a team made for one piece of
-- work. It is the same workflow or team as any other, created through the
-- same path; this table is its lifetime option. Temporary work runs once:
-- `run_id` is the one run it started (a workflow), or the case its one
-- assignment opened (a team). When that run has ended and its outcome has
-- reached the owner, the workflow or team is deleted and this row goes with
-- it. Its run history, receipts and cost stay where every run's do.
CREATE TABLE IF NOT EXISTS temporary_work (
    kind       TEXT    NOT NULL CHECK (kind IN ('workflow', 'team')),
    -- The employee whose workflow it is; '' for a team.
    agent_id   TEXT    NOT NULL DEFAULT '',
    -- The workflow's binding name, or the team's id.
    name       TEXT    NOT NULL,
    -- The session that started it: woken with the outcome.
    report_to  TEXT    NOT NULL DEFAULT '',
    run_id     TEXT,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    PRIMARY KEY (kind, agent_id, name)
);
CREATE INDEX IF NOT EXISTS idx_temporary_work_run ON temporary_work (run_id);
