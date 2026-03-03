-- +goose Up
-- Fix memories table: remove inline UNIQUE(namespace, key) constraint
-- SQLite doesn't support DROP CONSTRAINT, so we must recreate the table

-- Step 1: Create temp table with correct constraints
CREATE TABLE _memories_rebuild (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    namespace TEXT NOT NULL DEFAULT 'default',
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    tags TEXT,
    metadata TEXT,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    accessed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    access_count INTEGER DEFAULT 0,
    user_id TEXT NOT NULL DEFAULT ''
);

-- Step 2: Copy data
INSERT INTO _memories_rebuild (id, namespace, key, value, tags, metadata, created_at, updated_at, accessed_at, access_count, user_id)
SELECT id, namespace, key, value, tags, metadata, created_at, updated_at, accessed_at, access_count, user_id
FROM memories;

-- Step 3: Drop triggers first (they reference the old table)
DROP TRIGGER IF EXISTS memories_ai;
DROP TRIGGER IF EXISTS memories_ad;
DROP TRIGGER IF EXISTS memories_au;

-- Step 4: Drop the old table
DROP TABLE memories;

-- Step 5: Rename temp table
ALTER TABLE _memories_rebuild RENAME TO memories;

-- Step 6: Recreate indexes
CREATE INDEX idx_memories_namespace ON memories(namespace);
CREATE INDEX idx_memories_key ON memories(key);
CREATE INDEX idx_memories_tags ON memories(tags);
CREATE INDEX idx_memories_user_id ON memories(user_id);
CREATE UNIQUE INDEX idx_memories_namespace_key_user ON memories(namespace, key, user_id);

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
CREATE TABLE _memories_rebuild (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    namespace TEXT NOT NULL DEFAULT 'default',
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    tags TEXT,
    metadata TEXT,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    accessed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    access_count INTEGER DEFAULT 0,
    user_id TEXT NOT NULL DEFAULT '',
    UNIQUE(namespace, key)
);

INSERT INTO _memories_rebuild SELECT * FROM memories;

DROP TRIGGER IF EXISTS memories_ai;
DROP TRIGGER IF EXISTS memories_ad;
DROP TRIGGER IF EXISTS memories_au;
DROP TABLE memories;
ALTER TABLE _memories_rebuild RENAME TO memories;

CREATE INDEX idx_memories_namespace ON memories(namespace);
CREATE INDEX idx_memories_key ON memories(key);
CREATE INDEX idx_memories_tags ON memories(tags);
CREATE INDEX idx_memories_user_id ON memories(user_id);
CREATE UNIQUE INDEX idx_memories_namespace_key_user ON memories(namespace, key, user_id);

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
