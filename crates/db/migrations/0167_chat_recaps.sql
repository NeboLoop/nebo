-- The owner recap: one or two plain sentences the harness writes after a
-- chat turn (Turn-Controller Technical Design SS2.7 / WP2.5), for the owner
-- coming back to the thread. One row per turn; never read back into a model
-- request (harness/recap.rs enforces this).
-- +goose Up
CREATE TABLE IF NOT EXISTS chat_recaps (
    chat_id    TEXT NOT NULL,
    turn_id    TEXT NOT NULL,
    text       TEXT NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    PRIMARY KEY (chat_id, turn_id),
    FOREIGN KEY (chat_id) REFERENCES chats(id) ON DELETE CASCADE
);

CREATE INDEX idx_chat_recaps_chat_id ON chat_recaps(chat_id, created_at DESC);

-- +goose Down
DROP INDEX IF EXISTS idx_chat_recaps_chat_id;
DROP TABLE IF EXISTS chat_recaps;
