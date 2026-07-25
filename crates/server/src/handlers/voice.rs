use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Path, Query, State, WebSocketUpgrade};
use axum::response::Response;
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use super::to_error_response;
use crate::state::AppState;

/// Voice conversation — speech-to-speech via the xAI Grok realtime API
/// (Janus metered relay or BYOK direct). Dictation was removed: the OS does
/// it natively (macOS dictation, Win+H) straight into the composer, on
/// device — a local whisper pathway was a worse competing implementation.
///
/// Voice is a MODALITY of the chat, not a separate surface: the session binds
/// to `agent_id` + `chat_id`, every finished turn persists as a normal chat
/// message (broadcast as `voice_message` so the open thread updates live),
/// and recent chat history is fed into the session so the agent continues the
/// conversation instead of greeting blind.
#[derive(Debug, Deserialize)]
pub struct ConversationQuery {
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub chat_id: Option<String>,
}

pub async fn conversation_ws_handler(
    State(state): State<AppState>,
    Query(q): Query<ConversationQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    info!(agent = ?q.agent_id, chat = ?q.chat_id, "conversation WebSocket upgrade requested");
    ws.on_upgrade(move |socket| handle_conversation_ws(socket, state, q))
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

/// The xAI voices offered in the Identity tab (brand display names live in
/// the frontend; the raw id is what's stored and sent upstream).
const SAMPLE_VOICES: [&str; 5] = ["eve", "ara", "rex", "sal", "leo"];

/// GET /api/v1/agent/voice-sample/{voice_id}
///
/// Short spoken sample so the Identity tab can preview each voice. Generated
/// once through the same realtime leg calls use (Janus-metered or BYOK),
/// then cached as WAV under data/voice-samples/ — every later play is free
/// and instant.
pub async fn voice_sample(
    State(state): State<AppState>,
    Path(voice_id): Path<String>,
) -> Result<Response, (axum::http::StatusCode, axum::Json<types::api::ErrorResponse>)> {
    if !SAMPLE_VOICES.contains(&voice_id.as_str()) {
        return Err(to_error_response(types::NeboError::Validation(
            "unknown voice id".into(),
        )));
    }

    let dir = config::data_dir()
        .map_err(to_error_response)?
        .join("voice-samples");
    let cache_path = dir.join(format!("{voice_id}.wav"));
    if let Ok(bytes) = tokio::fs::read(&cache_path).await {
        return Ok(wav_response(bytes));
    }

    let Some((endpoint, bearer)) = resolve_realtime_leg(&state) else {
        return Err(to_error_response(types::NeboError::Validation(
            "Voice needs a NeboAI account or an xAI API key (Settings → Providers).".into(),
        )));
    };

    let cfg = voice::realtime::RealtimeConfig {
        endpoint,
        bearer,
        voice: voice_id.clone(),
        tools: vec![],
        instructions: "You are demonstrating this voice for a short preview. \
                       Say exactly what you are asked to say, nothing else."
            .into(),
        ..Default::default()
    };
    let (tx, mut rx) = voice::realtime::connect(cfg).await.map_err(|e| {
        error!(error = %e, voice = %voice_id, "voice sample connect failed");
        to_error_response(types::NeboError::Internal(format!(
            "voice sample failed: {e}"
        )))
    })?;
    let _ = tx
        .send(voice::realtime::RealtimeCommand::Text(
            "Say exactly: \"Hi! This is how I sound. Ready when you are.\"".into(),
        ))
        .await;

    let mut pcm: Vec<u8> = Vec::new();
    let collect = async {
        while let Some(event) = rx.recv().await {
            match event {
                voice::conversation::ConversationEvent::AudioChunk(data) => {
                    pcm.extend_from_slice(&data);
                }
                voice::conversation::ConversationEvent::PlaybackEnd => break,
                voice::conversation::ConversationEvent::Error(msg) => {
                    warn!(error = %msg, "voice sample upstream error");
                    break;
                }
                _ => {}
            }
        }
    };
    let _ = tokio::time::timeout(std::time::Duration::from_secs(20), collect).await;
    let _ = tx.send(voice::realtime::RealtimeCommand::Close).await;

    if pcm.is_empty() {
        return Err(to_error_response(types::NeboError::Internal(
            "voice sample produced no audio".into(),
        )));
    }

    let wav = wav_from_pcm16_mono_24k(&pcm);
    if tokio::fs::create_dir_all(&dir).await.is_ok()
        && let Err(e) = tokio::fs::write(&cache_path, &wav).await
    {
        warn!(error = %e, "failed to cache voice sample");
    }
    Ok(wav_response(wav))
}

fn wav_response(bytes: Vec<u8>) -> Response {
    axum::response::Response::builder()
        .header("Content-Type", "audio/wav")
        .header("Cache-Control", "private, max-age=86400")
        .body(axum::body::Body::from(bytes))
        .unwrap_or_default()
}

/// Wrap raw PCM16 LE mono @ 24kHz (the realtime wire format) in a WAV header.
fn wav_from_pcm16_mono_24k(pcm: &[u8]) -> Vec<u8> {
    let data_len = pcm.len() as u32;
    let sample_rate = 24000u32;
    let mut w = Vec::with_capacity(44 + pcm.len());
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data_len).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    w.extend_from_slice(&1u16.to_le_bytes()); // PCM
    w.extend_from_slice(&1u16.to_le_bytes()); // mono
    w.extend_from_slice(&sample_rate.to_le_bytes());
    w.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // byte rate
    w.extend_from_slice(&2u16.to_le_bytes()); // block align
    w.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data_len.to_le_bytes());
    w.extend_from_slice(pcm);
    w
}

