-- Agent profile queries (singleton table)

-- name: GetAgentProfile :one
SELECT id, name, personality_preset, custom_personality, voice_style,
       response_length, emoji_usage, formality, proactivity,
       emoji, creature, vibe, role, avatar, agent_rules, tool_notes,
       quiet_hours_start, quiet_hours_end,
       created_at, updated_at
FROM agent_profile
WHERE id = 1;

-- name: UpdateAgentProfile :exec
UPDATE agent_profile
SET name = COALESCE(sqlc.narg(name), name),
    personality_preset = COALESCE(sqlc.narg(personality_preset), personality_preset),
    custom_personality = COALESCE(sqlc.narg(custom_personality), custom_personality),
    voice_style = COALESCE(sqlc.narg(voice_style), voice_style),
    response_length = COALESCE(sqlc.narg(response_length), response_length),
    emoji_usage = COALESCE(sqlc.narg(emoji_usage), emoji_usage),
    formality = COALESCE(sqlc.narg(formality), formality),
    proactivity = COALESCE(sqlc.narg(proactivity), proactivity),
    emoji = COALESCE(sqlc.narg(emoji), emoji),
    creature = COALESCE(sqlc.narg(creature), creature),
    vibe = COALESCE(sqlc.narg(vibe), vibe),
    role = COALESCE(sqlc.narg(role), role),
    avatar = COALESCE(sqlc.narg(avatar), avatar),
    agent_rules = COALESCE(sqlc.narg(agent_rules), agent_rules),
    tool_notes = COALESCE(sqlc.narg(tool_notes), tool_notes),
    quiet_hours_start = COALESCE(sqlc.narg(quiet_hours_start), quiet_hours_start),
    quiet_hours_end = COALESCE(sqlc.narg(quiet_hours_end), quiet_hours_end),
    updated_at = unixepoch()
WHERE id = 1;

-- name: EnsureAgentProfile :exec
INSERT INTO agent_profile (id, name, created_at, updated_at)
VALUES (1, 'Nebo', unixepoch(), unixepoch())
ON CONFLICT(id) DO NOTHING;

-- Personality presets queries

-- name: ListPersonalityPresets :many
SELECT id, name, description, system_prompt, icon, display_order
FROM personality_presets
ORDER BY display_order ASC;

-- name: GetPersonalityPreset :one
SELECT id, name, description, system_prompt, icon, display_order
FROM personality_presets
WHERE id = ?;

-- name: CreatePersonalityPreset :one
INSERT INTO personality_presets (id, name, description, system_prompt, icon, display_order)
VALUES (?, ?, ?, ?, ?, ?)
RETURNING *;

-- name: UpdatePersonalityPreset :exec
UPDATE personality_presets
SET name = COALESCE(sqlc.narg(name), name),
    description = COALESCE(sqlc.narg(description), description),
    system_prompt = COALESCE(sqlc.narg(system_prompt), system_prompt),
    icon = COALESCE(sqlc.narg(icon), icon),
    display_order = COALESCE(sqlc.narg(display_order), display_order)
WHERE id = sqlc.arg(id);

-- name: DeletePersonalityPreset :exec
DELETE FROM personality_presets WHERE id = ?;
