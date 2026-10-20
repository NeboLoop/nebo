-- Verified: code work is not reported done until independent reviewers in
-- fresh contexts fail to break it and the project's own build and tests
-- pass. One `verifications` row per piece of verified work (a builder's
-- change to one project), and one `verification_rounds` row per step of its
-- history: a review round (the whole change, then each round's delta), the
-- catch-up with the owner's project before landing, the landing itself, and
-- the live checks after it.
--
-- The row ties the work to where it is shown: the builder's conversation
-- (session_key, chat_id, the reply the badge sits on), a workflow run and
-- its activity, a hand-off and a project. project_root is the owner's
-- folder; work_dir the isolated copy the work happens in (the folder itself
-- for an app's pages); base_ref the snapshot the change is measured from;
-- landed_ref the commit it landed as on the owner's branch (landed_at when),
-- empty while the work is still in its copy.
--
-- state: running | paused (asking the owner) | verified | not_verified |
-- stopped. reason says why a row is not verified, in the owner's words.
-- The open findings and the deferred minors are kept whole as JSON; the
-- counts are for lists and badges.
-- +goose Up
CREATE TABLE verifications (
    id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL DEFAULT '',
    session_key TEXT NOT NULL,
    chat_id TEXT NOT NULL DEFAULT '',
    message_id TEXT,
    workflow_run_id TEXT,
    activity_id TEXT,
    handoff_id TEXT,
    project_id TEXT,
    spec TEXT NOT NULL DEFAULT '',
    project_root TEXT NOT NULL DEFAULT '',
    work_dir TEXT NOT NULL DEFAULT '',
    isolation TEXT NOT NULL DEFAULT 'none' CHECK (isolation IN ('worktree', 'copy', 'none')),
    branch TEXT NOT NULL DEFAULT '',
    base_ref TEXT NOT NULL DEFAULT '',
    state TEXT NOT NULL DEFAULT 'running' CHECK (state IN ('running', 'paused', 'verified', 'not_verified', 'stopped')),
    reason TEXT NOT NULL DEFAULT '',
    rounds INTEGER NOT NULL DEFAULT 0,
    open_blockers INTEGER NOT NULL DEFAULT 0,
    open_majors INTEGER NOT NULL DEFAULT 0,
    open_minors INTEGER NOT NULL DEFAULT 0,
    open_json TEXT NOT NULL DEFAULT '[]',
    deferred_json TEXT NOT NULL DEFAULT '[]',
    summary TEXT NOT NULL DEFAULT '',
    cost_microcents INTEGER NOT NULL DEFAULT 0,
    landed_ref TEXT NOT NULL DEFAULT '',
    landed_at INTEGER,
    created_at INTEGER NOT NULL,
    finished_at INTEGER
);
CREATE INDEX idx_verifications_session ON verifications(session_key, created_at);
CREATE INDEX idx_verifications_chat ON verifications(chat_id, created_at);
CREATE INDEX idx_verifications_run ON verifications(workflow_run_id) WHERE workflow_run_id IS NOT NULL;
CREATE INDEX idx_verifications_live ON verifications(project_root) WHERE state IN ('running', 'paused');

-- kind: review (round 1 reviews the whole change, later rounds its delta),
-- catch_up (brought up to date with the owner's project, re-tested), land
-- (merged as one commit), live (health, logs and data after it shipped).
-- detail_json carries what a non-review step did (caught up to which
-- commit, conflicts resolved, landed as which commit, each live check).
CREATE TABLE verification_rounds (
    verification_id TEXT NOT NULL REFERENCES verifications(id) ON DELETE CASCADE,
    round INTEGER NOT NULL,
    kind TEXT NOT NULL DEFAULT 'review' CHECK (kind IN ('review', 'catch_up', 'land', 'live')),
    lenses_json TEXT NOT NULL DEFAULT '[]',
    base_ref TEXT NOT NULL DEFAULT '',
    head_ref TEXT NOT NULL DEFAULT '',
    diff_stat TEXT NOT NULL DEFAULT '',
    findings_json TEXT NOT NULL DEFAULT '[]',
    verified_ok_json TEXT NOT NULL DEFAULT '[]',
    prior_json TEXT NOT NULL DEFAULT '[]',
    checks_json TEXT NOT NULL DEFAULT '[]',
    fixed_json TEXT NOT NULL DEFAULT '[]',
    decisions_json TEXT NOT NULL DEFAULT '[]',
    detail_json TEXT NOT NULL DEFAULT '{}',
    outcome TEXT NOT NULL DEFAULT '',
    cost_microcents INTEGER NOT NULL DEFAULT 0,
    started_at INTEGER NOT NULL,
    finished_at INTEGER,
    PRIMARY KEY (verification_id, round)
);

-- +goose Down
DROP TABLE verification_rounds;
DROP INDEX idx_verifications_live;
DROP INDEX idx_verifications_run;
DROP INDEX idx_verifications_chat;
DROP INDEX idx_verifications_session;
DROP TABLE verifications;
