-- +goose Up
-- Per-command allowlist for "Approve Always" on shell commands.
--
-- When a user picks "Approve Always" on a shell-command approval, the command's
-- prefix pattern (e.g. `mv`, `git push`) is stored here. The runner's approval
-- gate auto-approves future commands matching a stored pattern (exact /
-- first-word / two-word, via tools::policy) WITHOUT asking — even when the Shell
-- capability is off. Hard safeguards (rm -rf, sudo, …) still block regardless.
-- JSON array of patterns. Viewable/revocable in Settings → Permissions.
ALTER TABLE user_profiles ADD COLUMN approved_commands TEXT DEFAULT '[]';

-- +goose Down
ALTER TABLE user_profiles DROP COLUMN approved_commands;
