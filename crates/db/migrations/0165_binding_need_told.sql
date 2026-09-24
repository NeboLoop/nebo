-- What only the owner can supply before a duty can run, and what they were
-- told of it.
--
-- workflow_runs.owner_need: a run that ended blocked (a tool refused
-- terminally) keeps what the refusing tool named as missing — a plugin, or
-- an account on one — as JSON (`types::OwnerNeed`). NULL = nothing named.
--
-- agent_workflows.need_told: a binding that cannot do its duty for want of
-- something only the owner supplies says so on its record or its runs; the
-- owner hears it in the Inbox ONCE, not at every held fire or blocked run.
-- need_told is what they were told (a stable key); a need equal to it is not
-- news. It is forgotten when the need is met (the binding's degraded reason
-- clears, or a run of the binding completes), so a need that returns is
-- told again. NULL = nothing told, so a need standing before this column
-- existed is told once.
-- +goose Up
ALTER TABLE workflow_runs ADD COLUMN owner_need TEXT;
ALTER TABLE agent_workflows ADD COLUMN need_told TEXT;

-- +goose Down
ALTER TABLE agent_workflows DROP COLUMN need_told;
ALTER TABLE workflow_runs DROP COLUMN owner_need;
