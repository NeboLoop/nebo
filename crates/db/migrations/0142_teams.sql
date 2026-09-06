-- +goose Up
-- Teams: a team is a LOCAL object on this Nebo — a name, a mission, and the
-- employees in it — with its own local thread (session key `team:<id>`).
-- It needs no hub. `hub_channel_id` is NULL until (if ever) the team is
-- mirrored to a NeboAI hub loop channel; then posts are forwarded there too.
-- This replaces the hub-keyed `workrooms` registry: every existing room gets
-- a fresh local id and keeps its channel as the hub mirror.
CREATE TABLE IF NOT EXISTS teams (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    mission TEXT NOT NULL DEFAULT '',
    -- JSON array of local agent ids.
    member_agent_ids TEXT NOT NULL DEFAULT '[]',
    -- The employee that created the team ('' = the owner did). The organizer
    -- and the owner can re-open the floor; other members answer when asked.
    organizer_agent_id TEXT NOT NULL DEFAULT '',
    hub_channel_id TEXT,
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_teams_hub_channel
    ON teams(hub_channel_id) WHERE hub_channel_id IS NOT NULL;

INSERT INTO teams (id, name, mission, member_agent_ids, organizer_agent_id, hub_channel_id, created_at)
SELECT lower(
           hex(randomblob(4)) || '-' || hex(randomblob(2)) || '-4' ||
           substr(hex(randomblob(2)), 2) || '-' ||
           substr('89ab', abs(random()) % 4 + 1, 1) || substr(hex(randomblob(2)), 2) || '-' ||
           hex(randomblob(6))
       ),
       name, mission, member_agent_ids,
       COALESCE(json_extract(member_agent_ids, '$[0]'), ''),
       channel_id, created_at
FROM workrooms;

DROP TABLE IF EXISTS workrooms;

-- +goose Down
CREATE TABLE IF NOT EXISTS workrooms (
    channel_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    mission TEXT NOT NULL DEFAULT '',
    member_agent_ids TEXT NOT NULL DEFAULT '[]',
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);
INSERT INTO workrooms (channel_id, name, mission, member_agent_ids, created_at)
SELECT hub_channel_id, name, mission, member_agent_ids, created_at
FROM teams WHERE hub_channel_id IS NOT NULL;
DROP TABLE IF EXISTS teams;
