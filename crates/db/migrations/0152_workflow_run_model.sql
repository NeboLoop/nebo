-- +goose Up
-- Which model actually ran a workflow run, recorded by the runner the
-- moment routing resolves it — so the governance record of a turn names
-- the workflow, the policy, AND the model, and divergent behavior between
-- two turns can be explained.
ALTER TABLE workflow_runs ADD COLUMN model TEXT;

-- +goose Down
