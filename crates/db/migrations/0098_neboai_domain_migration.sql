-- Rewrite every stored `neboloop` token in user data to `neboai`. Covers:
--   * auth_profiles.provider — lookup key the gateway code reads
--   * auth_profiles.base_url / metadata — saved URLs that the http client uses
--   * plugin_registry.name — internal plugin slug
--   * plugin_settings.setting_value — any saved JSON or strings holding URLs
-- Idempotent — running again on already-migrated rows is a no-op.

UPDATE auth_profiles SET provider = 'neboai' WHERE provider = 'neboloop';

UPDATE auth_profiles
SET base_url = REPLACE(base_url, 'neboloop.com', 'neboai.com')
WHERE base_url LIKE '%neboloop.com%';

UPDATE auth_profiles
SET metadata = REPLACE(metadata, 'neboloop.com', 'neboai.com')
WHERE metadata LIKE '%neboloop.com%';

UPDATE auth_profiles
SET metadata = REPLACE(metadata, '"neboloop"', '"neboai"')
WHERE metadata LIKE '%"neboloop"%';

UPDATE plugin_registry SET name = 'neboai' WHERE name = 'neboloop';

UPDATE plugin_settings
SET setting_value = REPLACE(setting_value, 'neboloop.com', 'neboai.com')
WHERE setting_value LIKE '%neboloop.com%';

UPDATE plugin_settings
SET setting_value = REPLACE(setting_value, '"neboloop"', '"neboai"')
WHERE setting_value LIKE '%"neboloop"%';
