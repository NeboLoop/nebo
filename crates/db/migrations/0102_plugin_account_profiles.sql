-- Per-agent plugin account profiles.
--
-- Lets one agent hold multiple accounts for the same plugin (the "resource"
-- credential model — e.g. a Chief of Staff watching several Gmail inboxes).
-- Nebo does NOT store the account's tokens here; the plugin owns those inside
-- its config_dir. This table only records WHICH config dir maps to which
-- (agent, plugin, account), so Nebo can inject the right one per invocation.
--
-- Identity-model plugins (one bot per agent, e.g. Slack) don't use this —
-- their single credential lives in channel_bindings.config.
CREATE TABLE IF NOT EXISTS plugin_account_profiles (
    id            TEXT PRIMARY KEY,           -- ULID/uuid
    agent_id      TEXT NOT NULL,
    plugin_slug   TEXT NOT NULL,
    account_label TEXT NOT NULL,              -- user-facing handle, e.g. "work@acme.com"
    config_dir    TEXT NOT NULL,              -- absolute dir injected as the plugin's profile_dir_env
    is_primary    INTEGER NOT NULL DEFAULT 0, -- the default account when a tool call omits --account
    created_at    INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at    INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE(agent_id, plugin_slug, account_label)
);

CREATE INDEX IF NOT EXISTS idx_plugin_account_profiles_agent
    ON plugin_account_profiles(agent_id, plugin_slug);
