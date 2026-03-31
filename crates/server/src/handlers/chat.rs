use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::response::Json;
use serde::Deserialize;

use crate::state::AppState;
use super::{to_error_response, HandlerResult};

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
    let chats = state.store.list_chats(q.limit, q.offset).map_err(to_error_response)?;
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
    let chat = state.store.create_chat(&id, title).map_err(to_error_response)?;
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
        state.store.update_chat_title(&id, title).map_err(to_error_response)?;
    }
    Ok(Json(serde_json::json!({"success": true})))
}

/// DELETE /api/v1/chats/:id
pub async fn delete_chat(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    state.store.delete_chat_messages_by_chat_id(&id).map_err(to_error_response)?;
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

/// Reconstruct metadata JSON from tool_calls + tool_results columns.
///
/// Handles three cases:
/// 1. Old metadata with toolCalls already built — strip outputs, done
/// 2. New metadata with only contentBlocks (persisted block order) — build toolCalls, use persisted order
/// 3. No metadata — build everything, fall back to text→tools order
fn build_message_metadata(messages: &mut [db::models::ChatMessage]) {
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
                        let is_error =
                            r.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false);
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
        return Ok(Json(serde_json::json!({ "output": output.0, "isError": output.1 })));
    }

    // Fallback: check persisted metadata on assistant messages
    if let Some(output) = find_tool_output_in_metadata(&state, &chat_id, &tool_call_id) {
        return Ok(Json(serde_json::json!({ "output": output.0, "isError": output.1 })));
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
                    let is_error = tc
                        .get("status")
                        .and_then(|v| v.as_str())
                        == Some("error");
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

    // Messages are stored with chat_id = chat.id (the session_key used by the runner)
    let mut messages = state
        .store
        .get_chat_messages_budgeted(&chat.id, query.max_chars, None)
        .unwrap_or_default();
    build_message_metadata(&mut messages);
    let total = state.store.count_chat_messages(&chat.id).unwrap_or(messages.len() as i64);

    Ok(Json(serde_json::json!({
        "chat": chat,
        "messages": messages,
        "totalMessages": total,
    })))
}

/// POST /api/v1/chats/companion/new — create a fresh companion session
pub async fn create_companion_chat(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let id = uuid::Uuid::new_v4().to_string();
    let chat = state
        .store
        .create_companion_chat(&id, COMPANION_USER_ID)
        .map_err(to_error_response)?;

    Ok(Json(serde_json::json!({
        "chat": chat,
        "messages": [],
        "totalMessages": 0,
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
    let content = body["content"]
        .as_str()
        .ok_or_else(|| to_error_response(types::NeboError::Validation("content required".into())))?;
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
    let companion = state.store.get_companion_chat_by_user(COMPANION_USER_ID).map_err(to_error_response)?;
    let chat = match companion {
        Some(c) => c,
        None => return Ok(Json(serde_json::json!({"days": []}))),
    };

    let days = state
        .store
        .list_chat_days(&chat.id, q.limit, q.offset)
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
    let companion = state.store.get_companion_chat_by_user(COMPANION_USER_ID).map_err(to_error_response)?;
    let chat = match companion {
        Some(c) => c,
        None => return Ok(Json(serde_json::json!({"messages": []}))),
    };

    let mut messages = state
        .store
        .get_chat_messages_by_day(&chat.id, &day)
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
) -> HandlerResult<serde_json::Value> {
    let mut messages = state
        .store
        .get_chat_messages_budgeted(&id, query.max_chars, query.before.as_deref())
        .map_err(to_error_response)?;
    build_message_metadata(&mut messages);
    let total = state.store.count_chat_messages(&id).unwrap_or(messages.len() as i64);
    Ok(Json(serde_json::json!({"messages": messages, "totalMessages": total})))
}
