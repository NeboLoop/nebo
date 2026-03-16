-- Rename roles.code → roles.kind and drop UNIQUE constraint.
-- Allows multiple role instances to share the same marketplace kind.

PRAGMA foreign_keys=OFF;

-- Rebuild roles: code → kind, no UNIQUE
CREATE TABLE roles_new (
    id TEXT PRIMARY KEY,
    kind TEXT,
    name TEXT NOT NULL,
    description TEXT NOT NULL,
    role_md TEXT NOT NULL,
    frontmatter TEXT NOT NULL,
    pricing_model TEXT,
    pricing_cost REAL,
    is_enabled INTEGER NOT NULL DEFAULT 1,
    installed_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    napp_path TEXT
);
INSERT INTO roles_new SELECT id, code, name, description, role_md, frontmatter,
    pricing_model, pricing_cost, is_enabled, installed_at, updated_at, napp_path FROM roles;
DROP TABLE roles;
ALTER TABLE roles_new RENAME TO roles;

-- Rebuild role_workflows: add workflow_ref, workflow_id columns, re-establish FK
CREATE TABLE role_workflows_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    role_id TEXT NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
    binding_name TEXT NOT NULL,
    workflow_ref TEXT NOT NULL DEFAULT '',
    workflow_id TEXT,
    trigger_type TEXT NOT NULL,
    trigger_config TEXT NOT NULL,
    description TEXT,
    inputs TEXT,
    is_active INTEGER NOT NULL DEFAULT 1,
    last_fired TEXT,
    UNIQUE(role_id, binding_name)
);
INSERT INTO role_workflows_new (id, role_id, binding_name, workflow_ref, workflow_id,
    trigger_type, trigger_config, description, inputs, is_active, last_fired)
    SELECT id, role_id, binding_name, '', NULL,
    trigger_type, trigger_config, description, inputs, is_active, last_fired FROM role_workflows;
DROP TABLE role_workflows;
ALTER TABLE role_workflows_new RENAME TO role_workflows;

CREATE INDEX idx_roles_kind ON roles(kind);
CREATE INDEX idx_role_workflows_role ON role_workflows(role_id);

PRAGMA foreign_keys=ON;
