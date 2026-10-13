-- The conversation whose turn started a workflow run (the work tool's run,
-- from a chat): that chat lists the run among its running tasks. NULL =
-- no conversation started it (every scheduled, watched or triggered run),
-- so no chat lists it.
-- +goose Up
ALTER TABLE workflow_runs ADD COLUMN launched_from TEXT;

-- +goose Down
ALTER TABLE workflow_runs DROP COLUMN launched_from;
