-- +goose Up
ALTER TABLE agent_workflows ADD COLUMN connections TEXT;

-- +goose Down
ALTER TABLE agent_workflows DROP COLUMN connections;
