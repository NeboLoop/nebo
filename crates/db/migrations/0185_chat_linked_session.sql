-- A linked employee's chat names its session the way Open Agent Link does:
-- the agent's own session id, and the agent it belongs to.
--
-- Nebo reached linked agents through the phone contract, whose chat ids put
-- the hosting member in front of the session on every member but the first
-- (`<member>~<session>`, e.g. `claude-code~s-1`). Nebo now speaks Open Agent
-- Link, where a session is the agent's own id on that agent's channel. Each
-- stored id loses its member prefix, so the chat reaches the same session
-- and its history. A member id is lowercase letters, digits and hyphens, so
-- only such a prefix is taken off; a first member's session was stored raw
-- and stays as it is.
--
-- The agent is recorded from the chat's next turn on (the employee's brain,
-- `linked/<bot>/<agent>`, names it): the member in an old id is not always
-- the agent (`openclaw~s` can be `openclaw-research`'s).
-- +goose Up
ALTER TABLE chats ADD COLUMN linked_agent_id TEXT;

UPDATE chats
   SET linked_chat_id = substr(linked_chat_id, instr(linked_chat_id, '~') + 1)
 WHERE instr(linked_chat_id, '~') > 1
   AND substr(linked_chat_id, 1, instr(linked_chat_id, '~') - 1) NOT GLOB '*[^a-z0-9-]*';

-- +goose Down
