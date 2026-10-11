-- One stored shape for an assistant row's calls, so a thread is read one way.
--
-- A row with calls keeps them in `tool_calls`, their results in the `tool`
-- rows after it, and its block order in `metadata.contentBlocks`: each text
-- block with the verdict the stream gave it (`fold`), each call by its index.
-- Three older shapes were read by branches of their own; each is converted
-- here once, and the branches are gone:
--
-- 1. Calls kept only in `metadata.toolCalls` (with their outputs): they move
--    to `tool_calls`, their outputs to a `tool` row after the reply.
-- 2. Calls with no block order (rows from before it was stored, and
--    imported conversations): text first, then the calls, as they were read.
-- 3. A text block beside calls with no verdict (stored before verdicts):
--    `shown`, the note it was read as.
-- +goose Up
INSERT INTO chat_messages (id, chat_id, role, content, tool_results, created_at, day_marker)
SELECT m.id || ':results', m.chat_id, 'tool', '',
       (SELECT json_group_array(json_object(
                   'tool_call_id', json_extract(t.value, '$.id'),
                   'content', coalesce(json_extract(t.value, '$.output'), ''),
                   'is_error', json(CASE WHEN json_extract(t.value, '$.status') = 'error' THEN 'true' ELSE 'false' END)))
          FROM json_each(m.metadata, '$.toolCalls') t),
       m.created_at, m.day_marker
  FROM chat_messages m
 WHERE m.role = 'assistant'
   AND json_valid(m.metadata)
   AND json_type(m.metadata, '$.toolCalls') = 'array'
   AND json_array_length(m.metadata, '$.toolCalls') > 0
   AND (m.tool_calls IS NULL OR m.tool_calls IN ('', '[]'));

UPDATE chat_messages
   SET tool_calls = (SELECT json_group_array(json_object(
                               'id', json_extract(t.value, '$.id'),
                               'name', json_extract(t.value, '$.name'),
                               'input', CASE
                                   WHEN json_type(t.value, '$.input') = 'text' AND json_valid(json_extract(t.value, '$.input'))
                                       THEN json(json_extract(t.value, '$.input'))
                                   ELSE json_extract(t.value, '$.input')
                               END))
                       FROM json_each(chat_messages.metadata, '$.toolCalls') t)
 WHERE role = 'assistant'
   AND json_valid(metadata)
   AND json_type(metadata, '$.toolCalls') = 'array'
   AND json_array_length(metadata, '$.toolCalls') > 0
   AND (tool_calls IS NULL OR tool_calls IN ('', '[]'));

UPDATE chat_messages
   SET metadata = json_remove(metadata, '$.toolCalls')
 WHERE json_valid(metadata)
   AND json_type(metadata, '$.toolCalls') IS NOT NULL;

UPDATE chat_messages
   SET metadata = json_set(
           CASE WHEN coalesce(json_valid(metadata) AND json_type(metadata) = 'object', 0) THEN metadata ELSE '{}' END,
           '$.contentBlocks',
           (SELECT json_group_array(json(b.block))
              FROM (SELECT json_object('type', 'text', 'fold', 'shown') AS block, -1 AS at
                     WHERE chat_messages.content <> ''
                    UNION ALL
                    SELECT json_object('type', 'tool', 'toolCallIndex', CAST(c.key AS INTEGER)), CAST(c.key AS INTEGER)
                      FROM json_each(chat_messages.tool_calls) c
                    ORDER BY at) b))
 WHERE role = 'assistant'
   AND json_valid(tool_calls)
   AND json_array_length(tool_calls) > 0
   AND NOT coalesce(json_valid(metadata) AND json_type(metadata, '$.contentBlocks') = 'array', 0);

UPDATE chat_messages
   SET metadata = json_set(metadata, '$.contentBlocks',
           (SELECT json_group_array(json(b.block))
              FROM (SELECT CASE
                               WHEN json_extract(e.value, '$.type') = 'text' AND json_type(e.value, '$.fold') IS NULL
                                   THEN json_set(e.value, '$.fold', 'shown')
                               ELSE e.value
                           END AS block,
                           CAST(e.key AS INTEGER) AS at
                      FROM json_each(chat_messages.metadata, '$.contentBlocks') e
                    ORDER BY at) b))
 WHERE role = 'assistant'
   AND json_valid(tool_calls)
   AND json_array_length(tool_calls) > 0
   AND json_valid(metadata)
   AND EXISTS (SELECT 1 FROM json_each(chat_messages.metadata, '$.contentBlocks') e
                WHERE json_extract(e.value, '$.type') = 'text' AND json_type(e.value, '$.fold') IS NULL);

-- +goose Down
-- The conversion is one way: every reader reads the shape it leaves.
SELECT 1;
