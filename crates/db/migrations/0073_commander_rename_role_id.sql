-- Rename role_id → agent_id in commander_team_members.
-- 0072 added ALTER TABLE RENAME COLUMN but it was appended after the migration
-- had already been recorded as applied, so it never ran.
-- SQLite table recreation is the most reliable approach.

CREATE TABLE commander_team_members_new (
    team_id TEXT NOT NULL REFERENCES commander_teams(id) ON DELETE CASCADE,
    agent_id TEXT NOT NULL,
    PRIMARY KEY (team_id, agent_id)
);

INSERT INTO commander_team_members_new (team_id, agent_id)
  SELECT team_id, role_id FROM commander_team_members;

DROP TABLE commander_team_members;

ALTER TABLE commander_team_members_new RENAME TO commander_team_members;
