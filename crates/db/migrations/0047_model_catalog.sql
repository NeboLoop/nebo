-- +goose Up
-- Recreate provider_models as a global catalog table (no FK to auth_profiles).
-- The table now stores the model catalog seeded from models.yaml on startup.
-- profile_id becomes "provider" — stores the provider name (e.g., "anthropic").

CREATE TABLE IF NOT EXISTS provider_models_new (
    id TEXT PRIMARY KEY,
    provider TEXT NOT NULL,            -- e.g., "anthropic", "openai", "janus"
    model_id TEXT NOT NULL,            -- e.g., "claude-sonnet-4-20250514"
    display_name TEXT NOT NULL,        -- e.g., "Claude Sonnet 4"
    is_active INTEGER DEFAULT 1,       -- 1 = available for requests (user toggle)
    is_default INTEGER DEFAULT 0,      -- 1 = default model for this provider
    context_window INTEGER,            -- Max tokens
    input_price REAL,                  -- Price per 1M input tokens
    output_price REAL,                 -- Price per 1M output tokens
    capabilities TEXT,                 -- JSON: ["vision", "tools", "streaming"]
    kind TEXT,                         -- JSON: ["fast", "smart", "code"]
    preferred INTEGER DEFAULT 0,       -- 1 = user's preferred model for this kind
    seeded_version TEXT,               -- App version that last seeded this row
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE(provider, model_id)
);

INSERT INTO provider_models_new (id, provider, model_id, display_name, is_active, is_default, context_window, input_price, output_price, capabilities, created_at, updated_at)
SELECT id, profile_id, model_id, display_name, is_active, is_default, context_window, input_price, output_price, capabilities, created_at, updated_at
FROM provider_models;

DROP TABLE IF EXISTS provider_models;
ALTER TABLE provider_models_new RENAME TO provider_models;

CREATE INDEX IF NOT EXISTS idx_provider_models_provider ON provider_models(provider, is_active);
CREATE INDEX IF NOT EXISTS idx_provider_models_default ON provider_models(provider, is_default);

-- +goose Down
-- Revert to FK-constrained version (data loss if no matching auth_profiles)
DROP TABLE IF EXISTS provider_models;

CREATE TABLE IF NOT EXISTS provider_models (
    id TEXT PRIMARY KEY,
    profile_id TEXT NOT NULL REFERENCES auth_profiles(id) ON DELETE CASCADE,
    model_id TEXT NOT NULL,
    display_name TEXT NOT NULL,
    is_active INTEGER DEFAULT 0,
    is_default INTEGER DEFAULT 0,
    context_window INTEGER,
    input_price REAL,
    output_price REAL,
    capabilities TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE(profile_id, model_id)
);
