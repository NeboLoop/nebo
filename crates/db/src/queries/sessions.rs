use rusqlite::params;

use crate::models::Session;
use crate::Store;
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

    pub fn update_session_summary(&self, id: &str, summary: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE sessions SET summary = ?2, last_compacted_at = unixepoch(), updated_at = unixepoch() WHERE id = ?1",
            params![id, summary],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
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

    pub fn reset_session(&self, id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE sessions SET message_count = 0, token_count = 0, summary = NULL,
             last_compacted_at = NULL, compaction_count = 0, memory_flush_at = NULL,
             memory_flush_compaction_count = NULL, active_task = NULL,
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

    pub fn get_session_active_task(&self, id: &str) -> Result<String, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT COALESCE(active_task, '') FROM sessions WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn set_session_active_task(&self, id: &str, active_task: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE sessions SET active_task = ?2, updated_at = unixepoch() WHERE id = ?1",
            params![id, active_task],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn clear_session_active_task(&self, id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE sessions SET active_task = NULL, updated_at = unixepoch() WHERE id = ?1",
            params![id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn get_session_work_tasks(&self, id: &str) -> Result<String, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT COALESCE(work_tasks, '') FROM sessions WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn set_session_work_tasks(&self, id: &str, work_tasks: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE sessions SET work_tasks = ?2, updated_at = unixepoch() WHERE id = ?1",
            params![id, work_tasks],
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
}

fn row_to_session(row: &rusqlite::Row) -> rusqlite::Result<Session> {
    Ok(Session {
        id: row.get("id")?,
        name: row.get("name")?,
        scope: row.get("scope")?,
        scope_id: row.get("scope_id")?,
        summary: row.get("summary")?,
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
        active_task: row.get("active_task")?,
        last_summarized_count: row.get("last_summarized_count")?,
        work_tasks: row.get("work_tasks")?,
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
