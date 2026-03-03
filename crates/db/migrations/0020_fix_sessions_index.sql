-- +goose Up
-- Fix sessions unique index to include name
-- Previously: (scope, scope_id) - only allows ONE session per scope
-- Fixed: (name, scope, scope_id) - allows multiple named sessions per scope

DROP INDEX IF EXISTS idx_sessions_scope;
CREATE UNIQUE INDEX idx_sessions_scope ON sessions(name, scope, scope_id);

-- +goose Down
DROP INDEX IF EXISTS idx_sessions_scope;
CREATE UNIQUE INDEX idx_sessions_scope ON sessions(scope, scope_id);
