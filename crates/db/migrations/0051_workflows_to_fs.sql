-- Move workflow definitions from DB to filesystem.
-- Keep lightweight index rows; definition content lives in .napp or user/ directory.

ALTER TABLE workflows ADD COLUMN napp_path TEXT;

-- Roles: add napp_path, keep definition columns for now (migration exports to fs)
ALTER TABLE roles ADD COLUMN napp_path TEXT;
