use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use db::Store;
use db::models::{ChatMessage, Session};
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
    /// In-memory: session_id -> detected mode (e.g. "research"). Ephemeral, not persisted.
    detected_modes: Arc<RwLock<HashMap<String, String>>>,
}

impl SessionManager {
    pub fn new(store: Arc<Store>) -> Self {
        Self {
            store,
            chat_ids: Arc::new(RwLock::new(HashMap::new())),
            session_keys: Arc::new(RwLock::new(HashMap::new())),
            detected_modes: Arc::new(RwLock::new(HashMap::new())),
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

        let session =
            self.store
                .get_or_create_scoped_session(&id, session_key, scope, scope_id, None)?;

        tracing::info!(
            session_key = %session_key,
            session_id = %session.id,
            active_chat_id = ?session.active_chat_id,
            "[THREAD-DEBUG] get_or_create session"
        );

        // For thread session keys, the chat_id is always the embedded UUID —
        // regardless of what active_chat_id currently holds (it may have been
        // set to the full key string by older code).
        let chat_id = if session_key.contains(":thread:") {
            let extracted = extract_chat_id_from_key(session_key);
            if session.active_chat_id.as_deref() != Some(extracted.as_str()) {
                tracing::info!(
                    old = ?session.active_chat_id,
                    new = %extracted,
                    "[THREAD-DEBUG] correcting active_chat_id for thread session"
                );
                let _ = self.store.set_session_active_chat_id(&session.id, &extracted);
            }
            extracted
        } else if let Some(ref cid) = session.active_chat_id {
            cid.clone()
        } else {
            let fallback = extract_chat_id_from_key(session_key);
            if !fallback.is_empty() {
                if let Err(e) = self
                    .store
                    .set_session_active_chat_id(&session.id, &fallback)
                {
                    tracing::warn!(
                        "failed to backfill active_chat_id for session {}: {}",
                        session.id,
                        e
                    );
                }
            }
            fallback
        };
        tracing::info!(
            session_key = %session_key,
            chat_id = %chat_id,
            "[THREAD-DEBUG] get_or_create resolved chat_id"
        );

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
        let key = session.and_then(|s| s.name).unwrap_or_default();

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

        // Load via the one canonical derivation in the db crate
        // (active_chat_id → name → chat-{session_id}).
        let chat_id = self.store.resolve_session_chat_id(session_id);

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

    /// The conversation the harness sends: the active chat from its latest
    /// checkpoint boundary on (`harness::compact::checkpoint`), with typed
    /// attachment rows and notification rows kept, stored legacy steering
    /// dropped and tool results whose call is not loaded removed. The
    /// sliding-window path keeps `get_messages` until the cutover deletes it.
    pub fn get_messages_since_checkpoint(&self, session_id: &str) -> Result<Vec<ChatMessage>, NeboError> {
        let chat_id = self.resolve_chat_id(session_id);
        let messages = self
            .store
            .get_chat_messages_since_checkpoint(&chat_id)?
            .into_iter()
            .filter(|m| {
                !is_stored_steering(m)
                    || crate::harness::reminders::attachment_kind(m).is_some()
                    || crate::harness::delegation::notify::is_notification_row(m)
            })
            .collect();
        Ok(drop_orphan_results(messages))
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

        let session_name = self
            .session_keys
            .read()
            .ok()
            .and_then(|c| c.get(session_id).cloned());

        tracing::info!(
            session_id = %session_id,
            chat_id = %chat_id,
            session_name = ?session_name,
            role = %role,
            "[THREAD-DEBUG] append_message writing to chat_id"
        );

        let msg = self.store.create_chat_message_for_runner(
            &msg_id,
            &chat_id,
            role,
            content,
            tool_calls,
            tool_results,
            Some(token_estimate),
            metadata,
            session_name.as_deref(),
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

    /// Get the detected mode for a session (e.g. "research"). Returns empty string if none.
    pub fn get_detected_mode(&self, session_id: &str) -> String {
        self.detected_modes
            .read()
            .ok()
            .and_then(|m| m.get(session_id).cloned())
            .unwrap_or_default()
    }

    /// Set the detected mode for a session.
    pub fn set_detected_mode(&self, session_id: &str, mode: &str) {
        if let Ok(mut m) = self.detected_modes.write() {
            if mode.is_empty() {
                m.remove(session_id);
            } else {
                m.insert(session_id.to_string(), mode.to_string());
            }
        }
    }

    /// Switch the active chat for a session (updates DB and in-memory cache).
    pub fn set_active_chat(&self, session_id: &str, chat_id: &str) -> Result<(), NeboError> {
        // Switching conversations resets the session's rolling state (summary,
        // active task, compaction counters) exactly like rotate_chat does: the
        // active chat is the isolation context under context_isolated, and a
        // summary of matter A injected into matter B's prompt is a cross-matter
        // leak (isolation audit 2026-08-22, leak #4 — session state crosses
        // the ctx boundary).
        let changed = self
            .store
            .get_session(session_id)?
            .and_then(|s| s.active_chat_id)
            .is_none_or(|current| current != chat_id);
        if changed {
            self.store.reset_session_counters(session_id)?;
        }
        self.store.set_session_active_chat_id(session_id, chat_id)?;
        if let Ok(mut cache) = self.chat_ids.write() {
            cache.insert(session_id.to_string(), chat_id.to_string());
        }
        Ok(())
    }

    /// Create a new conversation under the same session, preserving old messages.
    /// Returns the new chat_id. Pass `user_id` to carry forward ownership (e.g. companion chats).
    pub fn rotate_chat(
        &self,
        session_id: &str,
        user_id: Option<&str>,
    ) -> Result<String, NeboError> {
        let session = self
            .store
            .get_session(session_id)?
            .ok_or(NeboError::NotFound)?;

        let session_name = session.name.clone().unwrap_or_default();
        let new_chat_id = uuid::Uuid::new_v4().to_string();

        let title = "New Chat".to_string();

        // Create a new chat row linked to this session.
        self.store
            .create_chat_for_session(&new_chat_id, &session_name, &title, user_id)?;

        // Point the session to the new chat.
        self.store
            .set_session_active_chat_id(session_id, &new_chat_id)?;

        // Reset conversation-scoped counters; preserve session-level preferences.
        self.store.reset_session_counters(session_id)?;

        // Clear stale compaction summary so failure narratives don't carry over.
        self.store.update_session_summary(session_id, "")?;

        // The agreed goal belonged to the old conversation, whose transcript
        // the done check can no longer read.
        self.store.delete_session_goal(session_id)?;

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
        let user_id = self
            .store
            .get_chat(&current_chat_id)
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
        // The chat row was lazily created for this session's messages — a
        // scratch conversation (workflow turn) must not linger in chat lists.
        self.store.delete_chat(&chat_id)?;
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

/// Extract the chat_id from a session key.
/// For thread keys like `agent:<id>:thread:<UUID>`, returns just the UUID.
/// For everything else, returns the full key (legacy behavior).
fn extract_chat_id_from_key(key: &str) -> String {
    if let Some(pos) = key.find(":thread:") {
        key[pos + 8..].to_string()
    } else {
        key.to_string()
    }
}

/// Steering an older build wrote into the thread. Steering rides the call it
/// was made for and is never history, but four kinds were stored as user
/// rows: the auto-continue nudge, a `<system-reminder>` briefing queued into a
/// running turn, a workroom's room briefing, and the budget-exhausted summary
/// request.
/// The rows stay on disk (owner data is never deleted); the model's history
/// never loads them — a stored "keep going" re-sent on every later turn is how
/// an employee stays fixated on old work.
fn is_stored_steering(msg: &ChatMessage) -> bool {
    if msg.role != "user" {
        return false;
    }
    if crate::goals::is_continuation_prompt(&msg.content)
        || msg.content == crate::runner::BUDGET_SUMMARY_REQUEST
    {
        return true;
    }
    let meta = msg
        .metadata
        .as_deref()
        .and_then(|m| serde_json::from_str::<serde_json::Value>(m).ok());
    let flag = |key: &str| {
        meta.as_ref()
            .and_then(|v| v.get(key))
            .and_then(|b| b.as_bool())
            .unwrap_or(false)
    };
    flag("autoContinue") || flag("roomBriefing") || (flag("isMeta") && msg.content.trim_start().starts_with("<system-reminder>"))
}

/// The model's history: stored steering dropped (see [`is_stored_steering`]),
/// then orphaned tool results that have no matching tool call removed.
fn sanitize_messages(messages: Vec<ChatMessage>) -> Vec<ChatMessage> {
    let messages: Vec<ChatMessage> = messages.into_iter().filter(|m| !is_stored_steering(m)).collect();
    drop_orphan_results(messages)
}

/// Tool results whose call is not in `messages` removed.
fn drop_orphan_results(messages: Vec<ChatMessage>) -> Vec<ChatMessage> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_manager() -> SessionManager {
        let path = std::env::temp_dir().join(format!("nebo-session-test-{}.db", uuid::Uuid::new_v4()));
        let store = Arc::new(Store::new(path.to_str().unwrap()).expect("test store"));
        SessionManager::new(store)
    }

    /// The harness load starts at the latest boundary, keeps typed
    /// attachment rows, drops stored legacy steering (no kind) and a tool
    /// result whose call is behind the boundary.
    #[test]
    fn harness_load_starts_at_the_boundary_and_keeps_attachments_only() {
        let mgr = test_manager();
        let sid = mgr.get_or_create("agent:a:web", "").unwrap().id;
        let calls = r#"[{"id":"c1","name":"os","input":{}}]"#;
        let results = r#"[{"tool_call_id":"c1","content":"ok","is_error":false}]"#;
        let reminder = crate::harness::reminders::wrap("The date is now Friday.");
        mgr.append_message(&sid, "user", "before", None, None, None).unwrap();
        mgr.append_message(&sid, "assistant", "", Some(calls), None, None).unwrap();
        mgr.append_message(&sid, "user", "summary", None, None, Some(r#"{"checkpoint":true}"#)).unwrap();
        mgr.append_message(&sid, "tool", "", None, Some(results), None).unwrap();
        mgr.append_message(&sid, "user", &reminder, None, None, Some(r#"{"attachment":{"kind":"date_changed"},"isMeta":true}"#))
            .unwrap();
        mgr.append_message(&sid, "user", &crate::harness::reminders::wrap("legacy"), None, None, Some(r#"{"isMeta":true}"#))
            .unwrap();

        let loaded = mgr.get_messages_since_checkpoint(&sid).unwrap();
        assert_eq!(loaded.iter().map(|m| m.content.as_str()).collect::<Vec<_>>(), vec!["summary", reminder.as_str()]);
        assert!(!mgr.get_messages(&sid).unwrap().iter().any(|m| m.content == reminder), "the old load drops it");
    }

    /// Switching the active chat is a matter switch under context isolation:
    /// the rolling summary (and task state) of the old conversation must not
    /// be injected into the new one.
    #[test]
    fn test_set_active_chat_resets_rolling_state_on_switch() {
        let mgr = test_manager();
        let session = mgr.get_or_create("agent:a1:web", "").expect("session");

        mgr.update_summary(&session.id, "matter A summary").expect("summary");
        let current = mgr
            .store
            .get_session(&session.id)
            .unwrap()
            .unwrap();
        assert_eq!(current.summary.as_deref(), Some("matter A summary"));
        let current_chat = current.active_chat_id.clone().unwrap_or_default();

        // Re-activating the SAME chat keeps the summary.
        mgr.set_active_chat(&session.id, &current_chat).expect("same chat");
        let same = mgr.store.get_session(&session.id).unwrap().unwrap();
        assert_eq!(same.summary.as_deref(), Some("matter A summary"));

        // Switching to a DIFFERENT chat clears it.
        mgr.set_active_chat(&session.id, "chat-matter-b").expect("switch");
        let switched = mgr.store.get_session(&session.id).unwrap().unwrap();
        assert_eq!(switched.summary, None);
        assert_eq!(switched.active_chat_id.as_deref(), Some("chat-matter-b"));
    }

    fn meta(v: serde_json::Value) -> Option<String> {
        Some(v.to_string())
    }

    /// Steering older builds stored — the auto-continue nudge (stamped
    /// `autoContinue` by migration 0131, or bare), a `<system-reminder>`
    /// briefing queued into a running turn, a room briefing, the
    /// budget-exhausted summary request — never loads into the model's
    /// history, and stays on disk. Everything that is conversation loads:
    /// the owner's words, tool results, the stop record, preloads, hidden
    /// platform prompts.
    #[test]
    fn stored_steering_never_loads_and_is_never_deleted() {
        let mgr = test_manager();
        let session = mgr.get_or_create("agent:a1:web", "").expect("session");
        let nudge = crate::goals::continuation_prompt("unfinished work in the previous response");
        let calls = r#"[{"id":"c1","name":"os","input":{}}]"#;
        let results = r#"[{"tool_call_id":"c1","content":"ok"}]"#;
        let rows: Vec<(&str, String, Option<&str>, Option<&str>, Option<String>)> = vec![
            ("user", "Draft the plan".into(), None, None, None),
            ("assistant", "".into(), Some(calls), None, None),
            ("tool", "".into(), None, Some(results), None),
            ("assistant", "Step one is done.".into(), None, None, None),
            ("user", nudge.clone(), None, None, meta(serde_json::json!({"isMeta": true, "autoContinue": true}))),
            ("user", nudge.clone(), None, None, None),
            (
                "user",
                crate::steering::wrap_system_reminder("Team \"Ops\" — mission: ship it."),
                None,
                None,
                meta(serde_json::json!({"isMeta": true})),
            ),
            (
                "user",
                "You are Ada, in the team \"Ops\". A team is where work gets DONE.".into(),
                None,
                None,
                meta(serde_json::json!({"isMeta": true, "roomBriefing": true})),
            ),
            ("user", crate::runner::BUDGET_SUMMARY_REQUEST.into(), None, None, None),
            ("user", crate::harness::conversation::INTERRUPT_MESSAGE.into(), None, None, meta(serde_json::json!({"isMeta": true}))),
            ("user", "[Loading skill: plan]\n\nsteps".into(), None, None, meta(serde_json::json!({"isMeta": true, "skillPreload": "plan"}))),
            ("user", "[Background event — not an owner message]".into(), None, None, meta(serde_json::json!({"isMeta": true, "hiddenPrompt": true}))),
            ("user", "Thanks".into(), None, None, None),
        ];
        for (role, content, tc, tr, md) in &rows {
            mgr.append_message(&session.id, role, content, *tc, *tr, md.as_deref()).expect("append");
        }

        let history = mgr.get_messages(&session.id).expect("history");
        assert!(
            history.iter().all(|m| !crate::goals::is_continuation_prompt(&m.content)),
            "a stored auto-continue nudge loaded into history"
        );
        assert!(
            history.iter().all(|m| !m.content.starts_with("<system-reminder>")),
            "a stored <system-reminder> loaded into history"
        );
        let contents: Vec<&str> = history.iter().map(|m| m.content.as_str()).collect();
        for kept in ["Draft the plan", "Step one is done.", crate::harness::conversation::INTERRUPT_MESSAGE, "Thanks"] {
            assert!(contents.contains(&kept), "conversation row dropped: {kept}");
        }
        assert!(history.iter().any(|m| m.role == "tool"), "the tool result is conversation");
        assert!(contents.iter().any(|c| c.starts_with("[Loading skill: plan]")), "a preload is not steering");
        assert!(contents.iter().any(|c| c.starts_with("[Background event")), "a hidden prompt is not steering");
        assert!(history.iter().all(|m| !m.content.contains("A team is where work gets DONE")), "a stored room briefing loaded");
        assert!(history.iter().all(|m| m.content != crate::runner::BUDGET_SUMMARY_REQUEST), "a stored summary request loaded");
        assert_eq!(history.len(), rows.len() - 5, "exactly the five steering rows are dropped");

        let chat_id = mgr.active_chat_id(&session.id);
        let on_disk = mgr.store.get_chat_messages(&chat_id).expect("raw rows");
        assert_eq!(on_disk.len(), rows.len(), "owner data is filtered at load, never deleted");
    }

    /// A scripted model: every call's messages are recorded; the main
    /// loop's calls follow the script, side calls answer "ok".
    struct Scripted {
        script: std::sync::Mutex<std::collections::VecDeque<Step>>,
        calls: std::sync::Mutex<Vec<Vec<ai::Message>>>,
        summaries: std::sync::Mutex<Vec<Vec<ai::Message>>>,
    }

    enum Step {
        Say(&'static str),
        Call,
        Drop,
    }

    #[async_trait::async_trait]
    impl ai::Provider for Scripted {
        fn id(&self) -> &str {
            "scripted"
        }
        async fn stream(&self, req: &ai::ChatRequest) -> Result<ai::EventReceiver, ai::ProviderError> {
            let step = if req.trace.purpose == "agent_turn" {
                self.calls.lock().unwrap().push(req.messages.clone());
                self.script.lock().unwrap().pop_front().expect("a main-loop call the script did not expect")
            } else {
                if req.trace.purpose == "budget_summary" {
                    self.summaries.lock().unwrap().push(req.messages.clone());
                }
                Step::Say("ok")
            };
            let events = match step {
                Step::Say(text) => vec![ai::StreamEvent::text(text)],
                Step::Call => vec![ai::StreamEvent::tool_call(ai::ToolCall {
                    id: format!("call-{}", uuid::Uuid::new_v4()),
                    name: "os".into(),
                    input: serde_json::json!({"resource": "file", "action": "read", "path": "/nonexistent"}),
                })],
                Step::Drop => return Err(ai::ProviderError::Request("connection reset".into())),
            };
            let (tx, rx) = tokio::sync::mpsc::channel(8);
            for e in events {
                let _ = tx.send(e).await;
            }
            let _ = tx.send(ai::StreamEvent::done()).await;
            Ok(rx)
        }
    }

    async fn turn(runner: &crate::runner::Runner, key: &str, prompt: String, max_iterations: usize) {
        let req = crate::runner::RunRequest {
            session_key: key.to_string(),
            prompt,
            max_iterations,
            skip_memory_extract: true,
            ..Default::default()
        };
        let mut rx = runner.run(req).await.expect("run");
        while rx.recv().await.is_some() {}
        for _ in 0..200 {
            if !runner.is_session_busy(key) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("the turn never released its session");
    }

    fn has_nudge(messages: &[ai::Message]) -> bool {
        messages.iter().any(|m| m.content.contains(crate::goals::CONTINUATION_PREFIX))
    }

    /// Steering is per turn. An auto-continuation's nudge rides the calls of
    /// its own run — the first call and that call's retry — as a stream
    /// reminder, then is gone: the run's next call does not carry it, the
    /// thread never stores it, and the next turn's assembled messages hold
    /// no steering from earlier turns, stored legacy rows included.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_continuation_nudge_rides_its_calls_and_never_reaches_the_thread() {
        let path = std::env::temp_dir().join(format!("nebo-steering-{}.db", uuid::Uuid::new_v4()));
        let store = Arc::new(Store::new(path.to_str().unwrap()).expect("store"));
        let model = Arc::new(Scripted {
            script: std::sync::Mutex::new(
                [
                    Step::Say("Step one is done; next I will write step two."),
                    Step::Drop,
                    Step::Call,
                    Step::Say("Step two is written."),
                    Step::Say("You're welcome."),
                ]
                .into(),
            ),
            calls: Default::default(),
            summaries: Default::default(),
        });
        let runner = crate::runner::Runner::new(
            store.clone(),
            Arc::new(tools::Registry::new(Arc::new(crate::harness::permissions::Check::new(store.clone())))),
            vec![model.clone() as Arc<dyn ai::Provider>],
            crate::selector::ModelSelector::new(Default::default()),
            Arc::new(crate::concurrency::ConcurrencyController::new(Some(2))),
            Arc::new(napp::HookDispatcher::new()),
            None,
            Default::default(),
            None,
        );
        let key = "agent:ops:web";

        turn(&runner, key, "Draft the plan".into(), 0).await;
        turn(&runner, key, crate::goals::continuation_prompt("unfinished work in the previous response"), 0).await;

        let sid = runner.sessions().resolve_session_id_by_key(key).expect("session");
        let chat_id = runner.sessions().active_chat_id(&sid);
        let stored = store.get_chat_messages(&chat_id).expect("rows");
        assert!(
            stored.iter().all(|m| !crate::goals::is_continuation_prompt(&m.content)
                && !m.content.contains("<system-reminder>")),
            "steering was written to the thread: {:?}",
            stored.iter().map(|m| (&m.role, &m.content)).collect::<Vec<_>>()
        );
        assert!(stored.iter().any(|m| m.role == "user" && m.content == "Draft the plan"), "the owner's words persist");
        assert!(stored.iter().any(|m| m.role == "tool"), "the tool result persists");
        assert!(stored.iter().any(|m| m.role == "assistant" && m.content == "Step two is written."));

        // Rows an older build stored: loaded by nothing, kept on disk.
        let legacy_nudge = crate::goals::continuation_prompt("unfinished work in the previous response");
        runner
            .sessions()
            .append_message(&sid, "user", &legacy_nudge, None, None, Some(r#"{"isMeta":true,"autoContinue":true}"#))
            .expect("legacy nudge");
        runner
            .sessions()
            .append_message(
                &sid,
                "user",
                &crate::steering::wrap_system_reminder("Team \"Ops\" — mission: ship it."),
                None,
                None,
                Some(r#"{"isMeta":true}"#),
            )
            .expect("legacy briefing");

        turn(&runner, key, "Thanks".into(), 0).await;

        let calls = model.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 5, "turn one, the continuation's dropped call, its retry, its next call, turn three");
        for (i, call) in calls[1..=2].iter().enumerate() {
            assert!(has_nudge(call), "continuation call {i} does not carry the nudge");
            let last = call.last().expect("messages");
            assert_eq!(last.role, "user", "continuation call {i} ends on the model's own turn");
            assert!(last.content.starts_with("<system-reminder>"), "the nudge rides as a stream reminder");
        }
        assert!(!has_nudge(&calls[3]), "the nudge outlived the call it was for");
        let next_turn = &calls[4];
        assert!(!has_nudge(next_turn), "the nudge reached the next turn");
        assert!(!next_turn.iter().any(|m| m.content.contains("mission: ship it")), "a stored briefing reached the next turn");
        let timestamps = next_turn.iter().filter(|m| m.content.contains("Message sent at")).count();
        assert_eq!(timestamps, 1, "only this turn's own stream reminder rides its first call");

        let on_disk = store.get_chat_messages(&chat_id).expect("rows");
        assert_eq!(
            on_disk.iter().filter(|m| m.content == legacy_nudge || m.content.contains("mission: ship it")).count(),
            2,
            "stored rows are filtered at load, never deleted"
        );
    }

    /// When the iteration budget runs out mid-task, one toolless call asks
    /// for a summary. The ask rides that call on the one steering channel and
    /// is never stored; the summary the model writes is conversation and is.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_budget_summary_request_rides_its_call_and_is_never_stored() {
        let path = std::env::temp_dir().join(format!("nebo-steering-{}.db", uuid::Uuid::new_v4()));
        let store = Arc::new(Store::new(path.to_str().unwrap()).expect("store"));
        let model = Arc::new(Scripted {
            script: std::sync::Mutex::new([Step::Call, Step::Call, Step::Call, Step::Call].into()),
            calls: Default::default(),
            summaries: Default::default(),
        });
        let runner = crate::runner::Runner::new(
            store.clone(),
            Arc::new(tools::Registry::new(Arc::new(crate::harness::permissions::Check::new(store.clone())))),
            vec![model.clone() as Arc<dyn ai::Provider>],
            crate::selector::ModelSelector::new(Default::default()),
            Arc::new(crate::concurrency::ConcurrencyController::new(Some(2))),
            Arc::new(napp::HookDispatcher::new()),
            None,
            Default::default(),
            None,
        );
        let key = "agent:ops:web";
        turn(&runner, key, "Read every file".into(), 1).await;

        let summaries = model.summaries.lock().unwrap().clone();
        assert_eq!(summaries.len(), 1, "one summary call after the budget ran out");
        let ask = summaries[0].last().expect("messages");
        assert_eq!(ask.role, "user");
        assert!(ask.content.starts_with("<system-reminder>") && ask.content.contains(crate::runner::BUDGET_SUMMARY_REQUEST));

        let sid = runner.sessions().resolve_session_id_by_key(key).expect("session");
        let stored = store.get_chat_messages(&runner.sessions().active_chat_id(&sid)).expect("rows");
        assert!(
            stored.iter().all(|m| !m.content.contains(crate::runner::BUDGET_SUMMARY_REQUEST)),
            "the summary request was stored"
        );
        assert!(stored.iter().any(|m| m.role == "assistant" && m.content == "ok"), "the summary is conversation");
    }
}
