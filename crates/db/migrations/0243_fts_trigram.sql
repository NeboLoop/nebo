-- The full-text indexes with FTS5's trigram tokenizer: a word is found
-- anywhere inside a sentence, also in Japanese, Chinese, Korean and Thai,
-- which write no spaces between words and which the default tokenizer kept
-- as one token per run of text (searching 会議 never found 今日は会議です).
-- Each index is rebuilt from its rows, with the same sync triggers.
-- Words under three characters are matched with LIKE (db::queries::fts).
-- chat_messages_fts was created by the startup FTS check until now; it is
-- built here too, and that check builds the same trigram table.
-- +goose Up
DROP TRIGGER IF EXISTS memories_ai;
DROP TRIGGER IF EXISTS memories_au;
DROP TRIGGER IF EXISTS memories_ad;
DROP TABLE IF EXISTS memories_fts;

CREATE VIRTUAL TABLE memories_fts USING fts5(
    key, value, tags,
    content='memories',
    content_rowid='id',
    tokenize='trigram'
);

INSERT INTO memories_fts(rowid, key, value, tags)
SELECT id, key, value, tags FROM memories;

-- +goose StatementBegin
CREATE TRIGGER memories_ai AFTER INSERT ON memories BEGIN
    INSERT INTO memories_fts(rowid, key, value, tags)
    VALUES (new.id, new.key, new.value, new.tags);
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

-- +goose StatementBegin
CREATE TRIGGER memories_ad AFTER DELETE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, key, value, tags)
    VALUES ('delete', old.id, old.key, old.value, old.tags);
END;
-- +goose StatementEnd

DROP TRIGGER IF EXISTS memory_chunks_ai;
DROP TRIGGER IF EXISTS memory_chunks_au;
DROP TRIGGER IF EXISTS memory_chunks_ad;
DROP TABLE IF EXISTS memory_chunks_fts;

CREATE VIRTUAL TABLE memory_chunks_fts USING fts5(
    text, source, path,
    content='memory_chunks',
    content_rowid='id',
    tokenize='trigram'
);

INSERT INTO memory_chunks_fts(rowid, text, source, path)
SELECT id, text, source, path FROM memory_chunks;

-- +goose StatementBegin
CREATE TRIGGER memory_chunks_ai AFTER INSERT ON memory_chunks BEGIN
    INSERT INTO memory_chunks_fts(rowid, text, source, path)
    VALUES (new.id, new.text, new.source, new.path);
END;
-- +goose StatementEnd

-- +goose StatementBegin
CREATE TRIGGER memory_chunks_au AFTER UPDATE ON memory_chunks BEGIN
    INSERT INTO memory_chunks_fts(memory_chunks_fts, rowid, text, source, path)
    VALUES ('delete', old.id, old.text, old.source, old.path);
    INSERT INTO memory_chunks_fts(rowid, text, source, path)
    VALUES (new.id, new.text, new.source, new.path);
END;
-- +goose StatementEnd

-- +goose StatementBegin
CREATE TRIGGER memory_chunks_ad AFTER DELETE ON memory_chunks BEGIN
    INSERT INTO memory_chunks_fts(memory_chunks_fts, rowid, text, source, path)
    VALUES ('delete', old.id, old.text, old.source, old.path);
END;
-- +goose StatementEnd

DROP TRIGGER IF EXISTS chat_messages_fts_ai;
DROP TRIGGER IF EXISTS chat_messages_fts_au;
DROP TRIGGER IF EXISTS chat_messages_fts_ad;
DROP TABLE IF EXISTS chat_messages_fts;

CREATE VIRTUAL TABLE chat_messages_fts USING fts5(
    content,
    content='chat_messages',
    content_rowid='rowid',
    tokenize='trigram'
);

INSERT INTO chat_messages_fts(rowid, content)
SELECT rowid, content FROM chat_messages;

-- +goose StatementBegin
CREATE TRIGGER chat_messages_fts_ai AFTER INSERT ON chat_messages BEGIN
    INSERT INTO chat_messages_fts(rowid, content)
    VALUES (new.rowid, new.content);
END;
-- +goose StatementEnd

-- +goose StatementBegin
CREATE TRIGGER chat_messages_fts_au AFTER UPDATE OF content ON chat_messages BEGIN
    INSERT INTO chat_messages_fts(chat_messages_fts, rowid, content)
    VALUES ('delete', old.rowid, old.content);
    INSERT INTO chat_messages_fts(rowid, content)
    VALUES (new.rowid, new.content);
END;
-- +goose StatementEnd

