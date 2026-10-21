-- A helper's questions for the owner, kept on its run so they outlive the
-- run that asked them and a restart: which are open, which were answered
-- (and how), and which its parent was already given. JSON; NULL for every
-- run that never asked.
-- +goose Up
ALTER TABLE engine_runs ADD COLUMN pending_questions TEXT;

-- +goose Down
ALTER TABLE engine_runs DROP COLUMN pending_questions;
