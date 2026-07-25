use axum::extract::State;
use axum::extract::WebSocketUpgrade;
use axum::extract::ws::{Message, WebSocket};
use axum::http::StatusCode;
use axum::http::header;
use axum::response::{IntoResponse, Json, Response};
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use voice::streaming::{StreamingConfig, StreamingTranscriber, TranscriptEvent};

use super::{ApiError, ApiResult};
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
pub async fn tts(State(state): State<AppState>, Json(body): Json<TtsBody>) -> Response {
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
pub async fn status(State(state): State<AppState>) -> Json<serde_json::Value> {
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
pub async fn dictation_ws_handler(State(state): State<AppState>, ws: WebSocketUpgrade) -> Response {
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
                    serde_json::json!({"type": "Error", "message": e.to_string()})
                        .to_string()
                        .into(),
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
                        .to_string()
                        .into(),
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

/// Resolve the realtime upstream leg — the same direct-vs-Janus split as text
/// providers: a user-owned xAI key dials api.x.ai directly (no Janus, no
/// metering — their key); otherwise the NeboAI account rides Janus's metered
/// relay with the user's Janus JWT.
fn resolve_realtime_leg(state: &AppState) -> Option<(String, String)> {
    if let Ok(Some(profile)) = state.store.get_best_auth_profile("xai")
        && !profile.api_key.is_empty()
    {
        return Some(("wss://api.x.ai/v1/realtime".into(), profile.api_key));
    }
    let token = crate::codes::neboai_token(state)?;
    let url = state
        .config
        .neboai
        .janus_url
        .replacen("https://", "wss://", 1)
        .replacen("http://", "ws://", 1);
    Some((format!("{}/v1/realtime", url.trim_end_matches('/')), token))
}

/// Voice tool surface: every registry tool the permission layer lists, as
/// xAI `type: "function"` entries. Execution still passes through the full
/// policy engine — an OFF capability fails closed with a clear error the
/// model can relay ("that needs approval on the desktop"), so exposing the
/// schema never bypasses a gate.
async fn voice_tools(state: &AppState) -> Vec<serde_json::Value> {
    state
        .tools
        .list()
        .await
        .into_iter()
        .map(|d| {
            serde_json::json!({
                "type": "function",
                "name": d.name,
                "description": d.description,
                "parameters": d.input_schema,
            })
        })
        .collect()
}

/// Brand-term pronunciation map + ASR bias so the model says product names
/// right while transcripts stay clean.
fn brand_voice_hints() -> (serde_json::Map<String, serde_json::Value>, Vec<String>) {
    let mut replace = serde_json::Map::new();
    replace.insert("NeboAI".into(), serde_json::Value::String("Nee-bo A I".into()));
    replace.insert("NeboLoop".into(), serde_json::Value::String("Nee-bo Loop".into()));
    replace.insert("Nebo".into(), serde_json::Value::String("Nee-bo".into()));
    let keyterms = vec![
        "Nebo".into(),
        "NeboAI".into(),
        "NeboLoop".into(),
        "Janus".into(),
    ];
    (replace, keyterms)
}

async fn handle_conversation_ws(mut socket: WebSocket, state: AppState) {
    info!("conversation WebSocket connected");

    let Some((endpoint, bearer)) = resolve_realtime_leg(&state) else {
        let msg = serde_json::json!({
            "type": "Error",
            "message": "Voice conversation needs a NeboAI account or an xAI API key (Settings → Providers).",
        });
        let _ = socket.send(Message::Text(msg.to_string().into())).await;
        return;
    };

    let (replace, keyterms) = brand_voice_hints();
    let cfg = voice::realtime::RealtimeConfig {
        endpoint,
        bearer,
        tools: voice_tools(&state).await,
        instructions: "You are the user's Nebo AI employee, speaking with them by voice. \
                       Be concise and conversational — short sentences, no markdown, no lists. \
                       Use your tools to act on their behalf; if a tool reports that it needs \
                       approval or a permission, say so plainly and tell them to grant it in \
                       the Nebo desktop app."
            .into(),
        replace,
        keyterms,
        ..Default::default()
    };

    let (rt_tx, rt_rx) = match voice::realtime::connect(cfg).await {
        Ok(pair) => pair,
        Err(e) => {
            error!(error = %e, "realtime connect failed");
            let msg = serde_json::json!({
                "type": "Error",
                "message": format!("Voice connection failed: {e}"),
            });
            let _ = socket.send(Message::Text(msg.to_string().into())).await;
            return;
        }
    };

    handle_conversation_session(socket, state, rt_tx, rt_rx).await;
}

/// Bridge the browser WebSocket to the xAI realtime session, executing tool
/// calls through the tools registry (the ONE policy engine) as they surface.
///
/// Downstream wire protocol is unchanged from the cascade era — the frontend
/// store keeps working. New downstream frames: `conversation_id` (resumption
/// handle) rides alongside the existing set.
async fn handle_conversation_session(
    mut socket: WebSocket,
    state: AppState,
    rt_tx: mpsc::Sender<voice::realtime::RealtimeCommand>,
    mut rt_rx: mpsc::Receiver<voice::conversation::ConversationEvent>,
) {
    use voice::conversation::ConversationEvent;
    use voice::realtime::RealtimeCommand;

    // Voice tool execution context: user-origin, empty approved_categories —
    // OFF capabilities fail closed with a clear error (no autonomy bypass; the
    // model relays "grant it on desktop"). Same enforcement as every
    // non-interactive caller.
    let mut ctx = tools::ToolContext::new(tools::Origin::User);
    ctx.session_key = format!("voice:conversation:{}", uuid::Uuid::new_v4());
    ctx.session_id = ctx.session_key.clone();
    let ctx = std::sync::Arc::new(ctx);

    // Completed tool executions flow back through this channel so the select
    // loop below owns all rt_tx sends (all outputs before one continuation).
    let (tool_done_tx, mut tool_done_rx) = mpsc::channel::<(String, String)>(8);
    let mut pending_tools: usize = 0;

    loop {
        tokio::select! {
            // Events from the realtime engine -> client (+ tool dispatch)
            event = rt_rx.recv() => {
                let Some(event) = event else {
                    info!("realtime session ended");
                    break;
                };
                let frame = match event {
                    ConversationEvent::SessionInitialized =>
                        Some(serde_json::json!({"type": "session_initialized"})),
                    ConversationEvent::TranscriptionStart =>
                        Some(serde_json::json!({"type": "transcription_start"})),
                    // Cumulative transcript — the client replaces, never appends.
                    ConversationEvent::TranscriptionText(text) =>
                        Some(serde_json::json!({"type": "transcription_text", "text": text})),
                    ConversationEvent::TranscriptionEnd =>
                        Some(serde_json::json!({"type": "transcription_end"})),
                    ConversationEvent::PlaybackStart =>
                        Some(serde_json::json!({"type": "playback_start"})),
                    ConversationEvent::PlaybackEnd =>
                        Some(serde_json::json!({"type": "playback_end"})),
                    ConversationEvent::ResponseText(text) =>
                        Some(serde_json::json!({"type": "response_text", "text": text})),
                    ConversationEvent::ConversationId(id) =>
                        Some(serde_json::json!({"type": "conversation_id", "id": id})),
                    ConversationEvent::Error(message) =>
                        Some(serde_json::json!({"type": "Error", "message": message})),
                    ConversationEvent::AudioChunk(data) => {
                        if socket.send(Message::Binary(data.to_vec().into())).await.is_err() {
                            break;
                        }
                        None
                    }
                    ConversationEvent::ToolCall { call_id, name, arguments } => {
                        info!(tool = %name, call_id = %call_id, "voice tool call");
                        pending_tools += 1;
                        let registry = state.tools.clone();
                        let ctx = ctx.clone();
                        let done = tool_done_tx.clone();
                        tokio::spawn(async move {
                            let input = serde_json::from_str::<serde_json::Value>(&arguments)
                                .unwrap_or_else(|_| serde_json::json!({}));
                            let result = registry.execute(&ctx, &name, input).await;
                            // The model needs the outcome either way — errors
                            // included, so it can tell the user what blocked.
                            let output = serde_json::json!({
                                "ok": !result.is_error,
                                "content": result.content,
                            });
                            let _ = done.send((call_id, output.to_string())).await;
                        });
                        None
                    }
                };
                if let Some(frame) = frame
                    && socket.send(Message::Text(frame.to_string().into())).await.is_err()
                {
                    break;
                }
            }

            // Finished tool executions -> upstream. ALL outputs first, then
            // exactly one continuation once nothing is outstanding.
            Some((call_id, output)) = tool_done_rx.recv() => {
                if rt_tx.send(RealtimeCommand::ToolOutput { call_id, output }).await.is_err() {
                    break;
                }
                pending_tools = pending_tools.saturating_sub(1);
                if pending_tools == 0
                    && rt_tx.send(RealtimeCommand::ToolOutputsDone).await.is_err()
                {
                    break;
                }
            }

            // Messages from the WebSocket client -> upstream
            ws_msg = socket.recv() => {
                match ws_msg {
                    Some(Ok(Message::Binary(data))) => {
                        // PCM Int16 LE mono @ 24kHz — forwarded verbatim
                        // (binary transport end to end, no transcoding).
                        if rt_tx.send(RealtimeCommand::Audio(data.into())).await.is_err() {
                            warn!("realtime command channel closed");
                            break;
                        }
                    }
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text) {
                            match parsed.get("type").and_then(|t| t.as_str()) {
                                Some("KeepAlive") => {}
                                Some("interrupt") => {
                                    info!("conversation interrupt received");
                                    if rt_tx.send(RealtimeCommand::Interrupt).await.is_err() {
                                        break;
                                    }
                                }
                                // server_vad owns endpointing; the old
                                // push-to-talk end marker is a no-op kept for
                                // wire-protocol compatibility.
                                Some("manual_input_end") => {}
                                Some("text_input") => {
                                    if let Some(t) = parsed.get("text").and_then(|v| v.as_str())
                                        && rt_tx.send(RealtimeCommand::Text(t.to_string())).await.is_err()
                                    {
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
                        let _ = rt_tx.send(RealtimeCommand::Close).await;
                        break;
                    }
                    Some(Ok(_)) => {} // Ping/Pong handled by Axum
                    Some(Err(e)) => {
                        warn!(error = %e, "conversation WebSocket error");
                        let _ = rt_tx.send(RealtimeCommand::Close).await;
                        break;
                    }
                }
            }
        }
    }
}
