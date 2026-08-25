use rusqlite::params;

use crate::Store;
use types::NeboError;

/// One pending wake for a sleeping session (session wake rail, R1).
#[derive(Debug, Clone)]
pub struct SessionWake {
    pub id: i64,
    pub session_key: String,
    pub kind: String,
    pub payload: String,
    /// JSON array of `ProvenanceClass` strings — the payload's taint.
    pub provenance: String,
    pub handoff_depth: u8,
    pub attempts: i64,
}

/// A wake is poisoned (stamped failed, never retried) after this many
/// delivery attempts. Claiming counts as the attempt, and a busy-claim whose
/// run ends before draining re-claims later — live coalescing runs showed
/// healthy payloads reaching 3 claims from that churn alone, so the ceiling
/// leaves margin above it.
pub const WAKE_MAX_ATTEMPTS: i64 = 5;

impl Store {
    /// Write-ahead: persist the wake BEFORE any delivery attempt.
    pub fn enqueue_session_wake(
        &self,
        session_key: &str,
        kind: &str,
        payload: &str,
        provenance: &str,
        handoff_depth: u8,
    ) -> Result<i64, NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO session_wakes (session_key, kind, payload, provenance, handoff_depth)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![session_key, kind, payload, provenance, handoff_depth as i64],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(conn.last_insert_rowid())
    }

    /// Undelivered wakes for one session, FIFO, attempts bumped in the same
    /// statement — claiming IS the attempt, so a delivery that crashes the
    /// process still counts toward the poison threshold on the next sweep.
    /// Rows that cross the threshold are stamped failed (not returned); the
    /// second tuple element is how many were poisoned this claim — a wake may
    /// fail loudly, never loop silently.
    pub fn claim_session_wakes(
        &self,
        session_key: &str,
    ) -> Result<(Vec<SessionWake>, usize), NeboError> {
        let conn = self.conn()?;
        let poisoned = conn.execute(
            "UPDATE session_wakes
             SET delivered_at = unixepoch(),
                 note = 'poisoned: exceeded delivery attempts'
             WHERE session_key = ?1 AND delivered_at IS NULL AND attempts >= ?2",
            params![session_key, WAKE_MAX_ATTEMPTS],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        let mut stmt = conn
            .prepare(
                "UPDATE session_wakes SET attempts = attempts + 1
                 WHERE session_key = ?1 AND delivered_at IS NULL
                 RETURNING id, session_key, kind, payload, provenance, handoff_depth, attempts",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![session_key], |r| {
                Ok(SessionWake {
                    id: r.get(0)?,
                    session_key: r.get(1)?,
                    kind: r.get(2)?,
                    payload: r.get(3)?,
                    provenance: r.get(4)?,
                    handoff_depth: r.get::<_, i64>(5)?.min(u8::MAX as i64) as u8,
                    attempts: r.get(6)?,
                })
            })
            .map_err(|e| NeboError::Database(e.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok((rows, poisoned))
    }

    /// Stamp a batch delivered — the woken run carried these payloads.
    pub fn mark_session_wakes_delivered(&self, ids: &[i64]) -> Result<(), NeboError> {
        let conn = self.conn()?;
        for id in ids {
            conn.execute(
                "UPDATE session_wakes SET delivered_at = unixepoch() WHERE id = ?1",
                params![id],
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        }
        Ok(())
    }

    /// Sessions with undelivered, un-poisoned wakes — the boot sweep's worklist.
    pub fn sessions_with_pending_wakes(&self) -> Result<Vec<String>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT DISTINCT session_key FROM session_wakes
                 WHERE delivered_at IS NULL AND attempts < ?1",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![WAKE_MAX_ATTEMPTS], |r| r.get(0))
            .map_err(|e| NeboError::Database(e.to_string()))?
            .collect::<Result<Vec<String>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use crate::Store;

    fn store() -> Store {
        let path = std::env::temp_dir().join(format!("nebo-wakes-test-{}.db", uuid::Uuid::new_v4()));
        Store::new(&path.to_string_lossy()).expect("store")
    }

    #[test]
    fn fifo_claim_and_stamp() {
        let s = store();
        let a = s.enqueue_session_wake("agent:x:web", "coworker_reply", "first", "[]", 1).unwrap();
        let b = s.enqueue_session_wake("agent:x:web", "coworker_reply", "second", "[\"coworker\"]", 1).unwrap();
        s.enqueue_session_wake("agent:y:web", "task_done", "other", "[]", 0).unwrap();

        let (claimed, poisoned) = s.claim_session_wakes("agent:x:web").unwrap();
        assert_eq!(poisoned, 0);
        assert_eq!(claimed.iter().map(|w| w.id).collect::<Vec<_>>(), vec![a, b], "FIFO per session");
        assert_eq!(claimed[0].payload, "first");
        assert_eq!(claimed[1].provenance, "[\"coworker\"]");
        assert_eq!(claimed[0].attempts, 1, "claiming counts as the attempt");

        s.mark_session_wakes_delivered(&[a, b]).unwrap();
        assert!(s.claim_session_wakes("agent:x:web").unwrap().0.is_empty(), "delivered = gone");
        assert_eq!(s.sessions_with_pending_wakes().unwrap(), vec!["agent:y:web"]);
    }

    #[test]
    fn poison_after_max_attempts() {
        let s = store();
        s.enqueue_session_wake("agent:z:web", "coworker_reply", "cursed", "[]", 0).unwrap();
        for round in 1..=super::WAKE_MAX_ATTEMPTS {
            let (claimed, _) = s.claim_session_wakes("agent:z:web").unwrap();
            assert_eq!(claimed.len(), 1, "round {round} still claimable");
            // Delivery fails — never stamped.
        }
        // Next sweep: over the threshold, stamped poisoned, never returned.
        let (claimed, poisoned) = s.claim_session_wakes("agent:z:web").unwrap();
        assert!(claimed.is_empty());
        assert_eq!(poisoned, 1, "the failure is counted, never silent");
        assert!(s.sessions_with_pending_wakes().unwrap().is_empty(), "poisoned wakes leave the worklist");
    }
}
