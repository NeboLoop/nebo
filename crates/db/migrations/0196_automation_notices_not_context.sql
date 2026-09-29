-- An automation's notices ("Automation started", "completed", "failed",
-- "paused for your approval") are status for the owner, posted into the
-- employee's main conversation. They were stored as the employee's own
-- words, so every turn read them back as context: a five-minute workflow
-- left one cloud employee's conversation with 36,980 of them (2026-09-29),
-- enough that the next turn would have sent over a million tokens. They now
-- carry `{"automation": true}`, which the model's view and memory extraction
-- leave out; the owner's thread keeps them. This marks the ones already
-- stored.
-- +goose Up
UPDATE chat_messages
SET metadata = json_set(CASE WHEN json_valid(metadata) THEN metadata ELSE '{}' END, '$.automation', json('true'))
WHERE role = 'assistant'
  AND (content LIKE '**Automation started** — %'
    OR content LIKE '**Automation completed** — %'
    OR content LIKE '**Automation failed** — %'
    OR content LIKE '**Automation paused for your approval** — %');

-- +goose Down
