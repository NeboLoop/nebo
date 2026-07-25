//! Staged self-improvement writes (learned skills awaiting owner approval).
//! See migration 0123 and docs/design/SELF_IMPROVEMENT.md WS3.

use rusqlite::params;

use crate::models::PendingWrite;
use crate::store::Store;
use types::NeboError;

/// Pending rows older than this are auto-expired (audited Hermes gap: their
/// pending store accumulates forever).
pub const PENDING_WRITE_TTL_SECS: i64 = 30 * 24 * 3600;

fn row_to_pending_write(row: &rusqlite::Row) -> rusqlite::Result<PendingWrite> {
    Ok(PendingWrite {
        id: row.get("id")?,
        agent_id: row.get("agent_id")?,
        kind: row.get("kind")?,
        action: row.get("action")?,
        target: row.get("target")?,
        content: row.get("content")?,
        gist: row.get("gist")?,
        target_hash: row.get("target_hash")?,
        status: row.get("status")?,
        created_at: row.get("created_at")?,
        resolved_at: row.get("resolved_at")?,
    })
}

impl Store {
    pub fn create_pending_write(
        &self,
        id: &str,
        agent_id: &str,
        kind: &str,
        action: &str,
        target: &str,
        content: Option<&str>,
        gist: &str,
        target_hash: &str,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO pending_writes (id, agent_id, kind, action, target, content, gist, target_hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![id, agent_id, kind, action, target, content, gist, target_hash],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn get_pending_write(&self, id: &str) -> Result<Option<PendingWrite>, NeboError> {
        let conn = self.conn()?;
        match conn.query_row(
            "SELECT * FROM pending_writes WHERE id = ?1",
            params![id],
            row_to_pending_write,
        ) {
            Ok(p) => Ok(Some(p)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(NeboError::Database(e.to_string())),
        }
    }

    /// Resolve a pending write: status must currently be 'pending' (first
    /// resolution wins). Returns false if the row was already resolved/missing.
    pub fn resolve_pending_write(&self, id: &str, status: &str) -> Result<bool, NeboError> {
        let conn = self.conn()?;
        let n = conn
            .execute(
                "UPDATE pending_writes SET status = ?1, resolved_at = unixepoch()
                 WHERE id = ?2 AND status = 'pending'",
                params![status, id],
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(n > 0)
    }

    /// Expire pending rows past the TTL. Returns the ids expired so the
    /// caller can clean up their Inbox notifications.
    pub fn expire_pending_writes(&self) -> Result<Vec<String>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id FROM pending_writes
                 WHERE status = 'pending' AND created_at < unixepoch() - ?1",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let ids: Vec<String> = stmt
            .query_map(params![PENDING_WRITE_TTL_SECS], |r| r.get(0))
            .map_err(|e| NeboError::Database(e.to_string()))?
            .filter_map(|r| r.ok())
            .collect();
        if !ids.is_empty() {
            conn.execute(
                "UPDATE pending_writes SET status = 'expired', resolved_at = unixepoch()
                 WHERE status = 'pending' AND created_at < unixepoch() - ?1",
                params![PENDING_WRITE_TTL_SECS],
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        }
        Ok(ids)
    }
}
