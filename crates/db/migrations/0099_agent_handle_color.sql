-- Per-agent loop identity: handle (stored as `bot_<chosen>`) and color.
-- Persists the primary agent's handle/color so the comms CONNECT frame can
-- reflect them on the NeboAI loop. Empty handle ⇒ gateway falls back to bot_<id>.
ALTER TABLE agents ADD COLUMN handle TEXT;
ALTER TABLE agents ADD COLUMN color TEXT;
