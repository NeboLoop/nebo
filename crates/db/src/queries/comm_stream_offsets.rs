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

    /// The harness gate seeds a fresh home with a database holding only this
    /// table (scripts/gate-server.sh, carry_seed); the store must migrate
    /// everything else onto it and keep the offsets.
    #[test]
    fn a_database_seeded_with_only_offsets_migrates_and_keeps_them() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nebo.db");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS comm_stream_offsets (
                    bot_id TEXT NOT NULL, stream TEXT NOT NULL, acked_seq INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL DEFAULT (unixepoch()), PRIMARY KEY (bot_id, stream));
                 INSERT INTO comm_stream_offsets (bot_id, stream, acked_seq) VALUES ('bot', 'installs', 10193);",
            )
            .unwrap();
        }
        let store = Store::new(path.to_str().unwrap()).unwrap();
        assert_eq!(store.comm_stream_offset("bot", "installs").unwrap(), 10193);
        store.record_comm_stream_offset("bot", "installs", 10200).unwrap();
        assert_eq!(store.comm_stream_offset("bot", "installs").unwrap(), 10200);
    }

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
