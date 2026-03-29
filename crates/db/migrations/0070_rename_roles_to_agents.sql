-- Rename roles → agents, role_workflows → agent_workflows
-- Zero backward compat: clean rename of tables, columns, and indexes.

ALTER TABLE roles RENAME TO agents;
ALTER TABLE role_workflows RENAME TO agent_workflows;

-- Rename columns
ALTER TABLE agents RENAME COLUMN role_md TO agent_md;
ALTER TABLE agent_workflows RENAME COLUMN role_id TO agent_id;

-- Rebuild indexes with new names (drop old, create new)
DROP INDEX IF EXISTS idx_role_workflows_role_id;
DROP INDEX IF EXISTS idx_role_workflows_unique;
DROP INDEX IF EXISTS idx_roles_name;

CREATE INDEX idx_agent_workflows_agent_id ON agent_workflows(agent_id);
CREATE UNIQUE INDEX idx_agent_workflows_unique ON agent_workflows(agent_id, binding_name);
CREATE INDEX idx_agents_name ON agents(name);
