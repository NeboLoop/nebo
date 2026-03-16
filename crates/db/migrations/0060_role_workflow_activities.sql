-- +goose Up
ALTER TABLE role_workflows ADD COLUMN activities TEXT;

-- +goose Down
ALTER TABLE role_workflows DROP COLUMN activities;
