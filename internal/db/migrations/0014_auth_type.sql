-- +goose Up
-- Add auth_type column for OAuth > Token > API Key priority ordering

ALTER TABLE auth_profiles ADD COLUMN auth_type TEXT DEFAULT 'api_key';
-- auth_type values: 'oauth', 'token', 'api_key'
-- Priority: oauth (highest) > token > api_key (lowest)

-- Create index for auth_type based selection
CREATE INDEX IF NOT EXISTS idx_auth_profiles_auth_type ON auth_profiles(provider, auth_type, priority DESC, is_active);

-- +goose Down
DROP INDEX IF EXISTS idx_auth_profiles_auth_type;
-- SQLite doesn't support DROP COLUMN directly, so we leave the column
