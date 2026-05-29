-- Per-agent "Expose to Loop" flag: controls whether an agent is registered on
-- the user's personal NeboAI loop. Defaults to 0 (hidden) so agents are opt-in;
-- the primary agent ("assistant") is exposed by default to preserve prior behavior.
ALTER TABLE agents ADD COLUMN loop_exposed INTEGER NOT NULL DEFAULT 0;
UPDATE agents SET loop_exposed = 1 WHERE id = 'assistant';
