-- +goose Up
ALTER TABLE role_workflows ADD COLUMN emit TEXT;

-- +goose Down
ALTER TABLE role_workflows DROP COLUMN emit;
