use rusqlite::params;

use crate::Store;
use crate::models::Session;
use types::NeboError;

impl Store {
    pub fn create_session(
        &self,
        id: &str,
        name: Option<&str>,
        scope: Option<&str>,
        scope_id: Option<&str>,
        metadata: Option<&str>,
    ) -> Result<Session, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "INSERT INTO sessions (id, name, scope, scope_id, metadata, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, unixepoch(), unixepoch()) RETURNING *",
            params![id, name, scope, scope_id, metadata],
            row_to_session,
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn get_session(&self, id: &str) -> Result<Option<Session>, NeboError> {
        let conn = self.conn()?;
        conn.query_row("SELECT * FROM sessions WHERE id = ?1", params![id], |row| {
            row_to_session(row)
        })
        .optional()
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// The chat id a session's messages are stored under, if the session
    /// records one: `active_chat_id`, falling back to `name` (legacy
    /// pre-decoupling sessions). Returns `None` when the session row is
    /// missing or carries neither — callers that need a best-effort id use
    /// [`Self::resolve_session_chat_id`], which layers the synthetic
    /// `chat-{session_id}` fallback on top of this ONE derivation. Callers
    /// that must know whether a real chat exists (context-isolated memory
    /// scoping fails closed without one) use this directly.
    pub fn session_chat_id(&self, session_id: &str) -> Option<String> {
        self.get_session(session_id)
            .ok()
            .flatten()
            .and_then(|s| s.active_chat_id.or(s.name))
    }

    /// Resolve a session id to the chat id its messages are stored under.
    /// Sessions are decoupled from chats: prefer `active_chat_id`, fall back to
    /// `name` (legacy pre-decoupling sessions), then `chat-{session_id}`.
    /// The ONE derivation — used by both the write side (SessionManager) and
    /// readers (tools); do not mirror this chain anywhere else.
    pub fn resolve_session_chat_id(&self, session_id: &str) -> String {
        self.session_chat_id(session_id)
            .unwrap_or_else(|| format!("chat-{}", session_id))
    }

    pub fn get_session_by_name(&self, name: &str) -> Result<Option<Session>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT * FROM sessions WHERE name = ?1",
            params![name],
            row_to_session,
        )
        .optional()
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn get_session_by_scope(
        &self,
        scope: &str,
        scope_id: &str,
    ) -> Result<Option<Session>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT * FROM sessions WHERE scope = ?1 AND scope_id = ?2",
            params![scope, scope_id],
            row_to_session,
        )
        .optional()
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn get_session_by_name_and_scope(
        &self,
        name: &str,
        scope: &str,
        scope_id: &str,
    ) -> Result<Option<Session>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT * FROM sessions WHERE name = ?1 AND scope = ?2 AND scope_id = ?3",
            params![name, scope, scope_id],
            row_to_session,
        )
        .optional()
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn get_or_create_scoped_session(
        &self,
        id: &str,
        name: &str,
        scope: &str,
        scope_id: &str,
        metadata: Option<&str>,
    ) -> Result<Session, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "INSERT INTO sessions (id, name, scope, scope_id, metadata, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, unixepoch(), unixepoch())
             ON CONFLICT(name, scope, scope_id) DO UPDATE SET updated_at = unixepoch()
             RETURNING *",
            params![id, name, scope, scope_id, metadata],
            row_to_session,
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn list_sessions(&self, limit: i64, offset: i64) -> Result<Vec<Session>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare("SELECT * FROM sessions ORDER BY updated_at DESC LIMIT ?1 OFFSET ?2")
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![limit, offset], row_to_session)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn list_sessions_by_scope(&self, scope: &str) -> Result<Vec<Session>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare("SELECT * FROM sessions WHERE scope = ?1 ORDER BY updated_at DESC")
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![scope], row_to_session)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn update_session_stats(
        &self,
        id: &str,
        token_count: i64,
        message_count: i64,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE sessions SET token_count = ?2, message_count = ?3, updated_at = unixepoch() WHERE id = ?1",
            params![id, token_count, message_count],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn increment_session_message_count(&self, id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE sessions SET message_count = COALESCE(message_count, 0) + 1, updated_at = unixepoch() WHERE id = ?1",
            params![id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// One more compaction happened (the sliding window evicted messages).
    /// The pre-eviction memory flush gate compares this against
    /// `memory_flush_compaction_count`; until 2026-09-02 nothing incremented
    /// it, so that gate could never open.
    pub fn increment_session_compaction_count(&self, id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE sessions SET compaction_count = COALESCE(compaction_count, 0) + 1,
             last_compacted_at = unixepoch(), updated_at = unixepoch() WHERE id = ?1",
            params![id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn reset_session(&self, id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE sessions SET message_count = 0, token_count = 0,
             last_compacted_at = NULL, compaction_count = 0, memory_flush_at = NULL,
             memory_flush_compaction_count = NULL,
             updated_at = unixepoch() WHERE id = ?1",
            params![id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn delete_session(&self, id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM sessions WHERE id = ?1", params![id])
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn set_session_model_override(
        &self,
        id: &str,
        model_override: Option<&str>,
        provider_override: Option<&str>,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE sessions SET model_override = ?2, provider_override = ?3, updated_at = unixepoch() WHERE id = ?1",
            params![id, model_override, provider_override],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn set_session_auth_profile_override(
        &self,
        id: &str,
        auth_profile_override: Option<&str>,
        auth_profile_override_source: Option<&str>,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE sessions SET auth_profile_override = ?2, auth_profile_override_source = ?3, updated_at = unixepoch() WHERE id = ?1",
            params![id, auth_profile_override, auth_profile_override_source],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn clear_session_overrides(&self, id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE sessions SET model_override = NULL, provider_override = NULL,
             auth_profile_override = NULL, auth_profile_override_source = NULL,
             verbose_level = NULL, updated_at = unixepoch() WHERE id = ?1",
            params![id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn set_session_send_policy(&self, id: &str, send_policy: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE sessions SET send_policy = ?2, updated_at = unixepoch() WHERE id = ?1",
            params![id, send_policy],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn set_session_label(&self, id: &str, custom_label: Option<&str>) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE sessions SET custom_label = ?2, updated_at = unixepoch() WHERE id = ?1",
            params![id, custom_label],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn get_session_last_embedded_message_id(&self, id: &str) -> Result<i64, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT COALESCE(last_embedded_message_id, 0) FROM sessions WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn update_session_last_embedded_message_id(
        &self,
        id: &str,
        last_embedded_message_id: i64,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE sessions SET last_embedded_message_id = ?2, updated_at = unixepoch() WHERE id = ?1",
            params![id, last_embedded_message_id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Update memory flush tracking after a pre-compaction flush.
    pub fn update_session_memory_flush(
        &self,
        id: &str,
        compaction_count: i64,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE sessions SET memory_flush_at = unixepoch(), memory_flush_compaction_count = ?2, updated_at = unixepoch() WHERE id = ?1",
            params![id, compaction_count],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Point a session to a different active chat (conversation).
    pub fn set_session_active_chat_id(&self, id: &str, chat_id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE sessions SET active_chat_id = ?2, updated_at = unixepoch() WHERE id = ?1",
            params![id, chat_id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Where a turn woken in this session sends its reply (`None` clears it):
    /// the conversation the session's work came from. Stored in the
    /// session's metadata as `replyRoute`; the server owns its shape.
    pub fn set_session_reply_route(&self, id: &str, route: Option<&str>) -> Result<(), NeboError> {
        let conn = self.conn()?;
        match route {
            Some(route) => conn.execute(
                "UPDATE sessions SET metadata = json_set(COALESCE(NULLIF(metadata, ''), '{}'), '$.replyRoute', json(?2)) WHERE id = ?1",
                params![id, route],
            ),
            None => conn.execute(
                "UPDATE sessions SET metadata = json_remove(COALESCE(NULLIF(metadata, ''), '{}'), '$.replyRoute') WHERE id = ?1",
                params![id],
            ),
        }
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// The reply route [`Self::set_session_reply_route`] stored, if any.
    pub fn session_reply_route(&self, id: &str) -> Result<Option<String>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT json_extract(COALESCE(NULLIF(metadata, ''), '{}'), '$.replyRoute') FROM sessions WHERE id = ?1",
            params![id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map(Option::flatten)
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// Reset conversation-scoped counters without clearing session-level preferences.
    /// Preserves model_override, provider_override, auth_profile_override, send_policy,
    /// custom_label, verbose_level — only clears per-conversation state.
    pub fn reset_session_counters(&self, id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE sessions SET
                message_count = 0,
                token_count = 0,
                last_compacted_at = NULL,
                updated_at = unixepoch()
             WHERE id = ?1",
            params![id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }
}

fn row_to_session(row: &rusqlite::Row) -> rusqlite::Result<Session> {
    Ok(Session {
        id: row.get("id")?,
        name: row.get("name")?,
        scope: row.get("scope")?,
        scope_id: row.get("scope_id")?,
        token_count: row.get("token_count")?,
        message_count: row.get("message_count")?,
        last_compacted_at: row.get("last_compacted_at")?,
        metadata: row.get("metadata")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
        compaction_count: row.get("compaction_count")?,
        memory_flush_at: row.get("memory_flush_at")?,
        memory_flush_compaction_count: row.get("memory_flush_compaction_count")?,
        send_policy: row.get("send_policy")?,
        model_override: row.get("model_override")?,
        provider_override: row.get("provider_override")?,
        auth_profile_override: row.get("auth_profile_override")?,
        auth_profile_override_source: row.get("auth_profile_override_source")?,
        verbose_level: row.get("verbose_level")?,
        custom_label: row.get("custom_label")?,
        last_embedded_message_id: row.get("last_embedded_message_id")?,
        active_chat_id: row.get("active_chat_id")?,
    })
}

trait OptionalExt<T> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error>;
}

impl<T> OptionalExt<T> for rusqlite::Result<T> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error> {
        match self {
            Ok(val) => Ok(Some(val)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::Store;

    #[test]
    fn test_session_chat_id_derivation_chain() {
        let path = std::env::temp_dir().join(format!("nebo-sessq-test-{}.db", std::process::id()));
        let path_str = path.to_string_lossy().to_string();
        let _ = std::fs::remove_file(&path);
        let store = Store::new(&path_str).unwrap();

        // No session row → no chat derivable (context-isolated memory fails
        // closed on this), while resolve_ keeps its synthetic fallback.
        assert_eq!(store.session_chat_id("missing"), None);
        assert_eq!(store.resolve_session_chat_id("missing"), "chat-missing");

        // active_chat_id wins.
        store
            .create_session("s1", Some("legacy-name"), None, None, None)
            .unwrap();
        store.set_session_active_chat_id("s1", "chat-42").unwrap();
        assert_eq!(store.session_chat_id("s1").as_deref(), Some("chat-42"));
        assert_eq!(store.resolve_session_chat_id("s1"), "chat-42");

        // Legacy session: name only.
        store
            .create_session("s2", Some("legacy-name"), None, None, None)
            .unwrap();
        assert_eq!(store.session_chat_id("s2").as_deref(), Some("legacy-name"));

        // Row exists but carries neither → still None.
        store.create_session("s3", None, None, None, None).unwrap();
        assert_eq!(store.session_chat_id("s3"), None);
        assert_eq!(store.resolve_session_chat_id("s3"), "chat-s3");

        let _ = std::fs::remove_file(&path);
    }
}

#[cfg(test)]
mod counter_tests {
    use crate::Store;

    /// The eviction site increments the counter that gates the pre-eviction
    /// memory flush; the flush marks its own count, and the gate reopens only
    /// on the next compaction.
    #[test]
    fn compaction_count_increments_on_eviction_and_opens_the_flush_gate() {
        let (_dir, store) = store();
        store.create_session("s1", Some("s1"), None, None, None).unwrap();
        let count = |s: &Store| s.get_session("s1").unwrap().unwrap().compaction_count.unwrap_or(0);
        let flushed = |s: &Store| s.get_session("s1").unwrap().unwrap().memory_flush_compaction_count.unwrap_or(0);
        assert_eq!(count(&store), 0);
        store.increment_session_compaction_count("s1").unwrap();
        assert_eq!(count(&store), 1);
        assert!(count(&store) > flushed(&store), "gate open after the first eviction");
        store.update_session_memory_flush("s1", count(&store)).unwrap();
        assert!(count(&store) <= flushed(&store), "gate closed once the flush caught up");
        store.increment_session_compaction_count("s1").unwrap();
        assert!(count(&store) > flushed(&store), "and reopens on the next eviction");
    }

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nebo-sessions-test.db");
        let store = Store::new(&path.to_string_lossy()).expect("store");
        (dir, store)
    }

    /// reset_session_counters (the rotate/new-conversation reset) clears ONLY
    /// per-conversation state; session-level preferences (model override,
    /// custom label) survive the rotation — that is what makes rotate a
    /// non-destructive "new conversation" and not a session wipe.
    #[test]
    fn reset_session_counters_preserves_preferences() {
        let (_dir, store) = store();
        store
            .create_session("s1", Some("agent:test:web"), None, None, None)
            .unwrap();
        store
            .set_session_model_override("s1", Some("model-x"), Some("provider-y"))
            .unwrap();
        store.set_session_label("s1", Some("My label")).unwrap();
        store.update_session_stats("s1", 5000, 12).unwrap();

        store.reset_session_counters("s1").unwrap();

        let s = store.get_session("s1").unwrap().unwrap();
        assert_eq!(s.message_count, Some(0));
        assert_eq!(s.token_count, Some(0));
        assert_eq!(s.last_compacted_at, None);
        // Preferences survive.
        assert_eq!(s.model_override.as_deref(), Some("model-x"));
        assert_eq!(s.provider_override.as_deref(), Some("provider-y"));
        assert_eq!(s.custom_label.as_deref(), Some("My label"));
    }

    /// get_or_create_scoped_session is an upsert on (name, scope, scope_id):
    /// a second call with a NEW id returns the EXISTING session row instead
    /// of creating a competing one.
    #[test]
    fn get_or_create_scoped_session_returns_existing_row() {
        let (_dir, store) = store();
        let first = store
            .get_or_create_scoped_session("id-1", "main", "agent", "emp1", None)
            .unwrap();
        assert_eq!(first.id, "id-1");

        let second = store
            .get_or_create_scoped_session("id-2", "main", "agent", "emp1", None)
            .unwrap();
        assert_eq!(second.id, "id-1", "same (name, scope, scope_id) upserts");
        assert_eq!(store.list_sessions_by_scope("agent").unwrap().len(), 1);
    }

    /// A freshly created session has no active_chat_id — decoupled sessions
    /// only get one when a conversation is attached or rotated in.
    #[test]
    fn new_session_round_trips_with_no_active_chat() {
        let (_dir, store) = store();
        let created = store
            .create_session("s-rt", Some("agent:rt:web"), Some("agent"), Some("rt"), Some("{}"))
            .unwrap();
        assert_eq!(created.active_chat_id, None);

        let fetched = store.get_session("s-rt").unwrap().unwrap();
        assert_eq!(fetched.id, "s-rt");
        assert_eq!(fetched.name.as_deref(), Some("agent:rt:web"));
        assert_eq!(fetched.scope.as_deref(), Some("agent"));
        assert_eq!(fetched.scope_id.as_deref(), Some("rt"));
        assert_eq!(fetched.metadata.as_deref(), Some("{}"));
        assert_eq!(fetched.active_chat_id, None);
    }

    /// A reply route is kept beside whatever else the metadata holds, and
    /// clearing it leaves the rest.
    #[test]
    fn a_reply_route_round_trips_and_clears() {
        let (_dir, store) = store();
        store
            .create_session("s-route", Some("agent:r:web"), None, None, Some(r#"{"keep":1}"#))
            .unwrap();
        assert_eq!(store.session_reply_route("s-route").unwrap(), None);
        store.set_session_reply_route("s-route", Some(r#"{"kind":"comm","topic":"dm"}"#)).unwrap();
        let route: serde_json::Value =
            serde_json::from_str(&store.session_reply_route("s-route").unwrap().unwrap()).unwrap();
        assert_eq!(route["topic"], "dm");
        store.set_session_reply_route("s-route", None).unwrap();
        assert_eq!(store.session_reply_route("s-route").unwrap(), None);
        let meta: serde_json::Value =
            serde_json::from_str(&store.get_session("s-route").unwrap().unwrap().metadata.unwrap()).unwrap();
        assert_eq!(meta["keep"], 1);
        assert_eq!(store.session_reply_route("missing").unwrap(), None);
    }
}
