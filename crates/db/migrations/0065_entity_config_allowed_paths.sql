-- Allowed filesystem paths for per-entity file/shell scope restriction.
ALTER TABLE entity_config ADD COLUMN allowed_paths TEXT;