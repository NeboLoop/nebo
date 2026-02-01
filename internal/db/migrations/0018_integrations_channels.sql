-- +goose Up
-- MCP Integrations: External MCP servers that Nebo connects to
-- Examples: Notion, GitHub, Linear, Slack, custom MCP servers

CREATE TABLE IF NOT EXISTS mcp_integrations (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,                   -- User-friendly name (e.g., "My Notion")
    server_type TEXT NOT NULL,            -- notion, github, linear, slack, custom
    server_url TEXT,                      -- For custom MCP servers
    auth_type TEXT NOT NULL DEFAULT 'api_key', -- api_key, oauth, none
    is_enabled INTEGER DEFAULT 1,
    connection_status TEXT DEFAULT 'disconnected', -- connected, disconnected, error
    last_connected_at INTEGER,
    last_error TEXT,
    metadata TEXT,                        -- JSON for server-specific config
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE INDEX idx_mcp_integrations_type ON mcp_integrations(server_type);
CREATE INDEX idx_mcp_integrations_enabled ON mcp_integrations(is_enabled);

-- MCP Integration Credentials: API keys or OAuth tokens for integrations
CREATE TABLE IF NOT EXISTS mcp_integration_credentials (
    id TEXT PRIMARY KEY,
    integration_id TEXT NOT NULL REFERENCES mcp_integrations(id) ON DELETE CASCADE,
    credential_type TEXT NOT NULL,        -- api_key, oauth_token
    credential_value TEXT NOT NULL,       -- The actual key/token (encrypted in production)
    refresh_token TEXT,                   -- For OAuth
    expires_at INTEGER,                   -- Token expiration
    scopes TEXT,                          -- Comma-separated scopes
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE INDEX idx_mcp_creds_integration ON mcp_integration_credentials(integration_id);

-- MCP Server Registry: Known MCP servers and their OAuth configurations
-- Pre-populated with popular services for easy setup
CREATE TABLE IF NOT EXISTS mcp_server_registry (
    id TEXT PRIMARY KEY,                  -- notion, github, linear, etc.
    name TEXT NOT NULL,                   -- Display name
    description TEXT,
    icon TEXT,                            -- Emoji or URL
    auth_type TEXT NOT NULL,              -- api_key, oauth, none
    oauth_authorize_url TEXT,             -- For OAuth flow
    oauth_token_url TEXT,
    oauth_scopes TEXT,                    -- Default scopes to request
    api_key_url TEXT,                     -- Where to get an API key
    api_key_placeholder TEXT,             -- e.g., "ntn_..."
    default_server_url TEXT,              -- Default MCP server URL if hosted
    is_builtin INTEGER DEFAULT 0,         -- 1 = Nebo provides MCP server
    display_order INTEGER DEFAULT 0,
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);

-- Pre-populate with known MCP servers
INSERT INTO mcp_server_registry (id, name, description, icon, auth_type, api_key_url, api_key_placeholder, is_builtin, display_order) VALUES
('notion', 'Notion', 'Connect to your Notion workspace', '📝', 'api_key', 'https://www.notion.so/my-integrations', 'ntn_...', 0, 1),
('github', 'GitHub', 'Access GitHub repositories and issues', '🐙', 'oauth', 'https://github.com/settings/tokens', 'ghp_...', 0, 2),
('linear', 'Linear', 'Manage Linear issues and projects', '📊', 'api_key', 'https://linear.app/settings/api', 'lin_...', 0, 3),
('slack', 'Slack', 'Connect to Slack workspaces', '💬', 'oauth', NULL, NULL, 0, 4),
('filesystem', 'File System', 'Access local files', '📁', 'none', NULL, NULL, 1, 5),
('memory', 'Memory', 'Persistent memory storage', '🧠', 'none', NULL, NULL, 1, 6);

-- Channels: Communication channels (Telegram, Discord, Slack bots)
CREATE TABLE IF NOT EXISTS channels (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,                   -- User-friendly name (e.g., "My Telegram")
    channel_type TEXT NOT NULL,           -- telegram, discord, slack
    is_enabled INTEGER DEFAULT 1,
    connection_status TEXT DEFAULT 'disconnected', -- connected, connecting, disconnected, error
    last_connected_at INTEGER,
    last_error TEXT,
    message_count INTEGER DEFAULT 0,      -- Total messages processed
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE INDEX idx_channels_type ON channels(channel_type);
CREATE INDEX idx_channels_enabled ON channels(is_enabled);

-- Channel Credentials: Bot tokens and API keys for channels
CREATE TABLE IF NOT EXISTS channel_credentials (
    id TEXT PRIMARY KEY,
    channel_id TEXT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    credential_key TEXT NOT NULL,         -- bot_token, api_key, webhook_secret, etc.
    credential_value TEXT NOT NULL,       -- The actual value (encrypted in production)
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE(channel_id, credential_key)
);

CREATE INDEX idx_channel_creds_channel ON channel_credentials(channel_id);

-- Channel Config: Additional settings per channel
CREATE TABLE IF NOT EXISTS channel_config (
    id TEXT PRIMARY KEY,
    channel_id TEXT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
    config_key TEXT NOT NULL,
    config_value TEXT NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE(channel_id, config_key)
);

CREATE INDEX idx_channel_config_channel ON channel_config(channel_id);

-- Channel Registry: Known channel types and their configurations
CREATE TABLE IF NOT EXISTS channel_registry (
    id TEXT PRIMARY KEY,                  -- telegram, discord, slack
    name TEXT NOT NULL,
    description TEXT,
    icon TEXT,
    setup_instructions TEXT,              -- Markdown instructions for setup
    required_credentials TEXT,            -- JSON array of required credential keys
    optional_credentials TEXT,            -- JSON array of optional credential keys
    display_order INTEGER DEFAULT 0,
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);

-- Pre-populate with known channels
INSERT INTO channel_registry (id, name, description, icon, setup_instructions, required_credentials, optional_credentials, display_order) VALUES
('telegram', 'Telegram', 'Connect via Telegram bot', '📱',
 '1. Talk to @BotFather on Telegram\n2. Create a new bot with /newbot\n3. Copy the bot token',
 '["bot_token"]', '["allowed_users"]', 1),
('discord', 'Discord', 'Connect via Discord bot', '🎮',
 '1. Go to Discord Developer Portal\n2. Create a new application\n3. Add a bot and copy the token',
 '["bot_token"]', '["guild_id", "channel_id"]', 2),
('slack', 'Slack', 'Connect via Slack app', '💼',
 '1. Create a Slack App at api.slack.com\n2. Enable Socket Mode\n3. Copy the App Token and Bot Token',
 '["app_token", "bot_token"]', '["channel_id"]', 3);

-- +goose Down
DROP TABLE IF EXISTS channel_registry;
DROP INDEX IF EXISTS idx_channel_config_channel;
DROP TABLE IF EXISTS channel_config;
DROP INDEX IF EXISTS idx_channel_creds_channel;
DROP TABLE IF EXISTS channel_credentials;
DROP INDEX IF EXISTS idx_channels_enabled;
DROP INDEX IF EXISTS idx_channels_type;
DROP TABLE IF EXISTS channels;
DROP TABLE IF EXISTS mcp_server_registry;
DROP INDEX IF EXISTS idx_mcp_creds_integration;
DROP TABLE IF EXISTS mcp_integration_credentials;
DROP INDEX IF EXISTS idx_mcp_integrations_enabled;
DROP INDEX IF EXISTS idx_mcp_integrations_type;
DROP TABLE IF EXISTS mcp_integrations;
