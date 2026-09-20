-- The model one conversation runs at.
--
-- Settings → General → MODEL is the employee's default for NEW conversations.
-- This column is the override the owner sets from the composer, and it belongs
-- to the chat because that is the thing being overridden: a conversation, not
-- an employee. NULL = no override, fall back to the employee's preference,
-- then to the selector's choice.
--
-- Same shape as workflow_runs.model (0152): one nullable TEXT column naming a
-- resolved "provider/model", no second table, no second pathway.
-- +goose Up
ALTER TABLE chats ADD COLUMN model TEXT;

-- +goose Down
