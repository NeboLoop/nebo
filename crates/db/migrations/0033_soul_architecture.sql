-- +goose Up
-- Soul Architecture: rich agent identity fields
ALTER TABLE agent_profile ADD COLUMN emoji TEXT DEFAULT '';
ALTER TABLE agent_profile ADD COLUMN creature TEXT DEFAULT '';
ALTER TABLE agent_profile ADD COLUMN vibe TEXT DEFAULT '';
ALTER TABLE agent_profile ADD COLUMN avatar TEXT DEFAULT '';
ALTER TABLE agent_profile ADD COLUMN agent_rules TEXT DEFAULT '';
ALTER TABLE agent_profile ADD COLUMN tool_notes TEXT DEFAULT '';

-- +goose Down
-- Additive columns — no destructive down migration
