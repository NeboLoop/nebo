-- The owner's roster order, kept on the bot so the desktop and every phone
-- show the same lists: what is pinned (employees and teams, in order), the
-- order of the teams and the order of the employees. JSON
-- ({"pinned":[{"kind","id"}],"teams":[ids],"employees":[ids]}); NULL until
-- the owner first pins or reorders, when every list is A to Z.
-- +goose Up
ALTER TABLE user_preferences ADD COLUMN roster_order TEXT;

-- +goose Down
ALTER TABLE user_preferences DROP COLUMN roster_order;
