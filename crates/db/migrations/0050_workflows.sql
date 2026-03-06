-- +goose Up

CREATE TABLE IF NOT EXISTS workflows (
    id TEXT PRIMARY KEY,
    code TEXT UNIQUE,
    name TEXT NOT NULL,
    version TEXT NOT NULL DEFAULT '1.0',
    definition TEXT NOT NULL,
    skill_md TEXT,
    manifest TEXT,
    is_enabled INTEGER NOT NULL DEFAULT 1,
    installed_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE IF NOT EXISTS workflow_tool_bindings (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    workflow_id TEXT NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
    interface_name TEXT NOT NULL,
    tool_code TEXT NOT NULL,
    UNIQUE(workflow_id, interface_name)
);

CREATE TABLE IF NOT EXISTS workflow_runs (
    id TEXT PRIMARY KEY,
    workflow_id TEXT NOT NULL REFERENCES workflows(id),
    trigger_type TEXT NOT NULL,
    trigger_detail TEXT,
    status TEXT NOT NULL DEFAULT 'running',
    inputs TEXT,
    current_activity TEXT,
    total_tokens_used INTEGER DEFAULT 0,
    error TEXT,
    error_activity TEXT,
    session_key TEXT,
    started_at INTEGER NOT NULL DEFAULT (unixepoch()),
    completed_at INTEGER
);

CREATE TABLE IF NOT EXISTS workflow_activity_results (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id TEXT NOT NULL REFERENCES workflow_runs(id) ON DELETE CASCADE,
    activity_id TEXT NOT NULL,
    status TEXT NOT NULL,
    tokens_used INTEGER DEFAULT 0,
    attempts INTEGER DEFAULT 1,
    error TEXT,
    started_at INTEGER NOT NULL,
    completed_at INTEGER
);

CREATE TABLE IF NOT EXISTS roles (
    id TEXT PRIMARY KEY,
    code TEXT UNIQUE,
    name TEXT NOT NULL,
    description TEXT NOT NULL,
    role_md TEXT NOT NULL,
    frontmatter TEXT NOT NULL,
    pricing_model TEXT,
    pricing_cost REAL,
    is_enabled INTEGER NOT NULL DEFAULT 1,
    installed_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

-- +goose Down
DROP TABLE IF EXISTS workflow_activity_results;
DROP TABLE IF EXISTS workflow_runs;
DROP TABLE IF EXISTS workflow_tool_bindings;
DROP TABLE IF EXISTS workflows;
DROP TABLE IF EXISTS roles;
