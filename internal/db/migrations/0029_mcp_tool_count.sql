-- +goose Up
ALTER TABLE mcp_integrations ADD COLUMN tool_count INTEGER DEFAULT 0;

-- +goose Down
-- SQLite doesn't support DROP COLUMN directly, but goose handles this
