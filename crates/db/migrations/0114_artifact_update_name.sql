-- Persist the artifact's display name alongside its update-tracking row so the
-- Updates panel can show "Chief of Staff" instead of a raw UUID. The name is
-- already known at check time (agent.name / plugin.name / skill detail name);
-- this column lets us store it once and read it back without per-type lookups.
ALTER TABLE artifact_update_prefs ADD COLUMN name TEXT;
