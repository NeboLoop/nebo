-- +goose Up
-- The owner's answer to each permission an app installed from the
-- marketplace or shared through a loop asks for (filesystem:, network:,
-- subagent:). No row: not answered yet, so not granted.
CREATE TABLE app_permission_grants (
    app_id TEXT NOT NULL,
    permission TEXT NOT NULL,
    allowed INTEGER NOT NULL,
    decided_at INTEGER NOT NULL DEFAULT (unixepoch()),
    PRIMARY KEY (app_id, permission)
);

-- +goose Down
DROP TABLE app_permission_grants;
