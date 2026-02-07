-- User profile queries

-- name: GetUserProfile :one
SELECT *
FROM user_profiles
WHERE user_id = ?;

-- name: UpsertUserProfile :one
INSERT INTO user_profiles (user_id, display_name, bio, location, timezone, occupation,
                           interests, communication_style, goals, context,
                           onboarding_completed, onboarding_step,
                           tool_permissions, terms_accepted_at, created_at, updated_at)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, unixepoch(), unixepoch())
ON CONFLICT(user_id) DO UPDATE SET
    display_name = COALESCE(excluded.display_name, user_profiles.display_name),
    bio = COALESCE(excluded.bio, user_profiles.bio),
    location = COALESCE(excluded.location, user_profiles.location),
    timezone = COALESCE(excluded.timezone, user_profiles.timezone),
    occupation = COALESCE(excluded.occupation, user_profiles.occupation),
    interests = COALESCE(excluded.interests, user_profiles.interests),
    communication_style = COALESCE(excluded.communication_style, user_profiles.communication_style),
    goals = COALESCE(excluded.goals, user_profiles.goals),
    context = COALESCE(excluded.context, user_profiles.context),
    onboarding_completed = COALESCE(excluded.onboarding_completed, user_profiles.onboarding_completed),
    onboarding_step = COALESCE(excluded.onboarding_step, user_profiles.onboarding_step),
    tool_permissions = COALESCE(excluded.tool_permissions, user_profiles.tool_permissions),
    terms_accepted_at = COALESCE(excluded.terms_accepted_at, user_profiles.terms_accepted_at),
    updated_at = unixepoch()
RETURNING *;

-- name: UpdateUserProfile :exec
UPDATE user_profiles
SET display_name = COALESCE(sqlc.narg(display_name), display_name),
    bio = COALESCE(sqlc.narg(bio), bio),
    location = COALESCE(sqlc.narg(location), location),
    timezone = COALESCE(sqlc.narg(timezone), timezone),
    occupation = COALESCE(sqlc.narg(occupation), occupation),
    interests = COALESCE(sqlc.narg(interests), interests),
    communication_style = COALESCE(sqlc.narg(communication_style), communication_style),
    goals = COALESCE(sqlc.narg(goals), goals),
    context = COALESCE(sqlc.narg(context), context),
    updated_at = unixepoch()
WHERE user_id = sqlc.arg(user_id);

-- name: SetOnboardingCompleted :exec
UPDATE user_profiles
SET onboarding_completed = 1,
    onboarding_step = NULL,
    updated_at = unixepoch()
WHERE user_id = ?;

-- name: SetOnboardingStep :exec
UPDATE user_profiles
SET onboarding_step = ?,
    updated_at = unixepoch()
WHERE user_id = ?;

-- name: GetToolPermissions :one
SELECT COALESCE(tool_permissions, '{}') AS tool_permissions
FROM user_profiles
WHERE user_id = ?;

-- name: UpdateToolPermissions :exec
UPDATE user_profiles
SET tool_permissions = ?,
    updated_at = unixepoch()
WHERE user_id = ?;

-- name: AcceptTerms :exec
UPDATE user_profiles
SET terms_accepted_at = unixepoch(),
    updated_at = unixepoch()
WHERE user_id = ?;

-- name: GetTermsAcceptedAt :one
SELECT terms_accepted_at
FROM user_profiles
WHERE user_id = ?;

-- name: DeleteUserProfile :exec
DELETE FROM user_profiles WHERE user_id = ?;
