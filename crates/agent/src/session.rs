use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use db::models::{ChatMessage, Session};
use db::Store;
use types::NeboError;

/// Manages agent sessions backed by the database.
///
/// Sessions are containers that hold conversation-scoped state (model overrides,
/// preferences, etc.). Each session points to an `active_chat_id` which identifies
/// the current conversation's messages. Rotating the chat creates a new conversation
/// under the same session, preserving old messages and session-level settings.
#[derive(Clone)]
pub struct SessionManager {
    store: Arc<Store>,
    /// Cache: session_id -> active_chat_id for fast message lookups.
    chat_ids: Arc<RwLock<HashMap<String, String>>>,
    /// Cache: session_id -> session_key (name) for routing lookups.
    session_keys: Arc<RwLock<HashMap<String, String>>>,
}

impl SessionManager {
    pub fn new(store: Arc<Store>) -> Self {
        Self {
            store,
            chat_ids: Arc::new(RwLock::new(HashMap::new())),
            session_keys: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Get or create a session by key, optionally scoped to a user.
    /// Ensures the session has a valid `active_chat_id` for message storage.
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

        // Ensure active_chat_id is set. Existing sessions get it from the migration
        // backfill; truly new sessions or missed migrations need a runtime fallback.
        let chat_id = if let Some(ref cid) = session.active_chat_id {
            cid.clone()
        } else {
            // Legacy session without active_chat_id — use session name (= old behavior).
            let fallback = session.name.clone().unwrap_or_default();
            if !fallback.is_empty() {
                if let Err(e) = self.store.set_session_active_chat_id(&session.id, &fallback) {
                    tracing::warn!("failed to backfill active_chat_id for session {}: {}", session.id, e);
                }
            }
            fallback
        };

        // Cache both mappings.
        let key = session.name.clone().unwrap_or_default();
        if let Ok(mut cache) = self.chat_ids.write() {
            cache.insert(session.id.clone(), chat_id);
        }
        if let Ok(mut cache) = self.session_keys.write() {
            cache.insert(session.id.clone(), key);
        }

        Ok(session)
    }

    /// Resolve a session key (name) to the session's internal UUID.
    /// Used by WS handlers that receive the frontend's session identifier.
    pub fn resolve_session_id_by_key(&self, session_key: &str) -> Result<String, NeboError> {
        // Check reverse cache (key → id) via session_keys which maps id → key
        if let Ok(cache) = self.session_keys.read() {
            for (id, key) in cache.iter() {
                if key == session_key {
                    return Ok(id.clone());
                }
            }
        }

        // Fallback to DB lookup by name
        match self.store.get_session_by_name(session_key)? {
            Some(session) => Ok(session.id),
            None => Err(NeboError::NotFound),
        }
    }

    /// Resolve session ID to session key (name), using cache.
    /// Still needed for routing, keyparser, and compact handler.
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
    /// Returns the session's active_chat_id, falling back to session_key (name)
    /// for backward compatibility with sessions that predate the decoupling.
    fn resolve_chat_id(&self, session_id: &str) -> String {
        // Check cache first
        if let Ok(cache) = self.chat_ids.read() {
            if let Some(id) = cache.get(session_id) {
                return id.clone();
            }
        }

        // Load from DB — prefer active_chat_id, fall back to name
        let chat_id = self.store.get_session(session_id)
            .ok()
            .flatten()
            .and_then(|s| s.active_chat_id.or(s.name))
            .unwrap_or_else(|| format!("chat-{}", session_id));

        if let Ok(mut cache) = self.chat_ids.write() {
            cache.insert(session_id.to_string(), chat_id.clone());
        }

        tracing::debug!(
            session_id = %session_id,
            chat_id = %chat_id,
            "resolved chat_id for message storage"
        );

        chat_id
    }

    /// Public accessor for the resolved chat_id.
    pub fn active_chat_id(&self, session_id: &str) -> String {
        self.resolve_chat_id(session_id)
    }

    /// Get messages for a session's active conversation.
    pub fn get_messages(&self, session_id: &str) -> Result<Vec<ChatMessage>, NeboError> {
        let chat_id = self.resolve_chat_id(session_id);
        let messages = self.store.get_chat_messages(&chat_id)?;
        Ok(sanitize_messages(messages))
    }

    /// Append a message to the session's active conversation.
    pub fn append_message(
        &self,
        session_id: &str,
        role: &str,
        content: &str,
        tool_calls: Option<&str>,
        tool_results: Option<&str>,
        metadata: Option<&str>,
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
            metadata,
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

    /// Create a new conversation under the same session, preserving old messages.
    /// Returns the new chat_id. Pass `user_id` to carry forward ownership (e.g. companion chats).
    pub fn rotate_chat(&self, session_id: &str, user_id: Option<&str>) -> Result<String, NeboError> {
        let session = self.store.get_session(session_id)?
            .ok_or(NeboError::NotFound)?;

        let session_name = session.name.clone().unwrap_or_default();
        let new_chat_id = uuid::Uuid::new_v4().to_string();

        // Create a new chat row linked to this session.
        self.store.create_chat_for_session(
            &new_chat_id,
            &session_name,
            "New Chat",
            user_id,
        )?;

        // Point the session to the new chat.
        self.store.set_session_active_chat_id(session_id, &new_chat_id)?;

        // Reset conversation-scoped counters; preserve session-level preferences.
        self.store.reset_session_counters(session_id)?;

        // Update cache.
        if let Ok(mut cache) = self.chat_ids.write() {
            cache.insert(session_id.to_string(), new_chat_id.clone());
        }

        Ok(new_chat_id)
    }

    /// Reset a session by rotating to a new conversation.
    /// Old messages are preserved. Returns the new chat_id.
    /// Carries forward the user_id from the current active chat so the new chat
    /// remains discoverable by get_companion_chat_by_user().
    pub fn reset(&self, session_id: &str) -> Result<String, NeboError> {
        let current_chat_id = self.resolve_chat_id(session_id);
        let user_id = self.store.get_chat(&current_chat_id)
            .ok()
            .flatten()
            .and_then(|c| c.user_id);
        self.rotate_chat(session_id, user_id.as_deref())
    }

    /// Clear messages within the current conversation (used by compact).
    /// Unlike reset/rotate, this stays in the same conversation.
    pub fn clear_current_messages(&self, session_id: &str) -> Result<(), NeboError> {
        let chat_id = self.resolve_chat_id(session_id);
        self.store.delete_chat_messages_by_chat_id(&chat_id)?;
        self.store.reset_session_counters(session_id)?;
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
