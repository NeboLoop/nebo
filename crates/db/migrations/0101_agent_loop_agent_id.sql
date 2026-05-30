-- Store the NeboAI loop agent UUID returned at registration for each exposed
-- custom agent. The web composer emits `<@{loop_agent_id}>` mention tokens; the
-- desktop maps that token back to the matching local agent so a channel mention
-- routes to the SPECIFIC exposed agent it addresses (not just the primary bot).
-- Nullable: NULL means "not registered / not addressable by loop_agent_id".
ALTER TABLE agents ADD COLUMN loop_agent_id TEXT;
