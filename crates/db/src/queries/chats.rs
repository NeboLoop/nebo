use rusqlite::params;

use crate::Store;
use crate::models::{Chat, ChatMessage};
use types::NeboError;

/// One full-text search hit across all chats — enough context to cite the
/// conversation (chat + title + when) without loading it.
#[derive(Debug, Clone)]
pub struct ChatSearchHit {
    pub chat_id: String,
    pub chat_title: String,
    pub message_id: String,
    pub role: String,
    pub snippet: String,
    pub created_at: i64,
}

/// created_at of a message row — the shared cursor lookup for paginated
/// history queries (was inlined twice; CODE_AUDITOR Rule 8).
fn message_created_at(conn: &rusqlite::Connection, id: &str) -> Result<i64, NeboError> {
    conn.query_row(
        "SELECT created_at FROM chat_messages WHERE id = ?1",
        params![id],
        |row| row.get(0),
    )
    .map_err(|e| NeboError::Database(e.to_string()))
}

/// A chat's preview line is its last VISIBLE message: not a tool result,
/// not empty, not a hidden system-injected message (reminders carry
/// metadata {"hidden":true}). `chat_id` is the SQL expression to match on.
fn last_visible_message_sql(chat_id: &str) -> String {
    format!(
        "(SELECT m2.content FROM chat_messages m2
          WHERE m2.chat_id = {chat_id}
            AND m2.role != 'tool'
            AND m2.content != ''
            AND (m2.metadata IS NULL OR m2.metadata NOT LIKE '%\"hidden\":true%')
          ORDER BY m2.created_at DESC, m2.id DESC LIMIT 1)"
    )
}

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
            .prepare("SELECT * FROM chats
                 WHERE session_name IS NULL
                    OR (session_name NOT LIKE 'agent:%:workflow:%'
                        AND session_name NOT LIKE 'workflow:%')
                 ORDER BY updated_at DESC LIMIT ?1 OFFSET ?2")
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

    /// Update a chat's title. `custom` marks it as a user rename (the auto-namer
    /// skips title_custom chats so it never clobbers a chosen name).
    pub fn update_chat_title(&self, id: &str, title: &str, custom: bool) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE chats SET title = ?2, title_custom = ?3, updated_at = unixepoch() WHERE id = ?1",
            params![id, title, custom as i64],
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
        // A message landing IS activity: the chat list orders by updated_at,
        // so bump it here (both inserters), not only on title edits.
        conn.execute(
            "UPDATE chats SET updated_at = unixepoch() WHERE id = ?1",
            params![chat_id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
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
        session_name: Option<&str>,
    ) -> Result<ChatMessage, NeboError> {
        let conn = self.conn()?;
        // Ensure parent chat row exists (role/channel sessions don't pre-create one).
        conn.execute(
            "INSERT OR IGNORE INTO chats (id, title, session_name, created_at, updated_at) VALUES (?1, ?1, ?2, unixepoch(), unixepoch())",
            params![chat_id, session_name],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        conn.execute(
            "UPDATE chats SET updated_at = unixepoch() WHERE id = ?1",
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
            .prepare(
                "SELECT * FROM chat_messages WHERE chat_id = ?1 AND rowid > COALESCE((SELECT compacted_below_rowid FROM chats WHERE id = ?1), 0) ORDER BY created_at ASC, rowid ASC",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![chat_id], row_to_chat_message)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// Get the most recent N messages for a chat. If `before` is provided, fetch messages older
    /// than that message ID (for "load more" pagination). Returns messages in ascending order.
    pub fn get_chat_messages_paginated(
        &self,
        chat_id: &str,
        limit: i64,
        before: Option<&str>,
    ) -> Result<Vec<ChatMessage>, NeboError> {
        let conn = self.conn()?;
        let messages = if let Some(before_id) = before {
            // Get the created_at of the cursor message
            let cursor_ts: i64 = message_created_at(&conn, before_id)?;

            let mut stmt = conn
                .prepare(
                    "SELECT * FROM chat_messages WHERE chat_id = ?1 AND created_at < ?2
                     AND rowid > COALESCE((SELECT compacted_below_rowid FROM chats WHERE id = ?1), 0)
                 ORDER BY created_at DESC, id DESC LIMIT ?3",
                )
                .map_err(|e| NeboError::Database(e.to_string()))?;
            let rows = stmt
                .query_map(params![chat_id, cursor_ts, limit], row_to_chat_message)
                .map_err(|e| NeboError::Database(e.to_string()))?;
            let mut msgs: Vec<ChatMessage> = rows
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| NeboError::Database(e.to_string()))?;
            msgs.reverse(); // back to ascending order
            msgs
        } else {
            // Get the last N messages (most recent)
            let mut stmt = conn
                .prepare(
                    "SELECT * FROM (
                    SELECT * FROM chat_messages WHERE chat_id = ?1 AND rowid > COALESCE((SELECT compacted_below_rowid FROM chats WHERE id = ?1), 0)
                    ORDER BY created_at DESC, id DESC LIMIT ?2
                ) ORDER BY created_at ASC, id ASC",
                )
                .map_err(|e| NeboError::Database(e.to_string()))?;
            let rows = stmt
                .query_map(params![chat_id, limit], row_to_chat_message)
                .map_err(|e| NeboError::Database(e.to_string()))?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| NeboError::Database(e.to_string()))?
        };
        Ok(messages)
    }

    /// Get the most recent messages for a chat, bounded by a character budget rather than
    /// a fixed count. Fetches newest-first, accumulates content + tool_calls + tool_results
    /// length, and stops when the budget is exceeded. Always returns at least 1 message.
    /// If `before` is provided, fetches messages older than that message ID.
    /// Returns messages in ascending chronological order.
    pub fn get_chat_messages_budgeted(
        &self,
        chat_id: &str,
        max_chars: i64,
        before: Option<&str>,
    ) -> Result<Vec<ChatMessage>, NeboError> {
        let conn = self.conn()?;
        // Fetch a generous batch newest-first. This must be large enough to reach
        // back PAST a long tool-execution run: tool calls/results are stored as
        // their own rows (role='tool') and pure tool-call assistant turns have
        // empty content, so a chat can have dozens of consecutive text-less rows
        // before the previous conversational turn. A small batch would return
        // nothing but that tool run and the chat would render as "Used N tools"
        // with no conversation. The content budget below still cuts text-heavy
        // chats short, so this only matters for tool-heavy ones.
        let batch_limit: i64 = 250;
        let mut msgs: Vec<ChatMessage> = if let Some(before_id) = before {
            let cursor_ts: i64 = message_created_at(&conn, before_id)?;
            // Use composite cursor (created_at, id) to avoid skipping messages
            // created in the same second as the cursor message.
            let mut stmt = conn
                .prepare(
                    "SELECT * FROM chat_messages WHERE chat_id = ?1
                     AND (created_at < ?2 OR (created_at = ?2 AND id < ?3))
                     AND rowid > COALESCE((SELECT compacted_below_rowid FROM chats WHERE id = ?1), 0)
                 ORDER BY created_at DESC, id DESC LIMIT ?4",
                )
                .map_err(|e| NeboError::Database(e.to_string()))?;
            let rows = stmt
                .query_map(
                    params![chat_id, cursor_ts, before_id, batch_limit],
                    row_to_chat_message,
                )
                .map_err(|e| NeboError::Database(e.to_string()))?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| NeboError::Database(e.to_string()))?
        } else {
            let mut stmt = conn
                .prepare(
                    "SELECT * FROM chat_messages WHERE chat_id = ?1 AND rowid > COALESCE((SELECT compacted_below_rowid FROM chats WHERE id = ?1), 0)
                 ORDER BY created_at DESC, id DESC LIMIT ?2",
                )
                .map_err(|e| NeboError::Database(e.to_string()))?;
            let rows = stmt
                .query_map(params![chat_id, batch_limit], row_to_chat_message)
                .map_err(|e| NeboError::Database(e.to_string()))?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| NeboError::Database(e.to_string()))?
        };
        // msgs is newest-first — accumulate budget and truncate.
        // The budget is measured in CONVERSATIONAL TEXT (message content) only.
        // Tool calls/results are collapsed in the UI ("Used N tools") and do NOT
        // count against the window — otherwise a long tool run (or one huge
        // web-search result) fills the budget and the chat loads showing only
        // tool activity with no conversation. By counting content alone, the
        // window keeps extending back through tool activity until it has gathered
        // a real slice of the conversation. batch_limit bounds the payload.
        let mut budget: i64 = 0;
        let mut keep = 0usize;
        for msg in &msgs {
            budget += msg.content.len() as i64;
            keep += 1;
            if budget >= max_chars && keep > 1 {
                break;
            }
        }
        msgs.truncate(keep);
        msgs.reverse(); // back to ascending order
        Ok(msgs)
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
                        let is_error = r.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false);
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

    /// Atomically replace all of a chat's messages with a single assistant
    /// summary message (compaction). Delete + insert run in one transaction so
    /// a failure at any point leaves the original conversation intact.
    /// Compact a conversation as a projection: every existing row stays on
    /// disk, the chat's floor moves to the current last row, and the summary
    /// is inserted above the floor as the first visible message. Every read
    /// of the conversation (`get_chat_messages*`) starts above the floor, so
    /// the runner and the UI see [summary, ...new messages] while the bytes
    /// remain recoverable. Compacting twice moves the floor again; there is
    /// never more than one visible summary.
    pub fn compact_chat_history(
        &self,
        chat_id: &str,
        message_id: &str,
        summary: &str,
    ) -> Result<(), NeboError> {
        let mut conn = self.conn()?;
        let tx = conn
            .transaction()
            .map_err(|e| NeboError::Database(e.to_string()))?;
        tx.execute(
            "UPDATE chats SET compacted_below_rowid =
                 (SELECT COALESCE(MAX(rowid), 0) FROM chat_messages WHERE chat_id = ?1)
             WHERE id = ?1",
            params![chat_id],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        tx.execute(
            "INSERT INTO chat_messages (id, chat_id, role, content, created_at)
             VALUES (?1, ?2, 'assistant', ?3, unixepoch())",
            params![message_id, chat_id, summary],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        tx.commit()
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Frozen tool-result renderings for a chat (tool_call_id → rendering).
    pub fn get_chat_renderings(
        &self,
        chat_id: &str,
    ) -> Result<std::collections::HashMap<String, String>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare("SELECT tool_call_id, rendering FROM chat_renderings WHERE chat_id = ?1")
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![chat_id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<_, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// Freeze renderings. A rendering already stored for an id is never
    /// replaced: the first rendering the model saw is the rendering forever.
    pub fn insert_chat_renderings(
        &self,
        chat_id: &str,
        renderings: &[(String, String)],
    ) -> Result<(), NeboError> {
        if renderings.is_empty() {
            return Ok(());
        }
        let mut conn = self.conn()?;
        let tx = conn
            .transaction()
            .map_err(|e| NeboError::Database(e.to_string()))?;
        for (id, rendering) in renderings {
            tx.execute(
                "INSERT OR IGNORE INTO chat_renderings (chat_id, tool_call_id, rendering) VALUES (?1, ?2, ?3)",
                params![chat_id, id, rendering],
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        }
        tx.commit()
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

    /// Count CONVERSATIONAL messages only (user + assistant) — tool-call/result
    /// rows (role='tool') are not messages, they ride inside the assistant turn's
    /// tool_calls/tool_results columns. This matches what get_chat_messages_budgeted
    /// loads, so the "N messages" badge is honest and the client's
    /// `hasMore = loadedRawCount < totalMessages` paging math is correct (counting
    /// tool rows here inflated the total and broke scroll-up).
    /// Count USER turns across the whole chat — the title generator's gate.
    /// Counting within a recent-messages window instead made long chats
    /// re-title forever: the sliding window kept containing exactly 1 or 3
    /// user messages, so the title chased whatever was said most recently.
    pub fn count_chat_user_messages(&self, chat_id: &str) -> Result<i64, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT COUNT(*) FROM chat_messages WHERE chat_id = ?1 AND role = 'user'",
            params![chat_id],
            |row| row.get(0),
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn count_chat_messages(&self, chat_id: &str) -> Result<i64, NeboError> {
        let conn = self.conn()?;
        // Internal turns (`isMeta` — preloads, auto-continuation nudges) never
        // reach the transcript, so they must not inflate its count either: the
        // client pages until it has loaded as many as this says exist.
        conn.query_row(
            "SELECT COUNT(*) FROM chat_messages
              WHERE chat_id = ?1
                AND role IN ('user', 'assistant')
                AND COALESCE(json_extract(metadata, '$.isMeta'), 0) NOT IN (1, 'true')",
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

    /// Create a chat with caller-supplied timestamps — the migration-importer
    /// variant of [`Self::create_chat_for_session`]: imported conversations
    /// keep their original creation/last-activity times instead of appearing
    /// to have all happened at import time.
    pub fn create_chat_imported(
        &self,
        id: &str,
        session_name: &str,
        title: &str,
        created_at: i64,
        updated_at: i64,
    ) -> Result<Chat, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "INSERT INTO chats (id, session_name, title, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5) RETURNING *",
            params![id, session_name, title, created_at, updated_at],
            row_to_chat,
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// Insert a chat message with a caller-supplied timestamp — the
    /// migration-importer variant of [`Self::create_chat_message_for_runner`].
    /// `day_marker` is derived from the supplied timestamp so imported history
    /// groups under its original days.
    pub fn create_chat_message_imported(
        &self,
        id: &str,
        chat_id: &str,
        role: &str,
        content: &str,
        tool_calls: Option<&str>,
        metadata: Option<&str>,
        created_at: i64,
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO chat_messages (id, chat_id, role, content, metadata, tool_calls, day_marker, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, date(?7, 'unixepoch', 'localtime'), ?7)",
            params![id, chat_id, role, content, metadata, tool_calls, created_at],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// Create a new chat linked to a session.
    pub fn create_chat_for_session(
        &self,
        id: &str,
        session_name: &str,
        title: &str,
        user_id: Option<&str>,
    ) -> Result<Chat, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "INSERT INTO chats (id, session_name, title, user_id, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, unixepoch(), unixepoch())
             RETURNING *",
            params![id, session_name, title, user_id],
            row_to_chat,
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// List all chats belonging to a session with message count and last-message
    /// preview in a single query (avoids N+1).
    /// Accepts a prefix — e.g. `agent:<id>:` matches both legacy `agent:<id>:web`
    /// and per-thread `agent:<id>:thread:<uuid>` session names.
    pub fn list_chats_by_session_enriched(
        &self,
        session_name_prefix: &str,
    ) -> Result<Vec<(Chat, i64, String)>, NeboError> {
        let conn = self.conn()?;
        let like_pattern = format!("{}%", session_name_prefix);
        let mut stmt = conn
            .prepare(&format!(
                "SELECT c.*,
                        COALESCE(s.cnt, 0) AS msg_count,
                        COALESCE(s.last_content, '') AS last_content
                 FROM chats c
                 LEFT JOIN (
                     SELECT m.chat_id,
                            COUNT(*) AS cnt,
                            {last_visible} AS last_content
                     FROM chat_messages m
                     GROUP BY m.chat_id
                 ) s ON s.chat_id = c.id
                 WHERE c.session_name LIKE ?1
                   -- Internal tooling surfaces (Architect/help sessions) are
                   -- not conversations; they bled raw session keys into the
                   -- Chats tab.
                   AND c.session_name NOT LIKE '%:help:%'
                 ORDER BY c.updated_at DESC",
                last_visible = last_visible_message_sql("m.chat_id")
            ))
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![like_pattern], |row| {
                let chat = row_to_chat(row)?;
                let msg_count: i64 = row.get("msg_count")?;
                let last_content: String = row.get("last_content")?;
                Ok((chat, msg_count, last_content))
            })
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// Chats that were mid-turn when the server stopped: the last message is the user's (or a
    /// tool result) with no assistant reply, and recent enough (`since_epoch`) to be a live
    /// interruption rather than an abandoned thread. Returns (chat_id, session_name). Used at
    /// startup to notify the user that an in-flight run was lost (runs aren't resumed).
    pub fn find_interrupted_chats(
        &self,
        since_epoch: i64,
    ) -> Result<Vec<(String, String)>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT c.id, COALESCE(c.session_name, '')
                 FROM chats c
                 WHERE (SELECT m.role FROM chat_messages m
                        WHERE m.chat_id = c.id
                        ORDER BY m.created_at DESC, m.id DESC LIMIT 1) IN ('user', 'tool')
                   AND (SELECT m2.created_at FROM chat_messages m2
                        WHERE m2.chat_id = c.id
                        ORDER BY m2.created_at DESC, m2.id DESC LIMIT 1) >= ?1
                 ORDER BY c.updated_at DESC
                 LIMIT 20",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![since_epoch], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// List all chats belonging to a session, newest first.
    pub fn list_chats_by_session(&self, session_name: &str) -> Result<Vec<Chat>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare("SELECT * FROM chats WHERE session_name = ?1 ORDER BY updated_at DESC")
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![session_name], row_to_chat)
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// Create a new companion chat for the given user_id.
    pub fn create_companion_chat(&self, id: &str, user_id: &str) -> Result<Chat, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "INSERT INTO chats (id, user_id, title, created_at, updated_at)
             VALUES (?1, ?2, 'Companion', unixepoch(), unixepoch())
             RETURNING *",
            params![id, user_id],
            row_to_chat,
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// Get the most recent companion chat for a user, or None.
    pub fn get_companion_chat_by_user(&self, user_id: &str) -> Result<Option<Chat>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT * FROM chats WHERE user_id = ?1 ORDER BY updated_at DESC LIMIT 1",
            params![user_id],
            row_to_chat,
        )
        .optional()
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// Attach run-produced artifact URLs to the chat's most recent assistant
    /// message (metadata.artifacts). Artifacts are otherwise only carried on
    /// the live chat_complete event — without this they vanish from the Work
    /// panel on history reload.
    /// Id of the chat's most recent assistant message (the one run-produced
    /// artifacts attach to). Used to record per-version provenance.
    pub fn latest_assistant_message_id(&self, chat_id: &str) -> Result<Option<String>, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT id FROM chat_messages
             WHERE chat_id = ?1 AND role = 'assistant'
             ORDER BY created_at DESC, rowid DESC LIMIT 1",
            params![chat_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    pub fn attach_artifacts_to_latest_assistant_message(
        &self,
        chat_id: &str,
        artifacts: &[serde_json::Value],
    ) -> Result<(), NeboError> {
        let conn = self.conn()?;
        let json = serde_json::to_string(artifacts)
            .map_err(|e| NeboError::Internal(format!("serialize artifacts: {e}")))?;
        conn.execute(
            "UPDATE chat_messages
             SET metadata = json_set(COALESCE(NULLIF(metadata, ''), '{}'), '$.artifacts', json(?2))
             WHERE id = (SELECT id FROM chat_messages
                         WHERE chat_id = ?1 AND role = 'assistant'
                         ORDER BY created_at DESC, rowid DESC LIMIT 1)",
            params![chat_id, json],
        )
        .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(())
    }

    /// The agent's most recently active conversation — the secondary-agent
    /// counterpart of `get_companion_chat_by_user`, used to unify inbound loop
    /// DMs with the agent's current local conversation. Ordered by last message
    /// activity because chats.updated_at is set at creation, not per message.
    pub fn get_latest_agent_chat(&self, agent_id: &str) -> Result<Option<Chat>, NeboError> {
        Ok(self.list_recent_agent_chats(agent_id, 1)?.into_iter().next().map(|(c, _)| c))
    }

    /// The roster's one-line preview for an employee: the last visible message
    /// of its latest thread. None when it has never chatted.
    pub fn latest_agent_chat_preview(&self, agent_id: &str) -> Result<Option<String>, NeboError> {
        let Some(chat) = self.get_latest_agent_chat(agent_id)? else { return Ok(None) };
        let conn = self.conn()?;
        let content: Option<String> = conn
            .query_row(&format!("SELECT {}", last_visible_message_sql("?1")), params![chat.id], |row| row.get(0))
            .map_err(|e| NeboError::Database(e.to_string()))?;
        Ok(content.filter(|c| !c.is_empty()))
    }

    /// How many threads this employee has (an isolated employee's matters).
    pub fn count_agent_chats(&self, agent_id: &str) -> Result<i64, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT COUNT(*) FROM chats WHERE session_name LIKE 'agent:' || ?1 || ':%'",
            params![agent_id],
            |row| row.get(0),
        )
        .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// (local day, agent id, count) of owner messages in employee threads at
    /// or after `since` (unix seconds): one chat turn per message the owner
    /// sent. The agent id is the segment after "agent:" in the session name.
    pub fn count_chat_turns_by_day(
        &self,
        since: i64,
    ) -> Result<Vec<(String, String, i64)>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT date(m.created_at, 'unixepoch', 'localtime') AS day,
                        substr(c.session_name, 7, instr(substr(c.session_name, 7), ':') - 1) AS agent_id,
                        COUNT(*)
                 FROM chat_messages m JOIN chats c ON c.id = m.chat_id
                 WHERE m.role = 'user' AND m.created_at >= ?1
                   AND c.session_name LIKE 'agent:%:%'
                 GROUP BY day, agent_id",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![since], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// This employee's threads, newest activity first, each with that
    /// activity's time in unix seconds (last message, else the row's update).
    pub fn list_recent_agent_chats(
        &self,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<(Chat, i64)>, NeboError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT chats.*, COALESCE(
                     (SELECT MAX(m.created_at) FROM chat_messages m WHERE m.chat_id = chats.id),
                     updated_at
                 ) AS last_activity
                 FROM chats
                 WHERE session_name LIKE 'agent:' || ?1 || ':%'
                 ORDER BY last_activity DESC
                 LIMIT ?2",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![agent_id, limit as i64], |row| {
                Ok((row_to_chat(row)?, row.get::<_, i64>("last_activity")?))
            })
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
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
                "SELECT * FROM chat_messages WHERE chat_id = ?1 AND day_marker = ?2 AND rowid > COALESCE((SELECT compacted_below_rowid FROM chats WHERE id = ?1), 0)
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
            .query_map(params![chat_id, created_at], |row| row_to_chat_message(row))
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
        session_name: row.get("session_name")?,
        title_custom: row.get("title_custom")?,
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
        html: None,
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

impl Store {
    /// Full-text search across every chat's messages (FTS5, ranked). Distinct
    /// from search_chat_messages, which is the in-chat find (one chat, LIKE,
    /// full rows). Tool/system rows are excluded — this searches the
    /// conversation, not tool output. Terms are quoted so user text can't
    /// break FTS syntax.
    pub fn search_chats(&self, query: &str, limit: i64) -> Result<Vec<ChatSearchHit>, NeboError> {
        let fts_query = query
            .split_whitespace()
            .map(|t| format!("\"{}\"", t.replace('"', "")))
            .collect::<Vec<_>>()
            .join(" ");
        if fts_query.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT m.id, m.chat_id, COALESCE(c.title, '') AS chat_title, m.role, m.created_at,
                        snippet(chat_messages_fts, 0, '\u{ab}', '\u{bb}', '\u{2026}', 16) AS snip
                 FROM chat_messages_fts
                 JOIN chat_messages m ON m.rowid = chat_messages_fts.rowid
                 LEFT JOIN chats c ON c.id = m.chat_id
                 WHERE chat_messages_fts MATCH ?1
                   AND m.role IN ('user', 'assistant')
                 ORDER BY rank
                 LIMIT ?2",
            )
            .map_err(|e| NeboError::Database(e.to_string()))?;
        let rows = stmt
            .query_map(params![fts_query, limit], |row| {
                Ok(ChatSearchHit {
                    message_id: row.get(0)?,
                    chat_id: row.get(1)?,
                    chat_title: row.get(2)?,
                    role: row.get(3)?,
                    created_at: row.get(4)?,
                    snippet: row.get(5)?,
                })
            })
            .map_err(|e| NeboError::Database(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| NeboError::Database(e.to_string()))
    }

    /// Check the chat-message FTS index and (re)build if missing or corrupt.
    /// Same self-healing contract as the memories FTS — called from the one
    /// startup hook, ensure_fts_healthy().
    pub(crate) fn ensure_chat_fts_healthy(&self) -> Result<(), NeboError> {
        let conn = self.conn()?;
        let fts_exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='chat_messages_fts')",
                [],
                |row: &rusqlite::Row| row.get(0),
            )
            .unwrap_or(false);
        if !fts_exists {
            tracing::warn!("chat_messages_fts table missing — rebuilding");
            self.rebuild_chat_fts()?;
            return Ok(());
        }
        let fts_ok = conn
            .execute(
                "INSERT INTO chat_messages_fts(chat_messages_fts) VALUES('integrity-check')",
                [],
            )
            .is_ok();
        if !fts_ok {
            tracing::warn!("chat_messages_fts integrity check failed — rebuilding");
            self.rebuild_chat_fts()?;
        }
        Ok(())
    }

    /// Rebuild the chat FTS5 table and sync triggers from scratch.
    fn rebuild_chat_fts(&self) -> Result<(), NeboError> {
        let conn = self.conn()?;
        conn.execute_batch(
            "DROP TRIGGER IF EXISTS chat_messages_fts_ai;
             DROP TRIGGER IF EXISTS chat_messages_fts_au;
             DROP TRIGGER IF EXISTS chat_messages_fts_ad;
             DROP TABLE IF EXISTS chat_messages_fts;

             CREATE VIRTUAL TABLE chat_messages_fts USING fts5(
                 content,
                 content='chat_messages',
                 content_rowid='rowid'
             );

             INSERT INTO chat_messages_fts(rowid, content)
                 SELECT rowid, content FROM chat_messages;

             CREATE TRIGGER chat_messages_fts_ai AFTER INSERT ON chat_messages BEGIN
                 INSERT INTO chat_messages_fts(rowid, content)
                 VALUES (new.rowid, new.content);
             END;

             CREATE TRIGGER chat_messages_fts_au AFTER UPDATE OF content ON chat_messages BEGIN
                 INSERT INTO chat_messages_fts(chat_messages_fts, rowid, content)
                 VALUES ('delete', old.rowid, old.content);
                 INSERT INTO chat_messages_fts(rowid, content)
                 VALUES (new.rowid, new.content);
             END;

             CREATE TRIGGER chat_messages_fts_ad AFTER DELETE ON chat_messages BEGIN
                 INSERT INTO chat_messages_fts(chat_messages_fts, rowid, content)
                 VALUES ('delete', old.rowid, old.content);
             END;",
        )
        .map_err(|e| NeboError::Database(format!("rebuild_chat_fts failed: {}", e)))?;
        tracing::info!("chat_messages_fts rebuilt successfully");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::Store;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nebo-chats-test.db");
        let store = Store::new(&path.to_string_lossy()).expect("store");
        (dir, store)
    }

    fn set_created_at(store: &Store, message_id: &str, ts: i64) {
        store
            .conn()
            .unwrap()
            .execute(
                "UPDATE chat_messages SET created_at = ?2 WHERE id = ?1",
                rusqlite::params![message_id, ts],
            )
            .unwrap();
    }

    /// Manual compaction keeps every row and projects the summary: reads
    /// start above the floor, the summary is the first visible message, a
    /// second compaction yields one visible summary, and the rows are still
    /// on disk.
    #[test]
    fn manual_compact_keeps_rows_and_projects_the_summary() {
        let (_dir, store) = store();
        store.create_chat("c1", "Chat").unwrap();
        for id in ["m1", "m2", "m3"] {
            store.create_chat_message(id, "c1", "user", id, None).unwrap();
        }
        store.compact_chat_history("c1", "s1", "**Conversation Summary**\nfirst").unwrap();
        let visible = store.get_chat_messages("c1").unwrap();
        assert_eq!(visible.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), vec!["s1"]);
        let on_disk: i64 = store
            .conn()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM chat_messages WHERE chat_id = 'c1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(on_disk, 4, "rows are kept, not deleted");
        // New messages land above the floor; the paginated and budgeted reads agree.
        store.create_chat_message("m4", "c1", "user", "m4", None).unwrap();
        assert_eq!(store.get_chat_messages_paginated("c1", 10, None).unwrap().len(), 2);
        assert_eq!(store.get_chat_messages_budgeted("c1", 100_000, None).unwrap().len(), 2);
        // Compact again: one visible summary, floor moved.
        store.compact_chat_history("c1", "s2", "**Conversation Summary**\nsecond").unwrap();
        let visible = store.get_chat_messages("c1").unwrap();
        assert_eq!(visible.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), vec!["s2"]);
    }

    /// Frozen renderings round-trip and never overwrite.
    #[test]
    fn frozen_map_round_trips_through_the_store() {
        let (_dir, store) = store();
        store.create_chat("c1", "Chat").unwrap();
        store
            .insert_chat_renderings("c1", &[("call_1".into(), "[os:shell] ls, 40 lines trimmed".into())])
            .unwrap();
        store
            .insert_chat_renderings("c1", &[("call_1".into(), "DIFFERENT".into()), ("call_2".into(), "two".into())])
            .unwrap();
        let map = store.get_chat_renderings("c1").unwrap();
        assert_eq!(map.get("call_1").map(String::as_str), Some("[os:shell] ls, 40 lines trimmed"), "first rendering wins");
        assert_eq!(map.get("call_2").map(String::as_str), Some("two"));
        assert!(store.get_chat_renderings("other").unwrap().is_empty());
    }

    /// get_chat_messages returns rows ordered by created_at ASC, not by
    /// insertion order — history renders chronologically.
    #[test]
    fn messages_read_back_in_created_at_order() {
        let (_dir, store) = store();
        store.create_chat("c1", "Chat").unwrap();
        for id in ["m1", "m2", "m3"] {
            store.create_chat_message(id, "c1", "user", id, None).unwrap();
        }
        // Rewrite timestamps out of insertion order.
        set_created_at(&store, "m3", 100);
        set_created_at(&store, "m1", 200);
        set_created_at(&store, "m2", 300);

        let ids: Vec<String> = store
            .get_chat_messages("c1")
            .unwrap()
            .into_iter()
            .map(|m| m.id)
            .collect();
        assert_eq!(ids, vec!["m3", "m1", "m2"]);
    }

    /// Messages landing in the same second keep insertion order (rowid
    /// tiebreak) — a burst of tool rows must not shuffle within the second.
    #[test]
    fn same_second_messages_keep_insertion_order() {
        let (_dir, store) = store();
        store.create_chat("c1", "Chat").unwrap();
        for id in ["a", "b", "c"] {
            store.create_chat_message(id, "c1", "assistant", id, None).unwrap();
            set_created_at(&store, id, 1000);
        }
        let ids: Vec<String> = store
            .get_chat_messages("c1")
            .unwrap()
            .into_iter()
            .map(|m| m.id)
            .collect();
        assert_eq!(ids, vec!["a", "b", "c"]);
    }

    /// tool_calls / tool_results JSON survives the write/read round trip
    /// byte-comparable as JSON, and the runner write auto-creates the parent
    /// chat row carrying session_name (role/channel sessions never pre-create
    /// their chat).
    #[test]
    fn runner_message_json_round_trip_and_parent_chat() {
        let (_dir, store) = store();
        let tool_calls = r#"[{"id":"t1","name":"system","input":{"cmd":"ls","n":2}}]"#;
        let tool_results = r#"[{"tool_use_id":"t1","content":"ok \"quoted\""}]"#;
        let msg = store
            .create_chat_message_for_runner(
                "m1",
                "agent:emp:web",
                "assistant",
                "ran a tool",
                Some(tool_calls),
                Some(tool_results),
                Some(42),
                Some(r#"{"k":"v"}"#),
                Some("agent:emp:web"),
            )
            .unwrap();
        assert_eq!(msg.token_estimate, Some(42));

        let read = store.get_chat_messages("agent:emp:web").unwrap();
        assert_eq!(read.len(), 1);
        let m = &read[0];
        let got_calls: serde_json::Value =
            serde_json::from_str(m.tool_calls.as_deref().unwrap()).unwrap();
        let want_calls: serde_json::Value = serde_json::from_str(tool_calls).unwrap();
        assert_eq!(got_calls, want_calls);
        let got_results: serde_json::Value =
            serde_json::from_str(m.tool_results.as_deref().unwrap()).unwrap();
        let want_results: serde_json::Value = serde_json::from_str(tool_results).unwrap();
        assert_eq!(got_results, want_results);

        // Parent chat row was auto-created and linked to the session.
        let chat = store.get_chat("agent:emp:web").unwrap().expect("chat row");
        assert_eq!(chat.session_name.as_deref(), Some("agent:emp:web"));
    }

    /// Deleting a chat cascades to its messages (FK ON DELETE CASCADE with
    /// foreign_keys pragma live on pooled connections) — no orphan rows.
    #[test]
    fn delete_chat_cascades_messages() {
        let (_dir, store) = store();
        store.create_chat("c1", "Chat").unwrap();
        store.create_chat("c2", "Other").unwrap();
        store.create_chat_message("m1", "c1", "user", "hi", None).unwrap();
        store.create_chat_message("m2", "c1", "assistant", "yo", None).unwrap();
        store.create_chat_message("m3", "c2", "user", "keep", None).unwrap();

        store.delete_chat("c1").unwrap();

        assert!(store.get_chat("c1").unwrap().is_none());
        let orphans: i64 = store
            .conn()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM chat_messages WHERE chat_id = 'c1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(orphans, 0, "messages must cascade with their chat");
        // The other chat's messages are untouched.
        assert_eq!(store.get_chat_messages("c2").unwrap().len(), 1);
    }

    /// Pagination returns the most recent N in ascending order, and a
    /// `before` cursor walks strictly older messages.
    #[test]
    fn paginated_window_and_before_cursor() {
        let (_dir, store) = store();
        store.create_chat("c1", "Chat").unwrap();
        for (id, ts) in [("m1", 100), ("m2", 200), ("m3", 300), ("m4", 400), ("m5", 500)] {
            store.create_chat_message(id, "c1", "user", id, None).unwrap();
            set_created_at(&store, id, ts);
        }

        let last_two: Vec<String> = store
            .get_chat_messages_paginated("c1", 2, None)
            .unwrap()
            .into_iter()
            .map(|m| m.id)
            .collect();
        assert_eq!(last_two, vec!["m4", "m5"], "latest window, ascending");

        let older: Vec<String> = store
            .get_chat_messages_paginated("c1", 2, Some("m3"))
            .unwrap()
            .into_iter()
            .map(|m| m.id)
            .collect();
        assert_eq!(older, vec!["m1", "m2"], "before-cursor page, ascending");
    }

    /// The rotate_chat DB contract (SessionManager::rotate_chat in
    /// crates/agent composes exactly these Store calls): a new conversation
    /// under the same session is NON-destructive — the old chat row and all
    /// its messages survive, both chats list under the session, and only the
    /// session's conversation-scoped counters reset.
    #[test]
    fn rotate_chat_sequence_preserves_old_conversation() {
        let (_dir, store) = store();
        let session_name = "agent:emp:web";
        store
            .create_session("s1", Some(session_name), None, None, None)
            .unwrap();

        // Conversation A with history.
        store
            .create_chat_for_session("chat-a", session_name, "Chat", None)
            .unwrap();
        store.set_session_active_chat_id("s1", "chat-a").unwrap();
        store.create_chat_message("m1", "chat-a", "user", "hello", None).unwrap();
        store.create_chat_message("m2", "chat-a", "assistant", "hi", None).unwrap();
        store.update_session_stats("s1", 1234, 2).unwrap();
        store.set_session_model_override("s1", Some("model-x"), None).unwrap();

        // The rotate sequence.
        store
            .create_chat_for_session("chat-b", session_name, "New Chat", None)
            .unwrap();
        store.set_session_active_chat_id("s1", "chat-b").unwrap();
        store.reset_session_counters("s1").unwrap();

        // Old conversation is fully intact.
        assert!(store.get_chat("chat-a").unwrap().is_some());
        let old_ids: Vec<String> = store
            .get_chat_messages("chat-a")
            .unwrap()
            .into_iter()
            .map(|m| m.id)
            .collect();
        assert_eq!(old_ids, vec!["m1", "m2"], "rotate must not touch old messages");

        // Both conversations belong to the session.
        let chats = store.list_chats_by_session(session_name).unwrap();
        let mut chat_ids: Vec<String> = chats.into_iter().map(|c| c.id).collect();
        chat_ids.sort();
        assert_eq!(chat_ids, vec!["chat-a", "chat-b"]);

        // Session now points at the new conversation with fresh counters,
        // preferences intact.
        let s = store.get_session("s1").unwrap().unwrap();
        assert_eq!(s.active_chat_id.as_deref(), Some("chat-b"));
        assert_eq!(s.message_count, Some(0));
        assert_eq!(s.token_count, Some(0));
        assert_eq!(s.model_override.as_deref(), Some("model-x"));
        // And the ONE chat-id derivation resolves to the new conversation.
        assert_eq!(store.resolve_session_chat_id("s1"), "chat-b");
    }
}
