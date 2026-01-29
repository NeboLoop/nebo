-- +goose Up
-- Provider Models: Track available models per provider API key

CREATE TABLE IF NOT EXISTS provider_models (
    id TEXT PRIMARY KEY,
    profile_id TEXT NOT NULL REFERENCES auth_profiles(id) ON DELETE CASCADE,
    model_id TEXT NOT NULL,           -- e.g., "claude-sonnet-4-20250514"
    display_name TEXT NOT NULL,       -- e.g., "Claude Sonnet 4"
    is_active INTEGER DEFAULT 0,      -- 1 = can be used for requests
    is_default INTEGER DEFAULT 0,     -- 1 = default model for this provider
    context_window INTEGER,           -- Max tokens
    input_price REAL,                 -- Price per 1M input tokens
    output_price REAL,                -- Price per 1M output tokens
    capabilities TEXT,                -- JSON: ["vision", "tools", "streaming"]
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE(profile_id, model_id)
);

-- Index for finding active models by provider
CREATE INDEX IF NOT EXISTS idx_provider_models_profile ON provider_models(profile_id, is_active);

-- Index for finding default model
CREATE INDEX IF NOT EXISTS idx_provider_models_default ON provider_models(profile_id, is_default);

-- +goose Down
DROP TABLE IF EXISTS provider_models;
