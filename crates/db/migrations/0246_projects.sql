-- Projects: one piece of work a team's lead owns, with its own thread. A
-- project is a workflow run made visible, not a second engine: its runs are
-- `workflow_runs` rows (project_id), its cards are `handoffs` rows
-- (project_id), its thread is a team-style thread (session key
-- `team:<team>:project:<id>`, written by the one team-row writer).
--
-- team_id is '' for a one-off project (an employee asked directly, leading
-- the work itself). asker_session_key is the conversation the work came
-- from ('' when the owner opened it in the team), asker_agent_id the
-- employee that asked ('' = the owner). playbook_binding names the lead's
-- workflow binding a recurring project runs ('agent:<lead>:<binding>'), NULL
-- for a project planned from an ask. limit_steps / limit_spend_cents are the
-- run limit that pauses the project and asks (0 = the default).
--
-- teams.standing is what doesn't change for the team (brand kit, rules, who
-- does what): every project and every member's briefing carries it, next to
-- the existing mission.
-- +goose Up
CREATE TABLE projects (
    id TEXT PRIMARY KEY,
    team_id TEXT NOT NULL DEFAULT '',
    lead_agent_id TEXT NOT NULL,
    title TEXT NOT NULL,
    ask TEXT NOT NULL DEFAULT '',
    asker_session_key TEXT NOT NULL DEFAULT '',
    asker_agent_id TEXT NOT NULL DEFAULT '',
    thread_session_key TEXT NOT NULL UNIQUE,
    playbook_binding TEXT,
    state TEXT NOT NULL DEFAULT 'live' CHECK (state IN ('live', 'paused', 'done')),
    limit_steps INTEGER NOT NULL DEFAULT 0,
    limit_spend_cents INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    closed_at INTEGER
);
CREATE INDEX idx_projects_team ON projects(team_id, state, created_at);

ALTER TABLE workflow_runs ADD COLUMN project_id TEXT;
ALTER TABLE handoffs ADD COLUMN project_id TEXT;
CREATE INDEX idx_handoffs_project ON handoffs(project_id) WHERE project_id IS NOT NULL;
ALTER TABLE teams ADD COLUMN standing TEXT NOT NULL DEFAULT '';

-- +goose Down
ALTER TABLE teams DROP COLUMN standing;
DROP INDEX idx_handoffs_project;
ALTER TABLE handoffs DROP COLUMN project_id;
ALTER TABLE workflow_runs DROP COLUMN project_id;
DROP INDEX idx_projects_team;
DROP TABLE projects;
