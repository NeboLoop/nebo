use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::response::Json;
use serde::Deserialize;

use super::{HandlerResult, to_error_response};
use crate::state::AppState;
use db::models::ChatMessage;
use types::api::ActiveTurnStatus;

#[derive(Debug, Deserialize)]
pub struct ListChatsQuery {
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
}

fn default_limit() -> i64 {
    50
}

/// GET /api/v1/chats
pub async fn list_chats(
    State(state): State<AppState>,
    Query(q): Query<ListChatsQuery>,
) -> HandlerResult<serde_json::Value> {
    let chats = state
        .store
        .list_chats(q.limit, q.offset)
        .map_err(to_error_response)?;
    let total = state.store.count_chats().unwrap_or(0);
    Ok(Json(serde_json::json!({
        "chats": chats,
        "total": total,
    })))
}

/// POST /api/v1/chats
pub async fn create_chat(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    let title = body["title"].as_str().unwrap_or("New Chat");
    let id = uuid::Uuid::new_v4().to_string();
    let chat = state
        .store
        .create_chat(&id, title)
        .map_err(to_error_response)?;
    Ok(Json(serde_json::json!(chat)))
}

/// GET /api/v1/chats/:id
pub async fn get_chat(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let chat = state
        .store
        .get_chat(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;
    Ok(Json(serde_json::json!(chat)))
}

/// PUT /api/v1/chats/:id
pub async fn update_chat(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    if let Some(title) = body["title"].as_str() {
        // User rename → mark custom so the auto-namer never overwrites it.
        state
            .store
            .update_chat_title(&id, title, true)
            .map_err(to_error_response)?;
    }
    Ok(Json(serde_json::json!({"success": true})))
}

/// DELETE /api/v1/chats/:id
pub async fn delete_chat(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    state
        .store
        .delete_chat_messages_by_chat_id(&id)
        .map_err(to_error_response)?;
    state.store.delete_chat(&id).map_err(to_error_response)?;
    Ok(Json(serde_json::json!({"success": true})))
}

/// Build toolCalls array (without output) from the tool_calls column.
fn build_ui_tool_calls(
    tc_json: &str,
    tool_statuses: &HashMap<String, bool>,
) -> Option<(Vec<serde_json::Value>, Vec<serde_json::Value>)> {
    let calls: Vec<serde_json::Value> = serde_json::from_str(tc_json).ok()?;
    if calls.is_empty() {
        return None;
    }
    let ui_calls: Vec<serde_json::Value> = calls
        .iter()
        .map(|tc| {
            let id = tc.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let name = tc.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let input = tc.get("input").cloned().unwrap_or(serde_json::Value::Null);
            let input_str = if input.is_string() {
                input.as_str().unwrap_or("").to_string()
            } else {
                serde_json::to_string(&input).unwrap_or_default()
            };
            let status = match tool_statuses.get(id) {
                Some(true) => "error",
                _ => "complete",
            };
            serde_json::json!({
                "id": id,
                "name": name,
                "input": input_str,
                "status": status
            })
        })
        .collect();
    Some((ui_calls, calls))
}

/// Build default contentBlocks: text first, then tools (fallback for old messages).
fn default_content_blocks(content: &str, call_count: usize) -> Vec<serde_json::Value> {
    let mut blocks = Vec::new();
    if !content.is_empty() {
        blocks.push(serde_json::json!({"type": "text", "text": content}));
    }
    for i in 0..call_count {
        blocks.push(serde_json::json!({"type": "tool", "toolCallIndex": i}));
    }
    blocks
}

/// Prepare stored messages for a human transcript.
///
/// Internal turns are dropped: a message carrying `isMeta` was written by the
/// system (a preloaded skill, an auto-continuation nudge), not by the owner or
/// the employee. The model keeps them in its history; the owner never sees the
/// house talking to itself in their conversation.
///
/// Then metadata JSON is reconstructed from tool_calls + tool_results columns:
/// 1. Old metadata with toolCalls already built — strip outputs, done
/// 2. New metadata with only contentBlocks (persisted block order) — build toolCalls, use persisted order
/// 3. No metadata — build everything, fall back to text→tools order
pub fn build_message_metadata(messages: &mut Vec<db::models::ChatMessage>) {
    messages.retain(|m| {
        m.metadata
            .as_deref()
            .and_then(|meta| serde_json::from_str::<serde_json::Value>(meta).ok())
            .and_then(|v| v.get("isMeta").and_then(|f| f.as_bool()))
            != Some(true)
    });
    // Phase 1: Collect tool result statuses from role="tool" messages
    let mut tool_statuses: HashMap<String, bool> = HashMap::new();
    for msg in messages.iter() {
        if msg.role != "tool" {
            continue;
        }
        if let Some(tr_json) = msg.tool_results.as_deref() {
            if let Ok(results) = serde_json::from_str::<Vec<serde_json::Value>>(tr_json) {
                for r in &results {
                    if let Some(id) = r.get("tool_call_id").and_then(|v| v.as_str()) {
                        let is_error = r.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false);
                        tool_statuses.insert(id.to_string(), is_error);
                    }
                }
            }
        }
    }

    // Phase 2: For each assistant message, build/augment metadata
    for msg in messages.iter_mut() {
        if msg.role != "assistant" {
            continue;
        }

        let existing_meta: Option<serde_json::Value> = msg
            .metadata
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok());

        // Case 1: Old metadata already has toolCalls — strip outputs, done
        if let Some(ref meta) = existing_meta {
            if meta.get("toolCalls").is_some() {
                let mut m = meta.clone();
                if let Some(tcs) = m.get_mut("toolCalls").and_then(|v| v.as_array_mut()) {
                    for tc in tcs.iter_mut() {
                        if let Some(obj) = tc.as_object_mut() {
                            obj.remove("output");
                        }
                    }
                }
                msg.metadata = Some(m.to_string());
                continue;
            }
        }

        // Need tool_calls column to build toolCalls array
        let tc_json = match &msg.tool_calls {
            Some(tc) if !tc.is_empty() => tc.clone(),
            _ => continue,
        };
        let (ui_calls, raw_calls) = match build_ui_tool_calls(&tc_json, &tool_statuses) {
            Some(v) => v,
            None => continue,
        };

        // Case 2: Metadata has persisted contentBlocks (block order from streaming) — use it
        // Case 3: No metadata — fall back to text→tools order
        let blocks = if let Some(ref meta) = existing_meta {
            if let Some(persisted) = meta.get("contentBlocks").and_then(|v| v.as_array()) {
                // Hydrate persisted blocks: add text content to "text" entries
                persisted
                    .iter()
                    .map(|b| {
                        if b.get("type").and_then(|v| v.as_str()) == Some("text") {
                            serde_json::json!({"type": "text", "text": msg.content})
                        } else {
                            b.clone()
                        }
                    })
                    .collect()
            } else {
                default_content_blocks(&msg.content, raw_calls.len())
            }
        } else {
            default_content_blocks(&msg.content, raw_calls.len())
        };

        msg.metadata = Some(
            serde_json::json!({
                "toolCalls": ui_calls,
                "contentBlocks": blocks,
            })
            .to_string(),
        );
    }
}

