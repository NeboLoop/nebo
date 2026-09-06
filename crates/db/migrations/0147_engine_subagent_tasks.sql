-- +goose Up
-- Sub-agent and DAG tasks ride the engine (design of record, 2026-09-06).
-- Every orchestrator task becomes an engine run of its own kind ('subagent'
-- or 'dag') with the prompt and its options in `inputs`; attempts, output,
-- error and timing carry over; a task left pending or running by a dead
-- process is queued for the orchestrator's recovery, as before.
-- pending_tasks keeps only the runner's checklist items (task_type
-- 'tracking'): a work-panel list, not durable execution.
INSERT INTO engine_runs (id, kind, state, session_key, agent_id, lane, parent_run_id, inputs, attempts, result, error, created_at, started_at, ended_at)
SELECT p.id,
       p.task_type,
       CASE p.status
         WHEN 'pending' THEN 'queued'
         WHEN 'running' THEN 'running'
         WHEN 'completed' THEN 'done'
         WHEN 'failed' THEN 'failed'
         WHEN 'cancelled' THEN 'cancelled'
         ELSE 'failed'
       END,
       p.session_key,
       '',
       COALESCE(p.lane, 'subagent'),
       CASE WHEN EXISTS (SELECT 1 FROM pending_tasks q WHERE q.id = p.parent_task_id) THEN p.parent_task_id ELSE NULL END,
       json_object(
         'prompt', p.prompt,
         'system_prompt', p.system_prompt,
         'description', p.description,
         'user_id', p.user_id,
         'priority', COALESCE(p.priority, 0),
         'max_attempts', COALESCE(p.max_attempts, 3)
       ),
       COALESCE(p.attempts, 0),
       p.output,
       p.last_error,
       p.created_at,
       p.started_at,
       p.completed_at
FROM pending_tasks p
WHERE p.task_type != 'tracking'
ORDER BY p.created_at, p.rowid;

DELETE FROM pending_tasks WHERE task_type != 'tracking';

-- +goose Down
