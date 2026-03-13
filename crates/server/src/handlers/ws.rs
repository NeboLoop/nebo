use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{State, WebSocketUpgrade};
use axum::response::Response;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use tools::Origin;
use types::constants::lanes;

use crate::chat_dispatch::{ActiveRuns, ChatConfig, run_chat};
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
    let active_runs: ActiveRuns = Default::default();

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
                                    dispatch_chat(&state, &parsed, active_runs.clone()).await;
                                }
                                "cancel" => {
                                    let session_id = parsed["data"]["session_id"]
                                        .as_str()
                                        .unwrap_or("default")
                                        .to_string();
                                    let runs = active_runs.lock().await;
                                    if let Some(token) = runs.get(&session_id) {
                                        token.cancel();
                                        info!(session_id = %session_id, "cancelled agent run");
                                    }
                                    state.hub.broadcast("chat_cancelled", serde_json::json!({
                                        "session_id": session_id,
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
                                    let session_id = parsed["data"]["session_id"]
                                        .as_str()
                                        .unwrap_or("default")
                                        .to_string();
                                    let result = state.runner.sessions().reset(&session_id);
                                    let reply = match result {
                                        Ok(()) => serde_json::json!({
                                            "type": "session_reset",
                                            "data": {"session_id": session_id, "success": true},
                                        }),
                                        Err(e) => serde_json::json!({
                                            "type": "session_reset",
                                            "data": {"session_id": session_id, "success": false, "error": e.to_string()},
                                        }),
                                    };
                                    state.hub.broadcast("session_reset", reply["data"].clone());
                                }
                                "check_stream" => {
                                    let session_id = parsed["data"]["session_id"]
                                        .as_str()
                                        .unwrap_or("default")
                                        .to_string();
                                    let running = active_runs.lock().await.contains_key(&session_id);
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

    info!("ws client disconnected");
}

/// Dispatch a chat message to the agent runner via the unified chat pipeline.
async fn dispatch_chat(state: &AppState, msg: &serde_json::Value, active_runs: ActiveRuns) {
    let data = &msg["data"];
    let session_id = data["session_id"]
        .as_str()
        .unwrap_or("default")
        .to_string();
    let prompt = data["prompt"].as_str().unwrap_or("").to_string();
    let system = data["system"].as_str().unwrap_or("").to_string();
    let user_id = data["user_id"].as_str().unwrap_or("").to_string();
    let channel = data["channel"].as_str().unwrap_or("web").to_string();
    let role_id = data["role_id"].as_str().unwrap_or("").to_string();

    info!(
        session_id = %session_id,
        prompt_len = prompt.len(),
        channel = %channel,
        "dispatch_chat called"
    );

    // Intercept marketplace codes before they reach the agent
    if let Some((code_type, code)) = crate::codes::detect_code(&prompt) {
        crate::codes::handle_code(state, code_type, code, &session_id).await;
        return;
    }

    if prompt.is_empty() {
        warn!("dispatch_chat: empty prompt, rejecting");
        state.hub.broadcast(
            "chat_error",
            serde_json::json!({"error": "empty prompt", "session_id": session_id}),
        );
        return;
    }

    // If role_id is set, build a role-scoped session key for isolation
    let session_key = if !role_id.is_empty() {
        agent::keyparser::build_role_session_key(&role_id, &channel)
    } else {
        session_id
    };

    info!(session_id = %session_key, role_id = %role_id, "dispatching chat to agent");

    // Resolve entity config for the active entity
    let entity_config = {
        let (etype, eid) = if !role_id.is_empty() {
            ("role", role_id.as_str())
        } else {
            ("main", "main")
        };
        crate::entity_config::resolve_for_chat(&state.store, etype, eid)
    };

    let config = ChatConfig {
        session_key,
        prompt,
        system,
        user_id,
        channel,
        origin: Origin::User,
        role_id,
        cancel_token: CancellationToken::new(),
        lane: lanes::MAIN.to_string(),
        comm_reply: None,
        entity_config,
    };

    run_chat(state, config, Some(active_runs)).await;
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
            let msg = serde_json::json!({
                "type": "execute_tool",
                "id": req.id,
                "tool": req.tool,
                "args": req.args,
            });
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
