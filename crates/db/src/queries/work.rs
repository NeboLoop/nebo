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
        let conn = self.conn()?;
        if let Some(doc) = conn
            .query_row(
                "SELECT * FROM work_documents WHERE chat_id = ?1 AND filename = ?2",
                params![chat_id, filename],
                row_to_document,
            )
            .optional()
            .map_err(|e| NeboError::Database(e.to_string()))?
        {
            return Ok(doc);
        }
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
    }
}
