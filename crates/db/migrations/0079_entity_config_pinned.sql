-- Add pinned flag to entity_config for sidebar pin state.
ALTER TABLE entity_config ADD COLUMN pinned INTEGER DEFAULT 0;

-- Pin the main companion by default.
INSERT OR IGNORE INTO entity_config (entity_type, entity_id, pinned)
VALUES ('main', 'main', 1);
UPDATE entity_config SET pinned = 1 WHERE entity_type = 'main' AND entity_id = 'main';
