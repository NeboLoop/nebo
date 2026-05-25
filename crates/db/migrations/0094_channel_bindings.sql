-- Channel bindings — user-configured routing of plugin channel bridges to agents.
-- When a plugin declares a channel capability (e.g., Slack, Discord), the user
-- can enable it per-agent in the agent's settings screen.

CREATE TABLE IF NOT EXISTS channel_bindings (
    agent_id TEXT NOT NULL,
    plugin_slug TEXT NOT NULL,
    is_enabled INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (agent_id, plugin_slug)
);
