-- Marketplace artifact update tracking and preferences.
-- Stores per-artifact version state and auto-update toggles.

ALTER TABLE settings ADD COLUMN auto_update_artifacts TEXT NOT NULL DEFAULT '{"agents":true,"skills":true,"plugins":true,"check_interval_hours":6}';

CREATE TABLE IF NOT EXISTS artifact_update_prefs (
    artifact_id TEXT NOT NULL,
    artifact_type TEXT NOT NULL,
    auto_update INTEGER NOT NULL DEFAULT 1,
    local_version TEXT NOT NULL DEFAULT '',
    remote_version TEXT NOT NULL DEFAULT '',
    last_checked_at INTEGER NOT NULL DEFAULT 0,
    update_available INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (artifact_id, artifact_type)
);
