-- name: UpsertAppOAuthGrant :exec
INSERT INTO app_oauth_grants (id, app_id, provider, scopes, oauth_state, pkce_verifier, connection_status)
VALUES (?, ?, ?, ?, ?, ?, ?)
ON CONFLICT(app_id, provider) DO UPDATE SET
    scopes = excluded.scopes,
    oauth_state = excluded.oauth_state,
    pkce_verifier = excluded.pkce_verifier,
    connection_status = excluded.connection_status,
    updated_at = CURRENT_TIMESTAMP;

-- name: GetAppOAuthGrant :one
SELECT * FROM app_oauth_grants WHERE app_id = ? AND provider = ?;

-- name: GetAppOAuthGrantByState :one
SELECT * FROM app_oauth_grants WHERE oauth_state = ?;

-- name: ListAppOAuthGrants :many
SELECT * FROM app_oauth_grants WHERE app_id = ? ORDER BY provider;

-- name: UpdateAppOAuthTokens :exec
UPDATE app_oauth_grants SET
    access_token = ?,
    refresh_token = ?,
    token_type = ?,
    expires_at = ?,
    oauth_state = NULL,
    pkce_verifier = NULL,
    connection_status = 'connected',
    updated_at = CURRENT_TIMESTAMP
WHERE app_id = ? AND provider = ?;

-- name: DeleteAppOAuthGrant :exec
DELETE FROM app_oauth_grants WHERE app_id = ? AND provider = ?;

-- name: ListExpiringOAuthGrants :many
SELECT * FROM app_oauth_grants
WHERE connection_status = 'connected'
    AND refresh_token != ''
    AND expires_at IS NOT NULL
    AND expires_at <= datetime('now', '+' || ? || ' minutes')
ORDER BY expires_at;
