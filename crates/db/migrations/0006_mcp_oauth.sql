-- MCP OAuth tables for Dynamic Client Registration and OAuth flows
-- +goose Up

-- OAuth Clients registered via Dynamic Client Registration
CREATE TABLE IF NOT EXISTS mcp_oauth_clients (
    id TEXT PRIMARY KEY,
    client_id TEXT NOT NULL UNIQUE,
    client_secret_hash TEXT NOT NULL,
    name TEXT NOT NULL,
    description TEXT,
    redirect_uris TEXT NOT NULL,
    scopes TEXT,
    is_confidential INTEGER DEFAULT 1,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE INDEX idx_mcp_oauth_clients_client_id ON mcp_oauth_clients(client_id);

-- OAuth Authorization Codes
CREATE TABLE IF NOT EXISTS mcp_oauth_codes (
    id TEXT PRIMARY KEY,
    client_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    code_hash TEXT NOT NULL,
    redirect_uri TEXT NOT NULL,
    scopes TEXT NOT NULL,
    code_challenge TEXT,
    code_challenge_method TEXT,
    expires_at TEXT NOT NULL,
    used_at TEXT,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    FOREIGN KEY (client_id) REFERENCES mcp_oauth_clients(id) ON DELETE CASCADE,
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE INDEX idx_mcp_oauth_codes_code_hash ON mcp_oauth_codes(code_hash);
CREATE INDEX idx_mcp_oauth_codes_user_id ON mcp_oauth_codes(user_id);

-- OAuth Access and Refresh Tokens
CREATE TABLE IF NOT EXISTS mcp_oauth_tokens (
    id TEXT PRIMARY KEY,
    client_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    access_token_hash TEXT NOT NULL,
    refresh_token_hash TEXT,
    scopes TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    refresh_expires_at TEXT,
    revoked_at TEXT,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    FOREIGN KEY (client_id) REFERENCES mcp_oauth_clients(id) ON DELETE CASCADE,
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE INDEX idx_mcp_oauth_tokens_access_hash ON mcp_oauth_tokens(access_token_hash);
CREATE INDEX idx_mcp_oauth_tokens_refresh_hash ON mcp_oauth_tokens(refresh_token_hash);
CREATE INDEX idx_mcp_oauth_tokens_user_id ON mcp_oauth_tokens(user_id);

-- +goose Down
DROP INDEX IF EXISTS idx_mcp_oauth_tokens_user_id;
DROP INDEX IF EXISTS idx_mcp_oauth_tokens_refresh_hash;
DROP INDEX IF EXISTS idx_mcp_oauth_tokens_access_hash;
DROP TABLE IF EXISTS mcp_oauth_tokens;

DROP INDEX IF EXISTS idx_mcp_oauth_codes_user_id;
DROP INDEX IF EXISTS idx_mcp_oauth_codes_code_hash;
DROP TABLE IF EXISTS mcp_oauth_codes;

DROP INDEX IF EXISTS idx_mcp_oauth_clients_client_id;
DROP TABLE IF EXISTS mcp_oauth_clients;
