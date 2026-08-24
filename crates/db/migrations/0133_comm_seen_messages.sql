-- Durable inbound dedupe: the hub replays an agent space's backlog from the
-- last ACKED offset on every reconnect, and acks are best-effort — so a
-- processed-but-unacked message is redelivered on the next connect. The old
-- guard was a per-connection in-memory window, which forgets exactly when it
-- matters (reconnect). Result: webhook-triggered workflows re-fired and
-- duplicate emails went out while a connection flapped.
-- Processed wire msg_ids are recorded here; a redelivery is dropped no matter
-- which connection (or process restart) it arrives on.
CREATE TABLE IF NOT EXISTS comm_seen_messages (
    id TEXT PRIMARY KEY,
    seen_at INTEGER NOT NULL DEFAULT (unixepoch())
);
