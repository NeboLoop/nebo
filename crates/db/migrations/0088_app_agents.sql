-- Add app-specific columns to agents table.
-- Apps are agents with artifact_type='app' that bundle a UI + optional sidecar binary.
ALTER TABLE agents ADD COLUMN is_app INTEGER NOT NULL DEFAULT 0;
ALTER TABLE agents ADD COLUMN app_ui_path TEXT;
ALTER TABLE agents ADD COLUMN app_binary_path TEXT;
ALTER TABLE agents ADD COLUMN app_window_config TEXT;
