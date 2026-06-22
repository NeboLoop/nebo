use std::collections::HashSet;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Path, State, WebSocketUpgrade};
use axum::response::{IntoResponse, Response};
use futures::{SinkExt, StreamExt};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use tools::Origin;
use types::constants::lanes;

use crate::chat_dispatch::{ChatConfig, run_chat};
use crate::state::AppState;

/// Broadcast channel for real-time events to connected clients.
#[derive(Clone)]
pub struct ClientHub {
    tx: broadcast::Sender<HubEvent>,
}

/// An event broadcast to connected WebSocket clients.
#[derive(Clone, Debug)]
pub struct HubEvent {
    pub event_type: String,
    pub payload: serde_json::Value,
}

impl ClientHub {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(256);
        Self { tx }
    }

    /// Broadcast an event to all connected clients.
    pub fn broadcast(&self, event_type: &str, payload: serde_json::Value) {
        let _ = self.tx.send(HubEvent {
            event_type: event_type.to_string(),
            payload,
        });
    }

    /// Subscribe to events.
    pub fn subscribe(&self) -> broadcast::Receiver<HubEvent> {
        self.tx.subscribe()
    }
}

/// GET /ws — Main client WebSocket endpoint.
pub async fn client_ws_handler(State(state): State<AppState>, ws: WebSocketUpgrade) -> Response {
    info!("ws upgrade request received");
    ws.on_upgrade(move |socket| handle_client_ws(socket, state))
}

/// GET /ws/app/{agent_id} — App frontend WebSocket endpoint.
pub async fn app_ws_handler(
    Path(agent_id): Path<String>,
    State(state): State<AppState>,
    ws: WebSocketUpgrade,
) -> Response {
    match state.store.get_agent(&agent_id) {
        Ok(Some(agent)) if agent.is_app.unwrap_or(0) != 0 => {
            ws.on_upgrade(move |socket| handle_app_ws(socket, agent_id, state))
        }
        Ok(_) | Err(_) => axum::http::StatusCode::NOT_FOUND.into_response(),
    }
}

