-- +goose Up
-- Workflow runs ride the engine (design of record, 2026-09-06). A workflow
-- run's durable identity — state, snapshotted definition, inputs, session,
-- result, error, the one resume after a restart, the wait it is parked on —
-- is an engine run under the SAME id. workflow_runs keeps what is specific
-- to a workflow: which workflow, what triggered it, the current activity,
-- tokens, the failing activity, the reporting stamp. A parked approval
-- (workflow_run_suspensions) is the run's live wait: action resume, woken
-- by an approval, carrying the parked conversation.

INSERT INTO engine_runs (id, kind, state, session_key, agent_id, lane, definition, inputs, resume_attempted, result, error, summary, created_at, started_at, ended_at)
SELECT id,
       'workflow',
       CASE status
         WHEN 'running' THEN 'running'
         WHEN 'completed' THEN 'done'
         WHEN 'exited' THEN 'done'
         WHEN 'failed' THEN 'failed'
         WHEN 'cancelled' THEN 'cancelled'
         WHEN 'denied' THEN 'cancelled'
         WHEN 'interrupted' THEN 'interrupted'
         WHEN 'awaiting_approval' THEN 'waiting'
         WHEN 'suspended' THEN 'waiting'
         ELSE 'failed'
       END,
       COALESCE(session_key, ''),
       CASE WHEN workflow_id LIKE 'agent:%' THEN substr(workflow_id, 7) ELSE '' END,
       'main',
       definition,
       inputs,
       COALESCE(resume_attempted, 0),
       output,
       error,
       CASE status WHEN 'exited' THEN 'exited' WHEN 'denied' THEN 'denied' ELSE '' END,
       started_at,
       started_at,
       completed_at
FROM workflow_runs
WHERE true
ON CONFLICT(id) DO NOTHING;

INSERT INTO engine_waits (run_id, action, on_kind, key, parked, reason, created_at)
SELECT s.run_id, 'resume', 'approval', 'approval:' || s.run_id,
       json_object(
         'agent_id', s.agent_id,
         'binding_name', s.binding_name,
         'activity_id', s.activity_id,
         'iteration', s.iteration,
         'step_index', s.step_index,
         'messages', s.messages,
         'pending_tool', s.pending_tool,
         'operation', s.operation,
         'display', s.display
       ),
       s.display,
       s.created_at
FROM workflow_run_suspensions s
WHERE EXISTS (SELECT 1 FROM engine_runs r WHERE r.id = s.run_id);

UPDATE engine_runs
SET state = 'waiting',
    current_wait_id = (SELECT w.id FROM engine_waits w WHERE w.run_id = engine_runs.id AND w.superseded_at IS NULL ORDER BY w.id DESC LIMIT 1)
WHERE id IN (SELECT run_id FROM workflow_run_suspensions);

DROP TABLE workflow_run_suspensions;

DROP INDEX IF EXISTS idx_workflow_runs_workflow_status;
ALTER TABLE workflow_runs DROP COLUMN status;
ALTER TABLE workflow_runs DROP COLUMN inputs;
ALTER TABLE workflow_runs DROP COLUMN error;
ALTER TABLE workflow_runs DROP COLUMN session_key;
ALTER TABLE workflow_runs DROP COLUMN output;
ALTER TABLE workflow_runs DROP COLUMN definition;
ALTER TABLE workflow_runs DROP COLUMN resume_attempted;

-- +goose Down
