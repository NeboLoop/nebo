-- name: CreatePendingTask :one
INSERT INTO pending_tasks (
    id, task_type, status, session_key, user_id, prompt, system_prompt,
    description, lane, priority, created_at
) VALUES (?, ?, 'pending', ?, ?, ?, ?, ?, ?, ?, ?)
RETURNING *;

-- name: GetPendingTask :one
SELECT * FROM pending_tasks WHERE id = ?;

-- name: GetPendingTasksByStatus :many
SELECT * FROM pending_tasks
WHERE status = ?
ORDER BY priority DESC, created_at ASC;

-- name: GetRecoverableTasks :many
-- Get tasks that were running or pending when agent shut down
SELECT * FROM pending_tasks
WHERE status IN ('pending', 'running')
ORDER BY priority DESC, created_at ASC;

-- name: GetTasksByLaneAndStatus :many
SELECT * FROM pending_tasks
WHERE lane = ? AND status = ?
ORDER BY priority DESC, created_at ASC;

-- name: GetTasksByUser :many
SELECT * FROM pending_tasks
WHERE user_id = ? AND status IN ('pending', 'running')
ORDER BY created_at DESC;

-- name: GetChildTasks :many
SELECT * FROM pending_tasks
WHERE parent_task_id = ?
ORDER BY created_at ASC;

-- name: UpdateTaskStatus :exec
UPDATE pending_tasks
SET status = @status,
    started_at = CASE WHEN @status = 'running' THEN unixepoch() ELSE started_at END,
    completed_at = CASE WHEN @status IN ('completed', 'failed', 'cancelled') THEN unixepoch() ELSE completed_at END
WHERE id = @id;

-- name: UpdateTaskRunning :exec
UPDATE pending_tasks
SET status = 'running',
    started_at = unixepoch(),
    attempts = attempts + 1
WHERE id = ?;

-- name: UpdateTaskCompleted :exec
UPDATE pending_tasks
SET status = 'completed',
    completed_at = unixepoch()
WHERE id = ?;

-- name: UpdateTaskFailed :exec
UPDATE pending_tasks
SET status = CASE WHEN attempts >= max_attempts THEN 'failed' ELSE 'pending' END,
    last_error = ?,
    completed_at = CASE WHEN attempts >= max_attempts THEN unixepoch() ELSE NULL END
WHERE id = ?;

-- name: CancelTask :exec
UPDATE pending_tasks
SET status = 'cancelled',
    completed_at = unixepoch()
WHERE id = ?;

-- name: CancelChildTasks :exec
UPDATE pending_tasks
SET status = 'cancelled',
    completed_at = unixepoch()
WHERE parent_task_id = ? AND status IN ('pending', 'running');

-- name: DeleteCompletedTasks :exec
-- Clean up old completed tasks (keep last 7 days)
DELETE FROM pending_tasks
WHERE status IN ('completed', 'failed', 'cancelled')
  AND completed_at < unixepoch() - (7 * 24 * 60 * 60);

-- name: CountTasksByStatus :one
SELECT
    COUNT(*) FILTER (WHERE status = 'pending') as pending,
    COUNT(*) FILTER (WHERE status = 'running') as running,
    COUNT(*) FILTER (WHERE status = 'completed') as completed,
    COUNT(*) FILTER (WHERE status = 'failed') as failed
FROM pending_tasks;
