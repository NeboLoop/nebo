//! Workrooms — mission rooms where the owner and several employees share one
//! conversation. A room IS a loop channel (the hub owns the conversation and
//! history); the desktop registers which channels are rooms and which of this
//! bot's employees participate. Channel dispatch is already mention-driven,
//! which is the room's addressed-only rule.

use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;

use crate::handlers::{to_error_response, HandlerResult};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateWorkroomRequest {
    pub name: String,
    #[serde(default)]
    pub mission: String,
    #[serde(default)]
    pub agent_ids: Vec<String>,
}

/// POST /workrooms — create the channel on the hub (find-or-create by name,
/// idempotent) and register it as a room with its member employees.
pub async fn create_workroom(
    State(state): State<AppState>,
    Json(body): Json<CreateWorkroomRequest>,
) -> HandlerResult<serde_json::Value> {
    let name = body.name.trim();
    if name.is_empty() {
        return Err(to_error_response(types::NeboError::Validation(
            "workroom name required".into(),
        )));
    }

    let channel_id = state
        .comm_manager
        .ensure_channel(name, (!body.mission.is_empty()).then_some(body.mission.as_str()))
        .await
        .map_err(|e| {
            to_error_response(types::NeboError::Internal(format!(
                "create workroom channel: {e}"
            )))
        })?;

    state
        .store
        .create_workroom(&channel_id, name, &body.mission, &body.agent_ids)
        .map_err(to_error_response)?;

    let room = state
        .store
        .get_workroom(&channel_id)
        .map_err(to_error_response)?;

    Ok(Json(serde_json::json!({ "workroom": room })))
}

/// GET /workrooms — the sidebar's room list.
pub async fn list_workrooms(
    State(state): State<AppState>,
) -> HandlerResult<serde_json::Value> {
    let workrooms = state.store.list_workrooms().map_err(to_error_response)?;
    let total = workrooms.len();
    Ok(Json(serde_json::json!({
        "workrooms": workrooms,
        "total": total,
    })))
}

#[derive(Debug, Deserialize)]
pub struct SendWorkroomMessageRequest {
    pub text: String,
}

/// One room transcript row, frontend-shaped (camelCase; genapi emits this).
/// Mapped from the comm layer's hub-normalized item — that struct serializes
/// snake_case for wire compatibility and must not change shape.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkroomMessage {
    pub id: String,
    pub from: String,
    pub content: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

/// POST /workrooms/{channelId}/send — the owner speaks in the room, over the
/// real wire (LoopChannel + human_injected, the same human-leg message shape
/// share_artifact sends). The local-mirror channel send is NOT this — it
/// never reaches the hub.
pub async fn send_workroom_message(
    State(state): State<AppState>,
    Path(channel_id): Path<String>,
    Json(body): Json<SendWorkroomMessageRequest>,
) -> HandlerResult<serde_json::Value> {
    let text = body.text.trim();
    if text.is_empty() {
        return Err(to_error_response(types::NeboError::Validation(
            "text required".into(),
        )));
    }
    if state
        .store
        .get_workroom(&channel_id)
        .map_err(to_error_response)?
        .is_none()
    {
        return Err(to_error_response(types::NeboError::NotFound));
    }

    let msg = comm::CommMessage {
        id: uuid::Uuid::new_v4().to_string(),
        from: String::new(),
        to: String::new(),
        topic: channel_id.clone(),
        conversation_id: channel_id.clone(),
        msg_type: comm::CommMessageType::LoopChannel,
        content: text.to_string(),
        metadata: std::collections::HashMap::new(),
        timestamp: 0,
        human_injected: true,
        human_id: None,
        task_id: None,
        correlation_id: None,
        task_status: None,
        artifacts: Vec::new(),
        error: None,
        attachments: Vec::new(),
    };
    state.comm_manager.send(msg).await.map_err(|e| {
        to_error_response(types::NeboError::Internal(format!(
            "send workroom message: {e}"
        )))
    })?;

    Ok(Json(serde_json::json!({ "message": "Sent" })))
}

/// GET /workrooms/{channelId}/messages — the room transcript from the hub
/// (the hub owns the conversation; the local mirror only holds mentioned
/// traffic). Initial load only — live updates arrive as `workroom_message`
/// hub events, never by polling.
pub async fn get_workroom_messages(
    State(state): State<AppState>,
    Path(channel_id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    // Only registered rooms — this endpoint is the room surface, not a
    // general channel browser.
    if state
        .store
        .get_workroom(&channel_id)
        .map_err(to_error_response)?
        .is_none()
    {
        return Err(to_error_response(types::NeboError::NotFound));
    }
    let messages: Vec<WorkroomMessage> = state
        .comm_manager
        .list_channel_messages(&channel_id, 200)
        .await
        .map_err(|e| {
            to_error_response(types::NeboError::Internal(format!(
                "load workroom messages: {e}"
            )))
        })?
        .into_iter()
        .map(|m| WorkroomMessage {
            id: m.id,
            from: m.from,
            content: m.content,
            created_at: m.created_at,
            role: m.role,
        })
        .collect();
    Ok(Json(serde_json::json!({ "messages": messages })))
}

/// DELETE /workrooms/{channelId} — forget the room registration. The channel
/// itself stays on the hub (conversations are records; deleting the room is a
/// sidebar decision, not a history purge).
pub async fn delete_workroom(
    State(state): State<AppState>,
    Path(channel_id): Path<String>,
) -> HandlerResult<serde_json::Value> {
    state
        .store
        .delete_workroom(&channel_id)
        .map_err(to_error_response)?;
    Ok(Json(serde_json::json!({
        "message": "Workroom removed"
    })))
}
