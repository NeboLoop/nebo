-- The permission mode the owner picked for one conversation: its turns run
-- in it instead of the employee's own mode. NULL = the employee's own (the
-- employee's mode, else the company default). Kept on the bot, so the
-- desktop, the web console and the phone show the same one.
--
-- A workflow run started from a conversation in a mode stricter than its
-- employee's own is held to that mode: workflow_runs.permission_mode. NULL
-- = nothing carried (every scheduled, watched or triggered run): the
-- employee's own mode.
-- +goose Up
ALTER TABLE chats ADD COLUMN permission_mode TEXT;
ALTER TABLE workflow_runs ADD COLUMN permission_mode TEXT;

-- +goose Down
ALTER TABLE workflow_runs DROP COLUMN permission_mode;
ALTER TABLE chats DROP COLUMN permission_mode;
