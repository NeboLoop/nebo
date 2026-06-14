use rusqlite::params;

use crate::Store;
use crate::models::{Chat, ChatMessage};
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
                "SELECT * FROM chat_messages WHERE chat_id = ?1 ORDER BY created_at ASC, rowid ASC",
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
            let cursor_ts: i64 = conn
                .query_row(
                    "SELECT created_at FROM chat_messages WHERE id = ?1",
                    params![before_id],
                    |row| row.get(0),
                )
                .map_err(|e| NeboError::Database(e.to_string()))?;

            let mut stmt = conn
                .prepare(
                    "SELECT * FROM chat_messages WHERE chat_id = ?1 AND created_at < ?2
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
                    SELECT * FROM chat_messages WHERE chat_id = ?1
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
            let cursor_ts: i64 = conn
                .query_row(
                    "SELECT created_at FROM chat_messages WHERE id = ?1",
                    params![before_id],
                    |row| row.get(0),
                )
                .map_err(|e| NeboError::Database(e.to_string()))?;
            // Use composite cursor (created_at, id) to avoid skipping messages
            // created in the same second as the cursor message.
            let mut stmt = conn
                .prepare(
                    "SELECT * FROM chat_messages WHERE chat_id = ?1
                     AND (created_at < ?2 OR (created_at = ?2 AND id < ?3))
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
                    "SELECT * FROM chat_messages WHERE chat_id = ?1
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
    pub fn count_chat_messages(&self, chat_id: &str) -> Result<i64, NeboError> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT COUNT(*) FROM chat_messages WHERE chat_id = ?1 AND role IN ('user', 'assistant')",
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
            .prepare(
                "SELECT c.*,
                        COALESCE(s.cnt, 0) AS msg_count,
                        COALESCE(s.last_content, '') AS last_content
                 FROM chats c
                 LEFT JOIN (
                     SELECT m.chat_id,
                            COUNT(*) AS cnt,
                            -- Preview = last VISIBLE message: skip tool results (empty),
                            -- empty content, and hidden system-injected messages
                            -- (reminders carry metadata {\"hidden\":true}).
                            (SELECT m2.content FROM chat_messages m2
                             WHERE m2.chat_id = m.chat_id
                               AND m2.role != 'tool'
                               AND m2.content != ''
                               AND (m2.metadata IS NULL OR m2.metadata NOT LIKE '%\"hidden\":true%')
                             ORDER BY m2.created_at DESC, m2.id DESC LIMIT 1) AS last_content
                     FROM chat_messages m
                     GROUP BY m.chat_id
                 ) s ON s.chat_id = c.id
                 WHERE c.session_name LIKE ?1
                 ORDER BY c.updated_at DESC",
            )
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
        let conn = self.conn()?;
        conn.query_row(
            "SELECT * FROM chats
             WHERE session_name LIKE 'agent:' || ?1 || ':%'
             ORDER BY COALESCE(
                 (SELECT MAX(m.created_at) FROM chat_messages m WHERE m.chat_id = chats.id),
                 updated_at
             ) DESC
             LIMIT 1",
            params![agent_id],
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
