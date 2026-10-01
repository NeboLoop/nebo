-- Outbound hub messages waiting to be sent. Each durable message is written
-- here before it is sent; what a dropped connection or a restart left behind
-- goes out oldest first (by seq) on the next connection. `message` is the
-- whole message as JSON; `id` is its own id, which every send of it carries.
-- `handed_off_at` (unix ms) is set the moment it is handed to a connection:
-- from then on it may have reached the hub, so it is only sent again where
-- the hub dedupes by message id.
CREATE TABLE IF NOT EXISTS comm_outbox (
    seq           INTEGER PRIMARY KEY AUTOINCREMENT,
    id            TEXT NOT NULL UNIQUE,
    message       TEXT NOT NULL,
    handed_off_at INTEGER,
    created_at    INTEGER NOT NULL DEFAULT (unixepoch())
);