async fn handle_app_ws(socket: WebSocket, agent_id: String, state: AppState) {
    info!(agent = %agent_id, "app ws client connected");
    let mut hub_rx = state.hub.subscribe();
    let (mut sender, mut receiver) = socket.split();

    let welcome = serde_json::json!({
        "type": "connected",
        "data": { "agentId": agent_id },
    });
    if sender
        .send(Message::Text(
            serde_json::to_string(&welcome).unwrap().into(),
        ))
        .await
        .is_err()
    {
        return;
    }

    loop {
        tokio::select! {
            result = hub_rx.recv() => {
                match result {
                    Ok(event) => {
                        let msg = serde_json::json!({
                            "type": event.event_type,
                            "data": event.payload,
                        });
                        if sender
                            .send(Message::Text(serde_json::to_string(&msg).unwrap().into()))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(agent = %agent_id, lagged = n, "app ws client lagged");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            Some(msg) = receiver.next() => {
                match msg {
                    Ok(Message::Text(text)) => {
                        handle_app_ws_message(&state, &agent_id, &text).await;
                    }
                    Ok(Message::Ping(data)) => {
                        if sender.send(Message::Pong(data)).await.is_err() {
                            break;
                        }
                    }
                    Ok(Message::Close(frame)) => {
                        debug!(agent = %agent_id, ?frame, "app ws close");
                        break;
                    }
                    Err(e) => {
                        warn!(agent = %agent_id, error = %e, "app ws error");
                        break;
                    }
                    _ => {}
                }
            }
            else => break,
        }
    }
    info!(agent = %agent_id, "app ws client disconnected");
}

async fn handle_app_ws_message(state: &AppState, agent_id: &str, text: &str) {
    let parsed = match serde_json::from_str::<serde_json::Value>(text) {
        Ok(v) => v,
        Err(e) => {
            debug!(agent = %agent_id, error = %e, "invalid app ws json");
            return;
        }
    };
    let msg_type = parsed["type"].as_str().unwrap_or("");
    match msg_type {
        "a2ui_action" => {
            let data = &parsed["data"];
            let surface_id = data["surfaceId"]
                .as_str()
                .or_else(|| data["surface_id"].as_str())
                .or_else(|| data["message"]["action"]["surfaceId"].as_str())
                .unwrap_or("")
                .to_string();
            let action = data["name"]
                .as_str()
                .or_else(|| data["message"]["action"]["name"].as_str())
                .unwrap_or("unknown")
                .to_string();
            let component_id = data["sourceComponentId"]
                .as_str()
                .or_else(|| data["message"]["action"]["sourceComponentId"].as_str())
                .unwrap_or("")
                .to_string();
            let context = data
                .get("context")
                .cloned()
                .or_else(|| {
                    data.get("message")
                        .and_then(|m| m.get("action"))
                        .and_then(|a| a.get("context"))
                        .cloned()
                })
                .unwrap_or(serde_json::Value::Null);

            if surface_id.is_empty() {
                debug!(agent = %agent_id, "a2ui_action missing surface id");
                return;
            }

            let state_clone = state.clone();
            let agent_id = agent_id.to_string();
            tokio::spawn(async move {
                let handled = crate::a2ui_actions::dispatch(
                    &state_clone,
                    &state_clone.a2ui,
                    &agent_id,
                    &surface_id,
                    &action,
                    &component_id,
                    &context,
                )
                .await;

                if handled {
                    debug!(action = %action, "app a2ui_action handled deterministically");
                    return;
                }

                // Dedup: reject if this action is already in-flight
                if !state_clone.a2ui.try_begin_action(&surface_id, &action).await {
                    debug!(action = %action, surface = %surface_id, "app a2ui_action already in progress, skipping");
                    return;
                }

                // Fall through to LLM — same pattern as client WS handler
                let session_key = agent::keyparser::build_agent_session_key(&agent_id, "app");

                let prompt = if context.is_null() || context == serde_json::json!({}) {
                    format!(
                        "[App interaction] The user clicked the \"{}\" button (component: {}) in the app workspace.",
                        action, component_id
                    )
                } else {
                    format!(
                        "[App interaction] The user triggered the \"{}\" action (component: {}) in the app workspace. Context: {}",
                        action, component_id, context
                    )
                };

                let entity_config = crate::entity_config::resolve_for_chat(
                    &state_clone.store, "agent", &agent_id
                );

                let config = ChatConfig {
                    session_key,
                    prompt,
                    system: String::new(),
                    user_id: String::new(),
                    channel: "app".to_string(),
                    origin: Origin::User,
                    agent_id: agent_id.clone(),
                    cancel_token: CancellationToken::new(),
                    lane: lanes::EVENTS.to_string(),
                    comm_reply: None,
                    entity_config,
                    images: vec![],
                    entity_name: String::new(),
                    origin_agent_id: None,
                    mention_context: None,
                    tool_scope: None,
                    plan_mode: false,
                    channel_ctx: None,
                };

                run_chat(&state_clone, config).await;

                // Action complete — re-enable
                state_clone.a2ui.end_action(&surface_id, &action).await;
            });
        }
        "action" => {
            state.hub.broadcast(
                "app_action",
                serde_json::json!({
                    "agentId": agent_id,
                    "name": parsed["name"].clone(),
                    "data": parsed,
                }),
            );
        }
        "ping" => {
            state
                .hub
                .broadcast("app_ping", serde_json::json!({ "agentId": agent_id }));
        }
        _ => debug!(agent = %agent_id, msg_type, "unhandled app ws message"),
    }
}

async fn handle_client_ws(mut socket: WebSocket, state: AppState) {
    info!("ws client connected — starting handle_client_ws");
    let mut hub_rx = state.hub.subscribe();
    let seen_ids: Arc<tokio::sync::Mutex<HashSet<String>>> = Default::default();

    // Spawn periodic cleanup of stale runs in the global registry (10 min expiry).
    let cleanup_registry = state.run_registry.clone();
    let cleanup_tools = state.tools.clone();
    let cleanup_store = state.store.clone();
    let cleanup_token = CancellationToken::new();
    let cleanup_token_clone = cleanup_token.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            tokio::select! {
                _ = cleanup_token_clone.cancelled() => break,
                _ = interval.tick() => {
                    let stale_sessions = cleanup_registry.cleanup_stale(600).await;
                    if !stale_sessions.is_empty() {
                        warn!(cleaned = stale_sessions.len(), "expired stale runs from global registry");
                        // Clean up browser tab groups for expired sessions.
                        // Extension tracks tabs by session UUID, so resolve from session_key.
                        for sk in &stale_sessions {
                            let session_uuid = cleanup_store
                                .get_session_by_name(sk)
                                .ok()
                                .flatten()
                                .map(|s| s.id);
                            let id = session_uuid.as_deref().unwrap_or(sk.as_str());
                            cleanup_tools.close_browser_session(id).await;
                        }
                    }
                }
            }
        }
    });

    // Send initial connection confirmation
    let welcome = serde_json::json!({
        "type": "connected",
        "version": env!("CARGO_PKG_VERSION"),
    });
    let welcome_str = serde_json::to_string(&welcome).unwrap();
    info!("ws sending welcome: {}", welcome_str);
    match socket.send(Message::Text(welcome_str.into())).await {
        Ok(()) => info!("ws welcome sent successfully"),
        Err(e) => {
            warn!("ws welcome send failed: {}", e);
            return;
        }
    }

    // Surface a failed-and-rolled-back update from a prior session. The deferred update
    // helper writes UPDATE_FAILED.json on rollback; the (restored, working) app reads and
    // deletes it on the first client connect and toasts an error. (Startup broadcasts are
    // lost — no client is connected yet — so this is delivered on connect instead.)
    if let Ok(dir) = config::data_dir() {
        let marker = dir.join("UPDATE_FAILED.json");
        if let Ok(contents) = std::fs::read_to_string(&marker) {
            let _ = std::fs::remove_file(&marker);
            let payload = serde_json::from_str::<serde_json::Value>(&contents)
                .unwrap_or_else(|_| serde_json::json!({ "error": contents.trim() }));
            let msg = serde_json::json!({ "type": "update_error", "data": payload });
            let _ = socket
                .send(Message::Text(serde_json::to_string(&msg).unwrap().into()))
                .await;
        }
    }

    loop {
        tokio::select! {
            // Broadcast events to client
            result = hub_rx.recv() => {
                match result {
                    Ok(event) => {
                        let msg = serde_json::json!({
                            "type": event.event_type,
                            "data": event.payload,
                        });
                        if socket
                            .send(Message::Text(serde_json::to_string(&msg).unwrap().into()))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!("ws client lagged by {} messages, continuing", n);
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        break;
                    }
                }
            }
            // Handle incoming messages from client
            Some(msg) = socket.recv() => {
                match msg {
                    Ok(Message::Text(text)) => {
                        info!("ws client message: {}", text);
                        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text) {
                            let msg_type = parsed["type"].as_str().unwrap_or("");
                            match msg_type {
                                "chat" => {
                                    // Idempotency: skip duplicate messages
                                    if let Some(msg_id) = parsed.get("message_id").and_then(|v| v.as_str()) {
                                        let mut seen = seen_ids.lock().await;
                                        if !seen.insert(msg_id.to_string()) {
                                            debug!("duplicate message_id {}, skipping", msg_id);
                                            continue;
                                        }
                                        if seen.len() > 1000 {
                                            seen.clear();
                                        }
                                    }
                                    dispatch_chat(&state, &parsed, &cleanup_token).await;
                                }
                                "cancel" => {
                                    let data = &parsed["data"];
                                    let session_id = data["session_id"]
                                        .as_str()
                                        .unwrap_or("default")
                                        .to_string();
                                    let registry = &state.run_registry;

                                    // Support cancel by run_id, entity_id, or session_id
                                    if let Some(run_id) = data["run_id"].as_str() {
                                        if registry.cancel(run_id).await {
                                            info!(run_id, "cancelled run by run_id");
                                        } else {
                                            debug!(run_id, "cancel: run_id not found");
                                        }
                                    } else if let Some(entity_id) = data["entity_id"].as_str() {
                                        let count = registry.cancel_by_entity(entity_id).await;
                                        info!(entity_id, count, "cancelled runs by entity_id");
                                    } else if registry.cancel_by_session(&session_id).await {
                                        info!(session_id = %session_id, "cancelled run by session_id");
                                    } else {
                                        // Stop means stop: cancel ALL active runs.
                                        let count = registry.cancel_all().await;
                                        if count > 0 {
                                            warn!(
                                                requested = %session_id,
                                                "cancel key mismatch — cancelled all {} active runs",
                                                count
                                            );
                                        } else {
                                            debug!(session_id = %session_id, "cancel: no active runs");
                                        }
                                    }
                                    state.hub.broadcast("chat_cancelled", serde_json::json!({
                                        "session_id": session_id,
                                    }));
                                }
                                "cancel_all" => {
                                    let count = state.run_registry.cancel_all().await;
                                    info!(count, "emergency cancel_all");
                                    state.hub.broadcast("chat_cancelled", serde_json::json!({
                                        "session_id": "all",
                                    }));
                                }
                                "restore_version" => {
                                    // Restore an earlier version of a work document: append a
                                    // new version (= the chosen version's content) and surface it
                                    // as a "Restored …" assistant message so it joins the chain.
                                    let data = &parsed["data"];
                                    let document_id =
                                        data["document_id"].as_str().unwrap_or("").to_string();
                                    let version = data["version"].as_i64().unwrap_or(0);
                                    let agent_id =
                                        data["agent_id"].as_str().unwrap_or("").to_string();
                                    let session_id =
                                        data["session_id"].as_str().map(|s| s.to_string());
                                    if document_id.is_empty() || version <= 0 {
                                        debug!("restore_version: missing document_id/version");
                                    } else if let Ok(Some(doc)) =
                                        state.store.get_work_document(&document_id)
                                    {
                                        match state.store.restore_work_version(&document_id, version) {
                                            Ok(new_v) => {
                                                let artifact = serde_json::json!({
                                                    "documentId": doc.id,
                                                    "filename": doc.filename,
                                                    "kind": doc.kind,
                                                    "version": new_v.version_number,
                                                    "url": new_v.url,
                                                });
                                                let content = format!(
                                                    "Restored {} to version {}",
                                                    doc.filename, version
                                                );
                                                let msg_id = uuid::Uuid::new_v4().to_string();
                                                let metadata = serde_json::json!({
                                                    "artifacts": [artifact.clone()]
                                                })
                                                .to_string();
                                                let created_at = state
                                                    .store
                                                    .create_chat_message(
                                                        &msg_id,
                                                        &doc.chat_id,
                                                        "assistant",
                                                        &content,
                                                        Some(&metadata),
                                                    )
                                                    .map(|m| m.created_at)
                                                    .unwrap_or(0);
                                                state.hub.broadcast(
                                                    "chat_message",
                                                    serde_json::json!({
                                                        "id": msg_id,
                                                        "content": content,
                                                        "createdAt": created_at * 1000,
                                                        "agentId": agent_id,
                                                        "session_id": session_id,
                                                        "artifacts": [artifact],
                                                    }),
                                                );
                                            }
                                            Err(e) => warn!(
                                                error = %e, document_id = %document_id, version,
                                                "restore_version failed"
                                            ),
                                        }
                                    }
                                }
                                "auth" | "connect" => {
                                    info!("ws received '{}' message, sending auth_ok", msg_type);
                                    let auth_ok = serde_json::json!({"type": "auth_ok"});
                                    match socket
                                        .send(Message::Text(serde_json::to_string(&auth_ok).unwrap().into()))
                                        .await
                                    {
                                        Ok(()) => info!("ws auth_ok sent successfully"),
                                        Err(e) => {
                                            warn!("ws auth_ok send failed: {}", e);
                                            break;
                                        }
                                    }
                                }
                                "ping" => {
                                    let pong = serde_json::json!({"type": "pong"});
                                    if socket
                                        .send(Message::Text(serde_json::to_string(&pong).unwrap().into()))
                                        .await
                                        .is_err()
                                    {
                                        break;
                                    }
                                }
                                "session_reset" => {
                                    let session_key = parsed["data"]["session_id"]
                                        .as_str()
                                        .unwrap_or("default")
                                        .to_string();
                                    // Resolve frontend session key to internal session ID
                                    let result = state.runner.sessions()
                                        .resolve_session_id_by_key(&session_key)
                                        .and_then(|sid| state.runner.sessions().reset(&sid));
                                    let reply = match result {
                                        Ok(new_chat_id) => serde_json::json!({
                                            "type": "session_reset",
                                            "data": {"session_id": session_key, "success": true, "newChatId": new_chat_id},
                                        }),
                                        Err(e) => serde_json::json!({
                                            "type": "session_reset",
                                            "data": {"session_id": session_key, "success": false, "error": e.to_string()},
                                        }),
                                    };
                                    state.hub.broadcast("session_reset", reply["data"].clone());
                                }
                                "session_compact" => {
                                    let session_key = parsed["data"]["session_id"]
                                        .as_str()
                                        .unwrap_or("default")
                                        .to_string();

                                    let state_clone = state.clone();
                                    let skey = session_key.clone();
                                    tokio::spawn(async move {
                                        // Resolve frontend session key to internal session ID
                                        let internal_sid = match state_clone.runner.sessions()
                                            .resolve_session_id_by_key(&skey) {
                                            Ok(id) => id,
                                            Err(_) => {
                                                state_clone.hub.broadcast("session_compact", serde_json::json!({
                                                    "session_id": skey, "success": false, "error": "session not found"
                                                }));
                                                return;
                                            }
                                        };

                                        // 1. Get current messages
                                        let messages = match state_clone.runner.sessions().get_messages(&internal_sid) {
                                            Ok(msgs) => msgs,
                                            Err(_) => {
                                                state_clone.hub.broadcast("session_compact", serde_json::json!({
                                                    "session_id": skey, "success": false, "error": "failed to load messages"
                                                }));
                                                return;
                                            }
                                        };

                                        if messages.len() < 4 {
                                            state_clone.hub.broadcast("session_compact", serde_json::json!({
                                                "session_id": skey, "success": false, "error": "conversation too short to compact"
                                            }));
                                            return;
                                        }

                                        // 2. Build summary prompt from messages
                                        let mut transcript = String::new();
                                        for msg in &messages {
                                            let role = match msg.role.as_str() {
                                                "user" => "User",
                                                "assistant" => "Assistant",
                                                _ => continue,
                                            };
                                            if !msg.content.is_empty() {
                                                transcript.push_str(&format!("{}: {}\n\n", role, msg.content));
                                            }
                                        }

                                        // 3. Call LLM to summarize
                                        let providers = state_clone.runner.providers();
                                        let providers = providers.read().await;
                                        let provider = match providers.first() {
                                            Some(p) => p.clone(),
                                            None => {
                                                state_clone.hub.broadcast("session_compact", serde_json::json!({
                                                    "session_id": skey, "success": false, "error": "no AI provider available"
                                                }));
                                                return;
                                            }
                                        };
                                        drop(providers);

                                        let summary_prompt = format!(
                                            "Summarize this conversation concisely. Capture all key decisions, facts, requests, and context. \
                                             This summary will replace the full conversation so nothing important should be lost.\n\n---\n\n{}",
                                            transcript
                                        );

                                        let req = ai::ChatRequest {
                                            tool_choice: Default::default(),
                                            messages: vec![ai::Message {
                                                role: "user".into(),
                                                content: summary_prompt,
                                                ..Default::default()
                                            }],
                                            tools: vec![],
                                            max_tokens: 2000,
                                            temperature: 0.0,
                                            system: "You are a conversation summarizer. Produce a concise summary that preserves all important context.".into(),
                                            static_system: String::new(),
                                            model: String::new(),
                                            enable_thinking: false,
                                            metadata: None,
                                            cache_breakpoints: vec![],
                                            cancel_token: None,
                                            trace: None,
                                        };

                                        let mut rx = match provider.stream(&req).await {
                                            Ok(rx) => rx,
                                            Err(e) => {
                                                state_clone.hub.broadcast("session_compact", serde_json::json!({
                                                    "session_id": skey, "success": false, "error": format!("LLM error: {}", e)
                                                }));
                                                return;
                                            }
                                        };

                                        let mut summary = String::new();
                                        while let Some(event) = rx.recv().await {
                                            if event.event_type == ai::StreamEventType::Text {
                                                summary.push_str(&event.text);
                                            }
                                        }

                                        if summary.is_empty() {
                                            state_clone.hub.broadcast("session_compact", serde_json::json!({
                                                "session_id": skey, "success": false, "error": "empty summary generated"
                                            }));
                                            return;
                                        }

                                        // 4. Clear messages in current conversation and insert summary
                                        let _ = state_clone.runner.sessions().clear_current_messages(&internal_sid);
                                        let chat_id = state_clone.runner.sessions().active_chat_id(&internal_sid);
                                        let msg_id = uuid::Uuid::new_v4().to_string();
                                        let _ = state_clone.store.create_chat_message(
                                            &msg_id, &chat_id, "assistant",
                                            &format!("**Conversation Summary**\n\n{}", summary),
                                            None,
                                        );

                                        state_clone.hub.broadcast("session_compact", serde_json::json!({
                                            "session_id": skey, "success": true, "summary_length": summary.len()
                                        }));
                                    });
                                }
                                "list_active_runs" => {
                                    let runs = state.run_registry.list_top_level().await;
                                    let reply = serde_json::json!({
                                        "type": "active_runs",
                                        "data": { "runs": runs },
                                    });
                                    if socket
                                        .send(Message::Text(serde_json::to_string(&reply).unwrap().into()))
                                        .await
                                        .is_err()
                                    {
                                        break;
                                    }
                                }
                                "list_run_children" => {
                                    let run_id = parsed["data"]["run_id"]
                                        .as_str()
                                        .unwrap_or("");
                                    let children = state.run_registry.list_children(run_id).await;
                                    let reply = serde_json::json!({
                                        "type": "run_children",
                                        "data": { "run_id": run_id, "children": children },
                                    });
                                    if socket
                                        .send(Message::Text(serde_json::to_string(&reply).unwrap().into()))
                                        .await
                                        .is_err()
                                    {
                                        break;
                                    }
                                }
                                "check_stream" => {
                                    let session_id = parsed["data"]["session_id"]
                                        .as_str()
                                        .unwrap_or("default")
                                        .to_string();
                                    let running = state.run_registry.is_session_active(&session_id).await;
                                    let status = if running { "running" } else { "idle" };
                                    let reply = serde_json::json!({
                                        "type": "stream_status",
                                        "data": {"session_id": session_id, "status": status},
                                    });
                                    if socket
                                        .send(Message::Text(serde_json::to_string(&reply).unwrap().into()))
                                        .await
                                        .is_err()
                                    {
                                        break;
                                    }
                                }
                                "approval_response" => {
                                    let request_id = parsed["data"]["request_id"]
                                        .as_str()
                                        .unwrap_or("")
                                        .to_string();
                                    let approved = parsed["data"]["approved"]
                                        .as_bool()
                                        .unwrap_or(false);
                                    // The modal's "Approve Always" flag — previously
                                    // dropped here. Carry it as the decision string so
                                    // the runner can grant the capability for next time.
                                    let always = parsed["data"]["always"]
                                        .as_bool()
                                        .unwrap_or(false);
                                    let decision = if !approved {
                                        "deny"
                                    } else if always {
                                        "always"
                                    } else {
                                        "once"
                                    };
                                    let mut channels = state.approval_channels.lock().await;
                                    if let Some(tx) = channels.remove(&request_id) {
                                        let _ = tx.send(decision.to_string());
                                    }
                                }
                                "presence" => {
                                    let status = parsed["data"]["status"]
                                        .as_str()
                                        .unwrap_or("");
                                    if let Some(presence) = agent::proactive::Presence::from_str(status) {
                                        // Update global presence — applies to all sessions
                                        // from this client connection.
                                        state.presence.set("_global", presence).await;
                                        debug!("presence updated: {}", status);
                                    }
                                }
                                "request_introduction" => {
                                    // Introduction not yet implemented in Rust backend.
                                    // Send chat_complete so frontend resets isLoading.
                                    let session_id = parsed["data"]["session_id"]
                                        .as_str()
                                        .unwrap_or("")
                                        .to_string();
                                    state.hub.broadcast("chat_complete", serde_json::json!({
                                        "session_id": session_id,
                                        "skipped": true,
                                    }));
                                }
                                "ask_response" => {
                                    let request_id = parsed["data"]["request_id"]
                                        .as_str()
                                        .unwrap_or("")
                                        .to_string();
                                    let value = parsed["data"]["value"]
                                        .as_str()
                                        .unwrap_or("")
                                        .to_string();
                                    let mut channels = state.ask_channels.lock().await;
                                    if let Some(tx) = channels.remove(&request_id) {
                                        let _ = tx.send(value);
                                    }
                                }
                                "plan_response" => {
                                    let request_id = parsed["data"]["request_id"]
                                        .as_str()
                                        .unwrap_or("")
                                        .to_string();
                                    let approved = parsed["data"]["approved"]
                                        .as_bool()
                                        .unwrap_or(false);
                                    let value = if approved {
                                        "approved".to_string()
                                    } else {
                                        "rejected".to_string()
                                    };
                                    let mut channels = state.ask_channels.lock().await;
                                    if let Some(tx) = channels.remove(&request_id) {
                                        let _ = tx.send(value);
                                    }
                                }
                                // A2UI: user action on a surface component — dispatch deterministically or route to agent
                                "a2ui_action" => {
                                    let data = &parsed["data"];
                                    // ActionListener sends camelCase; try both forms
                                    let surface_id = data["surfaceId"].as_str()
                                        .or_else(|| data["surface_id"].as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    let action_name = data["name"].as_str()
                                        .unwrap_or("unknown")
                                        .to_string();
                                    let component_id = data["sourceComponentId"].as_str()
                                        .unwrap_or("")
                                        .to_string();
                                    let context = data["context"].clone();

                                    if surface_id.is_empty() {
                                        debug!("a2ui_action: missing surface_id, ignoring");
                                        continue;
                                    }

                                    // Parse agent_id from surface_id "agent:{id}:{view}"
                                    let parts: Vec<&str> = surface_id.split(':').collect();
                                    let agent_id = if parts.len() >= 2 { parts[1].to_string() } else { String::new() };

                                    if agent_id.is_empty() {
                                        debug!("a2ui_action: could not extract agent_id from {}", surface_id);
                                        continue;
                                    }

                                    debug!(surface_id = %surface_id, action = %action_name, component = %component_id, "a2ui_action received");

                                    let state_clone = state.clone();
                                    tokio::spawn(async move {
                                        // Try deterministic dispatch first
                                        let handled = crate::a2ui_actions::dispatch(
                                            &state_clone,
                                            &state_clone.a2ui,
                                            &agent_id,
                                            &surface_id,
                                            &action_name,
                                            &component_id,
                                            &context,
                                        ).await;

                                        if handled {
                                            debug!(action = %action_name, "a2ui_action handled deterministically");
                                            return;
                                        }

                                        // State-driven dedup: if this action is already being
                                        // processed (e.g., user double-clicked before UI updated),
                                        // reject the duplicate. Broadcasts "processing" status to
                                        // frontend so the button can show a loading state.
                                        if !state_clone.a2ui.try_begin_action(&surface_id, &action_name).await {
                                            debug!(action = %action_name, surface = %surface_id, "a2ui_action already in progress, skipping");
                                            return;
                                        }

                                        // Fall through to LLM: build ChatConfig and run_chat
                                        let session_key = agent::keyparser::build_agent_session_key(&agent_id, "web");

                                        let prompt = if context.is_null() || context == serde_json::json!({}) {
                                            format!(
                                                "[Workspace interaction] The user clicked the \"{}\" button (component: {}) in your workspace.",
                                                action_name, component_id
                                            )
                                        } else {
                                            format!(
                                                "[Workspace interaction] The user triggered the \"{}\" action (component: {}) in your workspace. Context: {}",
                                                action_name, component_id, context
                                            )
                                        };

                                        let entity_config = crate::entity_config::resolve_for_chat(
                                            &state_clone.store, "agent", &agent_id
                                        );

                                        let config = ChatConfig {
                                            session_key,
                                            prompt,
                                            system: String::new(),
                                            user_id: String::new(),
                                            channel: "web".to_string(),
                                            origin: Origin::User,
                                            agent_id: agent_id.clone(),
                                            cancel_token: CancellationToken::new(),
                                            lane: types::constants::lanes::EVENTS.to_string(),
                                            comm_reply: None,
                                            entity_config,
                                            images: vec![],
                                            entity_name: String::new(),
                                            origin_agent_id: None,
                                            mention_context: None,
                                            tool_scope: None, plan_mode: false,
                                            channel_ctx: None,
                                        };

                                        run_chat(&state_clone, config).await;

                                        // Action complete — re-enable the button
                                        state_clone.a2ui.end_action(&surface_id, &action_name).await;
                                    });
                                }
                                "ghost_text" => {
                                    let data = &parsed["data"];
                                    let partial_text = data["partial_text"]
                                        .as_str()
                                        .unwrap_or("")
                                        .to_string();
                                    let agent_id = data["agent_id"]
                                        .as_str()
                                        .unwrap_or("")
                                        .to_string();
                                    let raw_session_id = data["session_id"]
                                        .as_str()
                                        .unwrap_or("")
                                        .to_string();
                                    // Build session key from agent_id if not provided
                                    let session_id = if !raw_session_id.is_empty() {
                                        raw_session_id
                                    } else if !agent_id.is_empty() {
                                        agent::keyparser::build_agent_session_key(&agent_id, "thread")
                                    } else {
                                        String::new()
                                    };
                                    let request_id = data["request_id"]
                                        .as_str()
                                        .unwrap_or("")
                                        .to_string();

                                    // Guard: too short to predict
                                    if partial_text.len() < 10 {
                                        state.hub.broadcast("ghost_text", serde_json::json!({
                                            "request_id": request_id,
                                            "suggestion": "",
                                        }));
                                        continue;
                                    }

                                    let state_clone = state.clone();
                                    tokio::spawn(async move {
                                        // Build minimal context
                                        let mut context_msgs: Vec<ai::Message> = Vec::new();

                                        if !session_id.is_empty() {
                                            // Session summary (truncated)
                                            let summary = state_clone.runner.sessions()
                                                .get_summary(&session_id)
                                                .unwrap_or_default();
                                            if !summary.is_empty() {
                                                let trunc = if summary.len() > 500 {
                                                    format!("{}...", &summary[..500])
                                                } else {
                                                    summary
                                                };
                                                context_msgs.push(ai::Message {
                                                    role: "user".to_string(),
                                                    content: format!("[Context]: {}", trunc),
                                                    ..Default::default()
                                                });
                                                context_msgs.push(ai::Message {
                                                    role: "assistant".to_string(),
                                                    content: "Understood.".to_string(),
                                                    ..Default::default()
                                                });
                                            }

                                            // Last 4 user/assistant messages (truncated)
                                            if let Ok(messages) = state_clone.runner.sessions()
                                                .get_messages(&session_id)
                                            {
                                                let tail = if messages.len() > 4 {
                                                    &messages[messages.len() - 4..]
                                                } else {
                                                    &messages
                                                };
                                                for msg in tail {
                                                    if msg.role != "user" && msg.role != "assistant" {
                                                        continue;
                                                    }
                                                    let content = if msg.content.len() > 200 {
                                                        format!("{}...", &msg.content[..200])
                                                    } else {
                                                        msg.content.clone()
                                                    };
                                                    context_msgs.push(ai::Message {
                                                        role: msg.role.clone(),
                                                        content,
                                                        ..Default::default()
                                                    });
                                                }
                                            }
                                        }

                                        // Frame partial text as a completion task, NOT a conversation turn.
                                        // We use a single user message with the partial text clearly marked
                                        // so the model completes it rather than answering it.
                                        let completion_prompt = if context_msgs.is_empty() {
                                            format!("Complete this partial message: \"{partial_text}\"")
                                        } else {
                                            // Flatten context into a brief summary block
                                            let context_block: String = context_msgs.iter()
                                                .map(|m| format!("{}: {}", m.role, m.content))
                                                .collect::<Vec<_>>()
                                                .join("\n");
                                            format!(
                                                "Recent conversation:\n{context_block}\n\n\
                                                 Complete this partial message from the user: \"{partial_text}\""
                                            )
                                        };

                                        let model = state_clone.runner.selector().get_cheapest_model();
                                        let providers = state_clone.runner.providers();
                                        let providers = providers.read().await;
                                        let provider = match providers.first().cloned() {
                                            Some(p) => p,
                                            None => {
                                                state_clone.hub.broadcast("ghost_text", serde_json::json!({
                                                    "request_id": request_id,
                                                    "suggestion": "",
                                                }));
                                                return;
                                            }
                                        };
                                        drop(providers);

                                        let req = ai::ChatRequest {
                                            tool_choice: Default::default(),
                                            messages: vec![ai::Message {
                                                role: "user".to_string(),
                                                content: completion_prompt,
                                                ..Default::default()
                                            }],
                                            tools: vec![],
                                            max_tokens: 40,
                                            temperature: 0.0,
                                            system: "You are an autocomplete engine. The user is typing a message \
                                                     and you predict the REST of their sentence. Return ONLY the \
                                                     missing words that complete the sentence — nothing else. \
                                                     No quotes, no explanations, no full sentences. \
                                                     If the message looks complete, return an empty string. \
                                                     Keep completions under 8 words."
                                                .to_string(),
                                            static_system: String::new(),
                                            model,
                                            enable_thinking: false,
                                            metadata: None,
                                            cache_breakpoints: vec![],
                                            cancel_token: None,
                                            trace: None,
                                        };

                                        let mut rx = match provider.stream(&req).await {
                                            Ok(rx) => rx,
                                            Err(_) => {
                                                state_clone.hub.broadcast("ghost_text", serde_json::json!({
                                                    "request_id": request_id,
                                                    "suggestion": "",
                                                }));
                                                return;
                                            }
                                        };

                                        let mut text = String::new();
                                        while let Some(event) = rx.recv().await {
                                            match event.event_type {
                                                ai::StreamEventType::Text => text.push_str(&event.text),
                                                ai::StreamEventType::Done | ai::StreamEventType::Error => break,
                                                _ => {}
                                            }
                                        }

                                        let suggestion = text.trim().trim_matches('"').to_string();
                                        // Discard suggestions that are too long — the model misbehaved
                                        let suggestion = if suggestion.len() > 80
                                            || suggestion.split_whitespace().count() > 10
                                        {
                                            String::new()
                                        } else {
                                            suggestion
                                        };
                                        state_clone.hub.broadcast("ghost_text", serde_json::json!({
                                            "request_id": request_id,
                                            "suggestion": suggestion,
                                        }));
                                    });
                                }
                                "a2ui_close" => {
                                    let surface_id = parsed["data"]["surfaceId"]
                                        .as_str()
                                        .or_else(|| parsed["data"]["surface_id"].as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    if !surface_id.is_empty() {
                                        debug!(surface_id = %surface_id, "a2ui_close: deleting surface");
                                        let a2ui = state.a2ui.clone();
                                        tokio::spawn(async move {
                                            if let Err(e) = a2ui.delete_surface(&surface_id).await {
                                                warn!(error = %e, "failed to delete a2ui surface on close");
                                            }
                                        });
                                    }
                                }
                                _ => {
                                    debug!("unhandled ws message type: {}", msg_type);
                                }
                            }
                        }
                    }
                    Ok(Message::Close(frame)) => {
                        info!("ws client sent close frame: {:?}", frame);
                        break;
                    }
                    Ok(Message::Ping(data)) => {
                        if socket.send(Message::Pong(data)).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        warn!("ws error: {}", e);
                        break;
                    }
                    _ => {}
                }
            }
        }
    }

    cleanup_token.cancel();
    info!("ws client disconnected");
}

/// Scan prompt text for image file paths, read them, and return (cleaned_prompt, images).
/// Preserves the original prompt formatting (newlines, whitespace) when no images are found.
fn extract_images_from_prompt(prompt: &str) -> (String, Vec<ai::ImageContent>) {
    use base64::Engine;

    let image_extensions = ["png", "jpg", "jpeg", "gif", "webp", "bmp", "tiff"];
    let mut images = Vec::new();
    let mut image_paths: Vec<&str> = Vec::new();

    for token in prompt.split_whitespace() {
        let path = std::path::Path::new(token);
        let is_image = path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| image_extensions.contains(&ext.to_lowercase().as_str()))
            .unwrap_or(false);

        if is_image && path.exists() {
            if let Ok(bytes) = std::fs::read(path) {
                let media_type = match path.extension().and_then(|e| e.to_str()) {
                    Some("png") => "image/png",
                    Some("jpg" | "jpeg") => "image/jpeg",
                    Some("gif") => "image/gif",
                    Some("webp") => "image/webp",
                    Some("bmp") => "image/bmp",
                    Some("tiff") => "image/tiff",
                    _ => "image/png",
                };
                let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
                images.push(ai::ImageContent {
                    media_type: media_type.to_string(),
                    data,
                });
                image_paths.push(token);
            }
        }
    }

    // No images found — return original prompt with all formatting intact
    if images.is_empty() {
        return (prompt.to_string(), images);
    }

    // Remove only the image path tokens, preserving surrounding text and formatting
    let mut cleaned = prompt.to_string();
    for path in &image_paths {
        cleaned = cleaned.replacen(path, "", 1);
    }
    // Clean up whitespace artifacts left by removed paths (but preserve newlines)
    let cleaned: String = cleaned
        .lines()
        .map(|line| {
            // Collapse runs of spaces/tabs to single space, trim trailing
            let mut result = String::new();
            let mut prev_space = false;
            for ch in line.chars() {
                if ch == ' ' || ch == '\t' {
                    if !prev_space && !result.is_empty() {
                        result.push(' ');
                    }
                    prev_space = true;
                } else {
                    prev_space = false;
                    result.push(ch);
                }
            }
            result.trim_end().to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");
    let cleaned = cleaned.trim().to_string();

    // If the entire prompt was just image paths, add a generic prompt
    let cleaned = if cleaned.is_empty() && !images.is_empty() {
        "What's in this image?".to_string()
    } else {
        cleaned
    };

    (cleaned, images)
}

/// Handle built-in slash commands. Returns Some(response) if handled, None to
/// fall through to normal agent processing. These execute deterministically —
/// they never touch the LLM.
async fn handle_builtin_slash(
    state: &AppState,
    prompt: &str,
    session_id: &str,
    agent_id: &str,
    channel: &str,
) -> Option<String> {
    let trimmed = prompt.trim();
    let (cmd, args) = match trimmed.find(' ') {
        Some(i) => (&trimmed[..i], trimmed[i + 1..].trim()),
        None => (trimmed, ""),
    };
    let cmd = cmd.to_lowercase();

    match cmd.as_str() {
        "/new" => {
            // Rotate session → new conversation, old history preserved in DB.
            let session_key = if !agent_id.is_empty() {
                agent::keyparser::build_agent_session_key(agent_id, channel)
            } else {
                session_id.to_string()
            };
            match state
                .runner
                .sessions()
                .resolve_session_id_by_key(&session_key)
                .and_then(|sid| state.runner.sessions().reset(&sid))
            {
                Ok(new_chat_id) => {
                    state.hub.broadcast(
                        "session_reset",
                        serde_json::json!({
                            "session_id": session_key,
                            "success": true,
                            "newChatId": new_chat_id,
                        }),
                    );
                    Some("New conversation started.".to_string())
                }
                Err(e) => Some(format!("Failed to start new thread: {}", e)),
            }
        }

        "/clear" => {
            // Clear messages in the current conversation (stay in same session).
            let session_key = if !agent_id.is_empty() {
                agent::keyparser::build_agent_session_key(agent_id, channel)
            } else {
                session_id.to_string()
            };
            match state
                .runner
                .sessions()
                .resolve_session_id_by_key(&session_key)
            {
                Ok(sid) => {
                    match state.runner.sessions().clear_current_messages(&sid) {
                        Ok(()) => Some("Conversation cleared.".to_string()),
                        Err(e) => Some(format!("Failed to clear: {}", e)),
                    }
                }
                Err(e) => Some(format!("Failed to clear: {}", e)),
            }
        }

        "/help" => {
            let help = [
                "**Available commands:**\n",
                "| Command | Description |",
                "|---|---|",
                "| `/new` | Start a new conversation (preserves history) |",
                "| `/clear` | Clear current conversation messages |",
                "| `/compact` | Summarize & compress old messages |",
                "| `/model [name]` | Show or switch model |",
                "| `/status` | Show agent & system status |",
                "| `/help` | Show this help |",
            ]
            .join("\n");
            Some(help)
        }

        "/status" => {
            let registry = state.agent_registry.read().await;
            let active_agents: Vec<String> =
                registry.values().map(|a| a.name.clone()).collect();
            let agent_count = registry.len();
            drop(registry);

            let session_key = if !agent_id.is_empty() {
                agent::keyparser::build_agent_session_key(agent_id, channel)
            } else {
                session_id.to_string()
            };
            let msg_count = state
                .runner
                .sessions()
                .resolve_session_id_by_key(&session_key)
                .ok()
                .and_then(|sid| {
                    state.runner.sessions().get_messages(&sid).ok()
                })
                .map(|m| m.len())
                .unwrap_or(0);

            let status = format!(
                "**Status**\n\
                 - Session: `{}`\n\
                 - Messages in context: {}\n\
                 - Active agents: {} ({})\n\
                 - Plugins: {}",
                session_key,
                msg_count,
                agent_count,
                if active_agents.is_empty() {
                    "none".to_string()
                } else {
                    active_agents.join(", ")
                },
                state.plugin_store.list_installed().len(),
            );
            Some(status)
        }

        "/compact" => {
            // Trigger session compaction using the existing session_compact pipeline.
            let session_key = if !agent_id.is_empty() {
                agent::keyparser::build_agent_session_key(agent_id, channel)
            } else {
                session_id.to_string()
            };

            let state_clone = state.clone();
            let skey = session_key.clone();
            tokio::spawn(async move {
                let internal_sid = match state_clone.runner.sessions()
                    .resolve_session_id_by_key(&skey) {
                    Ok(id) => id,
                    Err(_) => {
                        state_clone.hub.broadcast("session_compact", serde_json::json!({
                            "session_id": skey, "success": false, "error": "session not found"
                        }));
                        return;
                    }
                };

                let messages = match state_clone.runner.sessions().get_messages(&internal_sid) {
                    Ok(msgs) => msgs,
                    Err(_) => {
                        state_clone.hub.broadcast("session_compact", serde_json::json!({
                            "session_id": skey, "success": false, "error": "failed to load messages"
                        }));
                        return;
                    }
                };

                if messages.len() < 4 {
                    state_clone.hub.broadcast("session_compact", serde_json::json!({
                        "session_id": skey, "success": false, "error": "conversation too short to compact"
                    }));
                    return;
                }

                let mut transcript = String::new();
                for msg in &messages {
                    let role = match msg.role.as_str() {
                        "user" => "User",
                        "assistant" => "Assistant",
                        _ => continue,
                    };
                    if !msg.content.is_empty() {
                        transcript.push_str(&format!("{}: {}\n\n", role, msg.content));
                    }
                }

                let providers = state_clone.runner.providers();
                let providers = providers.read().await;
                let provider = match providers.first() {
                    Some(p) => p.clone(),
                    None => {
                        state_clone.hub.broadcast("session_compact", serde_json::json!({
                            "session_id": skey, "success": false, "error": "no AI provider available"
                        }));
                        return;
                    }
                };
                drop(providers);

                let summary_prompt = format!(
                    "Summarize this conversation concisely. Capture all key decisions, facts, requests, and context. \
                     This summary will replace the full conversation so nothing important should be lost.\n\n---\n\n{}",
                    transcript
                );

                let req = ai::ChatRequest {
                    tool_choice: Default::default(),
                    messages: vec![ai::Message {
                        role: "user".into(),
                        content: summary_prompt,
                        ..Default::default()
                    }],
                    tools: vec![],
                    max_tokens: 2000,
                    temperature: 0.0,
                    system: "You are a conversation summarizer. Produce a concise summary that preserves all important context.".into(),
                    static_system: String::new(),
                    model: String::new(),
                    enable_thinking: false,
                    metadata: None,
                    cache_breakpoints: vec![],
                    cancel_token: None,
                    trace: None,
                };

                let mut rx = match provider.stream(&req).await {
                    Ok(rx) => rx,
                    Err(e) => {
                        state_clone.hub.broadcast("session_compact", serde_json::json!({
                            "session_id": skey, "success": false, "error": format!("LLM error: {}", e)
                        }));
                        return;
                    }
                };

                let mut summary = String::new();
                while let Some(event) = rx.recv().await {
                    if event.event_type == ai::StreamEventType::Text {
                        summary.push_str(&event.text);
                    }
                }

                if summary.is_empty() {
                    state_clone.hub.broadcast("session_compact", serde_json::json!({
                        "session_id": skey, "success": false, "error": "empty summary generated"
                    }));
                    return;
                }

                let _ = state_clone.runner.sessions().clear_current_messages(&internal_sid);
                let chat_id = state_clone.runner.sessions().active_chat_id(&internal_sid);
                let msg_id = uuid::Uuid::new_v4().to_string();
                let _ = state_clone.store.create_chat_message(
                    &msg_id, &chat_id, "assistant",
                    &format!("**Conversation Summary**\n\n{}", summary),
                    None,
                );

                state_clone.hub.broadcast("session_compact", serde_json::json!({
                    "session_id": skey, "success": true, "summary_length": summary.len()
                }));
            });
            Some("Compacting conversation...".to_string())
        }

        "/model" => {
            if args.is_empty() {
                // Show current model catalog
                let providers = &state.models_config.providers;
                if providers.is_empty() {
                    Some("No models configured.".to_string())
                } else {
                    let mut lines = vec!["**Available models:**\n".to_string()];
                    for (provider, models) in providers {
                        for m in models {
                            let name = if m.display_name.is_empty() {
                                &m.id
                            } else {
                                &m.display_name
                            };
                            let active = m.active.unwrap_or(true);
                            if active {
                                lines.push(format!("- `{}` — {} ({})", m.id, name, provider));
                            }
                        }
                    }
                    Some(lines.join("\n"))
                }
            } else {
                // Model switching: pass through to agent (needs entity config update)
                None
            }
        }

        _ => None,
    }
}

/// Dispatch a chat message to the agent runner via the unified chat pipeline.
async fn dispatch_chat(state: &AppState, msg: &serde_json::Value, conn_token: &CancellationToken) {
    let data = &msg["data"];
    let session_id = data["session_id"].as_str().unwrap_or("default").to_string();
    let prompt = data["prompt"].as_str().unwrap_or("").to_string();
    let system = data["system"].as_str().unwrap_or("").to_string();
    let user_id = data["user_id"].as_str().unwrap_or("").to_string();
    let channel = data["channel"].as_str().unwrap_or("web").to_string();
    let agent_id = data["agent_id"].as_str().unwrap_or("").to_string();
    let scope = data["scope"].as_str().unwrap_or("").to_string();

    // Extract uploaded attachment metadata from the WS payload
    let ws_attachments: Vec<comm::wire::Attachment> = data
        .get("attachments")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();

    info!(
        session_id = %session_id,
        prompt_len = prompt.len(),
        channel = %channel,
        attachments = ws_attachments.len(),
        "dispatch_chat called"
    );

    // Send ACK immediately so the client knows the message was received
    state.hub.broadcast(
        "chat_ack",
        serde_json::json!({
            "session_id": &session_id,
            "status": "accepted",
        }),
    );

    // Intercept marketplace codes before they reach the agent
    if let Some((code_type, code)) = crate::codes::detect_code(&prompt) {
        crate::codes::handle_code(state, code_type, code, &session_id).await;
        return;
    }

    // Intercept plugin slash commands (e.g., "/gmail triage")
    if prompt.starts_with('/') {
        if let Some(result) =
            crate::plugin_commands::try_dispatch(state, &prompt, &session_id).await
        {
            // Stream the plugin output as a chat response
            state.hub.broadcast(
                "chat_stream",
                serde_json::json!({
                    "session_id": &session_id,
                    "content": &result,
                }),
            );
            state.hub.broadcast(
                "chat_complete",
                serde_json::json!({
                    "session_id": &session_id,
                }),
            );
            return;
        }

        // Built-in slash commands — deterministic, never hit the LLM.
        if let Some(result) =
            handle_builtin_slash(state, &prompt, &session_id, &agent_id, &channel).await
        {
            state.hub.broadcast(
                "chat_stream",
                serde_json::json!({
                    "session_id": &session_id,
                    "content": &result,
                }),
            );
            state.hub.broadcast(
                "chat_complete",
                serde_json::json!({
                    "session_id": &session_id,
                }),
            );
            return;
        }

        // Not a recognized command — fall through to normal agent processing
    }

    // Extract images from file paths in the prompt (drag/drop, paste)
    let (prompt, mut images) = extract_images_from_prompt(&prompt);
    if !images.is_empty() {
        info!(count = images.len(), "extracted images from prompt");
    }

    // Convert uploaded image attachments to vision content
    if !ws_attachments.is_empty() {
        let mut att_prompt = prompt.clone();
        let att_images =
            crate::process_comm_attachments(state, &ws_attachments, &mut att_prompt).await;
        if !att_images.is_empty() {
            info!(count = att_images.len(), "extracted images from attachments");
            images.extend(att_images);
        }
        // att_prompt may have non-image descriptions appended — not needed for local user chat
        // since the user already sees the filenames in the composer
    }

    // Redact sensitive slash command arguments before the prompt enters storage
    // or logs. The original args were already used for plugin dispatch above.
    let prompt = crate::redact::redact_sensitive_args(&prompt).unwrap_or(prompt);

    if prompt.is_empty() {
        warn!("dispatch_chat: empty prompt, rejecting");
        state.hub.broadcast(
            "chat_error",
            serde_json::json!({"error": "empty prompt", "session_id": session_id}),
        );
        return;
    }

    // Auto-activate paused agents so their persona is loaded into the prompt.
    // Without this, a paused agent's chat works but has no personality.
    if !agent_id.is_empty() {
        let needs_activation = !state.agent_registry.read().await.contains_key(&agent_id);
        if needs_activation {
            if let Ok(Some(agent)) = state.store.get_agent(&agent_id) {
                let config = if !agent.frontmatter.is_empty() {
                    napp::agent::parse_agent_config(&agent.frontmatter).ok()
                } else {
                    None
                };
                let active = tools::ActiveAgent {
                    agent_id: agent.id.clone(),
                    name: agent.name.clone(),
                    agent_md: agent.agent_md.clone(),
                    config,
                    channel_id: None,
                    degraded: None,
                    soul: agent.soul.clone(),
                    rules: agent.rules.clone(),
                };
                state
                    .agent_registry
                    .write()
                    .await
                    .insert(agent.id.clone(), active);
                state.store.set_agent_enabled(&agent_id, true).ok();
                state
                    .agent_workers
                    .start_agent(&agent_id, &agent.name, None)
                    .await;
                state.hub.broadcast(
                    "agent_activated",
                    serde_json::json!({ "agentId": &agent_id, "name": &agent.name }),
                );
                info!(agent_id = %agent_id, name = %agent.name, "auto-activated paused agent for chat");
            }
        }
    }

    // Use the client-provided session_id if it already has the correct agent prefix
    // (e.g. "agent:brief:app:doc123" for document-scoped sessions).
    // Otherwise, build one from agent_id + channel.
    let expected_prefix = if !agent_id.is_empty() {
        format!("agent:{}:", agent_id)
    } else {
        String::new()
    };
    let session_key = if !expected_prefix.is_empty() && session_id.starts_with(&expected_prefix) {
        session_id
    } else if !agent_id.is_empty() {
        agent::keyparser::build_agent_session_key(&agent_id, &channel)
    } else {
        session_id
    };

    info!(session_key = %session_key, agent_id = %agent_id, channel = %channel, "[THREAD-DEBUG] dispatch_chat final session_key");

    // Resolve entity config for the active entity
    let entity_config = {
        let (etype, eid) = if !agent_id.is_empty() {
            ("agent", agent_id.as_str())
        } else {
            ("main", "main")
        };
        crate::entity_config::resolve_for_chat(&state.store, etype, eid)
    };

    // If NeboAI is connected, forward responses so the conversation stays in sync.
    // Works for both custom agents (agent_space by slug) and the companion (default bot).
    let comm_reply = if state.comm_manager.is_connected().await {
        let bot_id = config::read_bot_id().unwrap_or_default();
        // Per-chat agent spaces: mirror THIS chat's turns to ITS OWN loop
        // conversation, never the agent's merged stream. The chat for this
        // turn is the thread uuid embedded in the session key, or the
        // session's active chat.
        let turn_chat_id = if let Some(pos) = session_key.find(":thread:") {
            session_key[pos + 8..].to_string()
        } else {
            state
                .runner
                .sessions()
                .resolve_session_id_by_key(&session_key)
                .ok()
                .map(|sid| state.runner.sessions().active_chat_id(&sid))
                .unwrap_or_default()
        };
        let slug = if !agent_id.is_empty() {
            // Custom (secondary) agent: bot-scoped handle (`bot_<id8>_<slug>`),
            // matching how reconcile registers it.
            let registry = state.agent_registry.read().await;
            registry
                .get(&agent_id)
                .map(|r| comm::handle::secondary_handle(&bot_id, &r.name))
        } else {
            // Companion (primary): bot_<id8>.
            Some(comm::handle::default_bot_handle(&bot_id, ""))
        };
        let conv_id = if let Some(ref slug) = slug {
            // Prefer the conversation bound to this chat; fall back to the
            // agent's general conversation until the chat has been synced
            // (chats/sync on reconcile creates per-chat conversations).
            match state
                .comm_manager
                .agent_chat_conv_for_slug(slug, &turn_chat_id)
                .await
            {
                Some(cid) => Some(cid),
                None => state.comm_manager.agent_space_conv_for_slug(slug).await,
            }
        } else {
            None
        };
        conv_id.map(|cid| {
            // Write through the conv↔agent association (durable side of
            // ConvMaps) so an inbound DM on this conversation still resolves
            // to this agent after a restart, before the join repopulates
            // ConvMaps. Mirrors the inbound write-through in lib.rs.
            let row_id = if agent_id.is_empty() { "assistant" } else { agent_id.as_str() };
            if let Err(e) = state.store.set_agent_loop_conv_id(row_id, &cid) {
                warn!(error = %e, conv_id = %cid, "failed to persist loop_conv_id");
            }
            crate::chat_dispatch::CommReplyConfig {
                provider: "neboai".to_string(),
                topic: "agent_space".to_string(),
                conversation_id: cid,
            }
        })
    } else {
        None
    };

    // Send the user's prompt to NeboAI so it appears in the Loop conversation.
    // The relay markers are the canonical "this is the OWNER's own message
    // relayed by the bot" shape (same as the reconcile backfill) — without
    // them the web renders the mirrored prompt as the agent speaking.
    if let Some(ref reply_cfg) = comm_reply {
        let mut meta = std::collections::HashMap::new();
        meta.insert("relay".to_string(), "true".to_string());
        meta.insert("role".to_string(), "user".to_string());
        meta.insert("senderName".to_string(), "You".to_string());
        let user_msg = comm::CommMessage {
            id: uuid::Uuid::new_v4().to_string(),
            from: String::new(),
            to: String::new(),
            topic: reply_cfg.topic.clone(),
            conversation_id: reply_cfg.conversation_id.clone(),
            msg_type: comm::CommMessageType::Message,
            content: prompt.clone(),
            metadata: meta,
            timestamp: 0,
            human_injected: true,
            human_id: None,
            task_id: None,
            correlation_id: None,
            task_status: None,
            artifacts: vec![],
            error: None,
            attachments: ws_attachments.clone(),
        };
        if let Err(e) = state.comm_manager.send(user_msg).await {
            warn!(error = %e, "failed to forward user prompt to NeboAI");
        }
    }

    // Extract app-provided context (sent by chat embed's setContext)
    let app_context: Option<String> = data
        .get("context")
        .filter(|v| !v.is_null())
        .map(|v| {
            if let Some(s) = v.as_str() {
                s.to_string()
            } else {
                format!("App context: {}", v)
            }
        });

    // @mentions route the message to the addressed agent(s). When the user
    // @mentions other agents, ONLY they respond — the thread's own agent stays
    // quiet (Slack/Discord semantics: you addressed pam, pam answers, the thread
    // owner doesn't butt in). With no mentions, the thread's agent responds.
    let mentioned = parse_mention_tokens(&prompt, &agent_id);

    if mentioned.is_empty() {
        let config = ChatConfig {
            session_key,
            prompt: prompt.clone(),
            system,
            user_id: user_id.clone(),
            channel: channel.clone(),
            origin: Origin::User,
            agent_id: agent_id.clone(),
            cancel_token: conn_token.child_token(),
            lane: lanes::MAIN.to_string(),
            comm_reply,
            entity_config,
            images,
            entity_name: String::new(), // resolved from agent_registry in run_chat
            origin_agent_id: None,
            mention_context: app_context,
            tool_scope: if scope.is_empty() { None } else { Some(scope) },
            plan_mode: false,
            channel_ctx: None,
        };
        run_chat(state, config).await;
    } else {
        // Route only to the mentioned agent(s). Each runs in its own session and
        // broadcasts back into this thread (origin_agent_id).
        let state = state.clone();
        tokio::spawn(async move {
            for mid in mentioned {
                fork_mention_chat(&state, &mid, &prompt, &user_id, &channel, &agent_id).await;
            }
        });
    }
}

/// Extract unique agent IDs from `<@agent-id>` tokens in the prompt.
/// Deduplicates and excludes `exclude_agent_id` (the primary agent).
fn parse_mention_tokens(prompt: &str, exclude_agent_id: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut ids = Vec::new();
    let bytes = prompt.as_bytes();
    let mut i = 0;
    while i + 2 < bytes.len() {
        if bytes[i] == b'<' && bytes[i + 1] == b'@' {
            let start = i + 2;
            if let Some(end) = prompt[start..].find('>') {
                let id = &prompt[start..start + end];
                if !id.is_empty()
                    && id != exclude_agent_id
                    && id
                        .chars()
                        .all(|c| c.is_alphanumeric() || c == '.' || c == '_' || c == '-')
                    && seen.insert(id.to_string())
                {
                    ids.push(id.to_string());
                }
                i = start + end + 1;
                continue;
            }
        }
        i += 1;
    }
    ids
}

/// Dispatch an async chat to a mentioned agent.
/// Uses the mentioned agent's own session for history isolation,
/// but broadcasts WS events with `origin_agent_id` so the frontend
/// routes them to the mentioning agent's thread.
async fn fork_mention_chat(
    state: &AppState,
    mentioned_id: &str,
    prompt: &str,
    user_id: &str,
    channel: &str,
    origin_agent_id: &str,
) {
    use crate::chat_dispatch::{ChatConfig, run_chat};

    // Auto-activate the mentioned agent if needed
    let needs_activation = !state.agent_registry.read().await.contains_key(mentioned_id);
    if needs_activation {
        match state.store.get_agent(mentioned_id) {
            Ok(Some(agent)) => {
                let config = if !agent.frontmatter.is_empty() {
                    napp::agent::parse_agent_config(&agent.frontmatter).ok()
                } else {
                    None
                };
                let active = tools::ActiveAgent {
                    agent_id: agent.id.clone(),
                    name: agent.name.clone(),
                    agent_md: agent.agent_md.clone(),
                    config,
                    channel_id: None,
                    degraded: None,
                    soul: agent.soul.clone(),
                    rules: agent.rules.clone(),
                };
                state
                    .agent_registry
                    .write()
                    .await
                    .insert(agent.id.clone(), active);
                state.store.set_agent_enabled(mentioned_id, true).ok();
                state
                    .agent_workers
                    .start_agent(mentioned_id, &agent.name, None)
                    .await;
                info!(agent_id = %mentioned_id, "auto-activated mentioned agent for @mention");
            }
            _ => {
                warn!(agent_id = %mentioned_id, "mentioned agent not found, skipping");
                return;
            }
        }
    }

    let session_key = agent::keyparser::build_agent_session_key(mentioned_id, channel);
    let entity_config = crate::entity_config::resolve_for_chat(&state.store, "agent", mentioned_id);

    let contextualized = format!(
        "[You were @mentioned in a conversation. Respond helpfully.]\n\n{}",
        prompt,
    );

    let delegate_session_key = session_key.clone();
    let chat_config = ChatConfig {
        session_key,
        prompt: contextualized,
        system: String::new(),
        user_id: user_id.to_string(),
        channel: channel.to_string(),
        origin: Origin::User,
        agent_id: mentioned_id.to_string(),
        cancel_token: CancellationToken::new(),
        lane: lanes::MAIN.to_string(),
        comm_reply: None,
        entity_config,
        images: vec![],
        entity_name: String::new(),
        origin_agent_id: Some(origin_agent_id.to_string()),
        mention_context: None,
        tool_scope: None, plan_mode: false,
        channel_ctx: None,
    };

    run_chat(state, chat_config).await;

    // Inject the delegate's response back into the primary agent's session
    // so the primary agent has context about what the mentioned agent said.
    inject_delegate_response(
        state,
        mentioned_id,
        &delegate_session_key,
        origin_agent_id,
        channel,
    );
}

/// After a mentioned agent completes, read its last assistant message and
/// inject it into the primary agent's session as context.
fn inject_delegate_response(
    state: &AppState,
    mentioned_id: &str,
    delegate_session_key: &str,
    origin_agent_id: &str,
    channel: &str,
) {
    let sessions = state.runner.sessions();

    // Read the delegate's last assistant message
    let delegate_response = match sessions.resolve_session_id_by_key(delegate_session_key) {
        Ok(session_id) => {
            match sessions.get_messages(&session_id) {
                Ok(msgs) => {
                    // Find the last assistant message
                    msgs.into_iter()
                        .rev()
                        .find(|m| m.role == "assistant")
                        .map(|m| m.content)
                }
                Err(_) => None,
            }
        }
        Err(_) => None,
    };

    let response_text = match delegate_response {
        Some(ref text) if !text.is_empty() => text.as_str(),
        _ => {
            debug!(mentioned_id = %mentioned_id, "no delegate response to inject");
            return;
        }
    };

    // Look up the mentioned agent's display name
    let agent_name = state
        .store
        .get_agent(mentioned_id)
        .ok()
        .flatten()
        .map(|a| a.name)
        .unwrap_or_else(|| mentioned_id.to_string());

    // Build the primary agent's session key
    let primary_session_key = agent::keyparser::build_agent_session_key(origin_agent_id, channel);

    // Inject as a system-context message in the primary agent's session
    let injection = format!(
        "[Response from @{} ({})]\n{}",
        mentioned_id, agent_name, response_text,
    );

    match sessions.resolve_session_id_by_key(&primary_session_key) {
        Ok(session_id) => {
            match sessions.append_message(&session_id, "system", &injection, None, None, None) {
                Ok(_) => {
                    info!(
                        mentioned = %mentioned_id,
                        primary = %origin_agent_id,
                        len = response_text.len(),
                        "injected delegate response into primary session"
                    );
                }
                Err(e) => {
                    warn!(error = %e, "failed to inject delegate response");
                }
            }
        }
        Err(e) => {
            warn!(error = %e, primary = %origin_agent_id, "failed to resolve primary session for response injection");
        }
    }
}

/// GET /api/v1/agent/ws — Agent WebSocket endpoint for agent-to-server communication.
pub async fn agent_ws_handler(State(state): State<AppState>, ws: WebSocketUpgrade) -> Response {
    let hub = state.hub.clone();
    ws.on_upgrade(move |socket| handle_agent_ws(socket, hub))
}

async fn handle_agent_ws(mut socket: WebSocket, hub: Arc<ClientHub>) {
    debug!("agent ws connected");

    loop {
        match socket.recv().await {
            Some(Ok(Message::Text(text))) => {
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text) {
                    let event_type = parsed["type"].as_str().unwrap_or("agent_event").to_string();
                    // Forward agent events to client hub
                    hub.broadcast(&event_type, parsed);
                }
            }
            Some(Ok(Message::Close(_))) | None => break,
            Some(Ok(Message::Ping(data))) => {
                if socket.send(Message::Pong(data)).await.is_err() {
                    break;
                }
            }
            Some(Err(e)) => {
                warn!("agent ws error: {}", e);
                break;
            }
            _ => {}
        }
    }

    debug!("agent ws disconnected");
}

/// GET /ws/extension — Chrome extension bridge WebSocket endpoint.
/// The native messaging host process connects here to relay messages
/// between the Chrome extension and the Nebo agent.
pub async fn extension_ws_handler(State(state): State<AppState>, ws: WebSocketUpgrade) -> Response {
    let bridge = state.extension_bridge.clone();
    ws.on_upgrade(move |socket| handle_extension_ws(socket, bridge))
}

async fn handle_extension_ws(socket: WebSocket, bridge: Arc<browser::ExtensionBridge>) {
    use futures::SinkExt;
    use futures::StreamExt;

    let (mut ws_tx, mut ws_rx) = socket.split();

    // Wait for the first message to identify the browser.
    // The relay sends a "hello" with a "browser" field.
    let mut browser = "unknown".to_string();
    let first_msg = ws_rx.next().await;
    if let Some(Ok(Message::Text(text))) = first_msg {
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text) {
            if parsed["type"].as_str() == Some("hello") {
                browser = parsed["browser"].as_str().unwrap_or("unknown").to_string();
                debug!(browser = %browser, "extension relay identified");
            }
        }
    }

    // Register this connection — each browser gets its own request channel
    let (conn_id, mut request_rx) = bridge.connect(browser).await;

    // Task 1: Read tool requests from this connection's channel → send to WS
    let send_task = tokio::spawn(async move {
        while let Some(req) = request_rx.recv().await {
            debug!(
                tool = %req.tool,
                session_id = ?req.session_id,
                is_command = req.is_command,
                "extension_ws forwarding tool request"
            );
            let msg = if req.is_command {
                // Fire-and-forget command (show_indicators, hide_indicators)
                let mut m = serde_json::json!({ "type": req.tool });
                if let Some(ref sid) = req.session_id {
                    m["session_id"] = serde_json::Value::String(sid.clone());
                }
                m
            } else if req.is_batch {
                let mut m = serde_json::json!({
                    "type": "execute_batch",
                    "id": req.id,
                    "actions": req.args["actions"],
                    "stop_on_error": req.args["stop_on_error"],
                });
                if let Some(ref sid) = req.session_id {
                    m["session_id"] = serde_json::Value::String(sid.clone());
                }
                m
            } else {
                let mut m = serde_json::json!({
                    "type": "execute_tool",
                    "id": req.id,
                    "tool": req.tool,
                    "args": req.args,
                });
                if let Some(ref sid) = req.session_id {
                    m["session_id"] = serde_json::Value::String(sid.clone());
                }
                m
            };
            if ws_tx
                .send(Message::Text(serde_json::to_string(&msg).unwrap().into()))
                .await
                .is_err()
            {
                break;
            }
        }
    });

    // Task 2: Read messages from WS (bridge process) → deliver results to bridge
    let bridge_recv = bridge.clone();
    let recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_rx.next().await {
            match msg {
                Message::Text(text) => {
                    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text) {
                        let msg_type = parsed["type"].as_str().unwrap_or("");
                        match msg_type {
                            "tool_response" => {
                                if let Some(id) = parsed["id"].as_i64() {
                                    let result = if let Some(err) = parsed["error"].as_str() {
                                        Err(err.to_string())
                                    } else {
                                        Ok(parsed["result"].clone())
                                    };
                                    bridge_recv.deliver_result(id, result).await;
                                }
                            }
                            "batch_response" => {
                                if let Some(id) = parsed["id"].as_i64() {
                                    // Batch response: deliver the results array as-is
                                    let result = if let Some(err) = parsed["error"].as_str() {
                                        Err(err.to_string())
                                    } else {
                                        Ok(parsed["result"].clone())
                                    };
                                    bridge_recv.deliver_result(id, result).await;
                                }
                            }
                            "hello" | "connected" => {
                                debug!("extension bridge handshake: {}", msg_type);
                            }
                            _ => {
                                debug!("extension bridge unknown msg: {}", msg_type);
                            }
                        }
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    });

    // Wait for either task to finish (disconnect)
    tokio::select! {
        _ = send_task => {}
        _ = recv_task => {}
    }

    bridge.disconnect(conn_id).await;
}
