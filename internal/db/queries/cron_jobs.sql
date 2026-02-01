-- Cron job queries

-- name: ListCronJobs :many
SELECT id, name, schedule, command, task_type, message, deliver, enabled, last_run, run_count, last_error, created_at
FROM cron_jobs
ORDER BY created_at DESC
LIMIT sqlc.arg(limit) OFFSET sqlc.arg(offset);

-- name: GetCronJob :one
SELECT id, name, schedule, command, task_type, message, deliver, enabled, last_run, run_count, last_error, created_at
FROM cron_jobs
WHERE id = ?;

-- name: GetCronJobByName :one
SELECT id, name, schedule, command, task_type, message, deliver, enabled, last_run, run_count, last_error, created_at
FROM cron_jobs
WHERE name = ?;

-- name: CreateCronJob :one
INSERT INTO cron_jobs (name, schedule, command, task_type, message, deliver, enabled)
VALUES (?, ?, ?, ?, ?, ?, ?)
RETURNING id, name, schedule, command, task_type, message, deliver, enabled, last_run, run_count, last_error, created_at;

-- name: UpdateCronJob :exec
UPDATE cron_jobs
SET name = COALESCE(sqlc.narg(name), name),
    schedule = COALESCE(sqlc.narg(schedule), schedule),
    command = COALESCE(sqlc.narg(command), command),
    task_type = COALESCE(sqlc.narg(task_type), task_type),
    message = COALESCE(sqlc.narg(message), message),
    deliver = COALESCE(sqlc.narg(deliver), deliver)
WHERE id = sqlc.arg(id);

-- name: DeleteCronJob :exec
DELETE FROM cron_jobs WHERE id = ?;

-- name: ToggleCronJob :exec
UPDATE cron_jobs
SET enabled = NOT enabled
WHERE id = ?;

-- name: SetCronJobEnabled :exec
UPDATE cron_jobs
SET enabled = ?
WHERE id = ?;

-- name: UpdateCronJobLastRun :exec
UPDATE cron_jobs
SET last_run = CURRENT_TIMESTAMP,
    run_count = run_count + 1,
    last_error = sqlc.narg(last_error)
WHERE id = sqlc.arg(id);

-- name: CountCronJobs :one
SELECT COUNT(*) as total FROM cron_jobs;

-- name: ListEnabledCronJobs :many
SELECT id, name, schedule, command, task_type, message, deliver, enabled, last_run, run_count, last_error, created_at
FROM cron_jobs
WHERE enabled = 1
ORDER BY name;

-- Cron history queries

-- name: CreateCronHistory :one
INSERT INTO cron_history (job_id, started_at)
VALUES (?, CURRENT_TIMESTAMP)
RETURNING id, job_id, started_at, finished_at, success, output, error;

-- name: UpdateCronHistory :exec
UPDATE cron_history
SET finished_at = CURRENT_TIMESTAMP,
    success = ?,
    output = ?,
    error = ?
WHERE id = ?;

-- name: ListCronHistory :many
SELECT id, job_id, started_at, finished_at, success, output, error
FROM cron_history
WHERE job_id = ?
ORDER BY started_at DESC
LIMIT sqlc.arg(limit) OFFSET sqlc.arg(offset);

-- name: GetRecentCronHistory :many
SELECT id, job_id, started_at, finished_at, success, output, error
FROM cron_history
WHERE job_id = ?
ORDER BY started_at DESC
LIMIT 10;

-- name: CountCronHistory :one
SELECT COUNT(*) as total FROM cron_history WHERE job_id = ?;
