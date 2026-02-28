-- +goose Up
CREATE TABLE IF NOT EXISTS channel_skills (
    channel_id TEXT NOT NULL,
    skill_name TEXT NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    PRIMARY KEY (channel_id, skill_name)
);

-- +goose Down
DROP TABLE IF EXISTS channel_skills;
