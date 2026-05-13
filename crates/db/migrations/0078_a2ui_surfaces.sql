-- +goose Up

-- Active A2UI surfaces for agent-rendered UIs.
-- Each row represents a live or persisted surface tied to an agent.
CREATE TABLE IF NOT EXISTS a2ui_surfaces (
    id TEXT PRIMARY KEY,                  -- surface_id: "agent:{agent_id}:{view_id}"
    agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    view_id TEXT NOT NULL,                -- from views.json
    surface_type TEXT NOT NULL DEFAULT 'panel',  -- panel | window | overlay
    components TEXT,                      -- last-known component tree (JSON)
    data_model TEXT,                      -- last-known data model (JSON)
    window_geometry TEXT,                 -- serialized x,y,w,h for window restore
    is_active INTEGER NOT NULL DEFAULT 1,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE INDEX idx_a2ui_surfaces_agent ON a2ui_surfaces(agent_id);
CREATE UNIQUE INDEX idx_a2ui_surfaces_agent_view ON a2ui_surfaces(agent_id, view_id);

-- +goose Down
DROP INDEX IF EXISTS idx_a2ui_surfaces_agent_view;
DROP INDEX IF EXISTS idx_a2ui_surfaces_agent;
DROP TABLE IF EXISTS a2ui_surfaces;
