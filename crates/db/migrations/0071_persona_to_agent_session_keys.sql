-- Consolidate persona: session key prefix into agent:
-- The persona: prefix was the live prefix for agent-scoped sessions but
-- semantically should have been agent: all along.

-- Session keys (sessions.name IS the session key)
UPDATE sessions SET name = REPLACE(name, 'persona:', 'agent:')
  WHERE name LIKE 'persona:%';

-- Chat IDs: chats.id is PK, chat_messages.chat_id is FK with ON DELETE CASCADE.
-- Cannot UPDATE chats.id directly (FK violation). Cannot delete chats (CASCADE
-- would wipe messages). Solution: stash messages, recreate chats, restore messages.

CREATE TEMPORARY TABLE _msgs_rename AS
  SELECT id, REPLACE(chat_id, 'persona:', 'agent:') AS chat_id,
         role, content, metadata, tool_calls, tool_results,
         token_estimate, is_compacted, day_marker, created_at
  FROM chat_messages WHERE chat_id LIKE 'persona:%';

CREATE TEMPORARY TABLE _chats_rename AS
  SELECT REPLACE(id, 'persona:', 'agent:') AS id, title, created_at, updated_at
  FROM chats WHERE id LIKE 'persona:%';

-- Delete old chats (CASCADE deletes old chat_messages rows)
DELETE FROM chats WHERE id LIKE 'persona:%';

-- Insert new chats with agent: IDs
INSERT INTO chats (id, title, created_at, updated_at)
  SELECT id, title, created_at, updated_at FROM _chats_rename;

-- Restore messages with updated chat_id
INSERT INTO chat_messages (id, chat_id, role, content, metadata, tool_calls,
  tool_results, token_estimate, is_compacted, day_marker, created_at)
  SELECT id, chat_id, role, content, metadata, tool_calls,
         tool_results, token_estimate, is_compacted, day_marker, created_at
  FROM _msgs_rename;

DROP TABLE _msgs_rename;
DROP TABLE _chats_rename;

-- Workflow runs
UPDATE workflow_runs SET session_key = REPLACE(session_key, 'persona:', 'agent:')
  WHERE session_key LIKE 'persona:%';

-- Pending tasks
UPDATE pending_tasks SET session_key = REPLACE(session_key, 'persona:', 'agent:')
  WHERE session_key LIKE 'persona:%';

-- Memory namespace: "{uid}:persona:{agent_id}" → "{uid}:agent:{agent_id}"
UPDATE memories SET user_id = REPLACE(user_id, ':persona:', ':agent:')
  WHERE user_id LIKE '%:persona:%';
UPDATE memory_chunks SET user_id = REPLACE(user_id, ':persona:', ':agent:')
  WHERE user_id LIKE '%:persona:%';
