-- +goose Up
-- Replace hardcoded "Nebo" in personality presets with {name} placeholder.
-- The agent runtime replaces {name} with the actual bot name from agent_profile.

UPDATE personality_presets SET system_prompt = REPLACE(system_prompt, 'You are Nebo,', 'You are {name},')
WHERE system_prompt LIKE '%You are Nebo,%';

-- +goose Down
UPDATE personality_presets SET system_prompt = REPLACE(system_prompt, 'You are {name},', 'You are Nebo,')
WHERE system_prompt LIKE '%You are {name},%';
