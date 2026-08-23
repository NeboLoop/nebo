-- +goose Up
-- Workrooms: the owner's registry of mission rooms. A room IS a loop channel
-- (the hub owns the conversation); this table records which channels are
-- rooms, what the mission is, and which of this bot's employees participate.
CREATE TABLE IF NOT EXISTS workrooms (
    channel_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    mission TEXT NOT NULL DEFAULT '',
    -- JSON array of local agent ids participating in this room.
    member_agent_ids TEXT NOT NULL DEFAULT '[]',
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);

-- +goose Down
DROP TABLE IF EXISTS workrooms;
