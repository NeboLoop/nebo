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

/// Reconstruct metadata JSON from tool_calls + tool_results columns.
/// For each assistant message with tool_calls, builds a metadata JSON with:
/// - toolCalls: [{id, name, input, output, status}] (matched with tool results)
/// - contentBlocks: [{type:"text"}, {type:"tool", toolCallIndex:N}]
fn build_message_metadata(messages: &mut [db::models::ChatMessage]) {
    // Phase 1: Collect tool results from role="tool" messages
    let mut tool_results: HashMap<String, (String, bool)> = HashMap::new();
    for msg in messages.iter() {
        if msg.role != "tool" {
            continue;
        }
        if let Some(tr_json) = msg.tool_results.as_deref() {
            if let Ok(results) = serde_json::from_str::<Vec<serde_json::Value>>(tr_json) {
                for r in &results {
                    if let (Some(id), Some(content)) = (
                        r.get("tool_call_id").and_then(|v| v.as_str()),
                        r.get("content").and_then(|v| v.as_str()),
                    ) {
                        let is_error =
                            r.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false);
                        tool_results.insert(id.to_string(), (content.to_string(), is_error));
                    }
                }
            }
        }
    }

    // Phase 2: For each assistant message with tool_calls, build metadata
    for msg in messages.iter_mut() {
        if msg.role != "assistant" || msg.metadata.is_some() {
            continue;
        }
        let tc_json: String = match &msg.tool_calls {
            Some(tc) if !tc.is_empty() => tc.clone(),
            _ => continue,
        };
        let calls: Vec<serde_json::Value> = match serde_json::from_str(&tc_json) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if calls.is_empty() {
            continue;
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
                let (output, status) = match tool_results.get(id) {
                    Some((content, true)) => (content.clone(), "error"),
                    Some((content, false)) => (content.clone(), "complete"),
                    None => (String::new(), "complete"),
                };
                serde_json::json!({
                    "id": id,
                    "name": name,
                    "input": input_str,
                    "output": output,
                    "status": status
                })
            })
            .collect();

        let mut blocks: Vec<serde_json::Value> = Vec::new();
        if !msg.content.is_empty() {
            blocks.push(serde_json::json!({"type": "text", "text": msg.content}));
        }
        for (i, _) in calls.iter().enumerate() {
            blocks.push(serde_json::json!({"type": "tool", "toolCallIndex": i}));
        }

        msg.metadata = Some(
            serde_json::json!({
                "toolCalls": ui_calls,
                "contentBlocks": blocks,
            })
            .to_string(),
        );
    }
}

/// Stable user_id for the companion chat (matches Go's companionUserIDFallback).
const COMPANION_USER_ID: &str = "companion-default";

/// GET /api/v1/chats/companion
pub async fn get_companion_chat(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    // Use "companion-default" as user_id (matches Go behavior)
    let chat = if let Ok(Some(chat)) = state.store.get_companion_chat_by_user(COMPANION_USER_ID) {
        chat
    } else {
        let id = uuid::Uuid::new_v4().to_string();
        state
            .store
            .get_or_create_companion_chat(&id, COMPANION_USER_ID)
            .map_err(to_error_response)?
    };

    // Messages are stored with chat_id = chat.id (the session_key used by the runner)
    let mut messages = state
        .store
        .get_chat_messages(&chat.id)
        .unwrap_or_default();
    build_message_metadata(&mut messages);
    let total = messages.len() as i64;

    Ok(Json(serde_json::json!({
        "chat": chat,
        "messages": messages,
        "totalMessages": total,
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

/// GET /api/v1/chats/:id/messages (used by agent sessions endpoint too)
pub async fn get_chat_messages(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let mut messages = state.store.get_chat_messages(&id).map_err(to_error_response)?;
    build_message_metadata(&mut messages);
    Ok(Json(serde_json::json!({"messages": messages})))
}
