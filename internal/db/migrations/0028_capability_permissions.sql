-- +goose Up
-- Add capability permissions and terms acceptance to user profiles
ALTER TABLE user_profiles ADD COLUMN tool_permissions TEXT DEFAULT '{}';
ALTER TABLE user_profiles ADD COLUMN terms_accepted_at INTEGER;

-- +goose Down
-- SQLite doesn't support DROP COLUMN before 3.35.0, so we recreate
-- For simplicity, these columns are nullable and benign if left in place
