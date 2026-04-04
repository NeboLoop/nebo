use std::collections::HashSet;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{State, WebSocketUpgrade};
use axum::response::Response;
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
pub async fn client_ws_handler(
    State(state): State<AppState>,
    ws: WebSocketUpgrade,
) -> Response {
    info!("ws upgrade request received");
    ws.on_upgrade(move |socket| handle_client_ws(socket, state))
}

async fn handle_client_ws(mut socket: WebSocket, state: AppState) {
    info!("ws client connected — starting handle_client_ws");
    let mut hub_rx = state.hub.subscribe();
    let seen_ids: Arc<tokio::sync::Mutex<HashSet<String>>> = Default::default();

    // Spawn periodic cleanup of stale runs in the global registry (10 min expiry).
    let cleanup_registry = state.run_registry.clone();
    let cleanup_token = CancellationToken::new();
    let cleanup_token_clone = cleanup_token.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            tokio::select! {
                _ = cleanup_token_clone.cancelled() => break,
                _ = interval.tick() => {
                    let cleaned = cleanup_registry.cleanup_stale(600).await;
                    if cleaned > 0 {
                        warn!(cleaned, "expired stale runs from global registry");
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
                                    dispatch_chat(&state, &parsed).await;
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
                                    let mut channels = state.approval_channels.lock().await;
                                    if let Some(tx) = channels.remove(&request_id) {
                                        let _ = tx.send(approved);
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
                let data =
                    base64::engine::general_purpose::STANDARD.encode(&bytes);
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

/// Dispatch a chat message to the agent runner via the unified chat pipeline.
async fn dispatch_chat(state: &AppState, msg: &serde_json::Value) {
    let data = &msg["data"];
    let session_id = data["session_id"]
        .as_str()
        .unwrap_or("default")
        .to_string();
    let prompt = data["prompt"].as_str().unwrap_or("").to_string();
    let system = data["system"].as_str().unwrap_or("").to_string();
    let user_id = data["user_id"].as_str().unwrap_or("").to_string();
    let channel = data["channel"].as_str().unwrap_or("web").to_string();
    let agent_id = data["agent_id"].as_str().unwrap_or("").to_string();

    info!(
        session_id = %session_id,
        prompt_len = prompt.len(),
        channel = %channel,
        "dispatch_chat called"
    );

    // Send ACK immediately so the client knows the message was received
    state.hub.broadcast("chat_ack", serde_json::json!({
        "session_id": &session_id,
        "status": "accepted",
    }));

    // Intercept marketplace codes before they reach the agent
    if let Some((code_type, code)) = crate::codes::detect_code(&prompt) {
        crate::codes::handle_code(state, code_type, code, &session_id).await;
        return;
    }

    // Extract images from file paths in the prompt (drag/drop, paste)
    let (prompt, images) = extract_images_from_prompt(&prompt);
    if !images.is_empty() {
        info!(count = images.len(), "extracted images from prompt");
    }

    if prompt.is_empty() {
        warn!("dispatch_chat: empty prompt, rejecting");
        state.hub.broadcast(
            "chat_error",
            serde_json::json!({"error": "empty prompt", "session_id": session_id}),
        );
        return;
    }

    // If agent_id is set, build an agent-scoped session key for isolation
    let session_key = if !agent_id.is_empty() {
        agent::keyparser::build_agent_session_key(&agent_id, &channel)
    } else {
        session_id
    };

    info!(session_id = %session_key, agent_id = %agent_id, "dispatching chat to agent");

    // Resolve entity config for the active entity
    let entity_config = {
        let (etype, eid) = if !agent_id.is_empty() {
            ("agent", agent_id.as_str())
        } else {
            ("main", "main")
        };
        crate::entity_config::resolve_for_chat(&state.store, etype, eid)
    };

    // If NeboLoop is connected, forward responses so the conversation stays in sync.
    // Works for both custom agents (agent_space by slug) and the companion (default bot).
    let comm_reply = if state.comm_manager.is_connected().await {
        let conv_id = if !agent_id.is_empty() {
            // Custom agent: look up by slug
            let slug = {
                let registry = state.agent_registry.read().await;
                registry.get(&agent_id).map(|r| r.name.to_lowercase().replace(' ', "-"))
            };
            if let Some(slug) = slug {
                state.comm_manager.agent_space_conv_for_slug(&slug).await
            } else {
                None
            }
        } else {
            // Companion (default bot): look up by bot_* slug
            if let Some(bot_id) = config::read_bot_id() {
                let bot_slug = format!("bot_{}", &bot_id[..bot_id.len().min(12)]);
                state.comm_manager.agent_space_conv_for_slug(&bot_slug).await
            } else {
                None
            }
        };
        conv_id.map(|cid| crate::chat_dispatch::CommReplyConfig {
            provider: "neboloop".to_string(),
            topic: "agent_space".to_string(),
            conversation_id: cid,
        })
    } else {
        None
    };

    // Send the user's prompt to NeboLoop so it appears in the Loop conversation
    if let Some(ref reply_cfg) = comm_reply {
        let mut meta = std::collections::HashMap::new();
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
        };
        if let Err(e) = state.comm_manager.send(user_msg).await {
            warn!(error = %e, "failed to forward user prompt to NeboLoop");
        }
    }

    let config = ChatConfig {
        session_key,
        prompt,
        system,
        user_id,
        channel,
        origin: Origin::User,
        agent_id,
        cancel_token: CancellationToken::new(),
        lane: lanes::MAIN.to_string(),
        comm_reply,
        entity_config,
        images,
        entity_name: String::new(), // resolved from agent_registry in run_chat
    };

    run_chat(state, config).await;
}

/// GET /api/v1/agent/ws — Agent WebSocket endpoint for agent-to-server communication.
pub async fn agent_ws_handler(
    State(state): State<AppState>,
    ws: WebSocketUpgrade,
) -> Response {
    let hub = state.hub.clone();
    ws.on_upgrade(move |socket| handle_agent_ws(socket, hub))
}

async fn handle_agent_ws(mut socket: WebSocket, hub: Arc<ClientHub>) {
    debug!("agent ws connected");

    loop {
        match socket.recv().await {
            Some(Ok(Message::Text(text))) => {
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text) {
                    let event_type =
                        parsed["type"].as_str().unwrap_or("agent_event").to_string();
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
pub async fn extension_ws_handler(
    State(state): State<AppState>,
    ws: WebSocketUpgrade,
) -> Response {
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
            let msg = if req.is_batch {
                serde_json::json!({
                    "type": "execute_batch",
                    "id": req.id,
                    "actions": req.args["actions"],
                    "stop_on_error": req.args["stop_on_error"],
                })
            } else {
                serde_json::json!({
                    "type": "execute_tool",
                    "id": req.id,
                    "tool": req.tool,
                    "args": req.args,
                })
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
                                    let result = if let Some(err) =
                                        parsed["error"].as_str()
                                    {
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
                                        Ok(parsed["results"].clone())
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
