-- "Double-check before finishing" (Settings → Permissions): before an
-- employee ends a chat reply, the harness makes sure a save the owner asked
-- for was saved and a command he gave was run, each with at most one more
-- step. On by default; the owner turns it off.
-- +goose Up
ALTER TABLE settings ADD COLUMN finish_checks INTEGER NOT NULL DEFAULT 1;

-- +goose Down
ALTER TABLE settings DROP COLUMN finish_checks;
