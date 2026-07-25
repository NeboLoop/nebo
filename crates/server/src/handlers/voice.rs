use axum::extract::State;
use axum::extract::WebSocketUpgrade;
use axum::extract::ws::{Message, WebSocket};
use axum::response::Response;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::state::AppState;

/// Voice conversation — speech-to-speech via the xAI Grok realtime API
/// (Janus metered relay or BYOK direct). Dictation was removed: the OS does
/// it natively (macOS dictation, Win+H) straight into the composer, on
/// device — a local whisper pathway was a worse competing implementation.
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
