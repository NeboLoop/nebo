-- +goose Up
CREATE TABLE IF NOT EXISTS event_dedup (
    fingerprint TEXT PRIMARY KEY,
    source TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE INDEX idx_event_dedup_created_at ON event_dedup(created_at);

-- +goose Down
DROP TABLE IF EXISTS event_dedup;
