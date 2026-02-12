-- name: ListAdvisors :many
SELECT * FROM advisors ORDER BY priority DESC;

-- name: GetAdvisor :one
SELECT * FROM advisors WHERE name = ?;

-- name: GetAdvisorByID :one
SELECT * FROM advisors WHERE id = ?;

-- name: CreateAdvisor :one
INSERT INTO advisors (name, role, description, priority, enabled, memory_access, persona, timeout_seconds)
VALUES (?, ?, ?, ?, ?, ?, ?, ?)
RETURNING *;

-- name: UpdateAdvisor :exec
UPDATE advisors SET
    role = ?,
    description = ?,
    priority = ?,
    enabled = ?,
    memory_access = ?,
    persona = ?,
    timeout_seconds = ?,
    updated_at = strftime('%s', 'now')
WHERE name = ?;

-- name: DeleteAdvisor :exec
DELETE FROM advisors WHERE name = ?;

-- name: ListEnabledAdvisors :many
SELECT * FROM advisors WHERE enabled = 1 ORDER BY priority DESC;
