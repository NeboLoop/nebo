use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use db::models::{ChatMessage, Session};
use db::Store;
use types::NeboError;

/// Manages agent sessions backed by the database.
pub struct SessionManager {
    store: Arc<Store>,
    /// Cache: session_id -> session_key (name) for fast lookups.
    session_keys: Arc<RwLock<HashMap<String, String>>>,
}

impl SessionManager {
    pub fn new(store: Arc<Store>) -> Self {
        Self {
            store,
            session_keys: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Get or create a session by key, optionally scoped to a user.
    pub fn get_or_create(&self, session_key: &str, user_id: &str) -> Result<Session, NeboError> {
        let id = uuid::Uuid::new_v4().to_string();
        let (scope, scope_id) = if user_id.is_empty() {
            ("agent", "")
        } else {
            ("user", user_id)
        };

        let session = self.store.get_or_create_scoped_session(
            &id,
            session_key,
            scope,
            scope_id,
            None,
        )?;

        // Cache session_id -> session_key mapping synchronously.
        // The session_key IS the companion chat ID (sent by the frontend),
        // so we use it directly as chat_id for message storage.
        let key = session.name.clone().unwrap_or_default();
        if let Ok(mut cache) = self.session_keys.write() {
            cache.insert(session.id.clone(), key);
        }

        Ok(session)
    }

    /// Resolve session ID to session key (name), using cache.
    /// The session_key is used as the chat_id for message storage,
    /// matching what the frontend expects (companion chat ID).
    pub fn resolve_session_key(&self, session_id: &str) -> Result<String, NeboError> {
        // Check cache first
        if let Ok(cache) = self.session_keys.read() {
            if let Some(key) = cache.get(session_id) {
                return Ok(key.clone());
            }
        }

        // Fallback to DB
        let session = self.store.get_session(session_id)?;
        let key = session
            .and_then(|s| s.name)
            .unwrap_or_default();

        if let Ok(mut cache) = self.session_keys.write() {
            cache.insert(session_id.to_string(), key.clone());
        }

        Ok(key)
    }

    /// Resolve session_id to the chat_id used for message storage.
    /// This is the session_key (= companion chat ID from frontend).
    fn resolve_chat_id(&self, session_id: &str) -> String {
        match self.resolve_session_key(session_id) {
            Ok(key) if !key.is_empty() => key,
            _ => format!("chat-{}", session_id), // fallback for legacy data
        }
    }

    /// Get messages for a session (resolves session_id to chat_id via session_key).
    pub fn get_messages(&self, session_id: &str) -> Result<Vec<ChatMessage>, NeboError> {
        let chat_id = self.resolve_chat_id(session_id);
        let messages = self.store.get_chat_messages(&chat_id)?;
        Ok(sanitize_messages(messages))
    }

    /// Append a message to the session.
    pub fn append_message(
        &self,
        session_id: &str,
        role: &str,
        content: &str,
        tool_calls: Option<&str>,
        tool_results: Option<&str>,
    ) -> Result<ChatMessage, NeboError> {
        // Skip truly empty messages
        if content.is_empty()
            && tool_calls.map_or(true, |tc| tc.is_empty() || tc == "[]" || tc == "null")
            && tool_results.map_or(true, |tr| tr.is_empty() || tr == "[]" || tr == "null")
        {
            return Err(NeboError::Validation("empty message".to_string()));
        }

        let chat_id = self.resolve_chat_id(session_id);
        let msg_id = uuid::Uuid::new_v4().to_string();

        let token_estimate = estimate_tokens(content, tool_calls, tool_results);

        let msg = self.store.create_chat_message_for_runner(
            &msg_id,
            &chat_id,
            role,
            content,
            tool_calls,
            tool_results,
            Some(token_estimate),
        )?;

        let _ = self.store.increment_session_message_count(session_id);

        Ok(msg)
    }

    /// Get the rolling compaction summary.
    pub fn get_summary(&self, session_id: &str) -> Result<String, NeboError> {
        let session = self.store.get_session(session_id)?;
        Ok(session.and_then(|s| s.summary).unwrap_or_default())
    }

    /// Update the rolling summary.
    pub fn update_summary(&self, session_id: &str, summary: &str) -> Result<(), NeboError> {
        self.store.update_session_summary(session_id, summary)
    }

    /// Get the pinned active task/objective.
    pub fn get_active_task(&self, session_id: &str) -> Result<String, NeboError> {
        self.store.get_session_active_task(session_id)
    }

    /// Set the active task.
    pub fn set_active_task(&self, session_id: &str, task: &str) -> Result<(), NeboError> {
        self.store.set_session_active_task(session_id, task)
    }

    /// Clear the active task.
    pub fn clear_active_task(&self, session_id: &str) -> Result<(), NeboError> {
        self.store.clear_session_active_task(session_id)
    }

    /// Get work tasks JSON.
    pub fn get_work_tasks(&self, session_id: &str) -> Result<String, NeboError> {
        self.store.get_session_work_tasks(session_id)
    }

    /// Set work tasks JSON.
    pub fn set_work_tasks(&self, session_id: &str, tasks_json: &str) -> Result<(), NeboError> {
        self.store.set_session_work_tasks(session_id, tasks_json)
    }

    /// Reset a session (clear messages and counters).
    pub fn reset(&self, session_id: &str) -> Result<(), NeboError> {
        let chat_id = self.resolve_chat_id(session_id);
        self.store.delete_chat_messages_by_chat_id(&chat_id)?;
        self.store.reset_session(session_id)?;
        Ok(())
    }

    /// List sessions by scope.
    pub fn list_sessions(&self, scope: &str) -> Result<Vec<Session>, NeboError> {
        self.store.list_sessions_by_scope(scope)
    }

    /// Delete a session and its messages.
    pub fn delete_session(&self, session_id: &str) -> Result<(), NeboError> {
        let chat_id = self.resolve_chat_id(session_id);
        self.store.delete_chat_messages_by_chat_id(&chat_id)?;
        self.store.delete_session(session_id)?;
        Ok(())
    }

    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }
}

/// Estimate token count from content lengths (chars / 4 heuristic).
fn estimate_tokens(content: &str, tool_calls: Option<&str>, tool_results: Option<&str>) -> i64 {
    let mut chars = content.len();
    if let Some(tc) = tool_calls {
        chars += tc.len();
    }
    if let Some(tr) = tool_results {
        chars += tr.len();
    }
    (chars / 4) as i64
}

/// Remove orphaned tool results that have no matching tool call in the conversation.
fn sanitize_messages(messages: Vec<ChatMessage>) -> Vec<ChatMessage> {
    // Collect all tool call IDs from assistant messages
    let mut known_call_ids = std::collections::HashSet::new();
    for msg in &messages {
        if msg.role == "assistant" {
            if let Some(ref tc_json) = msg.tool_calls {
                if let Ok(calls) = serde_json::from_str::<Vec<serde_json::Value>>(tc_json) {
                    for call in &calls {
                        if let Some(id) = call.get("id").and_then(|v| v.as_str()) {
                            known_call_ids.insert(id.to_string());
                        }
                    }
                }
            }
        }
    }

    messages
        .into_iter()
        .filter(|msg| {
            // Keep all non-tool messages
            if msg.role != "tool" {
                return true;
            }

            // For tool messages, check if their tool results reference known calls
            if let Some(ref tr_json) = msg.tool_results {
                if let Ok(results) = serde_json::from_str::<Vec<serde_json::Value>>(tr_json) {
                    let has_valid = results.iter().any(|r| {
                        r.get("tool_call_id")
                            .and_then(|v| v.as_str())
                            .is_some_and(|id| known_call_ids.contains(id))
                    });
                    return has_valid;
                }
            }

            // Keep if we can't parse (be conservative)
            true
        })
        .collect()
}
