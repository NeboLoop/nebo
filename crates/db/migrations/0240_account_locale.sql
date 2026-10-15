-- The account's full locale tag as NeboAI holds it ("es-MX", "en-GB"),
-- beside the language Nebo speaks (`language`, e.g. "es"): the region is
-- what picks the guided first-run chat's local mountain names. NULL until
-- the account tells the bot.
-- +goose Up
ALTER TABLE user_preferences ADD COLUMN locale TEXT;

-- +goose Down
ALTER TABLE user_preferences DROP COLUMN locale;
