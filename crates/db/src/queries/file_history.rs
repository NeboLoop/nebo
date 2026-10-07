//! The owner's workspace index and its files' earlier content
//! (`tools::workspace_history` keeps both).

use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::Store;
use types::NeboError;

/// A workspace file as it was at the last look.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceIndexRow {
    /// Relative to `<data_dir>/files`, '/'-separated.
    pub path: String,
    pub size_bytes: i64,
    pub mtime_ns: i64,
    pub hash: String,
    pub ext: String,
}

/// One earlier content of a workspace file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileHistoryEntry {
    pub id: i64,
    pub path: String,
    pub hash: String,
    pub ext: String,
    pub size_bytes: i64,
    /// `modified` or `deleted`.
    pub reason: String,
    pub chat_id: Option<String>,
    pub captured_at: i64,
}

/// An earlier content to keep: what `path` was before it changed or went.
#[derive(Debug, Clone)]
pub struct NewFileHistory {
    pub path: String,
    pub hash: String,
    pub ext: String,
    pub size_bytes: i64,
    pub reason: &'static str,
}

fn row_to_entry(row: &rusqlite::Row) -> rusqlite::Result<FileHistoryEntry> {
    Ok(FileHistoryEntry {
        id: row.get("id")?,
        path: row.get("path")?,
        hash: row.get("hash")?,
        ext: row.get("ext")?,
        size_bytes: row.get("size_bytes")?,
        reason: row.get("reason")?,
        chat_id: row.get("chat_id")?,
        captured_at: row.get("captured_at")?,
    })
}

fn db_err(e: rusqlite::Error) -> NeboError {
    NeboError::Database(e.to_string())
}

impl Store {
    /// The whole workspace index.
    pub fn workspace_index(&self) -> Result<Vec<WorkspaceIndexRow>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare("SELECT path, size_bytes, mtime_ns, hash, ext FROM workspace_index")
            .map_err(db_err)?;
        let rows = stmt
            .query_map([], |r| {
                Ok(WorkspaceIndexRow {
                    path: r.get(0)?,
                    size_bytes: r.get(1)?,
                    mtime_ns: r.get(2)?,
                    hash: r.get(3)?,
                    ext: r.get(4)?,
                })
            })
            .map_err(db_err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
    }

    /// What one look at the workspace found, in one transaction: the index
    /// rows to write and to drop, and the earlier contents to keep (seen in
    /// `chat_id`'s turn). An earlier content already kept for its path is
    /// one entry, moved to now.
    pub fn record_workspace_look(
        &self,
        upserts: &[WorkspaceIndexRow],
        removed: &[String],
        history: &[NewFileHistory],
        chat_id: Option<&str>,
    ) -> Result<(), NeboError> {
        if upserts.is_empty() && removed.is_empty() && history.is_empty() {
            return Ok(());
        }
        let mut conn = self.conn()?;
        let tx = conn.transaction().map_err(db_err)?;
        {
            let mut put = tx
                .prepare(
                    "INSERT INTO workspace_index (path, size_bytes, mtime_ns, hash, ext)
                     VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(path) DO UPDATE SET size_bytes = excluded.size_bytes,
                       mtime_ns = excluded.mtime_ns, hash = excluded.hash, ext = excluded.ext",
                )
                .map_err(db_err)?;
            for r in upserts {
                put.execute(params![r.path, r.size_bytes, r.mtime_ns, r.hash, r.ext]).map_err(db_err)?;
            }
            let mut drop = tx.prepare("DELETE FROM workspace_index WHERE path = ?1").map_err(db_err)?;
            for p in removed {
                drop.execute(params![p]).map_err(db_err)?;
            }
            let mut keep = tx
                .prepare(
                    "INSERT INTO file_history (path, hash, ext, size_bytes, reason, chat_id, captured_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, unixepoch())
                     ON CONFLICT(path, hash) DO UPDATE SET reason = excluded.reason,
                       chat_id = excluded.chat_id, captured_at = excluded.captured_at",
                )
                .map_err(db_err)?;
            for h in history {
                keep.execute(params![h.path, h.hash, h.ext, h.size_bytes, h.reason, chat_id]).map_err(db_err)?;
            }
        }
        tx.commit().map_err(db_err)
    }

    /// A file's earlier contents, newest first.
    pub fn list_file_history(&self, path: &str, limit: i64) -> Result<Vec<FileHistoryEntry>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT * FROM file_history WHERE path = ?1
                 ORDER BY captured_at DESC, id DESC LIMIT ?2",
            )
            .map_err(db_err)?;
        let rows = stmt.query_map(params![path, limit], row_to_entry).map_err(db_err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
    }

    /// One earlier content by id.
    pub fn get_file_history(&self, id: i64) -> Result<Option<FileHistoryEntry>, NeboError> {
        let conn = self.conn()?;
        conn.query_row("SELECT * FROM file_history WHERE id = ?1", params![id], row_to_entry)
            .optional()
            .map_err(db_err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("t.db").to_str().unwrap()).unwrap();
        (dir, store)
    }

    fn row(path: &str, hash: &str) -> WorkspaceIndexRow {
        WorkspaceIndexRow { path: path.into(), size_bytes: 1, mtime_ns: 2, hash: hash.into(), ext: "txt".into() }
    }

    fn earlier(path: &str, hash: &str, reason: &'static str) -> NewFileHistory {
        NewFileHistory { path: path.into(), hash: hash.into(), ext: "txt".into(), size_bytes: 1, reason }
    }

    #[test]
    fn a_look_writes_the_index_and_keeps_earlier_content_once() {
        let (_d, store) = temp_store();
        store.record_workspace_look(&[row("a.txt", "h1"), row("b.txt", "h2")], &[], &[], None).unwrap();
        assert_eq!(store.workspace_index().unwrap().len(), 2);

        store
            .record_workspace_look(&[row("a.txt", "h3")], &["b.txt".into()], &[earlier("a.txt", "h1", "modified"), earlier("b.txt", "h2", "deleted")], Some("c1"))
            .unwrap();
        let index = store.workspace_index().unwrap();
        assert_eq!(index, vec![row("a.txt", "h3")]);
        let a = store.list_file_history("a.txt", 10).unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!((a[0].hash.as_str(), a[0].reason.as_str(), a[0].chat_id.as_deref()), ("h1", "modified", Some("c1")));
        assert_eq!(store.list_file_history("b.txt", 10).unwrap()[0].reason, "deleted");

        // The same earlier content seen again is the same entry.
        store.record_workspace_look(&[], &[], &[earlier("a.txt", "h1", "modified")], Some("c2")).unwrap();
        let a = store.list_file_history("a.txt", 10).unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].chat_id.as_deref(), Some("c2"));
        assert_eq!(store.get_file_history(a[0].id).unwrap().unwrap().hash, "h1");
    }
}
