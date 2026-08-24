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

#[cfg(test)]
mod deletion_memory_tests {
    use crate::Store;

    fn store() -> Store {
        let path = std::env::temp_dir()
            .join(format!("nebo-delmem-test-{}.db", uuid::Uuid::new_v4()));
        Store::new(&path.to_string_lossy()).expect("store")
    }

    /// A deleted employee's memory scopes vanish ENTIRELY — including the
    /// isolation-context scopes the old base-only pattern left alive.
    #[test]
    fn agent_delete_purges_ctx_scopes_too() {
        let s = store();
        for (scope, key) in [
            ("owner:agent:emp1", "a"),
            ("owner:agent:emp1:ctx:matter-1", "b"),
            ("owner:agent:emp1:ctx:matter-2", "c"),
            ("owner:agent:other", "keep"),
            ("owner", "keep2"),
        ] {
            s.upsert_memory("ns", key, "v", None, None, scope).expect("set");
        }
        s.delete_agent_memories("emp1").expect("purge");
        let mut left: Vec<String> = s
            .list_memories(100, 0)
            .unwrap()
            .into_iter()
            .map(|m| m.key)
            .collect();
        left.sort();
        assert_eq!(left, vec!["keep", "keep2"], "only non-emp1 scopes survive");
    }

    /// Mentions elsewhere are tombstoned (never destroyed), idempotently.
    #[test]
    fn mentions_get_one_tombstone() {
        let s = store();
        s.upsert_memory("project", "hvac-status", "All five Dispatcher agents are active", None, None, "owner")
            .expect("set");
        s.upsert_memory("project", "unrelated", "the sky is blue", None, None, "owner")
            .expect("set");
        let note = "NOTE: 'Dispatcher' was deleted on 2026-08-23; it no longer exists — do not report it as active";
        assert_eq!(s.tombstone_memories_mentioning("Dispatcher", note).unwrap(), 1);
        // Idempotent: the sweep never stacks a second note.
        assert_eq!(s.tombstone_memories_mentioning("Dispatcher", note).unwrap(), 0);
        // Short names never sweep — substring carnage guard.
        assert_eq!(s.tombstone_memories_mentioning("sky", note).unwrap(), 0);
        let mems = s.list_memories(100, 0).unwrap();
        let v = &mems.iter().find(|m| m.key == "hvac-status").unwrap().value;
        assert!(v.contains("do not report it as active"), "{v}");
        let u = &mems.iter().find(|m| m.key == "unrelated").unwrap().value;
        assert_eq!(u, "the sky is blue");
    }
}
