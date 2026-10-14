-- A proposed change says what it is for. An approval used to show a one-line
-- gist and then the raw proposal (a SKILL.md, or a workflow binding's JSON),
-- which nobody can approve without reading code. The employee that proposes
-- a change now writes three plain-language parts, and the approval leads
-- with them:
--
--   what_went_wrong   the observed problem, with evidence
--   the_fix           what changes
--   what_happens_now  the new behaviour
--
-- NULL on rows filed before this (they keep rendering from `gist`).
-- +goose Up
ALTER TABLE pending_writes ADD COLUMN what_went_wrong TEXT;
ALTER TABLE pending_writes ADD COLUMN the_fix TEXT;
ALTER TABLE pending_writes ADD COLUMN what_happens_now TEXT;

-- +goose Down
ALTER TABLE pending_writes DROP COLUMN what_happens_now;
ALTER TABLE pending_writes DROP COLUMN the_fix;
ALTER TABLE pending_writes DROP COLUMN what_went_wrong;
