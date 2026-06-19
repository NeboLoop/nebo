-- Plugin/artifact upgrade system: default to notify-and-approve + record history.
--
-- 1. Existing artifact_update_prefs rows defaulted auto_update = 1, which would
--    silently auto-apply every detected update. The chosen model is notify +
--    user approval, with auto-update as an explicit per-artifact opt-in, so flip
--    existing rows to manual. New rows are inserted with auto_update = 0.
UPDATE artifact_update_prefs SET auto_update = 0;

-- 2. An append-only log of applied/failed upgrades so the user can see what was
--    updated, when, and from→to which version (Settings → Updates → History).
CREATE TABLE IF NOT EXISTS artifact_update_history (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    artifact_id   TEXT NOT NULL,
    artifact_type TEXT NOT NULL,           -- agent | skill | plugin | app
    name          TEXT NOT NULL DEFAULT '',
    from_version  TEXT NOT NULL DEFAULT '',
    to_version    TEXT NOT NULL DEFAULT '',
    status        TEXT NOT NULL,           -- applied | failed
    detail        TEXT NOT NULL DEFAULT '', -- error message when failed
    applied_at    INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE INDEX IF NOT EXISTS idx_artifact_update_history_applied
    ON artifact_update_history(applied_at DESC);
