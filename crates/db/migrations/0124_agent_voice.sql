-- Per-agent voice id for realtime voice calls (xAI voice: eve, ara, rex, sal,
-- leo, or a custom voice id). Empty = server default. Voice is part of the
-- agent's identity — each employee sounds like themselves.
ALTER TABLE agents ADD COLUMN voice TEXT NOT NULL DEFAULT '';
