-- A model's card: what the owner reads about it before he picks it (its
-- maker, family, what it is good for, the level it fits, its abilities,
-- context and output, speed and customer price), as JSON. Janus's model
-- listing writes it for the models Nebo AI serves; a connection's model
-- reads the card of the same model there when Janus knows it.
-- +goose Up
ALTER TABLE provider_models ADD COLUMN card TEXT;

-- +goose Down
ALTER TABLE provider_models DROP COLUMN card;
