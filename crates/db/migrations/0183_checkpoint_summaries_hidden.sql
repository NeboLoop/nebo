-- A checkpoint's summary is the model's, never a message from the owner.
--
-- Checkpoints were stored as one visible user row ("This conversation
-- continues from an earlier part that was summarized: ..."), so the owner's
-- thread filled with summaries that read as if he had written them. The
-- harness now writes each checkpoint as a hidden boundary row (isMeta) plus
-- one owner-visible marker just before it: a system row
-- {"compactBoundary":true} the apps render as a quiet "Earlier conversation
-- summarized" divider (harness/compact/checkpoint.rs). Each visible
-- checkpoint row already stored becomes the same pair: a marker one second
-- before it (so the model's conversation, which loads from the boundary on,
-- never holds the marker), and the row hidden. Its content is untouched, so
-- the model's history is exactly what it was. Idempotent: a hidden row is
-- never matched again.
-- +goose Up
INSERT INTO chat_messages (id, chat_id, role, content, metadata, created_at, day_marker)
SELECT
    lower(hex(randomblob(16))),
    chat_id,
    'system',
    'Earlier conversation summarized',
    json_object(
        'compactBoundary', json('true'),
        'reason', COALESCE(CASE WHEN json_valid(metadata) THEN json_extract(metadata, '$.reason') END, 'migrated')
    ),
    created_at - 1,
    date(created_at - 1, 'unixepoch', 'localtime')
FROM chat_messages
WHERE role = 'user'
  AND content LIKE 'This conversation continues from an earlier part that was summarized%'
  AND COALESCE(CASE WHEN json_valid(metadata) THEN json_extract(metadata, '$.isMeta') END, 0) NOT IN (1, 'true');

UPDATE chat_messages
   SET metadata = json_set(
         CASE WHEN json_valid(metadata) AND metadata != '' THEN metadata ELSE '{}' END,
         '$.isMeta', json('true'))
 WHERE role = 'user'
   AND content LIKE 'This conversation continues from an earlier part that was summarized%'
   AND COALESCE(CASE WHEN json_valid(metadata) THEN json_extract(metadata, '$.isMeta') END, 0) NOT IN (1, 'true');
