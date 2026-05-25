-- Per-agent soul: voice, tone, personality, boundaries.
-- Separate from agent_md (AGENT.md = capabilities/persona) — soul is WHO the agent IS.
ALTER TABLE agents ADD COLUMN soul TEXT;
