-- Add multi_chat flag to entity_config (split from 0079 which may have partially applied).
ALTER TABLE entity_config ADD COLUMN multi_chat INTEGER DEFAULT 0;
