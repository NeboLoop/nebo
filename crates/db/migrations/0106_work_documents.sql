-- Work-document versioning: container + append-only version chain.
-- A "work document" (the html/md/pdf/jsx/etc. artifacts the AI produces in the
-- Work panel) is identified by (chat_id, filename). Each AI write becomes a new
-- version row rather than overwriting the file, so the viewer can refresh in
-- place and the user can navigate history. Mirrors Claude Desktop's
-- artifacts/artifact_versions split with a denormalized latest pointer.

CREATE TABLE IF NOT EXISTS work_documents (
    id              TEXT PRIMARY KEY,                 -- stable container id (uuid)
    chat_id         TEXT NOT NULL,
    filename        TEXT NOT NULL,
    kind            TEXT NOT NULL,                    -- document|code|table|slides
    latest_version  INTEGER NOT NULL DEFAULT 0,       -- denormalized "current" pointer
    created_at      INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at      INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE(chat_id, filename),
    FOREIGN KEY (chat_id) REFERENCES chats(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS work_document_versions (
    id                 TEXT PRIMARY KEY,              -- version id (uuid)
    document_id        TEXT NOT NULL,
    version_number     INTEGER NOT NULL,
    parent_version_id  TEXT,                          -- linear chain now, DAG-ready
    url                TEXT NOT NULL,                 -- /api/v1/files/work/<doc>/v<N>/<filename>
    content_hash       TEXT,                          -- SHA-256 of bytes (no-op-write detection)
    content_type       TEXT,
    message_id         TEXT,                          -- assistant message that produced it
    created_at         INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE(document_id, version_number),
    FOREIGN KEY (document_id) REFERENCES work_documents(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS ix_work_documents_chat ON work_documents(chat_id);
CREATE INDEX IF NOT EXISTS ix_work_doc_versions_doc ON work_document_versions(document_id);
