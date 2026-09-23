-- The last standing outcome of a workflow binding.
--
-- A run that ends because the step evaluator or the employee itself said
-- there is nothing to do (a workflow run with status `exited`) ended cleanly
-- with a reason: that reason is the binding's standing outcome. It is not a
-- failure and not a retirement: the binding stays armed and its next fire
-- reads the outcome (heartbeat triage), so a world that has not changed is
-- not rediscovered at full cost.
--
-- last_outcome: the reason's first line, capped. NULL = no standing outcome
-- recorded yet. last_outcome_at: unix seconds it was recorded.
-- +goose Up
ALTER TABLE agent_workflows ADD COLUMN last_outcome TEXT;
ALTER TABLE agent_workflows ADD COLUMN last_outcome_at INTEGER;

-- +goose Down
ALTER TABLE agent_workflows DROP COLUMN last_outcome_at;
ALTER TABLE agent_workflows DROP COLUMN last_outcome;
