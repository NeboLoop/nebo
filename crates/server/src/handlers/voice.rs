use axum::extract::State;
use axum::extract::ws::{Message, WebSocket};
use axum::extract::WebSocketUpgrade;
use axum::http::StatusCode;
use axum::http::header;
use axum::response::{IntoResponse, Json, Response};
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing::{info, warn, error};

use voice::streaming::{StreamingConfig, StreamingTranscriber, TranscriptEvent};

use super::{ApiResult, ApiError};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct TtsBody {
    pub text: String,
    #[serde(default = "default_voice")]
    pub voice: String,
    #[serde(default = "default_speed")]
    pub speed: f32,
}

fn default_voice() -> String {
    "af_heart".into()
}

fn default_speed() -> f32 {
    1.0
}

/// POST /api/v1/voice/tts
///
/// Accepts a JSON body with `text`, optional `voice` and `speed`.
/// Returns WAV audio bytes with `Content-Type: audio/wav`.
pub async fn tts(
    State(state): State<AppState>,
    Json(body): Json<TtsBody>,
) -> Response {
    info!(text = %body.text, voice = %body.voice, speed = body.speed, "voice tts request");

    let req = voice::TtsRequest {
        text: body.text,
        voice: body.voice,
        speed: body.speed,
    };

    match state.voice.synthesize(req).await {
        Ok(wav_bytes) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "audio/wav")],
            wav_bytes,
        )
            .into_response(),
        Err(e) => {
            error!(error = %e, "tts synthesis failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    }
}

/// POST /api/v1/voice/transcribe
///
/// Accepts raw audio bytes in the request body.
/// Returns JSON `{ "text": "..." }`.
pub async fn transcribe(
    State(state): State<AppState>,
    body: axum::body::Bytes,
) -> ApiResult<serde_json::Value> {
    if body.is_empty() {
        return Err(ApiError(types::NeboError::Validation(
            "empty audio body".into(),
        )));
    }

    info!(bytes = body.len(), "voice transcribe request");

    let result = state.voice.transcribe(&body).await.map_err(|e| {
        ApiError(types::NeboError::Internal(format!(
            "transcription failed: {e}"
        )))
    })?;

    Ok(Json(serde_json::json!({ "text": result.text })))
}

/// GET /api/v1/voice/status
///
/// Returns availability of local voice engines.
pub async fn status(
    State(state): State<AppState>,
) -> Json<serde_json::Value> {
    let status = state.voice.status();
    Json(serde_json::json!(status))
}

// ---------------------------------------------------------------------------
// WebSocket: Streaming Dictation
// ---------------------------------------------------------------------------

/// GET /ws/voice/dictation — Streaming speech-to-text via WebSocket.
///
/// Wire protocol:
/// - Client → Server: JSON `{"type": "Start", "route": "editor"}` or
///                          `{"type": "Start", "route": "agent", "agentId": "..."}`
/// - Client → Server: Binary PCM Int16 audio chunks (16kHz mono)
/// - Client → Server: JSON `{"type": "KeepAlive"}`
/// - Client → Server: JSON `{"type": "CloseStream"}`
/// - Server → Client: JSON `{"type": "TranscriptInterim", "text": "..."}`
/// - Server → Client: JSON `{"type": "TranscriptText", "text": "..."}`
/// - Server → Client: JSON `{"type": "TranscriptEndpoint"}`
/// - Server → Client: JSON `{"type": "Error", "message": "..."}`
pub async fn dictation_ws_handler(
    State(state): State<AppState>,
    ws: WebSocketUpgrade,
) -> Response {
    info!("dictation WebSocket upgrade requested");
    ws.on_upgrade(move |socket| handle_dictation_ws(socket, state))
}

/// Dictation routing mode — where transcript text goes.
#[derive(Debug, Clone)]
enum DictationRoute {
    /// Transcript sent to client only (for insertion into TipTap editor).
    Editor,
    /// Transcript sent to client AND fed to a specific agent.
    Agent { agent_id: String },
}

async fn handle_dictation_ws(mut socket: WebSocket, state: AppState) {
    info!("dictation WebSocket connected — waiting for Start message");

    // Wait for the Start message to determine routing
    let route = match wait_for_start(&mut socket).await {
        Some(r) => r,
        None => return, // Client disconnected or sent invalid start
    };

    info!(?route, "dictation session started");

    // Initialize whisper context
    let ctx = match state.voice.whisper_context().await {
        Ok(ctx) => ctx,
        Err(e) => {
            let _ = socket
                .send(Message::Text(
                    serde_json::json!({"type": "Error", "message": e.to_string()}).to_string().into(),
                ))
                .await;
            return;
        }
    };

    // Create streaming transcriber
    let config = StreamingConfig::default();
    let transcriber = StreamingTranscriber::new(ctx, config);
    let (audio_tx, event_rx) = transcriber.start();

    // Run the main dictation session loop (select between WS messages and transcript events)
    handle_dictation_session(socket, audio_tx, event_rx, route, state).await;
}

async fn handle_dictation_session(
    mut socket: WebSocket,
    audio_tx: mpsc::Sender<Vec<i16>>,
    mut event_rx: mpsc::Receiver<TranscriptEvent>,
    route: DictationRoute,
    state: AppState,
) {
    loop {
        tokio::select! {
            // Receive transcript events from the streaming transcriber
            event = event_rx.recv() => {
                match event {
                    Some(TranscriptEvent::Interim(text)) => {
                        let msg = serde_json::json!({"type": "TranscriptInterim", "text": text});
                        if socket.send(Message::Text(msg.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    Some(TranscriptEvent::Text(text)) => {
                        let msg = serde_json::json!({"type": "TranscriptText", "text": text});
                        if socket.send(Message::Text(msg.to_string().into())).await.is_err() {
                            break;
                        }
                        // If routing to agent, feed the confirmed text
                        if let DictationRoute::Agent { ref agent_id } = route {
                            feed_agent_transcript(&state, agent_id, &text).await;
                        }
                    }
                    Some(TranscriptEvent::Endpoint) => {
                        let msg = serde_json::json!({"type": "TranscriptEndpoint"});
                        if socket.send(Message::Text(msg.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    Some(TranscriptEvent::Error(message)) => {
                        let msg = serde_json::json!({"type": "Error", "message": message});
                        if socket.send(Message::Text(msg.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    None => {
                        // Transcriber channel closed
                        break;
                    }
                }
            }

            // Receive messages from the WebSocket client
            ws_msg = socket.recv() => {
                match ws_msg {
                    Some(Ok(Message::Binary(data))) => {
                        // PCM Int16 audio chunk (16kHz mono)
                        let samples: Vec<i16> = data
                            .chunks_exact(2)
                            .map(|b| i16::from_le_bytes([b[0], b[1]]))
                            .collect();
                        if audio_tx.send(samples).await.is_err() {
                            warn!("audio channel closed — transcriber died");
                            break;
                        }
                    }
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text) {
                            match parsed.get("type").and_then(|t| t.as_str()) {
                                Some("KeepAlive") => {
                                    // No-op, just keeps the connection alive
                                }
                                Some("CloseStream") => {
                                    info!("dictation CloseStream received");
                                    // Drop audio_tx to signal end-of-stream to transcriber
                                    drop(audio_tx);
                                    // Drain remaining events
                                    while let Some(event) = event_rx.recv().await {
                                        let msg = match event {
                                            TranscriptEvent::Interim(t) => serde_json::json!({"type": "TranscriptInterim", "text": t}),
                                            TranscriptEvent::Text(t) => {
                                                if let DictationRoute::Agent { ref agent_id } = route {
                                                    feed_agent_transcript(&state, agent_id, &t).await;
                                                }
                                                serde_json::json!({"type": "TranscriptText", "text": t})
                                            }
                                            TranscriptEvent::Endpoint => serde_json::json!({"type": "TranscriptEndpoint"}),
                                            TranscriptEvent::Error(m) => serde_json::json!({"type": "Error", "message": m}),
                                        };
                                        let _ = socket.send(Message::Text(msg.to_string().into())).await;
                                    }
                                    return;
                                }
                                _ => {
                                    warn!(msg = %text, "unknown dictation WS message");
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        info!("dictation WebSocket closed");
                        break;
                    }
                    Some(Ok(_)) => {} // Ping/Pong handled by Axum
                    Some(Err(e)) => {
                        warn!(error = %e, "dictation WebSocket error");
                        break;
                    }
                }
            }
        }
    }
}

/// Wait for the Start message that specifies the routing mode.
async fn wait_for_start(socket: &mut WebSocket) -> Option<DictationRoute> {
    // Give client 10 seconds to send the Start message
    let timeout = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while let Some(Ok(msg)) = socket.recv().await {
            if let Message::Text(text) = msg {
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text) {
                    if parsed.get("type").and_then(|t| t.as_str()) == Some("Start") {
                        let route = match parsed.get("route").and_then(|r| r.as_str()) {
                            Some("agent") => {
                                let agent_id = parsed
                                    .get("agentId")
                                    .and_then(|a| a.as_str())
                                    .unwrap_or("assistant")
                                    .to_string();
                                DictationRoute::Agent { agent_id }
                            }
                            _ => DictationRoute::Editor,
                        };
                        return Some(route);
                    }
                }
            }
        }
        None
    })
    .await;

    match timeout {
        Ok(route) => route,
        Err(_) => {
            warn!("dictation WebSocket timed out waiting for Start message");
            let _ = socket
                .send(Message::Text(
                    serde_json::json!({"type": "Error", "message": "timeout waiting for Start"})
                        .to_string().into(),
                ))
                .await;
            None
        }
    }
}

/// Feed a confirmed transcript segment to an agent as a user message.
async fn feed_agent_transcript(state: &AppState, agent_id: &str, text: &str) {
    // Send the transcript text as a user message to the agent's chat session.
    // This uses the hub broadcast to trigger the same chat dispatch as a typed message.
    state.hub.broadcast(
        "dictation_transcript",
        serde_json::json!({
            "agentId": agent_id,
            "text": text,
        }),
    );
}

// ---------------------------------------------------------------------------
// WebSocket: Voice Conversation
// ---------------------------------------------------------------------------

/// GET /ws/voice/conversation — Full-duplex voice conversation via WebSocket.
///
/// Wire protocol:
///
/// Client -> Server:
/// - Binary: PCM Int16 audio chunks (16kHz mono)
/// - JSON: `{"type": "KeepAlive"}`
/// - JSON: `{"type": "interrupt"}` — user interrupted during playback
/// - JSON: `{"type": "manual_input_end"}` — user explicitly ended input
///
/// Server -> Client:
/// - JSON: `{"type": "session_initialized"}`
/// - JSON: `{"type": "transcription_start"}`
/// - JSON: `{"type": "transcription_text", "text": "..."}`
/// - JSON: `{"type": "transcription_end"}`
/// - JSON: `{"type": "playback_start"}`
/// - Binary: TTS audio chunks (PCM Int16)
/// - JSON: `{"type": "playback_end"}`
/// - JSON: `{"type": "response_text", "text": "..."}` — agent's text response
/// - JSON: `{"type": "Error", "message": "..."}`
pub async fn conversation_ws_handler(
    State(state): State<AppState>,
    ws: WebSocketUpgrade,
) -> Response {
    info!("conversation WebSocket upgrade requested");
    ws.on_upgrade(move |socket| handle_conversation_ws(socket, state))
}

async fn handle_conversation_ws(socket: WebSocket, state: AppState) {
    info!("conversation WebSocket connected");

    // Create the agent request channel. The conversation orchestrator sends
    // AgentRequests here; we spawn a task that handles them by broadcasting
    // to the hub and waiting for a response.
    let (agent_tx, agent_rx) = mpsc::channel::<voice::conversation::AgentRequest>(4);

    // Spawn agent handler task
    tokio::spawn(handle_agent_requests(agent_rx, state.clone()));

    // Create and start the conversation orchestrator
    let config = voice::conversation::ConversationConfig::default();
    let orchestrator = voice::conversation::ConversationOrchestrator::new(
        config,
        state.voice.clone(),
        agent_tx,
    );
    let (cmd_tx, event_rx) = orchestrator.start();

    // Run the WebSocket bridge (translates between WS messages and orchestrator channels)
    handle_conversation_session(socket, cmd_tx, event_rx).await;
}

async fn handle_conversation_session(
    mut socket: WebSocket,
    cmd_tx: mpsc::Sender<voice::conversation::ConversationCommand>,
    mut event_rx: mpsc::Receiver<voice::conversation::ConversationEvent>,
) {
    use voice::conversation::{ConversationCommand, ConversationEvent};

    loop {
        tokio::select! {
            // Events from the conversation orchestrator -> send to client
            event = event_rx.recv() => {
                match event {
                    Some(ConversationEvent::SessionInitialized) => {
                        let msg = serde_json::json!({"type": "session_initialized"});
                        if socket.send(Message::Text(msg.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    Some(ConversationEvent::TranscriptionStart) => {
                        let msg = serde_json::json!({"type": "transcription_start"});
                        if socket.send(Message::Text(msg.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    Some(ConversationEvent::TranscriptionText(text)) => {
                        let msg = serde_json::json!({"type": "transcription_text", "text": text});
                        if socket.send(Message::Text(msg.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    Some(ConversationEvent::TranscriptionEnd) => {
                        let msg = serde_json::json!({"type": "transcription_end"});
                        if socket.send(Message::Text(msg.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    Some(ConversationEvent::PlaybackStart) => {
                        let msg = serde_json::json!({"type": "playback_start"});
                        if socket.send(Message::Text(msg.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    Some(ConversationEvent::AudioChunk(data)) => {
                        if socket.send(Message::Binary(data.to_vec().into())).await.is_err() {
                            break;
                        }
                    }
                    Some(ConversationEvent::PlaybackEnd) => {
                        let msg = serde_json::json!({"type": "playback_end"});
                        if socket.send(Message::Text(msg.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    Some(ConversationEvent::ResponseText(text)) => {
                        let msg = serde_json::json!({"type": "response_text", "text": text});
                        if socket.send(Message::Text(msg.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    Some(ConversationEvent::Error(message)) => {
                        let msg = serde_json::json!({"type": "Error", "message": message});
                        if socket.send(Message::Text(msg.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    None => {
                        // Orchestrator channel closed
                        info!("conversation orchestrator ended");
                        break;
                    }
                }
            }

            // Messages from the WebSocket client -> send to orchestrator
            ws_msg = socket.recv() => {
                match ws_msg {
                    Some(Ok(Message::Binary(data))) => {
                        // PCM Int16 audio chunk (16kHz mono)
                        let samples: Vec<i16> = data
                            .chunks_exact(2)
                            .map(|b| i16::from_le_bytes([b[0], b[1]]))
                            .collect();
                        if cmd_tx.send(ConversationCommand::Audio(samples)).await.is_err() {
                            warn!("conversation command channel closed");
                            break;
                        }
                    }
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text) {
                            match parsed.get("type").and_then(|t| t.as_str()) {
                                Some("KeepAlive") => {
                                    // No-op
                                }
                                Some("interrupt") => {
                                    info!("conversation interrupt received");
                                    if cmd_tx.send(ConversationCommand::Interrupt).await.is_err() {
                                        break;
                                    }
                                }
                                Some("manual_input_end") => {
                                    info!("conversation manual_input_end received");
                                    if cmd_tx.send(ConversationCommand::ManualInputEnd).await.is_err() {
                                        break;
                                    }
                                }
                                _ => {
                                    warn!(msg = %text, "unknown conversation WS message");
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        info!("conversation WebSocket closed");
                        break;
                    }
                    Some(Ok(_)) => {} // Ping/Pong handled by Axum
                    Some(Err(e)) => {
                        warn!(error = %e, "conversation WebSocket error");
                        break;
                    }
                }
            }
        }
    }
}

/// Handle agent requests from the conversation orchestrator.
///
/// Runs the user's utterance through the real agent pipeline and STREAMS
/// text chunks back as they arrive from the LLM. This enables the orchestrator
/// to start TTS at sentence boundaries without waiting for the full response.
async fn handle_agent_requests(
    mut rx: mpsc::Receiver<voice::conversation::AgentRequest>,
    state: AppState,
) {
    // Use a stable session key so voice conversation has memory continuity
    let session_key = format!("voice:conversation:{}", uuid::Uuid::new_v4());

    while let Some(req) = rx.recv().await {
        info!(text = %req.text, "conversation agent request");

        let runner = state.runner.clone();
        let cancel = tokio_util::sync::CancellationToken::new();

        let run_req = agent::RunRequest {
            session_key: session_key.clone(),
            prompt: req.text.clone(),
            origin: tools::Origin::User,
            channel: "voice".into(),
            cancel_token: cancel.clone(),
            max_iterations: 1, // Single-turn, no tool loops for voice
            ..Default::default()
        };

        match runner.run(run_req).await {
            Ok(mut rx_stream) => {
                let mut has_content = false;
                while let Some(event) = rx_stream.recv().await {
                    if event.event_type == ai::StreamEventType::Text && !event.text.is_empty() {
                        has_content = true;
                        // Stream each text chunk immediately to the orchestrator
                        if req.response_tx.send(event.text).await.is_err() {
                            break; // Orchestrator dropped (interrupted)
                        }
                    }
                }
                if !has_content {
                    let _ = req.response_tx
                        .send("I'm sorry, I wasn't able to generate a response.".to_string())
                        .await;
                }
                // Dropping response_tx signals end of stream
            }
            Err(e) => {
                error!(error = %e, "voice conversation agent run failed");
                let _ = req.response_tx
                    .send(format!("Sorry, I encountered an error: {}", e))
                    .await;
            }
        };
    }
}
