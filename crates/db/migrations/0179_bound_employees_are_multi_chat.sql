-- Owner rule (09-25): an employee reachable from outside is a multi-chat
-- employee. From now on binding a door turns multi-chat on
-- (`Store::mark_multi_chat`); this converts the employees already bound:
-- an enabled channel binding (Slack, Discord, Teams, the phone line's
-- bridge) or exposure on the loop. Running it again changes nothing.
INSERT INTO entity_config (entity_type, entity_id, multi_chat)
SELECT 'agent', a.id, 1
FROM agents a
WHERE a.loop_exposed = 1
   OR EXISTS (SELECT 1 FROM channel_bindings b WHERE b.agent_id = a.id AND b.is_enabled = 1)
ON CONFLICT (entity_type, entity_id) DO UPDATE SET multi_chat = 1, updated_at = unixepoch();
