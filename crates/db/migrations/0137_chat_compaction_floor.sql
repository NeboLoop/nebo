-- Compaction as a projection over immutable history (coding parity, Stage 1).
--
-- Manual /compact used to DELETE every row of the conversation and insert one
-- summary row. Rows are now kept; the chat carries a floor (the rowid of the
-- last message at compaction time) and every read of the conversation starts
-- above it. The bytes stay on disk, so a substituted tool result is always
-- recoverable and the memory flush still sees everything.
ALTER TABLE chats ADD COLUMN compacted_below_rowid INTEGER;

-- Frozen renderings: once a tool result has been compacted to a rendering the
-- model has seen, that rendering is the rendering forever, across runs and
-- restarts. Per run it lived in memory only, so the next turn could re-decide
-- and the model watched its own history change (and the prompt cache missed).
CREATE TABLE chat_renderings (
    chat_id TEXT NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
    tool_call_id TEXT NOT NULL,
    rendering TEXT NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    PRIMARY KEY (chat_id, tool_call_id)
);
