-- +goose Up

DROP TABLE IF EXISTS commander_dispatches;
DROP TABLE IF EXISTS commander_edges;
DROP TABLE IF EXISTS commander_node_positions;
DROP TABLE IF EXISTS commander_team_members;
DROP TABLE IF EXISTS commander_teams;

CREATE TABLE IF NOT EXISTS commander_teams (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    color TEXT NOT NULL DEFAULT '#6366f1',
    position_x REAL NOT NULL DEFAULT 0,
    position_y REAL NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS commander_team_members (
    team_id TEXT NOT NULL REFERENCES commander_teams(id) ON DELETE CASCADE,
    role_id TEXT NOT NULL,
    PRIMARY KEY (team_id, role_id)
);

CREATE TABLE IF NOT EXISTS commander_node_positions (
    node_id TEXT PRIMARY KEY,
    position_x REAL NOT NULL DEFAULT 0,
    position_y REAL NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS commander_edges (
    id TEXT PRIMARY KEY,
    source_node_id TEXT NOT NULL,
    target_node_id TEXT NOT NULL,
    edge_type TEXT NOT NULL DEFAULT 'reports_to',
    label TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);

-- +goose Down
DROP TABLE IF EXISTS commander_edges;
DROP TABLE IF EXISTS commander_node_positions;
DROP TABLE IF EXISTS commander_team_members;
DROP TABLE IF EXISTS commander_teams;
