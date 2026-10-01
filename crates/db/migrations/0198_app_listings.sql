-- An app's marketplace listing, one row per app employee: the draft the
-- owner shaped in conversation, the hub artifact it was published as, and
-- the review's outcome. app_publish writes it; the hub's artifact_reviewed
-- notification updates it and finds the app by its artifact. The chat it
-- was submitted from is where the outcome is said.
-- +goose Up
CREATE TABLE app_listings (
    app_id TEXT PRIMARY KEY,
    artifact_id TEXT NOT NULL DEFAULT '',
    draft TEXT NOT NULL DEFAULT '{}',
    status TEXT NOT NULL DEFAULT 'draft',
    version TEXT NOT NULL DEFAULT '',
    notes TEXT NOT NULL DEFAULT '',
    chat_session TEXT NOT NULL DEFAULT '',
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE INDEX idx_app_listings_artifact ON app_listings(artifact_id);

-- +goose Down
DROP TABLE app_listings;
