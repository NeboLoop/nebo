-- Auth profiles queries

-- name: CreateAuthProfile :one
INSERT INTO auth_profiles (id, name, provider, api_key, model, base_url, priority, is_active, auth_type, metadata, created_at, updated_at)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, unixepoch(), unixepoch())
RETURNING *;

-- name: GetAuthProfile :one
SELECT * FROM auth_profiles WHERE id = ?;

-- name: GetAuthProfileByName :one
SELECT * FROM auth_profiles WHERE name = ?;

-- name: ListAuthProfiles :many
SELECT * FROM auth_profiles ORDER BY provider, priority DESC;

-- name: ListActiveAuthProfilesByProvider :many
-- Returns active profiles NOT on cooldown - for request-level profile selection.
SELECT * FROM auth_profiles
WHERE provider = ? AND is_active = 1 AND (cooldown_until IS NULL OR cooldown_until < unixepoch())
ORDER BY
    CASE COALESCE(auth_type, 'api_key')
        WHEN 'oauth' THEN 0
        WHEN 'token' THEN 1
        WHEN 'api_key' THEN 2
        ELSE 3
    END ASC,
    priority DESC,
    COALESCE(last_used_at, 0) ASC,
    error_count ASC;

-- name: ListAllActiveAuthProfilesByProvider :many
-- Returns ALL active profiles regardless of cooldown - for provider loading.
-- Cooldown affects request routing, not provider existence.
SELECT * FROM auth_profiles
WHERE provider = ? AND is_active = 1
ORDER BY
    CASE COALESCE(auth_type, 'api_key')
        WHEN 'oauth' THEN 0
        WHEN 'token' THEN 1
        WHEN 'api_key' THEN 2
        ELSE 3
    END ASC,
    priority DESC;

-- name: GetBestAuthProfile :one
-- Get the best available profile for a provider
-- Priority: auth_type (OAuth > Token > API Key), then priority, then round-robin by last_used_at
SELECT * FROM auth_profiles
WHERE provider = ? AND is_active = 1 AND (cooldown_until IS NULL OR cooldown_until < unixepoch())
ORDER BY
    CASE COALESCE(auth_type, 'api_key')
        WHEN 'oauth' THEN 0
        WHEN 'token' THEN 1
        WHEN 'api_key' THEN 2
        ELSE 3
    END ASC,
    priority DESC,
    COALESCE(last_used_at, 0) ASC,
    error_count ASC
LIMIT 1;

-- name: UpdateAuthProfileUsage :exec
UPDATE auth_profiles
SET last_used_at = unixepoch(), usage_count = usage_count + 1, error_count = 0, updated_at = unixepoch()
WHERE id = ?;

-- name: UpdateAuthProfileError :exec
UPDATE auth_profiles
SET error_count = error_count + 1, updated_at = unixepoch()
WHERE id = ?;

-- name: SetAuthProfileCooldown :exec
UPDATE auth_profiles
SET cooldown_until = ?, updated_at = unixepoch()
WHERE id = ?;

-- name: ToggleAuthProfile :exec
UPDATE auth_profiles
SET is_active = ?, updated_at = unixepoch()
WHERE id = ?;

-- name: DeleteAuthProfile :exec
DELETE FROM auth_profiles WHERE id = ?;

-- name: UpdateAuthProfile :exec
UPDATE auth_profiles
SET name = ?, api_key = ?, model = ?, base_url = ?, priority = ?, auth_type = ?, metadata = ?, updated_at = unixepoch()
WHERE id = ?;

-- name: GetAuthProfileErrorCount :one
SELECT error_count FROM auth_profiles WHERE id = ?;

-- name: ResetAuthProfileErrorCountIfStale :exec
-- Reset error count if cooldown has expired and last update was > 24h ago
UPDATE auth_profiles
SET error_count = 0, updated_at = unixepoch()
WHERE id = ?
AND error_count > 0
AND (cooldown_until IS NULL OR cooldown_until < unixepoch())
AND updated_at < ?;
