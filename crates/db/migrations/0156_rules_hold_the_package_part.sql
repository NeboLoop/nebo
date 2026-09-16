-- +goose Up
-- The employee's reading of the industry, franchise, and company packages
-- lives in agents.rules (between markers), the one editable section; the
-- separate column it first went to is gone. context_stamp stays: what the
-- package part was reviewed against and whether that review finished.
ALTER TABLE agents DROP COLUMN context_section;

-- +goose Down
ALTER TABLE agents ADD COLUMN context_section TEXT;
