-- +goose Up
-- Full Access: the master "execute everything without asking" flag.
--
-- Distinct from `auto_install_deps` (which 0104 renamed away from autonomy and
-- which now gates only the boot-time dependency reconcile) and from the
-- per-capability toggles (user_profiles.tool_permissions). When ON, the runner's
-- per-tool approval gate is bypassed entirely. Default OFF (safe): an OFF
-- capability prompts via the Approval Modal instead.
ALTER TABLE settings ADD COLUMN full_access INTEGER NOT NULL DEFAULT 0;

-- +goose Down
ALTER TABLE settings DROP COLUMN full_access;
