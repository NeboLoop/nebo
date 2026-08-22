use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::Store;
use types::NeboError;

/// A work-document container: a logical document (by filename, within a chat)
/// that accumulates an append-only chain of versions as the AI revises it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkDocument {
    pub id: String,
    pub chat_id: String,
    pub filename: String,
    pub kind: String,
    pub latest_version: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

/// One immutable version of a work document.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkDocumentVersion {
    pub id: String,
    pub document_id: String,
    pub version_number: i64,
    pub parent_version_id: Option<String>,
    pub url: String,
    pub content_hash: Option<String>,
    pub content_type: Option<String>,
    pub message_id: Option<String>,
    pub created_at: i64,
}

/// One row of the account-wide document index: the container joined with its
/// latest version and the owning chat's title/session (for agent attribution).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkDocumentListing {
    pub id: String,
    pub chat_id: String,
    pub filename: String,
    pub kind: String,
    pub latest_version: i64,
    pub url: String,
    pub content_type: Option<String>,
    pub chat_title: Option<String>,
    pub session_name: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

fn row_to_document(row: &rusqlite::Row) -> rusqlite::Result<WorkDocument> {
    Ok(WorkDocument {
        id: row.get("id")?,
        chat_id: row.get("chat_id")?,
        filename: row.get("filename")?,
        kind: row.get("kind")?,
        latest_version: row.get("latest_version")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

fn row_to_version(row: &rusqlite::Row) -> rusqlite::Result<WorkDocumentVersion> {
    Ok(WorkDocumentVersion {
        id: row.get("id")?,
        document_id: row.get("document_id")?,
        version_number: row.get("version_number")?,
        parent_version_id: row.get("parent_version_id")?,
        url: row.get("url")?,
        content_hash: row.get("content_hash")?,
        content_type: row.get("content_type")?,
        message_id: row.get("message_id")?,
        created_at: row.get("created_at")?,
    })
}

impl Store {
    /// Get or create the work-document container for (chat_id, filename).
    pub fn upsert_work_document(
        &self,
        chat_id: &str,
        filename: &str,
        kind: &str,
    ) -> Result<WorkDocument, NeboError> {
        if let Some(doc) = self.work_document_for(chat_id, filename)? {
            return Ok(doc);
        }
        let conn = self.conn()?;
        let id = uuid::Uuid::new_v4().to_string();
        conn.query_row(
            "INSERT INTO work_documents
               (id, chat_id, filename, kind, latest_version, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 0, unixepoch(), unixepoch()) RETURNING *",
            params![id, chat_id, filename, kind],
            row_to_document,
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// The work-document container for (chat_id, filename), if it exists.
    pub fn work_document_for(
        &self,
        chat_id: &str,
        filename: &str,
    ) -> Result<Option<WorkDocument>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT * FROM work_documents WHERE chat_id = ?1 AND filename = ?2",
            params![chat_id, filename],
            row_to_document,
        )
        .optional()
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// A work-document container by id.
    pub fn get_work_document(&self, id: &str) -> Result<Option<WorkDocument>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT * FROM work_documents WHERE id = ?1",
            params![id],
            row_to_document,
        )
        .optional()
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// Restore an earlier version: append a NEW version whose content is the chosen
    /// version's (content-addressed → no copy, just reference the same blob),
    /// parented off the current latest. Non-destructive — full history is kept.
    pub fn restore_work_version(
        &self,
        document_id: &str,
        version_number: i64,
    ) -> Result<WorkDocumentVersion, NeboError> {
        let target = {
            let conn = self.conn()?;
            conn.query_row(
                "SELECT * FROM work_document_versions
                 WHERE document_id = ?1 AND version_number = ?2",
                params![document_id, version_number],
                row_to_version,
            )
            .optional()
            .map_err(|e| NeboError::Database(e.to_string()))?
            .ok_or(NeboError::NotFound)?
        };
        let latest = self.latest_work_version(document_id)?;
        let parent_id = latest.as_ref().map(|v| v.id.as_str());
        self.add_work_version(
            document_id,
            parent_id,
            &target.url,
            target.content_hash.as_deref(),
            target.content_type.as_deref(),
            None,
        )
    }

    /// The most recent version of a document, if any.
    pub fn latest_work_version(
        &self,
        document_id: &str,
    ) -> Result<Option<WorkDocumentVersion>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT * FROM work_document_versions
             WHERE document_id = ?1 ORDER BY version_number DESC LIMIT 1",
            params![document_id],
            row_to_version,
        )
        .optional()
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// Append a new version to a document and advance the container's latest
    /// pointer. Version numbers are 1-based and contiguous (UNIQUE guards dups).
    pub fn add_work_version(
        &self,
        document_id: &str,
        parent_version_id: Option<&str>,
        url: &str,
        content_hash: Option<&str>,
        content_type: Option<&str>,
        message_id: Option<&str>,
    ) -> Result<WorkDocumentVersion, NeboError> {
        let conn = self.conn()?;
        let next: i64 = conn
            .query_row(
                "SELECT latest_version + 1 FROM work_documents WHERE id = ?1",
                params![document_id],
                |r| r.get(0),
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let id = uuid::Uuid::new_v4().to_string();
        let version = conn
            .query_row(
                "INSERT INTO work_document_versions
                   (id, document_id, version_number, parent_version_id, url,
                    content_hash, content_type, message_id, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, unixepoch()) RETURNING *",
                params![
                    id,
                    document_id,
                    next,
                    parent_version_id,
                    url,
                    content_hash,
                    content_type,
                    message_id
                ],
                row_to_version,
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        conn.execute(
            "UPDATE work_documents SET latest_version = ?2, updated_at = unixepoch() WHERE id = ?1",
            params![document_id, next],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(version)
    }

    /// The account-wide document index, newest first: every work document with
    /// its latest version's URL and the owning chat's title/session. This is
    /// the read side the Library pulls through the tunnel — the Work panel is
    /// a per-thread view; this is the only cross-chat list.
    pub fn list_work_documents(
        &self,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<WorkDocumentListing>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT d.id, d.chat_id, d.filename, d.kind, d.latest_version,
                        v.url, v.content_type,
                        c.title AS chat_title, c.session_name,
                        d.created_at, d.updated_at
                 FROM work_documents d
                 JOIN work_document_versions v
                   ON v.document_id = d.id AND v.version_number = d.latest_version
                 LEFT JOIN chats c ON c.id = d.chat_id
                 ORDER BY d.updated_at DESC
                 LIMIT ?1 OFFSET ?2",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![limit, offset], |row| {
                Ok(WorkDocumentListing {
                    id: row.get("id")?,
                    chat_id: row.get("chat_id")?,
                    filename: row.get("filename")?,
                    kind: row.get("kind")?,
                    latest_version: row.get("latest_version")?,
                    url: row.get("url")?,
                    content_type: row.get("content_type")?,
                    chat_title: row.get("chat_title")?,
                    session_name: row.get("session_name")?,
                    created_at: row.get("created_at")?,
                    updated_at: row.get("updated_at")?,
                })
            })
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// One row of the document index by document id — the standalone /work
    /// viewer's lookup (same JOIN as [`Self::list_work_documents`]).
    pub fn get_work_document_listing(
        &self,
        id: &str,
    ) -> Result<Option<WorkDocumentListing>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT d.id, d.chat_id, d.filename, d.kind, d.latest_version,
                    v.url, v.content_type,
                    c.title AS chat_title, c.session_name,
                    d.created_at, d.updated_at
             FROM work_documents d
             JOIN work_document_versions v
               ON v.document_id = d.id AND v.version_number = d.latest_version
             LEFT JOIN chats c ON c.id = d.chat_id
             WHERE d.id = ?1",
            params![id],
            |row| {
                Ok(WorkDocumentListing {
                    id: row.get("id")?,
                    chat_id: row.get("chat_id")?,
                    filename: row.get("filename")?,
                    kind: row.get("kind")?,
                    latest_version: row.get("latest_version")?,
                    url: row.get("url")?,
                    content_type: row.get("content_type")?,
                    chat_title: row.get("chat_title")?,
                    session_name: row.get("session_name")?,
                    created_at: row.get("created_at")?,
                    updated_at: row.get("updated_at")?,
                })
            },
        )
        .optional()
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// Register a content-addressed blob (idempotent). The bytes themselves live
    /// on disk at <data_dir>/files/work/blobs/<hash>.<ext>; this is the registry
    /// many versions dedup against.
    pub fn register_content_blob(
        &self,
        hash: &str,
        ext: &str,
        size_bytes: i64,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT OR IGNORE INTO work_content_blobs (hash, ext, size_bytes, created_at)
             VALUES (?1, ?2, ?3, unixepoch())",
            params![hash, ext, size_bytes],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// All versions of a document, oldest first.
    pub fn list_work_versions(
        &self,
        document_id: &str,
    ) -> Result<Vec<WorkDocumentVersion>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT * FROM work_document_versions
                 WHERE document_id = ?1 ORDER BY version_number ASC",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![document_id], row_to_version)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use crate::Store;

    fn temp_store() -> Store {
        let path = std::env::temp_dir().join(format!(
            "nebo-work-test-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        Store::new(path.to_str().unwrap()).unwrap()
    }

    #[test]
    fn test_work_document_version_chain() {
        let store = temp_store();
        store.create_chat("c1", "Test").unwrap();

        // get-or-create container; no versions yet
        let doc = store
            .upsert_work_document("c1", "report.html", "document")
            .unwrap();
        assert_eq!(doc.latest_version, 0);
        assert!(store.latest_work_version(&doc.id).unwrap().is_none());

        // v1
        let v1 = store
            .add_work_version(&doc.id, None, "/api/v1/files/work/x/v1/report.html", Some("hashA"), None, None)
            .unwrap();
        assert_eq!(v1.version_number, 1);

        // upsert returns the SAME container with the advanced pointer
        let doc2 = store
            .upsert_work_document("c1", "report.html", "document")
            .unwrap();
        assert_eq!(doc2.id, doc.id);
        assert_eq!(doc2.latest_version, 1);

        // v2 links to v1; latest pointer + content_hash advance
        let v2 = store
            .add_work_version(&doc2.id, Some(&v1.id), "/api/v1/files/work/x/v2/report.html", Some("hashB"), None, None)
            .unwrap();
        assert_eq!(v2.version_number, 2);
        assert_eq!(v2.parent_version_id.as_deref(), Some(v1.id.as_str()));
        let latest = store.latest_work_version(&doc.id).unwrap().unwrap();
        assert_eq!(latest.version_number, 2);
        assert_eq!(latest.content_hash.as_deref(), Some("hashB"));

        // history is oldest-first
        let all = store.list_work_versions(&doc.id).unwrap();
        assert_eq!(all.iter().map(|v| v.version_number).collect::<Vec<_>>(), vec![1, 2]);

        // a different filename is a different container
        let other = store
            .upsert_work_document("c1", "data.csv", "table")
            .unwrap();
        assert_ne!(other.id, doc.id);

        // content-blob registry is idempotent (migration 0107 applied)
        store.register_content_blob("deadbeef", "html", 100).unwrap();
        store.register_content_blob("deadbeef", "html", 100).unwrap();

        // restore v1 → appends v3 with v1's content/url, parented off latest (v2)
        let restored = store.restore_work_version(&doc.id, 1).unwrap();
        assert_eq!(restored.version_number, 3);
        assert_eq!(restored.url, v1.url);
        assert_eq!(restored.content_hash.as_deref(), Some("hashA"));
        assert_eq!(restored.parent_version_id.as_deref(), Some(v2.id.as_str()));
    }

    #[test]
    fn test_list_work_documents_index() {
        let store = temp_store();
        store.create_chat("c1", "Alpha chat").unwrap();
        store.create_chat("c2", "Beta chat").unwrap();

        let d1 = store.upsert_work_document("c1", "report.md", "document").unwrap();
        store
            .add_work_version(&d1.id, None, "/api/v1/files/work/blobs/a.md", Some("hA"), Some("text/markdown"), None)
            .unwrap();
        let d2 = store.upsert_work_document("c2", "data.csv", "table").unwrap();
        store
            .add_work_version(&d2.id, None, "/api/v1/files/work/blobs/b.csv", Some("hB"), Some("text/csv"), None)
            .unwrap();
        store
            .add_work_version(&d2.id, None, "/api/v1/files/work/blobs/c.csv", Some("hC"), Some("text/csv"), None)
            .unwrap();

        let all = store.list_work_documents(50, 0).unwrap();
        assert_eq!(all.len(), 2);
        // Each row carries the LATEST version's url + the chat title.
        let row2 = all.iter().find(|r| r.id == d2.id).unwrap();
        assert_eq!(row2.latest_version, 2);
        assert_eq!(row2.url, "/api/v1/files/work/blobs/c.csv");
        assert_eq!(row2.chat_title.as_deref(), Some("Beta chat"));
        let row1 = all.iter().find(|r| r.id == d1.id).unwrap();
        assert_eq!(row1.url, "/api/v1/files/work/blobs/a.md");

        // Pagination applies.
        assert_eq!(store.list_work_documents(1, 0).unwrap().len(), 1);
        assert_eq!(store.list_work_documents(50, 2).unwrap().len(), 0);

        // A container with no versions yet never appears (JOIN, not LEFT).
        store.upsert_work_document("c1", "empty.md", "document").unwrap();
        assert_eq!(store.list_work_documents(50, 0).unwrap().len(), 2);
    }
}
