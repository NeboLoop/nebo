-- Extend plugin_registry table with .napp-specific fields for installed plugin tracking.
-- These columns complement the existing schema from 0027_plugin_settings.sql.

ALTER TABLE plugin_registry ADD COLUMN slug TEXT NOT NULL DEFAULT '';
ALTER TABLE plugin_registry ADD COLUMN author TEXT NOT NULL DEFAULT '';
ALTER TABLE plugin_registry ADD COLUMN source TEXT NOT NULL DEFAULT 'installed';
ALTER TABLE plugin_registry ADD COLUMN binary_path TEXT NOT NULL DEFAULT '';
ALTER TABLE plugin_registry ADD COLUMN manifest_hash TEXT NOT NULL DEFAULT '';
ALTER TABLE plugin_registry ADD COLUMN signature_status TEXT NOT NULL DEFAULT 'unverified';

CREATE UNIQUE INDEX IF NOT EXISTS idx_plugin_registry_slug ON plugin_registry(slug) WHERE slug != '';
