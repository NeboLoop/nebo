-- name: ListChannelSkills :many
SELECT * FROM channel_skills WHERE channel_id = ? ORDER BY created_at ASC;

-- name: GetChannelSkill :one
SELECT * FROM channel_skills WHERE channel_id = ? AND skill_name = ?;

-- name: CreateChannelSkill :exec
INSERT INTO channel_skills (channel_id, skill_name)
VALUES (?, ?)
ON CONFLICT (channel_id, skill_name) DO NOTHING;

-- name: DeleteChannelSkill :exec
DELETE FROM channel_skills WHERE channel_id = ? AND skill_name = ?;

-- name: DeleteChannelSkills :exec
DELETE FROM channel_skills WHERE channel_id = ?;

-- name: ListAllChannelSkills :many
SELECT * FROM channel_skills ORDER BY created_at ASC;
