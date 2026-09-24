-- The need the owner was last told a workflow binding stands on.
--
-- A binding that cannot do its duty for want of something only the owner
-- supplies (a plugin, an account) says so on its record; the owner hears it
-- in the Inbox ONCE, not at every held fire or blocked run. need_told is
-- what they were told: a need equal to it is not news. It is forgotten when
-- the need is met (the binding's degraded reason clears, or a run of the
-- binding completes), so a need that returns is told again.
--
-- need_told: the need's text, or NULL = nothing told. Every binding starts
-- NULL, so a need standing before this column existed is told once.
-- +goose Up
ALTER TABLE agent_workflows ADD COLUMN need_told TEXT;

-- +goose Down
ALTER TABLE agent_workflows DROP COLUMN need_told;
