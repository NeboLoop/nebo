-- +goose Up
CREATE TABLE IF NOT EXISTS error_logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp DATETIME NOT NULL DEFAULT (unixepoch()),
    level TEXT NOT NULL,     -- panic, error, warn
    module TEXT NOT NULL,    -- e.g. "runner", "lane", "hub", "apps"
    message TEXT NOT NULL,
    stacktrace TEXT,
    context TEXT,            -- JSON: operation, session_id, user_id, etc.
    resolved INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX idx_error_logs_timestamp ON error_logs(timestamp DESC);
CREATE INDEX idx_error_logs_level ON error_logs(level);

-- +goose Down
DROP INDEX IF EXISTS idx_error_logs_level;
DROP INDEX IF EXISTS idx_error_logs_timestamp;
DROP TABLE IF EXISTS error_logs;
