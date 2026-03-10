-- +goose Up
-- Rebuild memories_fts virtual table.
-- Migration 0021 recreated the memories table (DROP + RENAME) which broke
-- the FTS5 content table binding. The triggers insert into a stale FTS table,
-- silently rolling back memory inserts.

-- Drop old triggers first
DROP TRIGGER IF EXISTS memories_ai;
DROP TRIGGER IF EXISTS memories_ad;
DROP TRIGGER IF EXISTS memories_au;

-- Drop and rebuild the FTS virtual table
DROP TABLE IF EXISTS memories_fts;

CREATE VIRTUAL TABLE memories_fts USING fts5(
    key, value, tags,
    content='memories',
    content_rowid='id'
);

-- Rebuild FTS index from existing data
INSERT INTO memories_fts(rowid, key, value, tags)
SELECT id, key, value, tags FROM memories;

-- Recreate triggers
-- +goose StatementBegin
CREATE TRIGGER memories_ai AFTER INSERT ON memories BEGIN
    INSERT INTO memories_fts(rowid, key, value, tags)
    VALUES (new.id, new.key, new.value, new.tags);
END;
-- +goose StatementEnd

-- +goose StatementBegin
CREATE TRIGGER memories_ad AFTER DELETE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, key, value, tags)
    VALUES ('delete', old.id, old.key, old.value, old.tags);
END;
-- +goose StatementEnd

-- +goose StatementBegin
CREATE TRIGGER memories_au AFTER UPDATE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, key, value, tags)
    VALUES ('delete', old.id, old.key, old.value, old.tags);
    INSERT INTO memories_fts(rowid, key, value, tags)
    VALUES (new.id, new.key, new.value, new.tags);
END;
-- +goose StatementEnd

-- +goose Down
-- No-op: FTS table will be rebuilt by running the Up migration again
