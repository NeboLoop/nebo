-- Providers as connections and the rest of intelligence packs
-- (neboloop docs/prd/intelligence-packs.md §1a).
-- A connection's models live in provider_models under the connection's key
-- (`<kind>@<auth_profile id>`): `model_kind` says chat or decision (Jev and
-- other SystemOne-compatible models), `source` how it got there (the catalog,
-- added by hand, or picked while browsing the provider's list).
-- A pack gains whether Janus routes it, background lanes pinned to a level,
-- and each level's provider effort.
-- +goose Up
ALTER TABLE provider_models ADD COLUMN model_kind TEXT NOT NULL DEFAULT 'chat';
ALTER TABLE provider_models ADD COLUMN source TEXT NOT NULL DEFAULT 'catalog';
ALTER TABLE intelligence_packs ADD COLUMN route_through_janus INTEGER NOT NULL DEFAULT 0;
ALTER TABLE intelligence_packs ADD COLUMN lanes TEXT NOT NULL DEFAULT '{}';
ALTER TABLE intelligence_packs ADD COLUMN level_effort TEXT NOT NULL DEFAULT '{}';

-- +goose Down
ALTER TABLE intelligence_packs DROP COLUMN level_effort;
ALTER TABLE intelligence_packs DROP COLUMN lanes;
ALTER TABLE intelligence_packs DROP COLUMN route_through_janus;
ALTER TABLE provider_models DROP COLUMN source;
ALTER TABLE provider_models DROP COLUMN model_kind;
