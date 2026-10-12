-- Memories still stored under a date (`daily/<YYYY-MM-DD>`) move into the
-- `project` layer, where ongoing work lives, so nothing is read or swept by
-- date any more. Each keeps its id, value, scope and times; its key gains
-- the date it was written under (`invoice-status-2026-04-14`), so two days'
-- notes on the same thing stay two memories and none collides with a
-- project memory already there. A key that would still collide also gains
-- the row's id.
-- +goose Up
UPDATE OR IGNORE memories
SET namespace = 'project', key = key || '-' || substr(namespace, 7)
WHERE namespace LIKE 'daily/%';

UPDATE OR IGNORE memories
SET namespace = 'project', key = key || '-' || substr(namespace, 7) || '-' || id
WHERE namespace LIKE 'daily/%';

-- +goose Down
