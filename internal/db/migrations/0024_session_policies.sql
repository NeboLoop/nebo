-- +goose Up
-- Add session policy columns for session policy support
-- These enable per-session configuration overrides

-- Send policy: "allow" or "deny" for message delivery
ALTER TABLE sessions ADD COLUMN send_policy TEXT DEFAULT 'allow';

-- Model override: specify a different model for this session
ALTER TABLE sessions ADD COLUMN model_override TEXT;

-- Provider override: specify a different provider for this session
ALTER TABLE sessions ADD COLUMN provider_override TEXT;

-- Auth profile override: use a specific auth profile
ALTER TABLE sessions ADD COLUMN auth_profile_override TEXT;

-- Auth profile override source: "auto" (system set) or "user" (manually set)
ALTER TABLE sessions ADD COLUMN auth_profile_override_source TEXT;

-- Verbose level: "on" or "off" for detailed logging
ALTER TABLE sessions ADD COLUMN verbose_level TEXT;

-- Custom label for the session (max 64 chars recommended)
ALTER TABLE sessions ADD COLUMN custom_label TEXT;

-- +goose Down
-- SQLite doesn't support DROP COLUMN directly, so we recreate the table
-- In practice, these columns are nullable and won't break anything if left
