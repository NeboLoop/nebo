-- An intelligence pack no longer says whether it routes through Janus:
-- every paid provider on the owner's own key always goes through Janus
-- (the one way; it charges the platform fee), and what is not a paid
-- service (Ollama, local servers, coding agents, linked computers) never
-- does. The pack's toggle was stored and never acted on.
-- +goose Up
ALTER TABLE intelligence_packs DROP COLUMN route_through_janus;

-- +goose Down
ALTER TABLE intelligence_packs ADD COLUMN route_through_janus INTEGER NOT NULL DEFAULT 0;
