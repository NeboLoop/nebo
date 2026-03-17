use rusqlite::params;

use crate::models::{Chat, ChatMessage};
use crate::Store;
use types::NeboError;

impl Store {
    pub fn create_chat(&self, id: &str, title: &str) -> Result<Chat, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "INSERT INTO chats (id, title, created_at, updated_at)
             VALUES (?1, ?2, unixepoch(), unixepoch()) RETURNING *",
            params![id, title],
            row_to_chat,
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn get_chat(&self, id: &str) -> Result<Option<Chat>, NeboError> {
        let conn = self.conn()?;
        conn.query_row("SELECT * FROM chats WHERE id = ?1", params![id], |row| {
            row_to_chat(row)
        })
        .optional()
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn list_chats(&self, limit: i64, offset: i64) -> Result<Vec<Chat>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare("SELECT * FROM chats ORDER BY updated_at DESC LIMIT ?1 OFFSET ?2")
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![limit, offset], row_to_chat)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn count_chats(&self) -> Result<i64, NeboError> {
        let conn = self.conn()?;
        conn.query_row("SELECT COUNT(*) FROM chats", [], |row| row.get(0))
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn update_chat_title(&self, id: &str, title: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE chats SET title = ?2, updated_at = unixepoch() WHERE id = ?1",
            params![id, title],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn update_chat_timestamp(&self, id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE chats SET updated_at = unixepoch() WHERE id = ?1",
            params![id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn delete_chat(&self, id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM chats WHERE id = ?1", params![id])
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn create_chat_message(
        &self,
        id: &str,
        chat_id: &str,
        role: &str,
        content: &str,
        metadata: Option<&str>,
    ) -> Result<ChatMessage, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "INSERT INTO chat_messages (id, chat_id, role, content, metadata, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, unixepoch()) RETURNING *",
            params![id, chat_id, role, content, metadata],
            row_to_chat_message,
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn create_chat_message_for_runner(
        &self,
        id: &str,
        chat_id: &str,
        role: &str,
        content: &str,
        tool_calls: Option<&str>,
        tool_results: Option<&str>,
        token_estimate: Option<i64>,
        metadata: Option<&str>,
    ) -> Result<ChatMessage, NeboError> {
        let conn = self.conn()?;
        // Ensure parent chat row exists (role/channel sessions don't pre-create one).
        conn.execute(
            "INSERT OR IGNORE INTO chats (id, title, created_at, updated_at) VALUES (?1, ?1, unixepoch(), unixepoch())",
            params![chat_id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        conn.query_row(
            "INSERT INTO chat_messages (id, chat_id, role, content, metadata, tool_calls, tool_results, token_estimate, day_marker, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, date('now', 'localtime'), unixepoch()) RETURNING *",
            params![id, chat_id, role, content, metadata, tool_calls, tool_results, token_estimate],
            row_to_chat_message,
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn get_chat_messages(&self, chat_id: &str) -> Result<Vec<ChatMessage>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare("SELECT * FROM chat_messages WHERE chat_id = ?1 ORDER BY created_at ASC, rowid ASC")
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![chat_id], row_to_chat_message)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn get_chat_message(&self, id: &str) -> Result<Option<ChatMessage>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT * FROM chat_messages WHERE id = ?1",
            params![id],
            row_to_chat_message,
        )
        .optional()
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn get_recent_chat_messages(
        &self,
        chat_id: &str,
        limit: i64,
    ) -> Result<Vec<ChatMessage>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT * FROM (
                    SELECT *, rowid AS _rn FROM chat_messages WHERE chat_id = ?1 AND role IN ('user', 'assistant')
                    ORDER BY created_at DESC, _rn DESC LIMIT ?2
                 ) sub ORDER BY created_at ASC, _rn ASC",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![chat_id, limit], row_to_chat_message)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn get_recent_chat_messages_with_tools(
        &self,
        chat_id: &str,
        limit: i64,
    ) -> Result<Vec<ChatMessage>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT * FROM (
                    SELECT *, rowid AS _rn FROM chat_messages WHERE chat_id = ?1
                    ORDER BY created_at DESC, _rn DESC LIMIT ?2
                 ) sub ORDER BY created_at ASC, _rn ASC",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![chat_id, limit], row_to_chat_message)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// Find a tool call's output by searching role='tool' messages' tool_results JSON.
    /// Returns (output_content, is_error) if found.
    pub fn find_tool_output(
        &self,
        chat_id: &str,
        tool_call_id: &str,
    ) -> Result<Option<(String, bool)>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT tool_results FROM chat_messages
                 WHERE chat_id = ?1 AND role = 'tool' AND tool_results LIKE '%' || ?2 || '%'
                 LIMIT 1",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let result: Option<String> = stmt
            .query_row(params![chat_id, tool_call_id], |row| row.get(0))
            .optional()
            .map_err(|e| NeboError::Database(e.to_string()))?;
        if let Some(tr_json) = result {
            if let Ok(results) = serde_json::from_str::<Vec<serde_json::Value>>(&tr_json) {
                for r in &results {
                    if r.get("tool_call_id").and_then(|v| v.as_str()) == Some(tool_call_id) {
                        let content = r
                            .get("content")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let is_error =
                            r.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false);
                        return Ok(Some((content, is_error)));
                    }
                }
            }
        }
        Ok(None)
    }

    pub fn delete_chat_message(&self, id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM chat_messages WHERE id = ?1", params![id])
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn delete_chat_messages_after(
        &self,
        chat_id: &str,
        created_at: i64,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM chat_messages WHERE chat_id = ?1 AND created_at > ?2",
            params![chat_id, created_at],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn delete_chat_messages_after_id(
        &self,
        chat_id: &str,
        message_id: &str,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM chat_messages WHERE chat_id = ?1 AND rowid > (SELECT rowid FROM chat_messages WHERE id = ?2)",
            params![chat_id, message_id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn delete_chat_messages_by_chat_id(&self, chat_id: &str) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM chat_messages WHERE chat_id = ?1",
            params![chat_id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn update_chat_message_content(
        &self,
        id: &str,
        content: &str,
        metadata: Option<&str>,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE chat_messages SET content = ?2, metadata = ?3 WHERE id = ?1",
            params![id, content, metadata],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    pub fn count_chat_messages(&self, chat_id: &str) -> Result<i64, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT COUNT(*) FROM chat_messages WHERE chat_id = ?1",
            params![chat_id],
            |row| row.get(0),
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn search_chat_messages(
        &self,
        chat_id: &str,
        query: &str,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<ChatMessage>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT * FROM chat_messages WHERE chat_id = ?1 AND content LIKE '%' || ?2 || '%'
                 ORDER BY created_at DESC, rowid DESC LIMIT ?3 OFFSET ?4",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![chat_id, query, limit, offset], |row| {
                row_to_chat_message(row)
            })
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn get_or_create_companion_chat(
        &self,
        id: &str,
        user_id: &str,
    ) -> Result<Chat, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "INSERT INTO chats (id, user_id, title, created_at, updated_at)
             VALUES (?1, ?2, 'Companion', unixepoch(), unixepoch())
             ON CONFLICT(user_id) DO UPDATE SET updated_at = unixepoch()
             RETURNING *",
            params![id, user_id],
            row_to_chat,
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn get_companion_chat_by_user(&self, user_id: &str) -> Result<Option<Chat>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT * FROM chats WHERE user_id = ?1 LIMIT 1",
            params![user_id],
            row_to_chat,
        )
        .optional()
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn list_chat_days(
        &self,
        chat_id: &str,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<(String, i64)>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT day_marker, COUNT(*) as cnt FROM chat_messages
                 WHERE chat_id = ?1 AND day_marker IS NOT NULL
                 GROUP BY day_marker ORDER BY day_marker DESC LIMIT ?2 OFFSET ?3",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![chat_id, limit, offset], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn get_chat_messages_by_day(
        &self,
        chat_id: &str,
        day: &str,
    ) -> Result<Vec<ChatMessage>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT * FROM chat_messages WHERE chat_id = ?1 AND day_marker = ?2
                 ORDER BY created_at ASC, rowid ASC",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![chat_id, day], row_to_chat_message)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn get_chat_messages_after_timestamp(
        &self,
        chat_id: &str,
        created_at: i64,
    ) -> Result<Vec<ChatMessage>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT * FROM chat_messages WHERE chat_id = ?1 AND created_at > ?2
                 AND role IN ('user', 'assistant') ORDER BY created_at ASC, rowid ASC",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![chat_id, created_at], |row| {
                row_to_chat_message(row)
            })
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }
}

fn row_to_chat(row: &rusqlite::Row) -> rusqlite::Result<Chat> {
    Ok(Chat {
        id: row.get("id")?,
        title: row.get("title")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
        user_id: row.get("user_id")?,
    })
}

fn row_to_chat_message(row: &rusqlite::Row) -> rusqlite::Result<ChatMessage> {
    Ok(ChatMessage {
        id: row.get("id")?,
        chat_id: row.get("chat_id")?,
        role: row.get("role")?,
        content: row.get("content")?,
        metadata: row.get("metadata")?,
        created_at: row.get("created_at")?,
        day_marker: row.get("day_marker")?,
        tool_calls: row.get("tool_calls")?,
        tool_results: row.get("tool_results")?,
        token_estimate: row.get("token_estimate")?,
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