/// GET /api/v1/chats/:chat_id/tool-output/:tool_call_id
/// Lazily fetch a single tool call's output.
pub async fn get_tool_output(
    State(state): State<AppState>,
    Path((chat_id, tool_call_id)): Path<(String, String)>,
) -> HandlerResult<serde_json::Value> {
    // First check role='tool' messages for the tool_call_id
    if let Some(output) = state
        .store
        .find_tool_output(&chat_id, &tool_call_id)
        .unwrap_or(None)
    {
        return Ok(Json(
            serde_json::json!({ "output": output.0, "isError": output.1 }),
        ));
    }

    // Fallback: check persisted metadata on assistant messages
    if let Some(output) = find_tool_output_in_metadata(&state, &chat_id, &tool_call_id) {
        return Ok(Json(
            serde_json::json!({ "output": output.0, "isError": output.1 }),
        ));
    }

    Ok(Json(serde_json::json!({ "output": "", "isError": false })))
}

/// Search persisted assistant metadata for a tool call's output.
fn find_tool_output_in_metadata(
    state: &AppState,
    chat_id: &str,
    tool_call_id: &str,
) -> Option<(String, bool)> {
    let messages = state
        .store
        .get_recent_chat_messages_with_tools(chat_id, 100)
        .ok()?;
    for msg in &messages {
        if msg.role != "assistant" {
            continue;
        }
        let meta_str = msg.metadata.as_deref()?;
        let meta: serde_json::Value = serde_json::from_str(meta_str).ok()?;
        if let Some(tool_calls) = meta.get("toolCalls").and_then(|v| v.as_array()) {
            for tc in tool_calls {
                if tc.get("id").and_then(|v| v.as_str()) == Some(tool_call_id) {
                    let output = tc
                        .get("output")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let is_error = tc.get("status").and_then(|v| v.as_str()) == Some("error");
                    return Some((output, is_error));
                }
            }
        }
    }
    None
}

