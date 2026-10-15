-- The guided first-run chat (`crate::onboarding` in nebo-server). The step
-- lives in the existing `user_profiles.onboarding_step`; these keep the two
-- answers later steps read back: what the owner wants help with most
-- (store, content, clients, team, home) and the employee hired from it.
-- +goose Up
ALTER TABLE user_profiles ADD COLUMN onboarding_focus TEXT;
ALTER TABLE user_profiles ADD COLUMN onboarding_hire TEXT;

-- +goose Down
ALTER TABLE user_profiles DROP COLUMN onboarding_hire;
ALTER TABLE user_profiles DROP COLUMN onboarding_focus;
