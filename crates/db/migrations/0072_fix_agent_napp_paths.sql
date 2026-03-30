-- Complete the roles → agents data migration.
-- 0070 renamed schema (tables/columns/indexes). This migration fixes data values
-- and remaining schema references.

-- Fix napp_path filesystem paths: /roles/ → /agents/
UPDATE agents SET napp_path = REPLACE(napp_path, '/roles/', '/agents/')
  WHERE napp_path LIKE '%/roles/%';

UPDATE workflows SET napp_path = REPLACE(napp_path, '/roles/', '/agents/')
  WHERE napp_path LIKE '%/roles/%';

-- Migrate install code prefix: ROLE- → AGNT-
UPDATE agents SET kind = REPLACE(kind, 'ROLE-', 'AGNT-')
  WHERE kind LIKE 'ROLE-%';

-- Workflow runs: role:{agent_id} → agent:{agent_id}
UPDATE workflow_runs SET workflow_id = REPLACE(workflow_id, 'role:', 'agent:')
  WHERE workflow_id LIKE 'role:%';

-- Cron jobs: role:{agent_id}:{binding} → agent:{agent_id}:{binding}
UPDATE cron_jobs SET command = REPLACE(command, 'role:', 'agent:')
  WHERE command LIKE 'role:%';
UPDATE cron_jobs SET name = REPLACE(name, 'role-', 'agent-')
  WHERE name LIKE 'role-%';

-- Commander role_id → agent_id handled in 0073 (table recreation).
