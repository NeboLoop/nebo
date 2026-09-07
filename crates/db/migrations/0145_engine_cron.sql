-- +goose Up
-- Scheduled jobs ride the engine (design of record: "One Engine for Durable
-- Work", 2026-09-06). cron_jobs stays as the schedule's DEFINITION — name,
-- cron, what to run — the way agent_workflows defines a binding. Everything
-- durable about it moves: each fire is an engine run of kind 'task' with
-- external_ref 'cron:<job id>'; the next occurrence is ONE pending timer
-- event aimed at binding 'cron:<job id>'; last_run, run_count and last_error
-- are read from those runs. cron_history becomes engine runs and is dropped.

-- Every recorded fire becomes a run. A fire that never recorded an outcome
-- (the process died mid-run) is re-fired ONCE by the boot sweep if it is
-- recent, exactly as the old recovery sweep did; older ones read as failed.
INSERT INTO engine_runs (id, kind, state, session_key, agent_id, lane, inputs, external_ref, result, error, created_at, started_at, ended_at)
SELECT 'cron-legacy-' || h.id,
       'task',
       CASE
         WHEN h.finished_at IS NULL AND COALESCE(unixepoch(h.started_at), 0) >= unixepoch('now') - 86400 THEN 'running'
         WHEN h.finished_at IS NULL THEN 'failed'
         WHEN h.success = 1 THEN 'done'
         ELSE 'failed'
       END,
       'cron-' || COALESCE(j.name, 'job-' || h.job_id),
       COALESCE(j.agent_id, ''),
       'main',
       json_object('job_id', h.job_id),
       'cron:' || h.job_id,
       h.output,
       CASE WHEN h.finished_at IS NULL AND COALESCE(unixepoch(h.started_at), 0) < unixepoch('now') - 86400
            THEN 'interrupted by restart' ELSE h.error END,
       COALESCE(unixepoch(h.started_at), unixepoch('now')),
       unixepoch(h.started_at),
       unixepoch(h.finished_at)
FROM cron_history h
LEFT JOIN cron_jobs j ON j.id = h.job_id
ORDER BY h.id;

-- The floor the next occurrence is computed from: the last fire each job
-- consumed, carried over as an already-delivered timer so no job re-fires
-- an occurrence it already ran.
INSERT INTO engine_events (kind, target_type, target_id, idem_key, schedule, due_at, created_at, delivered_at, attempts, note)
SELECT 'timer', 'binding', 'cron:' || id, 'cron:' || id || ':legacy', schedule,
       unixepoch(last_run), unixepoch(last_run), unixepoch(last_run), 1, 'legacy: last_run carried over'
FROM cron_jobs
WHERE last_run IS NOT NULL AND unixepoch(last_run) IS NOT NULL;

DROP TABLE cron_history;
ALTER TABLE cron_jobs DROP COLUMN last_run;
ALTER TABLE cron_jobs DROP COLUMN run_count;
ALTER TABLE cron_jobs DROP COLUMN last_error;

-- +goose Down
