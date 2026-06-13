-- +goose Up
-- Rename settings.autonomous_mode -> auto_install_deps.
--
-- The flag was only ever read in ONE place: the dependency-install cascade,
-- where it gated whether declared deps install or sit "pending". It never
-- governed agent autonomy. Explicit installs now always cascade their deps;
-- the flag now gates only the implicit boot-time reconcile. Renaming it makes
-- the column mean what it does. Existing values (default 0 = OFF) carry over.
ALTER TABLE settings RENAME COLUMN autonomous_mode TO auto_install_deps;

-- +goose Down
ALTER TABLE settings RENAME COLUMN auto_install_deps TO autonomous_mode;
