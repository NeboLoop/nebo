-- Intelligence packs the owner makes: his own AI (his keys, an agent on his
-- computer) bundled under a name and assigned like a model. Each Effort level
-- (instant, low, medium, high, max) and capability (vision, voice) names the
-- `provider/model` it runs on, as JSON. The built-in Nebo AI pack is in code
-- (`types::packs::nebo_ai`), never a row.
-- Design: neboloop docs/prd/intelligence-packs.md.
-- +goose Up
CREATE TABLE IF NOT EXISTS intelligence_packs (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    levels      TEXT NOT NULL DEFAULT '{}',  -- JSON types::packs::PackLevels
    fallback    INTEGER NOT NULL DEFAULT 1,  -- run on Nebo AI when its AI can't
    created_at  INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at  INTEGER NOT NULL DEFAULT (unixepoch())
);

-- +goose Down
DROP TABLE IF EXISTS intelligence_packs;
