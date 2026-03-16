-- +goose Up
CREATE TABLE IF NOT EXISTS role_workflows (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    role_id TEXT NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
    binding_name TEXT NOT NULL,
    trigger_type TEXT NOT NULL,
    trigger_config TEXT NOT NULL,
    description TEXT,
    inputs TEXT,
    is_active INTEGER NOT NULL DEFAULT 1,
    UNIQUE(role_id, binding_name)
);
CREATE INDEX IF NOT EXISTS idx_role_workflows_role ON role_workflows(role_id);

-- +goose Down
DROP TABLE IF EXISTS role_workflows;
