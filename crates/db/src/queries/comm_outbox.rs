use rusqlite::params;
use types::NeboError;

use crate::Store;

// ponytail: the outbox's ceiling is 200 messages and 24 hours. A message
// older than a day, or beyond the 200 newest, is dropped as undeliverable
// when the outbox is read. A reconnect's backlog drains at the hub's
// sustained send rate (one a second), so 200 is a few minutes of catch-up;
// raise both only with a reason.
pub const COMM_OUTBOX_MAX_MESSAGES: i64 = 200;
pub const COMM_OUTBOX_MAX_AGE_SECS: i64 = 24 * 60 * 60;

impl Store {
    /// Keep an outbound message until it is sent. Writing the same id again
    /// keeps the first copy and its place in line.
    pub fn put_comm_outbox(&self, id: &str, message: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT OR IGNORE INTO comm_outbox (id, message) VALUES (?1, ?2)",
            params![id, message],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// What waits to be sent, oldest first, after dropping what is past the
    /// outbox's ceiling. Returns the waiting `(id, message, handed_off_at)`
    /// rows and how many were dropped.
    #[allow(clippy::type_complexity)]
    pub fn pending_comm_outbox(
        &self,
    ) -> Result<(Vec<(String, String, Option<i64>)>, usize), NeboError> {
        let conn = self.conn()?;
        let expired = conn
            .execute(
                "DELETE FROM comm_outbox WHERE created_at < unixepoch() - ?1",
                params![COMM_OUTBOX_MAX_AGE_SECS],
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let overflow = conn
            .execute(
                "DELETE FROM comm_outbox WHERE seq NOT IN
                 (SELECT seq FROM comm_outbox ORDER BY seq DESC LIMIT ?1)",
                params![COMM_OUTBOX_MAX_MESSAGES],
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let mut stmt = conn
            .prepare("SELECT id, message, handed_off_at FROM comm_outbox ORDER BY seq")
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .map_err(|e| NeboError::Database(e.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok((rows, expired + overflow))
    }

    /// Mark a message handed to a connection at `at` (unix ms), or clear the
    /// mark when it never left.
    pub fn hand_off_comm_outbox(&self, id: &str, at: Option<i64>) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE comm_outbox SET handed_off_at = ?2 WHERE id = ?1",
            params![id, at],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Forget a message that was sent or refused for good.
    pub fn remove_comm_outbox(&self, id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM comm_outbox WHERE id = ?1", params![id])
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::Store;

    /// What waits survives the process: a store reopened on the same file
    /// hands the messages back in the order they were written.
    #[test]
    fn the_outbox_survives_a_restart_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nebo.db");
        {
            let store = Store::new(&path.to_string_lossy()).unwrap();
            for id in ["m3", "m1", "m2"] {
                store.put_comm_outbox(id, &format!("{{\"id\":\"{id}\"}}")).unwrap();
            }
            store.remove_comm_outbox("m1").unwrap();
            store.hand_off_comm_outbox("m2", Some(42)).unwrap();
        }
        let store = Store::new(&path.to_string_lossy()).unwrap();
        let (rows, dropped) = store.pending_comm_outbox().unwrap();
        let ids: Vec<(&str, Option<i64>)> = rows.iter().map(|(id, _, at)| (id.as_str(), *at)).collect();
        assert_eq!(ids, [("m3", None), ("m2", Some(42))], "the hand-off mark survives too");
        assert_eq!(dropped, 0);
    }

    /// A second write of a waiting message keeps its place in line.
    #[test]
    fn a_rewrite_keeps_its_place() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(&dir.path().join("nebo.db").to_string_lossy()).unwrap();
        store.put_comm_outbox("a", "1").unwrap();
        store.put_comm_outbox("b", "2").unwrap();
        store.put_comm_outbox("a", "1").unwrap();
        let (rows, _) = store.pending_comm_outbox().unwrap();
        assert_eq!(rows, [("a".into(), "1".into(), None), ("b".into(), "2".into(), None)]);
    }

    /// The ceiling: past the age or the count, the oldest go.
    #[test]
    fn the_ceiling_drops_the_oldest() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(&dir.path().join("nebo.db").to_string_lossy()).unwrap();
        store.put_comm_outbox("stale", "x").unwrap();
        store
            .conn()
            .unwrap()
            .execute(
                "UPDATE comm_outbox SET created_at = unixepoch() - ?1 WHERE id = 'stale'",
                [super::COMM_OUTBOX_MAX_AGE_SECS + 1],
            )
            .unwrap();
        for i in 0..super::COMM_OUTBOX_MAX_MESSAGES + 1 {
            store.put_comm_outbox(&format!("m{i}"), "x").unwrap();
        }
        let (rows, dropped) = store.pending_comm_outbox().unwrap();
        assert_eq!(dropped, 2, "the stale one and the oldest over the count");
        assert_eq!(rows.len() as i64, super::COMM_OUTBOX_MAX_MESSAGES);
        assert_eq!(rows[0].0, "m1");
    }
}
