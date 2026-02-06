-- +goose Up
-- Add OAuth client state columns to mcp_integrations for tracking OAuth flow

ALTER TABLE mcp_integrations ADD COLUMN oauth_state TEXT;
ALTER TABLE mcp_integrations ADD COLUMN oauth_pkce_verifier TEXT;
ALTER TABLE mcp_integrations ADD COLUMN oauth_client_id TEXT;
ALTER TABLE mcp_integrations ADD COLUMN oauth_client_secret TEXT;
ALTER TABLE mcp_integrations ADD COLUMN oauth_authorization_endpoint TEXT;
ALTER TABLE mcp_integrations ADD COLUMN oauth_token_endpoint TEXT;
ALTER TABLE mcp_integrations ADD COLUMN oauth_registration_endpoint TEXT;

-- Index for state lookup during callback
CREATE INDEX idx_mcp_integrations_oauth_state ON mcp_integrations(oauth_state);

-- +goose Down
DROP INDEX IF EXISTS idx_mcp_integrations_oauth_state;

-- SQLite doesn't support DROP COLUMN directly, so we need to recreate the table
-- For down migration, we'll create a new table without the oauth columns
CREATE TABLE mcp_integrations_backup AS SELECT
    id, name, server_type, server_url, auth_type, is_enabled,
    connection_status, last_connected_at, last_error, metadata,
    created_at, updated_at
FROM mcp_integrations;

DROP TABLE mcp_integrations;

CREATE TABLE mcp_integrations (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    server_type TEXT NOT NULL,
    server_url TEXT,
    auth_type TEXT NOT NULL DEFAULT 'api_key',
    is_enabled INTEGER DEFAULT 1,
    connection_status TEXT DEFAULT 'disconnected',
    last_connected_at INTEGER,
    last_error TEXT,
    metadata TEXT,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

INSERT INTO mcp_integrations SELECT * FROM mcp_integrations_backup;
DROP TABLE mcp_integrations_backup;

CREATE INDEX idx_mcp_integrations_type ON mcp_integrations(server_type);
CREATE INDEX idx_mcp_integrations_enabled ON mcp_integrations(is_enabled);
