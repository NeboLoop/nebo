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
        prior_content: row.get("prior_content")?,
        status: row.get("status")?,
        created_at: row.get("created_at")?,
        resolved_at: row.get("resolved_at")?,
    })
}

impl Store {
    #[allow(clippy::too_many_arguments)]
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
        prior_content: Option<&str>,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO pending_writes (id, agent_id, kind, action, target, content, gist, target_hash, prior_content)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![id, agent_id, kind, action, target, content, gist, target_hash, prior_content],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Create a pending write that is already applied (auto-mode audit row).
    /// Same shape as `create_pending_write` but lands with status 'approved'
    /// so it becomes a revert anchor immediately — the auto-mode counterpart to
    /// the staged create + approve flow.
    #[allow(clippy::too_many_arguments)]
    pub fn record_applied_write(
        &self,
        id: &str,
        agent_id: &str,
        kind: &str,
        action: &str,
        target: &str,
        content: Option<&str>,
        gist: &str,
        target_hash: &str,
        prior_content: Option<&str>,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO pending_writes (id, agent_id, kind, action, target, content, gist, target_hash, prior_content, status, resolved_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'approved', unixepoch())",
            params![id, agent_id, kind, action, target, content, gist, target_hash, prior_content],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Mark an APPLIED learning as reverted (approved → reverted). Returns false
    /// if the row is missing or not in 'approved' state (a pending, rejected,
    /// conflicted, expired, or already-reverted row cannot be reverted).
    pub fn revert_pending_write(&self, id: &str) -> Result<bool, NeboError> {
        let conn = self.conn()?;
        let n = conn
            .execute(
                "UPDATE pending_writes SET status = 'reverted', resolved_at = unixepoch()
                 WHERE id = ?1 AND status = 'approved'",
                params![id],
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(n > 0)
    }

    /// Whether the agent has any pending_writes row of `kind` created in the
    /// last `within_secs`, regardless of status. Anti-spam gate for periodic
    /// producers like the workflow tuning pass: one proposal per window, even
    /// if the previous one was rejected.
    pub fn has_recent_pending_write(
        &self,
        agent_id: &str,
        kind: &str,
        within_secs: i64,
    ) -> Result<bool, NeboError> {
        let conn = self.conn()?;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pending_writes
                 WHERE agent_id = ?1 AND kind = ?2 AND created_at > unixepoch() - ?3",
                params![agent_id, kind, within_secs],
                |row| row.get(0),
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(count > 0)
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

#[cfg(test)]
mod tests {
    use crate::Store;

    fn temp_store() -> Store {
        let path = std::env::temp_dir().join(format!(
            "nebo-pending-writes-test-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        Store::new(path.to_str().unwrap()).unwrap()
    }

    #[test]
    fn prior_content_round_trips() {
        let s = temp_store();
        s.create_pending_write(
            "p1", "agent-a", "skill", "update", "my-skill",
            Some("new body"), "Update my-skill", "abc123", Some("old body"),
        )
        .unwrap();
        let row = s.get_pending_write("p1").unwrap().unwrap();
        assert_eq!(row.prior_content.as_deref(), Some("old body"));
        assert_eq!(row.content.as_deref(), Some("new body"));
        assert_eq!(row.status, "pending");
    }

    #[test]
    fn record_applied_write_lands_approved() {
        let s = temp_store();
        s.record_applied_write(
            "p2", "agent-a", "skill", "create", "made", Some("body"),
            "Create made", "", None,
        )
        .unwrap();
        let row = s.get_pending_write("p2").unwrap().unwrap();
        assert_eq!(row.status, "approved");
        assert!(row.resolved_at.is_some());
        assert_eq!(row.prior_content, None);
    }

    #[test]
    fn revert_only_transitions_applied_rows() {
        let s = temp_store();
        // An applied learning can be reverted exactly once.
        s.record_applied_write(
            "p3", "agent-a", "skill", "update", "made", Some("v2"),
            "Update made", "h", Some("v1"),
        )
        .unwrap();
        assert!(s.revert_pending_write("p3").unwrap());
        assert_eq!(s.get_pending_write("p3").unwrap().unwrap().status, "reverted");
        // A second revert is a no-op — it is no longer 'approved'.
        assert!(!s.revert_pending_write("p3").unwrap());

        // A pending (not-yet-applied) row cannot be reverted.
        s.create_pending_write(
            "p4", "agent-a", "skill", "create", "x", Some("b"), "g", "", None,
        )
        .unwrap();
        assert!(!s.revert_pending_write("p4").unwrap());
        assert_eq!(s.get_pending_write("p4").unwrap().unwrap().status, "pending");
    }
}