/// Stable user_id for the companion chat (matches Go's companionUserIDFallback).
const COMPANION_USER_ID: &str = "companion-default";

fn default_char_budget() -> i64 {
    12000
}

#[derive(Debug, Deserialize)]
pub struct CompanionQuery {
    #[serde(default = "default_char_budget")]
    pub max_chars: i64,
}

/// GET /api/v1/chats/:id/messages. `active_run` is present while a turn is
/// running on this thread, so a page opening it mid-run shows "working" at
/// once instead of after the next event.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessagesResponse {
    pub messages: Vec<ChatMessage>,
    pub total_messages: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_run: Option<ActiveTurnStatus>,
}

#[derive(Debug, Deserialize)]
pub struct ChatMessagesQuery {
    #[serde(default = "default_char_budget")]
    pub max_chars: i64,
    pub before: Option<String>,
}

/// GET /api/v1/chats/companion?limit=30
pub async fn get_companion_chat(
    State(state): State<AppState>,
    Query(query): Query<CompanionQuery>,
) -> HandlerResult<serde_json::Value> {
    // Get the most recent companion chat, or create one if none exists
    let chat = if let Ok(Some(chat)) = state.store.get_companion_chat_by_user(COMPANION_USER_ID) {
        chat
    } else {
        let id = uuid::Uuid::new_v4().to_string();
        state
            .store
            .create_companion_chat(&id, COMPANION_USER_ID)
            .map_err(to_error_response)?
    };

    // The session_name links back to the session key used by the runner.
    // For legacy chats (pre-decoupling), session_name may be None — use chat.id as fallback.
    let session_key = chat.session_name.as_deref().unwrap_or(&chat.id);

    // Resolve the active chat_id from the session. After a rotation (via /reset or
    // session_reset), the session's active_chat_id may point to a different chat than
    // what get_companion_chat_by_user() returns. Always load messages from the active chat.
    let active_chat_id = state
        .runner
        .sessions()
        .resolve_session_id_by_key(session_key)
        .ok()
        .map(|sid| state.runner.sessions().active_chat_id(&sid))
        .unwrap_or_else(|| chat.id.clone());

    let mut messages = state
        .store
        .get_chat_messages_budgeted(&active_chat_id, query.max_chars, None)
        .unwrap_or_default();
    build_message_metadata(&mut messages);
    let total = state
        .store
        .count_chat_messages(&active_chat_id)
        .unwrap_or(messages.len() as i64);

    Ok(Json(serde_json::json!({
        "chat": chat,
        "messages": messages,
        "totalMessages": total,
        "sessionKey": session_key,
    })))
}

/// POST /api/v1/chats/companion/new — create a fresh conversation under the existing session.
/// If an existing companion session exists, rotates the chat (preserving old messages).
/// Otherwise creates a fresh companion chat.
pub async fn create_companion_chat(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    // Try to find the existing companion session and rotate its chat.
    if let Ok(Some(existing_chat)) = state.store.get_companion_chat_by_user(COMPANION_USER_ID) {
        let session_key = existing_chat
            .session_name
            .as_deref()
            .unwrap_or(&existing_chat.id);
        if let Ok(Some(session)) = state.store.get_session_by_name(session_key) {
            let new_chat_id = state
                .runner
                .sessions()
                .rotate_chat(&session.id, Some(COMPANION_USER_ID))
                .map_err(to_error_response)?;
            let chat = state
                .store
                .get_chat(&new_chat_id)
                .map_err(to_error_response)?
                .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;
            return Ok(Json(serde_json::json!({
                "chat": chat,
                "messages": [],
                "totalMessages": 0,
                "sessionKey": session_key,
            })));
        }
    }

    // No existing session — create a fresh companion chat (first-time setup).
    let id = uuid::Uuid::new_v4().to_string();
    let chat = state
        .store
        .create_companion_chat(&id, COMPANION_USER_ID)
        .map_err(to_error_response)?;

    Ok(Json(serde_json::json!({
        "chat": chat,
        "messages": [],
        "totalMessages": 0,
        "sessionKey": &chat.id,
    })))
}

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    #[serde(default)]
    pub q: String,
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
    #[serde(default)]
    pub chat_id: Option<String>,
}

