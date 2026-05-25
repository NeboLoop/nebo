-- Per-agent channel credentials. Stores JSON env var overrides so each agent
-- can have its own Slack/Discord/Telegram bot identity.

ALTER TABLE channel_bindings ADD COLUMN config TEXT NOT NULL DEFAULT '{}';
