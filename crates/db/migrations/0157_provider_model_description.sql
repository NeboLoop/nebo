-- +goose Up
-- The one-line description a model picker shows under a model's name
-- ("For your toughest work"). Set by the Janus catalog sync; NULL for
-- models that never appear in a picker.
ALTER TABLE provider_models ADD COLUMN description TEXT;

-- +goose Down