/// Voice tool surface: exactly ONE function — `nebo(task)` — which delegates
/// to the agent Runner. The voice model (grok) is a weaker tool-caller and
/// gets none of the text harness (steering, corrections, first-call-success
/// tuning), so handing it raw STRAP schemas produced retry loops. Instead the
/// Runner stays the ONE brain: full harness, full tool loop, same policy
/// engine — and the voice model just narrates the result. Never re-expose the
/// raw registry here; that recreates a second, untuned tool pathway.
fn voice_tools() -> Vec<serde_json::Value> {
    vec![serde_json::json!({
        "type": "function",
        "name": "nebo",
        "description": "Hand a task to your Nebo employee brain — use this for ANYTHING that \
                        needs real data or action: files, printers, email, calendar, web, apps, \
                        documents, system info. Pass the user's request restated with all spoken \
                        context needed to complete it. It runs the full toolchain and returns the \
                        completed result for you to relay aloud.",
        "parameters": {
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "The user's request, self-contained (include names, files, choices already made in this conversation)."
                }
            },
            "required": ["task"]
        }
    })]
}

/// Execute a delegated voice task through the agent Runner and collect the
/// final text. This is the SAME pathway text chat uses — harness, steering,
/// corrections, policy — so voice inherits its first-call reliability.
async fn run_delegated_task(state: &AppState, session_key: &str, task: &str) -> String {
    let req = agent::RunRequest {
        session_key: session_key.to_string(),
        prompt: task.to_string(),
        origin: tools::Origin::User,
        channel: "voice".into(),
        cancel_token: tokio_util::sync::CancellationToken::new(),
        ..Default::default()
    };
    match state.runner.run(req).await {
        Ok(mut rx) => {
            let mut out = String::new();
            while let Some(event) = rx.recv().await {
                if event.event_type == ai::StreamEventType::Text {
                    out.push_str(&event.text);
                }
            }
            if out.trim().is_empty() {
                "The task completed but produced no text summary.".into()
            } else {
                out
            }
        }
        Err(e) => format!("The task failed: {e}"),
    }
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

/// Compact tail of the chat history, injected into the voice session's
/// instructions so the agent picks the conversation up mid-thread.
fn chat_history_context(state: &AppState, chat_id: &str) -> String {
    let Ok(messages) = state.store.get_chat_messages(chat_id) else {
        return String::new();
    };
    if messages.is_empty() {
        return String::new();
    }
    let tail: Vec<String> = messages
        .iter()
        .rev()
        .take(12)
        .rev()
        .filter(|m| !m.content.is_empty())
        .map(|m| {
            let who = if m.role == "user" { "User" } else { "You" };
            let text: String = m.content.chars().take(300).collect();
            format!("{who}: {text}")
        })
        .collect();
    if tail.is_empty() {
        String::new()
    } else {
        format!(
            "\n\nThis voice call continues an ongoing chat. Recent messages:\n{}\n\
             Continue naturally — do not greet from scratch.",
            tail.join("\n")
        )
    }
}

async fn handle_conversation_ws(mut socket: WebSocket, state: AppState, q: ConversationQuery) {
    info!("conversation WebSocket connected");

    // Voice is a modality of a chat — without a real thread to persist into,
    // every turn and delegated run would land in an orphan conversation.
    // Refuse instead of falling back.
    if q.agent_id.as_deref().unwrap_or_default().is_empty()
        || q.chat_id.as_deref().unwrap_or_default().is_empty()
    {
        let msg = serde_json::json!({
            "type": "Error",
            "message": "Voice needs a chat thread to bind to — start it from a chat.",
        });
        let _ = socket.send(Message::Text(msg.to_string().into())).await;
        return;
    }

    let Some((endpoint, bearer)) = resolve_realtime_leg(&state) else {
        let msg = serde_json::json!({
            "type": "Error",
            "message": "Voice conversation needs a NeboAI account or an xAI API key (Settings → Providers).",
        });
        let _ = socket.send(Message::Text(msg.to_string().into())).await;
        return;
    };

    let mut instructions = "You are the user's Nebo AI employee, speaking with them by voice. \
                       Be concise and conversational — short sentences, no markdown, no lists. \
                       For ANYTHING that needs real data or action (files, printers, email, \
                       calendar, web, documents, system info), call the `nebo` tool with the \
                       task and relay its result aloud — never guess and never claim you can't \
                       act. While it works, tell the user you're on it. If the result says \
                       something needs approval or a permission, say so plainly and point them \
                       to the Nebo desktop app."
        .to_string();
    if let Some(chat_id) = q.chat_id.as_deref() {
        instructions.push_str(&chat_history_context(&state, chat_id));
    }

    let (replace, keyterms) = brand_voice_hints();
    let mut cfg = voice::realtime::RealtimeConfig {
        endpoint,
        bearer,
        bot_id: q.agent_id.clone(),
        tools: voice_tools(),
        instructions,
        replace,
        keyterms,
        ..Default::default()
    };
    // Per-agent voice: each employee sounds like themselves (Identity tab).
    if let Some(id) = q.agent_id.as_deref()
        && let Ok(Some(agent)) = state.store.get_agent(id)
        && !agent.voice.is_empty()
    {
        cfg.voice = agent.voice;
    }

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

    handle_conversation_session(socket, state, q, rt_tx, rt_rx).await;
}

/// Persist one finished voice turn as a normal chat message and tell open
/// views about it. Same table, same shape as text turns — the transcript IS
/// chat history, so closing the call leaves the whole exchange in the thread
/// and the next text turn has full context.
fn persist_voice_turn(state: &AppState, chat_id: &str, role: &str, content: &str) {
    let content = content.trim();
    if content.is_empty() {
        return;
    }
    let msg_id = uuid::Uuid::new_v4().to_string();
    match state.store.create_chat_message_for_runner(
        &msg_id,
        chat_id,
        role,
        content,
        None,
        None,
        None,
        Some(r#"{"voice":true}"#),
        None,
    ) {
        Ok(_) => {
            state.hub.broadcast(
                "voice_message",
                serde_json::json!({
                    "id": msg_id,
                    "chatId": chat_id,
                    "role": role,
                    "content": content,
                }),
            );
        }
        Err(e) => error!(error = %e, chat = %chat_id, "failed to persist voice turn"),
    }
}

/// Decode a realtime function-call `arguments` payload into a tool input
/// object. Voice models sometimes DOUBLE-ENCODE: `arguments` contains a JSON
/// string whose contents are the real JSON object (`"{\"action\":...}"`), so
/// a plain parse yields `Value::String` — the tool then sees zero parameters
/// and rejects ("command parameter missing") on every retry. Unwrap string
/// layers until an object appears; anything else becomes `{}` so the tool's
/// own correction message guides the model.
fn decode_tool_arguments(arguments: &str) -> serde_json::Value {
    let mut v: serde_json::Value =
        serde_json::from_str(arguments).unwrap_or(serde_json::Value::Null);
    for _ in 0..2 {
        match v {
            serde_json::Value::String(ref s) => match serde_json::from_str(s) {
                Ok(inner) => v = inner,
                Err(_) => break,
            },
            _ => break,
        }
    }
    if v.is_object() { v } else { serde_json::json!({}) }
}

/// Join a transcript delta onto accumulated text, restoring the space xAI
/// omits between sentence-level segments (mirror of the frontend join).
fn join_transcript(acc: &mut String, delta: &str) {
    let needs_space = acc
        .chars()
        .rev()
        .find(|c| !matches!(c, '"' | '\'' | ')' | ']'))
        .is_some_and(|c| matches!(c, '.' | '!' | '?' | '…'))
        && delta
            .chars()
            .find(|c| !matches!(c, '"' | '\'' | '(' | '['))
            .is_some_and(|c| c.is_uppercase());
    if needs_space && !acc.is_empty() {
        acc.push(' ');
    }
    acc.push_str(delta);
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
    q: ConversationQuery,
    rt_tx: mpsc::Sender<voice::realtime::RealtimeCommand>,
    mut rt_rx: mpsc::Receiver<voice::conversation::ConversationEvent>,
) {
    use voice::conversation::ConversationEvent;
    use voice::realtime::RealtimeCommand;

    let chat_id = q.chat_id.filter(|c| !c.is_empty());

    // Voice tool execution context: user-origin, empty approved_categories —
    // OFF capabilities fail closed with a clear error (no autonomy bypass; the
    // model relays "grant it on desktop"). Same enforcement as every
    // non-interactive caller. Voice is a modality of the chat, so it uses the
    // SAME `agent:<id>:thread:<chat>` session key text chat uses — delegated
    // runs, tool activity, and history all land in the open thread. Both ids
    // are guaranteed non-empty by the guard at connection time.
    let mut ctx = tools::ToolContext::new(tools::Origin::User);
    ctx.session_key = format!(
        "agent:{}:thread:{}",
        q.agent_id.as_deref().unwrap_or_default(),
        chat_id.as_deref().unwrap_or_default()
    );
    ctx.session_id = ctx.session_key.clone();
    let ctx = std::sync::Arc::new(ctx);

    // Turn accumulation for transcript persistence: the user transcript is
    // cumulative (replace), the agent transcript arrives as deltas (join).
    // The user turn is final once the model starts responding; the agent turn
    // once playback ends. Anything left at session end is flushed.
    let mut user_partial = String::new();
    let mut agent_partial = String::new();

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
                    ConversationEvent::TranscriptionText(text) => {
                        user_partial = text.clone();
                        Some(serde_json::json!({"type": "transcription_text", "text": text}))
                    }
                    ConversationEvent::TranscriptionEnd =>
                        Some(serde_json::json!({"type": "transcription_end"})),
                    ConversationEvent::PlaybackStart => {
                        // Model turn started ⇒ the user's utterance is final
                        // (late transcript corrections have landed by now).
                        if let Some(cid) = chat_id.as_deref() {
                            persist_voice_turn(&state, cid, "user", &user_partial);
                        }
                        user_partial.clear();
                        Some(serde_json::json!({"type": "playback_start"}))
                    }
                    ConversationEvent::PlaybackEnd => {
                        if let Some(cid) = chat_id.as_deref() {
                            persist_voice_turn(&state, cid, "assistant", &agent_partial);
                        }
                        agent_partial.clear();
                        Some(serde_json::json!({"type": "playback_end"}))
                    }
                    ConversationEvent::ResponseText(text) => {
                        join_transcript(&mut agent_partial, &text);
                        Some(serde_json::json!({"type": "response_text", "text": text}))
                    }
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
                        info!(
                            tool = %name,
                            call_id = %call_id,
                            args = %arguments.chars().take(300).collect::<String>(),
                            "voice tool call"
                        );
                        pending_tools += 1;
                        let state = state.clone();
                        let ctx = ctx.clone();
                        let done = tool_done_tx.clone();
                        tokio::spawn(async move {
                            let input = decode_tool_arguments(&arguments);
                            // `nebo` delegates to the Runner (the ONE tuned
                            // tool brain); anything else the model improvises
                            // still runs through the policy-gated registry.
                            let output = if name == "nebo" {
                                let task = input
                                    .get("task")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or_default();
                                let content = if task.is_empty() {
                                    "The nebo tool needs a `task` string describing what to do.".to_string()
                                } else {
                                    run_delegated_task(&state, &ctx.session_key, task).await
                                };
                                serde_json::json!({ "ok": true, "content": content })
                            } else {
                                let result = state.tools.execute(&ctx, &name, input).await;
                                // The model needs the outcome either way —
                                // errors included, so it can say what blocked.
                                serde_json::json!({
                                    "ok": !result.is_error,
                                    "content": result.content,
                                })
                            };
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

    // Session over (hangup / barge-out / error): flush any half-finished turn
    // so the transcript in the chat never loses the last exchange.
    if let Some(cid) = chat_id.as_deref() {
        persist_voice_turn(&state, cid, "user", &user_partial);
        persist_voice_turn(&state, cid, "assistant", &agent_partial);
    }
}
