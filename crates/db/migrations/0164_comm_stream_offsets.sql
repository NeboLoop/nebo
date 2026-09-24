-- The last seq this bot acked on each of its own hub streams (dm, chat,
-- installs, account, voice, channels/inbound). On every connect the bot JOINs
-- each stream with this offset, so the hub replays what arrived while it was
-- disconnected. Keyed by bot id so a re-registered bot starts clean. Lives in
-- the database so it is restored with the comm dedupe records it pairs with.
CREATE TABLE IF NOT EXISTS comm_stream_offsets (
    bot_id     TEXT NOT NULL,
    stream     TEXT NOT NULL,
    acked_seq  INTEGER NOT NULL,
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    PRIMARY KEY (bot_id, stream)
);
