-- +goose Up
-- Auth Profiles: Store API keys and provider credentials

-- Auth profiles table for API key management
CREATE TABLE IF NOT EXISTS auth_profiles (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    provider TEXT NOT NULL,           -- anthropic, openai, google, etc.
    api_key TEXT NOT NULL,            -- Encrypted or plain API key
    model TEXT,                       -- Default model for this profile
    base_url TEXT,                    -- Custom API base URL (optional)
    priority INTEGER DEFAULT 0,       -- Higher = preferred, for rotation
    is_active INTEGER DEFAULT 1,      -- 0 = disabled, 1 = active
    cooldown_until INTEGER,           -- Unix timestamp when cooldown expires
    last_used_at INTEGER,             -- Unix timestamp of last use
    usage_count INTEGER DEFAULT 0,    -- Total API calls made
    error_count INTEGER DEFAULT 0,    -- Consecutive errors (reset on success)
    metadata TEXT,                    -- JSON metadata (org_id, tier, etc.)
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

-- Index for finding active profiles by provider
CREATE INDEX IF NOT EXISTS idx_auth_profiles_provider ON auth_profiles(provider, is_active);

-- Index for priority-based selection
CREATE INDEX IF NOT EXISTS idx_auth_profiles_priority ON auth_profiles(provider, priority DESC, is_active);

-- Sessions table for conversation persistence
CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY,
    name TEXT,                        -- Human-readable session name
    scope TEXT DEFAULT 'global',      -- global, user, channel
    scope_id TEXT,                    -- User ID or channel ID if scoped
    summary TEXT,                     -- Compacted conversation summary
    token_count INTEGER DEFAULT 0,    -- Estimated tokens in context
    message_count INTEGER DEFAULT 0,  -- Total messages
    last_compacted_at INTEGER,        -- When last compacted
    metadata TEXT,                    -- JSON metadata
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

-- Unique constraint for scoped sessions (one session per scope/scope_id combo)
CREATE UNIQUE INDEX IF NOT EXISTS idx_sessions_scope ON sessions(scope, scope_id);

-- Session messages table
CREATE TABLE IF NOT EXISTS session_messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    role TEXT NOT NULL,               -- user, assistant, system, tool
    content TEXT,                     -- Message content
    tool_calls TEXT,                  -- JSON array of tool calls
    tool_results TEXT,                -- JSON array of tool results
    token_estimate INTEGER DEFAULT 0, -- Estimated tokens for this message
    is_compacted INTEGER DEFAULT 0,   -- 1 = included in summary, can be pruned
    created_at INTEGER NOT NULL
);

-- Index for session message retrieval
CREATE INDEX IF NOT EXISTS idx_session_messages_session ON session_messages(session_id, created_at);

-- Index for finding non-compacted messages
CREATE INDEX IF NOT EXISTS idx_session_messages_compacted ON session_messages(session_id, is_compacted);
