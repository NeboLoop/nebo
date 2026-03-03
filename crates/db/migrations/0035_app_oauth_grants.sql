-- +goose Up
CREATE TABLE IF NOT EXISTS app_oauth_grants (
    id TEXT PRIMARY KEY,
    app_id TEXT NOT NULL,
    provider TEXT NOT NULL,
    scopes TEXT NOT NULL DEFAULT '',
    access_token TEXT NOT NULL DEFAULT '',
    refresh_token TEXT NOT NULL DEFAULT '',
    token_type TEXT NOT NULL DEFAULT 'Bearer',
    expires_at DATETIME,
    oauth_state TEXT,
    pkce_verifier TEXT,
    connection_status TEXT NOT NULL DEFAULT 'disconnected',
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(app_id, provider)
);

-- +goose Down
DROP TABLE IF EXISTS app_oauth_grants;
