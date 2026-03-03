-- +goose Up
-- Add role column to users table for admin/user distinction

ALTER TABLE users ADD COLUMN role TEXT NOT NULL DEFAULT 'user';

CREATE INDEX IF NOT EXISTS idx_users_role ON users(role);

-- +goose Down
DROP INDEX IF EXISTS idx_users_role;
-- SQLite doesn't support DROP COLUMN directly, but the column will be ignored
-- For a true rollback, would need to recreate the table
