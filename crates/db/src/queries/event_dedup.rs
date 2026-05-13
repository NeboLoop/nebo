use rusqlite::params;

use crate::Store;
use types::NeboError;

impl Store {
    /// Check if an event fingerprint was seen within the TTL window.
    pub fn check_event_dedup(&self, fingerprint: &str, ttl_secs: i64) -> Result<bool, NeboError> {
        let conn = self.conn()?;
        let exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM event_dedup
                 WHERE fingerprint = ?1 AND created_at > (unixepoch() - ?2)",
                params![fingerprint, ttl_secs],
                |row| row.get(0),
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(exists)
    }

    /// Record an event fingerprint for dedup tracking.
    pub fn record_event_dedup(&self, fingerprint: &str, source: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT OR REPLACE INTO event_dedup (fingerprint, source, created_at)
             VALUES (?1, ?2, unixepoch())",
            params![fingerprint, source],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Delete expired dedup entries older than TTL.
    pub fn cleanup_event_dedup(&self, ttl_secs: i64) -> Result<u64, NeboError> {
        let conn = self.conn()?;
        let deleted = conn
            .execute(
                "DELETE FROM event_dedup WHERE created_at < (unixepoch() - ?1)",
                params![ttl_secs],
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(deleted as u64)
    }
}
