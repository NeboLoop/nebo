-- Recreate agents table (dropped by erroneous prior migration during development).
-- Matches the schema from 0058 + 0064 + 0070 (renamed from roles → agents).

CREATE TABLE IF NOT EXISTS agents (
    id TEXT PRIMARY KEY,
    kind TEXT,
    name TEXT NOT NULL,
    description TEXT NOT NULL,
    agent_md TEXT NOT NULL,
    frontmatter TEXT NOT NULL,
    pricing_model TEXT,
    pricing_cost REAL,
    is_enabled INTEGER NOT NULL DEFAULT 1,
    installed_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    napp_path TEXT,
    input_values TEXT NOT NULL DEFAULT '{}'
);

CREATE INDEX IF NOT EXISTS idx_agents_name ON agents(name);
