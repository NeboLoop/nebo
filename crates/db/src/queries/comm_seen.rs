use rusqlite::params;

use crate::Store;
use types::NeboError;

impl Store {
    /// Durable inbound dedupe: record a hub wire msg_id as processed.
    /// Returns true the FIRST time an id is seen; false on a redelivery.
    /// Also prunes entries past the retention window — the hub's replay
    /// horizon is far shorter, so 14 days is generous.
    pub fn mark_comm_message_seen(&self, msg_id: &str) -> Result<bool, NeboError> {
        let conn = self.conn()?;
        let inserted = conn
            .execute(
                "INSERT OR IGNORE INTO comm_seen_messages (id) VALUES (?1)",
                params![msg_id],
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let _ = conn.execute(
            "DELETE FROM comm_seen_messages WHERE seen_at < (unixepoch() - 14*86400)",
            [],
        );
        Ok(inserted == 1)
    }
}

#[cfg(test)]
mod tests {
    use crate::Store;

    #[test]
    fn first_seen_true_redelivery_false() {
        let path = std::env::temp_dir()
            .join(format!("nebo-comm-seen-test-{}.db", uuid::Uuid::new_v4()));
        let store = Store::new(&path.to_string_lossy()).expect("store");
        assert!(store.mark_comm_message_seen("m-1").unwrap());
        // The redelivery — same wire id — must be recognized forever after,
        // across what would be reconnects and restarts in production.
        assert!(!store.mark_comm_message_seen("m-1").unwrap());
        assert!(store.mark_comm_message_seen("m-2").unwrap());
    }
}
