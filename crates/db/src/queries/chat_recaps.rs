use rusqlite::params;

use crate::models::ChatRecap;
use crate::{DbErrExt, OptionalExt, Store};
use types::NeboError;

fn row_to_recap(row: &rusqlite::Row) -> rusqlite::Result<ChatRecap> {
    Ok(ChatRecap {
        chat_id: row.get(0)?,
        turn_id: row.get(1)?,
        text: row.get(2)?,
        created_at: row.get(3)?,
    })
}

impl Store {
    /// Store the owner recap written for one turn. One row per
    /// `(chat_id, turn_id)`; a retry overwrites the same row rather than
    /// duplicating it.
    pub fn write_chat_recap(&self, chat_id: &str, turn_id: &str, text: &str) -> Result<ChatRecap, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "INSERT INTO chat_recaps (chat_id, turn_id, text, created_at)
             VALUES (?1, ?2, ?3, unixepoch())
             ON CONFLICT (chat_id, turn_id) DO UPDATE SET text = excluded.text, created_at = excluded.created_at
             RETURNING chat_id, turn_id, text, created_at",
            params![chat_id, turn_id, text],
            row_to_recap,
        )
        .db_err("chat_recaps")
    }

    /// The recap written for one turn, if any.
    pub fn get_chat_recap(&self, chat_id: &str, turn_id: &str) -> Result<Option<ChatRecap>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT chat_id, turn_id, text, created_at FROM chat_recaps
             WHERE chat_id = ?1 AND turn_id = ?2",
            params![chat_id, turn_id],
            row_to_recap,
        )
        .optional()
        .db_err("chat_recaps")
    }

    /// The most recent recap written for a chat — what the thread shows the
    /// owner coming back to it. Ties on `created_at` (same-second writes)
    /// break on `rowid`, the insert order.
    pub fn latest_chat_recap(&self, chat_id: &str) -> Result<Option<ChatRecap>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT chat_id, turn_id, text, created_at FROM chat_recaps
             WHERE chat_id = ?1 ORDER BY created_at DESC, rowid DESC LIMIT 1",
            params![chat_id],
            row_to_recap,
        )
        .optional()
        .db_err("chat_recaps")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nebo-chat-recaps-test.db");
        let store = Store::new(&path.to_string_lossy()).expect("store");
        (dir, store)
    }

    fn seed_chat(store: &Store, id: &str) {
        store.create_chat(id, "Test chat").expect("create chat");
    }

    #[test]
    fn write_then_get_round_trips() {
        let (_dir, store) = store();
        seed_chat(&store, "chat-1");

        let written = store.write_chat_recap("chat-1", "turn-1", "Recap text").unwrap();
        assert_eq!(written.chat_id, "chat-1");
        assert_eq!(written.turn_id, "turn-1");
        assert_eq!(written.text, "Recap text");

        let fetched = store.get_chat_recap("chat-1", "turn-1").unwrap();
        assert_eq!(fetched.unwrap().text, "Recap text");
    }

    #[test]
    fn write_is_idempotent_on_retry() {
        let (_dir, store) = store();
        seed_chat(&store, "chat-1");

        store.write_chat_recap("chat-1", "turn-1", "First").unwrap();
        store.write_chat_recap("chat-1", "turn-1", "Second").unwrap();

        // One row, last write wins — a retry never duplicates the recap.
        let conn = store.conn().unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM chat_recaps WHERE chat_id = 'chat-1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(store.get_chat_recap("chat-1", "turn-1").unwrap().unwrap().text, "Second");
    }

    #[test]
    fn latest_recap_is_the_most_recent_turn() {
        let (_dir, store) = store();
        seed_chat(&store, "chat-1");

        store.write_chat_recap("chat-1", "turn-1", "Older").unwrap();
        store.write_chat_recap("chat-1", "turn-2", "Newer").unwrap();

        assert_eq!(store.latest_chat_recap("chat-1").unwrap().unwrap().text, "Newer");
    }

    #[test]
    fn no_recap_yet_is_none() {
        let (_dir, store) = store();
        seed_chat(&store, "chat-1");
        assert!(store.get_chat_recap("chat-1", "turn-1").unwrap().is_none());
        assert!(store.latest_chat_recap("chat-1").unwrap().is_none());
    }
}
