-- +goose Up
CREATE TABLE IF NOT EXISTS license_keys (
    artifact_id   TEXT NOT NULL,
    artifact_type TEXT NOT NULL,
    scope         TEXT NOT NULL,
    encrypted_key TEXT NOT NULL,
    expires_at    INTEGER NOT NULL,
    created_at    INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
    PRIMARY KEY (artifact_id)
);

-- +goose Down
DROP TABLE IF EXISTS license_keys;
