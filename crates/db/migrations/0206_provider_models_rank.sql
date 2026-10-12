-- A synced speed's place on Janus's ladder (nebo-1 0, Fast 1, Balanced 2,
-- Deep 3), so every picker lists the speeds by weight. The table was read
-- ORDER BY display_name: Balanced, Deep, Fast. NULL for every other row.
-- +goose Up
ALTER TABLE provider_models ADD COLUMN rank INTEGER;

-- +goose Down
ALTER TABLE provider_models DROP COLUMN rank;
