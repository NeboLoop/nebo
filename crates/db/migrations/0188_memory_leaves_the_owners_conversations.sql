-- An employee's durable memory is its private memory, never one conversation.
--
-- A memory bound to one conversation exists only for a conversation with
-- someone other than the owner (a caller, a visitor, another bot) on an
-- employee the owner sealed (memory.context_isolated). Until now the seal also
-- split the owner's OWN runs: every desktop thread of a sealed employee
-- (`agent:<id>:thread:<chat>`) and every workflow activity
-- (`agent:<id>:workflow:<run>:<activity>::<n>`) filed its memories under
-- `<owner>:agent:<id>:ctx:<that thread or run>`, where no other conversation
-- of the same employee could read them (2026-09-27: the primary employee's
-- facts sat in fourteen single-thread scopes).
--
-- Those rows move up to the employee's private memory (`<owner>:agent:<id>`):
-- 1. A row qualifies when its conversation is one of the owner's own — an
--    agent thread session for that employee, or a workflow run — and nothing
--    it was learned from came from an outside party (no `phone` or `channel`
--    in its provenance). Every other sealed row stays sealed.
-- 2. Rows that meet one key in the private memory (moved or already there)
--    keep the most recently updated as the key. An older row with the same
--    value is a duplicate and goes; an older row with a different value is
--    kept beside it as `<key>/earlier-<id>`, so no fact is lost.
-- 3. Chunks follow their memory; a removed duplicate's chunks and vectors go
--    with it.
-- Running it again finds nothing to move. The migrator writes a full copy of
-- the database before this runs.
-- +goose Up
CREATE TEMP TABLE mem_move AS
WITH sealed AS (
    SELECT m.id, m.namespace, m.key, m.value, m.updated_at, m.metadata,
           substr(m.user_id, 1, instr(m.user_id, ':ctx:') - 1) AS target,
           substr(m.user_id, instr(m.user_id, ':agent:') + 7,
                  instr(m.user_id, ':ctx:') - instr(m.user_id, ':agent:') - 7) AS agent,
           substr(m.user_id, instr(m.user_id, ':ctx:') + 5) AS ctx
    FROM memories m
    WHERE m.user_id LIKE '%:agent:%:ctx:%'
      AND instr(m.user_id, ':agent:') < instr(m.user_id, ':ctx:')
)
SELECT s.id, s.namespace, s.key, s.value, s.updated_at, s.target
FROM sealed s
WHERE (
        EXISTS (SELECT 1 FROM sessions ss WHERE ss.name = 'agent:' || s.agent || ':thread:' || s.ctx)
        OR EXISTS (
            SELECT 1 FROM workflow_runs r
            WHERE r.id = CASE WHEN instr(s.ctx, ':') > 0 THEN substr(s.ctx, 1, instr(s.ctx, ':') - 1) ELSE s.ctx END
        )
      )
  AND NOT (CASE WHEN json_valid(s.metadata) THEN EXISTS (
        SELECT 1 FROM json_each(s.metadata, '$.provenance') WHERE value IN ('phone', 'channel')
      ) ELSE 0 END);

CREATE TEMP TABLE mem_pool AS
SELECT id, namespace, key, value, updated_at, target, 1 AS moving FROM mem_move
UNION ALL
SELECT m.id, m.namespace, m.key, m.value, m.updated_at, m.user_id AS target, 0 AS moving
FROM memories m
WHERE EXISTS (
    SELECT 1 FROM mem_move v
    WHERE v.target = m.user_id AND v.namespace = m.namespace AND v.key = m.key
);

CREATE TEMP TABLE mem_winner AS
SELECT p.* FROM mem_pool p
WHERE p.id = (
    SELECT q.id FROM mem_pool q
    WHERE q.target = p.target AND q.namespace = p.namespace AND q.key = p.key
    ORDER BY q.updated_at DESC, q.id DESC
    LIMIT 1
);

CREATE TEMP TABLE mem_loser AS
SELECT p.id, p.target, (p.value = w.value) AS same
FROM mem_pool p
JOIN mem_winner w ON w.target = p.target AND w.namespace = p.namespace AND w.key = p.key
WHERE p.id <> w.id;

DELETE FROM memory_embeddings
WHERE chunk_id IN (
    SELECT id FROM memory_chunks WHERE memory_id IN (SELECT id FROM mem_loser WHERE same)
);
DELETE FROM memory_chunks WHERE memory_id IN (SELECT id FROM mem_loser WHERE same);
DELETE FROM memories WHERE id IN (SELECT id FROM mem_loser WHERE same);

UPDATE memories
SET key = key || '/earlier-' || id,
    user_id = (SELECT l.target FROM mem_loser l WHERE l.id = memories.id)
WHERE id IN (SELECT id FROM mem_loser WHERE NOT same);

UPDATE memories
SET user_id = (SELECT w.target FROM mem_winner w WHERE w.id = memories.id)
WHERE id IN (SELECT id FROM mem_winner WHERE moving = 1);

UPDATE memory_chunks
SET user_id = (SELECT m.user_id FROM memories m WHERE m.id = memory_chunks.memory_id)
WHERE memory_id IN (SELECT id FROM mem_move)
  AND EXISTS (SELECT 1 FROM memories m WHERE m.id = memory_chunks.memory_id);

DROP TABLE mem_loser;
DROP TABLE mem_winner;
DROP TABLE mem_pool;
DROP TABLE mem_move;
