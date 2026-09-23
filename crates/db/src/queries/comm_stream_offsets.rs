use rusqlite::params;
use types::NeboError;

use crate::Store;

impl Store {
    /// Last seq acked on one of the bot's hub streams; 0 when it never acked
    /// one (the hub then replays nothing, as for a bot that predates offsets).
    pub fn comm_stream_offset(&self, bot_id: &str, stream: &str) -> Result<u64, NeboError> {
        let conn = self.conn()?;
        match conn.query_row(
            "SELECT acked_seq FROM comm_stream_offsets WHERE bot_id = ?1 AND stream = ?2",
            params![bot_id, stream],
            |row| row.get::<_, i64>(0),
        ) {
            Ok(seq) => Ok(seq.max(0) as u64),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(0),
            Err(e) => Err(NeboError::Database(e.to_string())),
        }
    }

    /// Record an acked seq on one of the bot's hub streams. Monotonic: a
    /// late or replayed delivery never moves the offset backwards.
    pub fn record_comm_stream_offset(
        &self,
        bot_id: &str,
        stream: &str,
        seq: u64,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO comm_stream_offsets (bot_id, stream, acked_seq, updated_at)
             VALUES (?1, ?2, ?3, unixepoch())
             ON CONFLICT(bot_id, stream) DO UPDATE
             SET acked_seq = MAX(acked_seq, excluded.acked_seq), updated_at = unixepoch()",
            params![bot_id, stream, seq as i64],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::Store;

    #[test]
    fn offsets_persist_per_bot_and_stream_and_never_regress() {
        let path = std::env::temp_dir().join(format!(
            "nebo-comm-offsets-test-{}.db",
            uuid::Uuid::new_v4()
        ));
        let store = Store::new(&path.to_string_lossy()).expect("store");

        assert_eq!(store.comm_stream_offset("bot-a", "chat").unwrap(), 0);
        store.record_comm_stream_offset("bot-a", "chat", 7).unwrap();
        store.record_comm_stream_offset("bot-a", "chat", 5).unwrap(); // late ack
        store
            .record_comm_stream_offset("bot-a", "installs", 3)
            .unwrap();
        assert_eq!(store.comm_stream_offset("bot-a", "chat").unwrap(), 7);
        assert_eq!(store.comm_stream_offset("bot-a", "installs").unwrap(), 3);
        assert_eq!(store.comm_stream_offset("bot-b", "chat").unwrap(), 0);

        // Survives a reopen — what the next process start reads.
        drop(store);
        let reopened = Store::new(&path.to_string_lossy()).expect("reopen");
        assert_eq!(reopened.comm_stream_offset("bot-a", "chat").unwrap(), 7);
    }
}
