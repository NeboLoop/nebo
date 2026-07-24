-- Attribute notifications to the AI employee that produced them, and record
-- which marketplace department an installed agent belongs to. Both nullable:
-- system notifications have no agent; user-authored agents have no department.
ALTER TABLE notifications ADD COLUMN agent_id TEXT;
ALTER TABLE agents ADD COLUMN department TEXT;
