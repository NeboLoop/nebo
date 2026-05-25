-- Per-agent rules: behavior constraints and guardrails.
ALTER TABLE agents ADD COLUMN rules TEXT;
