-- A team's conversation lives in the team thread, never in a member's own.
--
-- Every team post used to be copied, as a user row flagged `teamPost`, into
-- the seat of every member it did not ask to act
-- (agent:<member>:coworker:team:<team>) "so the member's model has the
-- context". Those copies made the seat the member's newest thread, so the app
-- opened the member onto a team exchange it never took part in (2026-09-26:
-- Neighbor Mail's chat showed the owner's post to Marketing & Growth and the
-- Social Media Manager's answer). The copies are no longer written: a member
-- asked to act is briefed from the team thread itself.
--
-- The copies already stored leave the members' threads:
-- 1. A restart note that follows a copy goes: the copy was the unanswered
--    "user" row restart recovery took for an interrupted turn, and nothing
--    was interrupted.
-- 2. A copy whose post is in its team's thread goes. The team thread is
--    written first for every post, so its original is there — matched by
--    team, sender, and the post's own time (the copy is written seconds
--    after; within five minutes counts).
-- 3. A copy whose original is missing from its team's thread is MOVED into
--    it, unwrapped into the team row it was a copy of, so the team history
--    keeps it.
-- Nothing else is touched: an asked member's own work in its seat (the post
-- it was asked on, its tool work, its reply) stays. The migrator writes a
-- full copy of the database before this runs.
-- +goose Up
DELETE FROM chat_messages
WHERE id IN (
    SELECT n.id
    FROM chat_messages n
    WHERE json_valid(n.metadata)
      AND json_extract(n.metadata, '$.restartNotice') = 1
      AND EXISTS (
          SELECT 1
          FROM chat_messages p
          WHERE p.id = (
                  SELECT q.id FROM chat_messages q
                  WHERE q.chat_id = n.chat_id
                    AND (q.created_at, q.rowid) < (n.created_at, n.rowid)
                  ORDER BY q.created_at DESC, q.rowid DESC
                  LIMIT 1)
            AND json_valid(p.metadata)
            AND json_extract(p.metadata, '$.teamPost') IS NOT NULL
      )
);

CREATE TEMP TABLE team_post_copies AS
SELECT m.id AS id,
       m.created_at AS created_at,
       json_extract(m.metadata, '$.teamId') AS team_id,
       t.active_chat_id AS team_chat_id,
       substr(rest, 1, instr(rest, ']' || char(10) || char(10)) - 1) AS sender,
       substr(rest, instr(rest, ']' || char(10) || char(10)) + 3) AS body
FROM (
    SELECT id, chat_id, created_at, metadata,
           substr(content, instr(content, char(10) || '[Post from ') + 12) AS rest
    FROM chat_messages
    WHERE json_valid(metadata)
      AND json_extract(metadata, '$.teamPost') IS NOT NULL
      AND instr(content, char(10) || '[Post from ') > 0
) m
JOIN sessions t ON t.name = 'team:' || json_extract(m.metadata, '$.teamId')
JOIN chats tc ON tc.id = t.active_chat_id
JOIN chats mc ON mc.id = m.chat_id
WHERE mc.id != t.active_chat_id;

DELETE FROM chat_messages
WHERE id IN (
    SELECT c.id FROM team_post_copies c
    WHERE EXISTS (
        SELECT 1 FROM chat_messages o
        WHERE o.chat_id = c.team_chat_id
          AND json_valid(o.metadata)
          AND json_extract(o.metadata, '$.senderName') = c.sender
          AND o.created_at BETWEEN c.created_at - 300 AND c.created_at
    )
);

UPDATE chat_messages
SET chat_id = (SELECT c.team_chat_id FROM team_post_copies c WHERE c.id = chat_messages.id),
    role = CASE WHEN (SELECT c.sender FROM team_post_copies c WHERE c.id = chat_messages.id) = 'Owner'
                THEN 'user' ELSE 'assistant' END,
    content = (SELECT c.body FROM team_post_copies c WHERE c.id = chat_messages.id),
    metadata = (
        SELECT json_object(
                   'senderName', c.sender,
                   'fromAgentId', CASE WHEN c.sender = 'Owner' THEN ''
                                       ELSE COALESCE((SELECT a.id FROM agents a WHERE a.name = c.sender LIMIT 1), '') END,
                   'teamId', c.team_id,
                   'attachments', json('[]'))
        FROM team_post_copies c
        WHERE c.id = chat_messages.id)
WHERE id IN (SELECT id FROM team_post_copies);

DROP TABLE team_post_copies;

-- +goose Down
