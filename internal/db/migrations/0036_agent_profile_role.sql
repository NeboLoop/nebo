-- +goose Up
ALTER TABLE agent_profile ADD COLUMN role TEXT;

-- +goose Down
ALTER TABLE agent_profile DROP COLUMN role;
