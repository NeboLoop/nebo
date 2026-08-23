-- +goose Up
-- Auto-continuation nudges are the house steering an employee, never the owner
-- speaking — but they were persisted as ordinary user turns, so they rendered
-- in the transcript as if the owner had typed them, and the employee's answers
-- read as it repeating itself. Mark the ones already on disk as internal; the
-- transcript read path drops `isMeta` rows. Content is untouched, so the
-- model's own history is exactly what it was.
UPDATE chat_messages
   SET metadata = json_set(
         COALESCE(NULLIF(metadata, ''), '{}'),
         '$.isMeta', json('true'),
         '$.autoContinue', json('true'))
 WHERE role = 'user'
   AND content LIKE 'Continue — your previous response committed to more work that isn''t done yet:%';

-- +goose Down
UPDATE chat_messages
   SET metadata = json_remove(metadata, '$.isMeta', '$.autoContinue')
 WHERE role = 'user'
   AND json_extract(metadata, '$.autoContinue') = 1;
