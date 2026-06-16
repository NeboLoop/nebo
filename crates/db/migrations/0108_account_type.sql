-- Account type captured during onboarding ("Who's Nebo for?"): "personal" | "business".
-- Drives the welcome copy and the tour/setup emphasis. NULL until the wizard sets it.
ALTER TABLE user_profiles ADD COLUMN account_type TEXT;