/// GET /api/v1/chats/search
pub async fn search_messages(
    State(state): State<AppState>,
    Query(q): Query<SearchQuery>,
) -> HandlerResult<serde_json::Value> {
    let chat_id = q.chat_id.as_deref().unwrap_or("");
    let messages = state
        .store
        .search_chat_messages(chat_id, &q.q, q.limit, q.offset)
        .map_err(to_error_response)?;
    Ok(Json(serde_json::json!({"messages": messages})))
}

/// POST /api/v1/chats/message
pub async fn send_message(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> HandlerResult<serde_json::Value> {
    let chat_id = body["chatId"]
        .as_str()
        .ok_or_else(|| to_error_response(types::NeboError::Validation("chatId required".into())))?;
    let content = body["content"].as_str().ok_or_else(|| {
        to_error_response(types::NeboError::Validation("content required".into()))
    })?;
    let role = body["role"].as_str().unwrap_or("user");

    let msg_id = uuid::Uuid::new_v4().to_string();
    let msg = state
        .store
        .create_chat_message(&msg_id, chat_id, role, content, None)
        .map_err(to_error_response)?;

    // Best-effort: non-critical timestamp update
    let _ = state.store.update_chat_timestamp(chat_id);

    Ok(Json(serde_json::json!(msg)))
}

/// GET /api/v1/chats/days
pub async fn list_chat_days(
    State(state): State<AppState>,
    Query(q): Query<ListChatsQuery>,
) -> HandlerResult<serde_json::Value> {
    // Use companion chat for day grouping
    let companion = state
        .store
        .get_companion_chat_by_user(COMPANION_USER_ID)
        .map_err(to_error_response)?;
    let chat = match companion {
        Some(c) => c,
        None => return Ok(Json(serde_json::json!({"days": []}))),
    };

    // Resolve active chat_id from session (same pattern as get_companion_chat)
    let session_key = chat.session_name.as_deref().unwrap_or(&chat.id);
    let active_chat_id = state
        .runner
        .sessions()
        .resolve_session_id_by_key(session_key)
        .ok()
        .map(|sid| state.runner.sessions().active_chat_id(&sid))
        .unwrap_or_else(|| chat.id.clone());

    let days = state
        .store
        .list_chat_days(&active_chat_id, q.limit, q.offset)
        .map_err(to_error_response)?;

    let day_infos: Vec<serde_json::Value> = days
        .iter()
        .map(|(day, count)| {
            serde_json::json!({
                "day": day,
                "messageCount": count,
            })
        })
        .collect();

    Ok(Json(serde_json::json!({"days": day_infos})))
}

/// GET /api/v1/chats/history/:day
pub async fn get_chat_history_by_day(
    State(state): State<AppState>,
    Path(day): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let companion = state
        .store
        .get_companion_chat_by_user(COMPANION_USER_ID)
        .map_err(to_error_response)?;
    let chat = match companion {
        Some(c) => c,
        None => return Ok(Json(serde_json::json!({"messages": []}))),
    };

    // Resolve active chat_id from session (same pattern as get_companion_chat)
    let session_key = chat.session_name.as_deref().unwrap_or(&chat.id);
    let active_chat_id = state
        .runner
        .sessions()
        .resolve_session_id_by_key(session_key)
        .ok()
        .map(|sid| state.runner.sessions().active_chat_id(&sid))
        .unwrap_or_else(|| chat.id.clone());

    let mut messages = state
        .store
        .get_chat_messages_by_day(&active_chat_id, &day)
        .map_err(to_error_response)?;
    build_message_metadata(&mut messages);

    Ok(Json(serde_json::json!({"messages": messages})))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditMessageBody {
    pub content: String,
}

/// POST /api/v1/chats/messages/:id/edit
pub async fn edit_message(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<EditMessageBody>,
) -> HandlerResult<serde_json::Value> {
    let msg = state
        .store
        .get_chat_message(&id)
        .map_err(to_error_response)?
        .ok_or_else(|| to_error_response(types::NeboError::NotFound))?;
    if msg.role != "user" {
        return Err(to_error_response(types::NeboError::Validation(
            "can only edit user messages".into(),
        )));
    }
    state
        .store
        .update_chat_message_content(&id, &body.content, None)
        .map_err(to_error_response)?;
    state
        .store
        .delete_chat_messages_after_id(&msg.chat_id, &id)
        .map_err(to_error_response)?;
    Ok(Json(
        serde_json::json!({ "success": true, "chatId": msg.chat_id }),
    ))
}

/// GET /api/v1/chats/:id/messages?max_chars=12000&before=msg_id
pub async fn get_chat_messages(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<ChatMessagesQuery>,
) -> HandlerResult<ChatMessagesResponse> {
    // The caller may pass a session key (e.g. "agent:UUID:web") instead of a raw
    // chat_id.  Resolve via the session's active_chat_id when possible so we
    // always load from the correct (possibly rotated) conversation.
    let resolved_id = state
        .runner
        .sessions()
        .resolve_session_id_by_key(&id)
        .ok()
        .map(|sid| state.runner.sessions().active_chat_id(&sid))
        .unwrap_or_else(|| id.clone());

    tracing::info!(
        raw_id = %id,
        resolved_id = %resolved_id,
        "[THREAD-DEBUG] get_chat_messages reading from"
    );

    let mut messages = state
        .store
        .get_chat_messages_budgeted(&resolved_id, query.max_chars, query.before.as_deref())
        .map_err(to_error_response)?;
    build_message_metadata(&mut messages);
    let total = state
        .store
        .count_chat_messages(&resolved_id)
        .unwrap_or(messages.len() as i64);
    // The chat's session name is the key the runner admits turns under.
    let active_run = state
        .store
        .get_chat(&resolved_id)
        .ok()
        .flatten()
        .and_then(|c| c.session_name)
        .and_then(|key| state.runner.active_turn_status(&key));
    Ok(Json(ChatMessagesResponse { messages, total_messages: total, active_run }))
}
#[cfg(test)]
mod transcript_metadata_tests {
    use super::{build_message_metadata, build_ui_tool_calls, default_content_blocks};
    use std::collections::HashMap;

    fn msg(role: &str, content: &str) -> db::models::ChatMessage {
        db::models::ChatMessage {
            id: format!("m-{role}-{}", content.len()),
            chat_id: "chat-1".to_string(),
            role: role.to_string(),
            content: content.to_string(),
            metadata: None,
            created_at: 0,
            day_marker: None,
            tool_calls: None,
            tool_results: None,
            token_estimate: None,
            html: None,
        }
    }

    /// Messages stamped `isMeta` (system steering, auto-continue nudges) are
    /// dropped from the human transcript — the owner never sees the house
    /// talking to itself.
    #[test]
    fn is_meta_messages_never_reach_the_transcript() {
        let mut meta_msg = msg("user", "system nudge");
        meta_msg.metadata = Some(r#"{"isMeta":true}"#.to_string());
        let mut messages = vec![msg("user", "hello"), meta_msg, msg("assistant", "hi")];
        build_message_metadata(&mut messages);
        assert_eq!(messages.len(), 2);
        assert!(messages.iter().all(|m| m.content != "system nudge"));
    }

    /// An assistant message with a tool_calls column gets UI metadata built:
    /// per-call status comes from the tool-role results (is_error → "error"),
    /// and with no persisted block order, blocks default to text→tools.
    #[test]
    fn tool_calls_column_builds_ui_metadata_with_result_statuses() {
        let mut assistant = msg("assistant", "Done.");
        assistant.tool_calls = Some(
            r#"[{"id":"t1","name":"os","input":{"resource":"file","action":"read"}},
                {"id":"t2","name":"web","input":{}}]"#
                .to_string(),
        );
        let mut tool = msg("tool", "");
        tool.tool_results = Some(
            r#"[{"tool_call_id":"t1","is_error":true},{"tool_call_id":"t2","is_error":false}]"#
                .to_string(),
        );
        let mut messages = vec![assistant, tool];
        build_message_metadata(&mut messages);

        let meta: serde_json::Value =
            serde_json::from_str(messages[0].metadata.as_deref().unwrap()).unwrap();
        let calls = meta["toolCalls"].as_array().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0]["id"], "t1");
        assert_eq!(calls[0]["status"], "error");
        assert_eq!(calls[1]["status"], "complete");
        let blocks = meta["contentBlocks"].as_array().unwrap();
        assert_eq!(blocks[0]["type"], "text");
        assert_eq!(blocks[0]["text"], "Done.");
        assert_eq!(blocks[1]["type"], "tool");
        assert_eq!(blocks[1]["toolCallIndex"], 0);
    }

    /// Old metadata that already carries toolCalls keeps its shape but the
    /// stored outputs are STRIPPED — outputs are fetched lazily, never
    /// shipped with the transcript.
    #[test]
    fn preexisting_tool_call_metadata_has_outputs_stripped() {
        let mut assistant = msg("assistant", "Done.");
        assistant.metadata = Some(
            r#"{"toolCalls":[{"id":"t1","status":"complete","output":"HUGE BLOB"}]}"#.to_string(),
        );
        let mut messages = vec![assistant];
        build_message_metadata(&mut messages);
        let meta: serde_json::Value =
            serde_json::from_str(messages[0].metadata.as_deref().unwrap()).unwrap();
        let call = &meta["toolCalls"][0];
        assert_eq!(call["id"], "t1");
        assert_eq!(call["status"], "complete");
        assert!(call.get("output").is_none());
    }

    /// Persisted contentBlocks (the streamed block order) win over the
    /// text→tools fallback, and text blocks are hydrated with the message
    /// content.
    #[test]
    fn persisted_block_order_is_preserved_and_hydrated() {
        let mut assistant = msg("assistant", "Here's the file.");
        assistant.tool_calls =
            Some(r#"[{"id":"t1","name":"os","input":{}}]"#.to_string());
        assistant.metadata = Some(
            r#"{"contentBlocks":[{"type":"tool","toolCallIndex":0},{"type":"text"}]}"#.to_string(),
        );
        let mut messages = vec![assistant];
        build_message_metadata(&mut messages);
        let meta: serde_json::Value =
            serde_json::from_str(messages[0].metadata.as_deref().unwrap()).unwrap();
        let blocks = meta["contentBlocks"].as_array().unwrap();
        assert_eq!(blocks[0]["type"], "tool"); // tool FIRST, as streamed
        assert_eq!(blocks[1]["type"], "text");
        assert_eq!(blocks[1]["text"], "Here's the file.");
    }

    /// build_ui_tool_calls: string inputs pass through raw, object inputs are
    /// serialized; an empty call list yields None (no metadata invented).
    #[test]
    fn ui_tool_calls_normalize_inputs_and_refuse_empty() {
        let statuses: HashMap<String, bool> = HashMap::new();
        let (ui, _) = build_ui_tool_calls(
            r#"[{"id":"t1","name":"bash","input":"ls -la"},
                {"id":"t2","name":"os","input":{"a":1}}]"#,
            &statuses,
        )
        .unwrap();
        assert_eq!(ui[0]["input"], "ls -la");
        assert_eq!(ui[1]["input"], r#"{"a":1}"#);
        assert!(build_ui_tool_calls("[]", &statuses).is_none());
        assert!(build_ui_tool_calls("not json", &statuses).is_none());
    }

    /// default_content_blocks: empty content yields tool blocks only — no
    /// empty text block padding the transcript.
    #[test]
    fn default_blocks_skip_empty_text() {
        let blocks = default_content_blocks("", 2);
        assert_eq!(blocks.len(), 2);
        assert!(blocks.iter().all(|b| b["type"] == "tool"));
        let blocks = default_content_blocks("hi", 1);
        assert_eq!(blocks[0]["type"], "text");
        assert_eq!(blocks[1]["toolCallIndex"], 0);
    }
}