-- +goose StatementBegin
CREATE TRIGGER chat_messages_fts_ad AFTER DELETE ON chat_messages BEGIN
    INSERT INTO chat_messages_fts(chat_messages_fts, rowid, content)
    VALUES ('delete', old.rowid, old.content);
END;
-- +goose StatementEnd

-- +goose Down
DROP TRIGGER IF EXISTS memories_ai;
DROP TRIGGER IF EXISTS memories_au;
DROP TRIGGER IF EXISTS memories_ad;
DROP TABLE IF EXISTS memories_fts;

CREATE VIRTUAL TABLE memories_fts USING fts5(
    key, value, tags,
    content='memories',
    content_rowid='id'
);

INSERT INTO memories_fts(rowid, key, value, tags)
SELECT id, key, value, tags FROM memories;

-- +goose StatementBegin
CREATE TRIGGER memories_ai AFTER INSERT ON memories BEGIN
    INSERT INTO memories_fts(rowid, key, value, tags)
    VALUES (new.id, new.key, new.value, new.tags);
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

-- +goose StatementBegin
CREATE TRIGGER memories_ad AFTER DELETE ON memories BEGIN
    INSERT INTO memories_fts(memories_fts, rowid, key, value, tags)
    VALUES ('delete', old.id, old.key, old.value, old.tags);
END;
-- +goose StatementEnd

DROP TRIGGER IF EXISTS memory_chunks_ai;
DROP TRIGGER IF EXISTS memory_chunks_au;
DROP TRIGGER IF EXISTS memory_chunks_ad;
DROP TABLE IF EXISTS memory_chunks_fts;

CREATE VIRTUAL TABLE memory_chunks_fts USING fts5(
    text, source, path,
    content='memory_chunks',
    content_rowid='id'
);

INSERT INTO memory_chunks_fts(rowid, text, source, path)
SELECT id, text, source, path FROM memory_chunks;

-- +goose StatementBegin
CREATE TRIGGER memory_chunks_ai AFTER INSERT ON memory_chunks BEGIN
    INSERT INTO memory_chunks_fts(rowid, text, source, path)
    VALUES (new.id, new.text, new.source, new.path);
END;
-- +goose StatementEnd

-- +goose StatementBegin
CREATE TRIGGER memory_chunks_au AFTER UPDATE ON memory_chunks BEGIN
    INSERT INTO memory_chunks_fts(memory_chunks_fts, rowid, text, source, path)
    VALUES ('delete', old.id, old.text, old.source, old.path);
    INSERT INTO memory_chunks_fts(rowid, text, source, path)
    VALUES (new.id, new.text, new.source, new.path);
END;
-- +goose StatementEnd

-- +goose StatementBegin
CREATE TRIGGER memory_chunks_ad AFTER DELETE ON memory_chunks BEGIN
    INSERT INTO memory_chunks_fts(memory_chunks_fts, rowid, text, source, path)
    VALUES ('delete', old.id, old.text, old.source, old.path);
END;
-- +goose StatementEnd

DROP TRIGGER IF EXISTS chat_messages_fts_ai;
DROP TRIGGER IF EXISTS chat_messages_fts_au;
DROP TRIGGER IF EXISTS chat_messages_fts_ad;
DROP TABLE IF EXISTS chat_messages_fts;

CREATE VIRTUAL TABLE chat_messages_fts USING fts5(
    content,
    content='chat_messages',
    content_rowid='rowid'
);

INSERT INTO chat_messages_fts(rowid, content)
SELECT rowid, content FROM chat_messages;

-- +goose StatementBegin
CREATE TRIGGER chat_messages_fts_ai AFTER INSERT ON chat_messages BEGIN
    INSERT INTO chat_messages_fts(rowid, content)
    VALUES (new.rowid, new.content);
END;
-- +goose StatementEnd

-- +goose StatementBegin
CREATE TRIGGER chat_messages_fts_au AFTER UPDATE OF content ON chat_messages BEGIN
    INSERT INTO chat_messages_fts(chat_messages_fts, rowid, content)
    VALUES ('delete', old.rowid, old.content);
    INSERT INTO chat_messages_fts(rowid, content)
    VALUES (new.rowid, new.content);
END;
-- +goose StatementEnd

-- +goose StatementBegin
CREATE TRIGGER chat_messages_fts_ad AFTER DELETE ON chat_messages BEGIN
    INSERT INTO chat_messages_fts(chat_messages_fts, rowid, content)
    VALUES ('delete', old.rowid, old.content);
END;
-- +goose StatementEnd
