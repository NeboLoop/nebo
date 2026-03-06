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
    let messages = state
        .store
        .get_chat_messages(&chat.id)
        .unwrap_or_default();
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

    let messages = state
        .store
        .get_chat_messages_by_day(&chat.id, &day)
        .map_err(to_error_response)?;

    Ok(Json(serde_json::json!({"messages": messages})))
}

/// GET /api/v1/chats/:id/messages (used by agent sessions endpoint too)
pub async fn get_chat_messages(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    let messages = state.store.get_chat_messages(&id).map_err(to_error_response)?;
    Ok(Json(serde_json::json!({"messages": messages})))
}
