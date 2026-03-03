-- +goose Up
-- User profile (rich context about the human)
CREATE TABLE user_profiles (
    user_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    display_name TEXT,
    bio TEXT,
    location TEXT,
    timezone TEXT,
    occupation TEXT,
    interests TEXT,              -- JSON array
    communication_style TEXT,    -- formal, casual, adaptive
    goals TEXT,
    context TEXT,                -- free-form context for agent
    onboarding_completed INTEGER DEFAULT 0,
    onboarding_step TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

-- Agent profile (personality settings) - singleton
CREATE TABLE agent_profile (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    name TEXT NOT NULL DEFAULT 'Nebo',
    personality_preset TEXT DEFAULT 'balanced',
    custom_personality TEXT,
    voice_style TEXT DEFAULT 'neutral',
    response_length TEXT DEFAULT 'adaptive',
    emoji_usage TEXT DEFAULT 'moderate',
    formality TEXT DEFAULT 'adaptive',
    proactivity TEXT DEFAULT 'moderate',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

-- Personality presets (reference data)
CREATE TABLE personality_presets (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    description TEXT,
    system_prompt TEXT NOT NULL,
    icon TEXT,
    display_order INTEGER DEFAULT 0
);

-- Initialize agent profile singleton
INSERT INTO agent_profile (id, name, created_at, updated_at)
VALUES (1, 'Nebo', strftime('%s','now'), strftime('%s','now'));

-- Initialize personality presets
INSERT INTO personality_presets (id, name, description, system_prompt, icon, display_order) VALUES
('balanced', 'Balanced', 'Friendly and capable assistant', 'You are Nebo, a helpful and friendly AI assistant. Be warm, clear, and supportive while remaining professional.', '🎯', 1),
('professional', 'Professional', 'Formal and business-focused', 'You are Nebo, a professional AI assistant. Be concise, business-focused, and maintain formal communication standards.', '💼', 2),
('creative', 'Creative', 'Imaginative and expressive', 'You are Nebo, a creative AI assistant. Be playful, think outside the box, and bring imagination to every interaction.', '🎨', 3),
('minimal', 'Minimal', 'Brief and efficient', 'You are Nebo, a concise AI assistant. Keep responses short, direct, and to the point. No fluff.', '⚡', 4),
('supportive', 'Supportive', 'Empathetic and encouraging', 'You are Nebo, a supportive AI assistant. Be empathetic, encouraging, and focus on the human side of every interaction.', '💚', 5);

-- +goose Down
DROP TABLE IF EXISTS personality_presets;
DROP TABLE IF EXISTS agent_profile;
DROP TABLE IF EXISTS user_profiles;
