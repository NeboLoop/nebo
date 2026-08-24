-- Owner-wins for display names: once the owner renames an agent (including the
-- christening rename of the primary), the name is theirs. Boot/watcher manifest
-- syncs must never overwrite a locked name.
ALTER TABLE agents ADD COLUMN name_locked INTEGER NOT NULL DEFAULT 0;
