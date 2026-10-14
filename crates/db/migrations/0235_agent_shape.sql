-- The character an employee wears (`types::character`): a mountain shape
-- the owner picked, beside its colour. NULL until the owner picks one; the
-- roster then derives a stable shape from the employee's role or id.
-- +goose Up
ALTER TABLE agents ADD COLUMN shape TEXT;

-- +goose Down
ALTER TABLE agents DROP COLUMN shape;
