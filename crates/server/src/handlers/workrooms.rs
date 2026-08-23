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

/// POST /workrooms — the platform API over the ONE creation core
/// (`tools::workroom::create`, shared with the loop tool's `workroom`
/// resource — rooms are normally opened by the employee that owns the task).
pub async fn create_workroom(
    State(state): State<AppState>,
    Json(body): Json<CreateWorkroomRequest>,
) -> HandlerResult<serde_json::Value> {
    let comm = state.comm_manager.active_plugin().await.ok_or_else(|| {
        to_error_response(types::NeboError::Internal(
            "no active NeboLoop connection".into(),
        ))
    })?;
    let room = tools::workroom::create(
        &comm,
        &state.store,
        &body.name,
        &body.mission,
        &body.agent_ids,
    )
    .await
    .map_err(|e| to_error_response(types::NeboError::Internal(e)))?;

    state.hub.broadcast(
        tools::workroom::WORKROOM_CREATED_EVENT,
        serde_json::json!({ "workroom": room }),
    );

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

    // The standard composer serializes mentions as <@localAgentId>. Hub-known
    // agents get their <@loop_agent_id>; room members WITHOUT a hub identity
    // keep the <@localAgentId> token — in a registered workroom the member
    // registry is the mention surface, and the channel dispatcher resolves
    // member tokens locally. One token grammar; plain text never triggers.
    let mut content = text.to_string();
    if content.contains("<@") {
        for a in state.store.list_agents(500, 0).unwrap_or_default() {
            let token = format!("<@{}>", a.id);
            if !content.contains(&token) {
                continue;
            }
            if let Some(loop_id) = a.loop_agent_id.as_deref().filter(|s| !s.is_empty()) {
                content = content.replace(&token, &format!("<@{loop_id}>"));
            }
        }
    }

    let msg = comm::CommMessage {
        id: uuid::Uuid::new_v4().to_string(),
        from: String::new(),
        to: String::new(),
        topic: channel_id.clone(),
        conversation_id: channel_id.clone(),
        msg_type: comm::CommMessageType::LoopChannel,
        content: content.clone(),
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

    // The hub does not echo a bot's own sends back to it — and in a room whose
    // members are all THIS bot's employees, the hub has nobody else to deliver
    // to. Without a local loopback the owner's message leaves and nothing ever
    // dispatches. Feed the message through the ONE inbound pathway
    // (handle_comm_message) exactly as the hub would deliver it: topic
    // "channel", the channel's real conversation id, senderName in the content
    // JSON. Mention tokens then resolve against the room's member registry and
    // the addressed employees run. (If the hub ever starts echoing sender
    // messages, dedupe by msg id here before dispatching twice.)
    //
    // human_injected stays FALSE on the loopback: handle_comm_message drops
    // every human_injected inbound as a gateway echo. The message's human-ness
    // is what the channel branch actually reads — the absence of
    // `senderKind: "agent"` — so the owner still opens engagement windows and
    // resets handoff chains.
    if let Some(conv_id) = state
        .comm_manager
        .conversation_for_channel(&channel_id)
        .await
    {
        let inbound = comm::CommMessage {
            id: uuid::Uuid::new_v4().to_string(),
            from: String::new(),
            to: String::new(),
            topic: "channel".to_string(),
            conversation_id: conv_id,
            msg_type: comm::CommMessageType::LoopChannel,
            content: serde_json::json!({ "text": content, "senderName": "Owner" }).to_string(),
            metadata: std::collections::HashMap::new(),
            timestamp: 0,
            human_injected: false,
            human_id: None,
            task_id: None,
            correlation_id: None,
            task_status: None,
            artifacts: Vec::new(),
            error: None,
            attachments: Vec::new(),
        };
        crate::spawn_comm_loopback(state.clone(), inbound);
    }

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
    // The hub speaks in bot ids; the owner reads names. Precedence: the
    // sender-stamped employee name (WS6 wire metadata) → the hub's channel
    // member roster (bot_id → bot_name) → the local agent registry → the raw
    // id. Resolved here so the contract carries names — no client has to
    // know the hub's id scheme.
    let mut name_of: std::collections::HashMap<String, String> = state
        .store
        .list_agents(500, 0)
        .unwrap_or_default()
        .into_iter()
        .flat_map(|a| {
            let mut keys = vec![(a.id.clone(), a.name.clone())];
            if let Some(loop_id) = a.loop_agent_id {
                keys.push((loop_id, a.name));
            }
            keys
        })
        .collect();
    if let Ok(members) = state.comm_manager.list_channel_members(&channel_id).await {
        for m in members {
            if !m.bot_name.is_empty() {
                name_of.entry(m.bot_id).or_insert(m.bot_name);
            }
        }
    }
    // This install's own unstamped messages carry its hub bot id — attribute
    // them to the primary employee rather than printing a UUID.
    if let Some(plugin) = state.comm_manager.active_plugin().await {
        let own_bot_id = plugin.bot_id().await;
        if !own_bot_id.is_empty() {
            let primary = state
                .store
                .get_agent("assistant")
                .ok()
                .flatten()
                .map(|a| a.name)
                .unwrap_or_else(|| "Nebo".to_string());
            name_of.entry(own_bot_id).or_insert(primary);
        }
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
            from: m
                .sender_name
                .filter(|s| !s.is_empty())
                .or_else(|| name_of.get(&m.from).cloned())
                .unwrap_or(m.from),
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
