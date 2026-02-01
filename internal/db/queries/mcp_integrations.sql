-- name: ListMCPIntegrations :many
SELECT * FROM mcp_integrations ORDER BY created_at DESC;

-- name: ListEnabledMCPIntegrations :many
SELECT * FROM mcp_integrations WHERE is_enabled = 1 ORDER BY created_at DESC;

-- name: GetMCPIntegration :one
SELECT * FROM mcp_integrations WHERE id = ?;

-- name: GetMCPIntegrationByType :one
SELECT * FROM mcp_integrations WHERE server_type = ? AND is_enabled = 1 LIMIT 1;

-- name: CreateMCPIntegration :one
INSERT INTO mcp_integrations (id, name, server_type, server_url, auth_type, is_enabled, metadata, created_at, updated_at)
VALUES (?, ?, ?, ?, ?, ?, ?, unixepoch(), unixepoch())
RETURNING *;

-- name: UpdateMCPIntegration :one
UPDATE mcp_integrations
SET name = ?, server_url = ?, is_enabled = ?, metadata = ?, updated_at = unixepoch()
WHERE id = ?
RETURNING *;

-- name: UpdateMCPIntegrationStatus :exec
UPDATE mcp_integrations
SET connection_status = ?, last_connected_at = ?, last_error = ?, updated_at = unixepoch()
WHERE id = ?;

-- name: DeleteMCPIntegration :exec
DELETE FROM mcp_integrations WHERE id = ?;

-- name: GetMCPIntegrationCredential :one
SELECT * FROM mcp_integration_credentials WHERE integration_id = ? ORDER BY created_at DESC LIMIT 1;

-- name: CreateMCPIntegrationCredential :one
INSERT INTO mcp_integration_credentials (id, integration_id, credential_type, credential_value, refresh_token, expires_at, scopes, created_at, updated_at)
VALUES (?, ?, ?, ?, ?, ?, ?, unixepoch(), unixepoch())
RETURNING *;

-- name: UpdateMCPIntegrationCredential :exec
UPDATE mcp_integration_credentials
SET credential_value = ?, refresh_token = ?, expires_at = ?, scopes = ?, updated_at = unixepoch()
WHERE id = ?;

-- name: DeleteMCPIntegrationCredentials :exec
DELETE FROM mcp_integration_credentials WHERE integration_id = ?;

-- name: ListMCPServerRegistry :many
SELECT * FROM mcp_server_registry ORDER BY display_order ASC;

-- name: GetMCPServerRegistry :one
SELECT * FROM mcp_server_registry WHERE id = ?;
