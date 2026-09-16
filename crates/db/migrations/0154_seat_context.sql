-- +goose Up
-- A seat's own reading of the layers above it (industry, franchise, company),
-- written by the seat in its update run and rendered into its static prompt.
-- context_stamp is JSON: what the section was written against and whether
-- the run that should have written it finished.
ALTER TABLE agents ADD COLUMN context_section TEXT;
ALTER TABLE agents ADD COLUMN context_stamp TEXT;

-- +goose Down
ALTER TABLE agents DROP COLUMN context_stamp;
ALTER TABLE agents DROP COLUMN context_section;
