//! xAI Grok realtime (speech-to-speech) client.
//!
//! One WebSocket to a realtime endpoint — either Janus's metered relay
//! (`wss://janus.neboai.com/v1/realtime`, Bearer = the user's Janus JWT) or
//! xAI direct (`wss://api.x.ai/v1/realtime`, Bearer = a user-owned xAI key).
//! Both speak the same protocol; Janus forwards frames verbatim and bills
//! wall-clock minutes.
//!
//! The session exposes the same channel shape as the old cascade
//! orchestrator: commands in, [`ConversationEvent`]s out — so the
//! `/ws/voice/conversation` handler keeps its downstream wire protocol
//! unchanged. Tool calls surface as [`ConversationEvent::ToolCall`]; the
//! server executes them through the tools registry (policy engine, origin
//! tagging) and feeds results back via [`RealtimeCommand::ToolOutput`] +
//! [`RealtimeCommand::ToolOutputsDone`] — xAI requires ALL outputs before a
//! single `response.create`.
//!
//! Audio is 24kHz PCM16 LE mono in BOTH directions with binary WS transport
//! (`transport: "binary"`), so no base64 framing and no resampling anywhere
//! in the chain. The JSON-transport delta events are still handled as a
//! fallback in case the server ignores the transport hint.

use base64::Engine as _;
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::Message;
use tracing::{debug, info, warn};

use crate::VoiceError;
use crate::conversation::ConversationEvent;

/// Configuration for one realtime session.
#[derive(Debug, Clone)]
pub struct RealtimeConfig {
    /// `wss://.../v1/realtime` (Janus relay or xAI direct).
    pub endpoint: String,
    /// Model, e.g. `grok-voice-latest`.
    pub model: String,
    /// Bearer token: Janus JWT for the relay leg, xAI API key for direct.
    pub bearer: String,
    /// Janus attribution header (X-Bot-ID); ignored by xAI direct.
    pub bot_id: Option<String>,
    /// Resume a previous conversation (xAI resumption; 30 min expiry).
    pub conversation_id: Option<String>,
    /// System prompt for the voice agent.
    pub instructions: String,
    /// Voice id: eve, ara, rex, sal, leo, or a custom voice id.
    pub voice: String,
    /// Playback speed multiplier, 0.7–1.5.
    pub speed: f64,
    /// Pronunciation map applied before TTS (transcripts stay clean).
    pub replace: serde_json::Map<String, Value>,
    /// ASR bias terms (max 100, 50 chars each).
    pub keyterms: Vec<String>,
    /// `type: "function"` tool entries (client-side only — server-side xAI
    /// tools like mcp/web_search are never exposed; they'd execute outside
    /// the policy engine).
    pub tools: Vec<Value>,
}

impl Default for RealtimeConfig {
    fn default() -> Self {
        Self {
            endpoint: "wss://api.x.ai/v1/realtime".into(),
            model: "grok-voice-latest".into(),
            bearer: String::new(),
            bot_id: None,
            conversation_id: None,
            instructions: String::new(),
            voice: "eve".into(),
            speed: 1.0,
            replace: serde_json::Map::new(),
            keyterms: Vec::new(),
            tools: Vec::new(),
        }
    }
}

/// Commands into a live realtime session.
#[derive(Debug)]
pub enum RealtimeCommand {
    /// Raw PCM16 LE mono audio @ 24kHz — forwarded as a binary WS frame.
    Audio(Bytes),
    /// Typed user input (no audio): creates a message item and requests a
    /// response.
    Text(String),
    /// Barge-in: clear the input buffer and cancel the in-flight response.
    Interrupt,
    /// One executed tool result. Queue every parallel call's output BEFORE
    /// sending [`RealtimeCommand::ToolOutputsDone`].
    ToolOutput { call_id: String, output: String },
    /// All tool outputs submitted — request the model's continuation.
    /// Callers must wait for current audio playback to finish first, or the
    /// next response overlaps the tail of the current one.
    ToolOutputsDone,
    /// Close the session.
    Close,
}

/// Start a realtime session. Returns the command sender and the event
/// receiver; the socket task runs until `Close`, upstream close, or error.
pub async fn connect(
    cfg: RealtimeConfig,
) -> Result<(mpsc::Sender<RealtimeCommand>, mpsc::Receiver<ConversationEvent>), VoiceError> {
    let mut url = format!("{}?model={}", cfg.endpoint, cfg.model);
    if let Some(cid) = &cfg.conversation_id {
        url.push_str(&format!("&conversation_id={}", cid));
    }

    let mut request = url
        .clone()
        .into_client_request()
        .map_err(|e| VoiceError::Realtime(format!("bad realtime url: {e}")))?;
    request.headers_mut().insert(
        "Authorization",
        format!("Bearer {}", cfg.bearer)
            .parse()
            .map_err(|_| VoiceError::Realtime("invalid bearer token".into()))?,
    );
    if let Some(bot_id) = &cfg.bot_id {
        if let Ok(v) = bot_id.parse() {
            request.headers_mut().insert("X-Bot-ID", v);
        }
    }

    let (ws, _resp) = tokio_tungstenite::connect_async(request)
        .await
        .map_err(|e| VoiceError::Realtime(format!("realtime dial failed: {e}")))?;
    info!(endpoint = %cfg.endpoint, model = %cfg.model, "realtime session connected");

    let (cmd_tx, cmd_rx) = mpsc::channel::<RealtimeCommand>(64);
    let (event_tx, event_rx) = mpsc::channel::<ConversationEvent>(64);

    tokio::spawn(run_session(ws, cfg, cmd_rx, event_tx));

    Ok((cmd_tx, event_rx))
}

/// Build the `session.update` frame from the config.
fn session_update(cfg: &RealtimeConfig) -> Value {
    let mut session = json!({
        "instructions": cfg.instructions,
        "voice": cfg.voice,
        "turn_detection": { "type": "server_vad" },
        // Opt in to resumption so a dropped connection can reconnect with
        // ?conversation_id= and replay history (both sides must opt in).
        "resumption": { "enabled": true },
        "audio": {
            "input": {
                "format": { "type": "audio/pcm", "rate": 24000 },
                "transport": "binary",
            },
            "output": {
                "format": { "type": "audio/pcm", "rate": 24000 },
                "transport": "binary",
                "speed": cfg.speed,
            },
        },
    });

    if !cfg.keyterms.is_empty() {
        session["audio"]["input"]["transcription"] = json!({ "keyterms": cfg.keyterms });
    }
    if !cfg.replace.is_empty() {
        session["replace"] = Value::Object(cfg.replace.clone());
    }
    if !cfg.tools.is_empty() {
        session["tools"] = Value::Array(cfg.tools.clone());
    }

    json!({ "type": "session.update", "session": session })
}

async fn run_session(
    ws: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    cfg: RealtimeConfig,
    mut cmd_rx: mpsc::Receiver<RealtimeCommand>,
    event_tx: mpsc::Sender<ConversationEvent>,
) {
    let (mut sink, mut stream) = ws.split();

    // Configure the session before any audio flows.
    if let Err(e) = sink
        .send(Message::Text(session_update(&cfg).to_string().into()))
        .await
    {
        let _ = event_tx
            .send(ConversationEvent::Error(format!("session.update failed: {e}")))
            .await;
        return;
    }

    let mut initialized = false;

    loop {
        tokio::select! {
            cmd = cmd_rx.recv() => {
                let Some(cmd) = cmd else { break };
                let result = match cmd {
                    RealtimeCommand::Audio(pcm) => {
                        // transport: "binary" — raw codec bytes, no base64.
                        sink.send(Message::Binary(pcm.to_vec().into())).await
                    }
                    RealtimeCommand::Text(text) => {
                        let item = json!({
                            "type": "conversation.item.create",
                            "item": {
                                "type": "message",
                                "role": "user",
                                "content": [{ "type": "input_text", "text": text }],
                            },
                        });
                        match sink.send(Message::Text(item.to_string().into())).await {
                            Ok(()) => {
                                sink.send(Message::Text(
                                    json!({ "type": "response.create" }).to_string().into(),
                                ))
                                .await
                            }
                            Err(e) => Err(e),
                        }
                    }
                    RealtimeCommand::Interrupt => {
                        let clear = json!({ "type": "input_audio_buffer.clear" });
                        match sink.send(Message::Text(clear.to_string().into())).await {
                            Ok(()) => {
                                sink.send(Message::Text(
                                    json!({ "type": "response.cancel" }).to_string().into(),
                                ))
                                .await
                            }
                            Err(e) => Err(e),
                        }
                    }
                    RealtimeCommand::ToolOutput { call_id, output } => {
                        let item = json!({
                            "type": "conversation.item.create",
                            "item": {
                                "type": "function_call_output",
                                "call_id": call_id,
                                "output": output,
                            },
                        });
                        sink.send(Message::Text(item.to_string().into())).await
                    }
                    RealtimeCommand::ToolOutputsDone => {
                        sink.send(Message::Text(
                            json!({ "type": "response.create" }).to_string().into(),
                        ))
                        .await
                    }
                    RealtimeCommand::Close => {
                        let _ = sink.send(Message::Close(None)).await;
                        break;
                    }
                };
                if let Err(e) = result {
                    let _ = event_tx
                        .send(ConversationEvent::Error(format!("realtime send failed: {e}")))
                        .await;
                    break;
                }
            }

            msg = stream.next() => {
                match msg {
                    Some(Ok(Message::Binary(data))) => {
                        // transport: "binary" — model audio as raw PCM16 @ 24kHz.
                        if event_tx
                            .send(ConversationEvent::AudioChunk(Bytes::from(data)))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Some(Ok(Message::Text(text))) => {
                        if handle_server_event(&text, &event_tx, &mut initialized).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(frame))) => {
                        info!(?frame, "realtime upstream closed");
                        break;
                    }
                    Some(Ok(_)) => {} // ping/pong handled by tungstenite
                    Some(Err(e)) => {
                        let _ = event_tx
                            .send(ConversationEvent::Error(format!("realtime stream error: {e}")))
                            .await;
                        break;
                    }
                    None => break,
                }
            }
        }
    }

    debug!("realtime session task ended");
}

/// Translate one xAI server event into `ConversationEvent`s. Returns Err when
/// the event channel is closed (downstream gone).
async fn handle_server_event(
    text: &str,
    event_tx: &mpsc::Sender<ConversationEvent>,
    initialized: &mut bool,
) -> Result<(), ()> {
    let Ok(ev) = serde_json::from_str::<Value>(text) else {
        warn!(frame = %text, "unparseable realtime event");
        return Ok(());
    };
    let typ = ev.get("type").and_then(|t| t.as_str()).unwrap_or("");

    let send = |e: ConversationEvent| async move { event_tx.send(e).await.map_err(|_| ()) };

    match typ {
        "session.created" | "session.updated" => {
            if !*initialized {
                *initialized = true;
                send(ConversationEvent::SessionInitialized).await?;
            }
            // session.created may carry the conversation id inline.
            if let Some(cid) = ev
                .pointer("/conversation/id")
                .or_else(|| ev.pointer("/session/conversation/id"))
                .and_then(|v| v.as_str())
            {
                send(ConversationEvent::ConversationId(cid.to_string())).await?;
            }
        }
        "conversation.created" => {
            if let Some(cid) = ev.pointer("/conversation/id").and_then(|v| v.as_str()) {
                send(ConversationEvent::ConversationId(cid.to_string())).await?;
            }
        }
        "input_audio_buffer.speech_started" => {
            send(ConversationEvent::TranscriptionStart).await?;
        }
        // xAI-specific rename of OpenAI's `...transcription.delta` — the
        // transcript is CUMULATIVE (includes corrections). Consumers replace,
        // never append.
        "conversation.item.input_audio_transcription.updated" => {
            if let Some(t) = ev.get("transcript").and_then(|v| v.as_str()) {
                send(ConversationEvent::TranscriptionText(t.to_string())).await?;
            }
        }
        "input_audio_buffer.speech_stopped" => {
            send(ConversationEvent::TranscriptionEnd).await?;
        }
        "response.created" => {
            send(ConversationEvent::PlaybackStart).await?;
        }
        // JSON-transport fallback (binary transport makes these unnecessary,
        // but the server is allowed to ignore the hint).
        "response.output_audio.delta" | "response.audio.delta" => {
            if let Some(b64) = ev.get("delta").and_then(|v| v.as_str())
                && let Ok(pcm) = base64::engine::general_purpose::STANDARD.decode(b64)
            {
                send(ConversationEvent::AudioChunk(Bytes::from(pcm))).await?;
            }
        }
        // Assistant transcript deltas (incremental, unlike input transcription).
        "response.text.delta" | "response.output_audio_transcript.delta" => {
            if let Some(t) = ev.get("delta").and_then(|v| v.as_str()) {
                send(ConversationEvent::ResponseText(t.to_string())).await?;
            }
        }
        "response.function_call_arguments.done" => {
            let call_id = ev.get("call_id").and_then(|v| v.as_str()).unwrap_or("");
            let name = ev.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let arguments = ev.get("arguments").and_then(|v| v.as_str()).unwrap_or("{}");
            if call_id.is_empty() || name.is_empty() {
                warn!(frame = %text, "function call event missing call_id/name");
            } else {
                send(ConversationEvent::ToolCall {
                    call_id: call_id.to_string(),
                    name: name.to_string(),
                    arguments: arguments.to_string(),
                })
                .await?;
            }
        }
        "response.done" => {
            send(ConversationEvent::PlaybackEnd).await?;
        }
        "error" => {
            let msg = ev
                .pointer("/error/message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown realtime error");
            warn!(frame = %text, "realtime upstream error");
            send(ConversationEvent::Error(msg.to_string())).await?;
        }
        // Deliberately ignored: item bookkeeping, argument streaming deltas
        // (we act on .done), buffer commits.
        _ => {
            debug!(event = typ, "unhandled realtime event");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The session.update frame must pin the invariants the relay depends on:
    /// binary transport + 24kHz PCM both directions, server VAD, resumption.
    #[test]
    fn session_update_pins_audio_contract() {
        let mut cfg = RealtimeConfig::default();
        cfg.keyterms = vec!["Nebo".into()];
        cfg.replace
            .insert("NeboAI".into(), Value::String("Neebo A I".into()));
        cfg.tools = vec![json!({"type": "function", "name": "os"})];

        let v = session_update(&cfg);
        assert_eq!(v["type"], "session.update");
        let s = &v["session"];
        assert_eq!(s["turn_detection"]["type"], "server_vad");
        assert_eq!(s["resumption"]["enabled"], true);
        for dir in ["input", "output"] {
            assert_eq!(s["audio"][dir]["format"]["type"], "audio/pcm");
            assert_eq!(s["audio"][dir]["format"]["rate"], 24000);
            assert_eq!(s["audio"][dir]["transport"], "binary");
        }
        assert_eq!(s["audio"]["input"]["transcription"]["keyterms"][0], "Nebo");
        assert_eq!(s["replace"]["NeboAI"], "Neebo A I");
        assert_eq!(s["tools"][0]["name"], "os");
    }

    /// Cumulative transcription events must map to TranscriptionText with the
    /// full transcript (consumers replace), and function calls must surface
    /// call_id + name + raw argument JSON.
    #[tokio::test]
    async fn server_events_translate() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut init = false;

        handle_server_event(
            r#"{"type":"session.created","conversation":{"id":"conv_1"}}"#,
            &tx,
            &mut init,
        )
        .await
        .unwrap();
        assert!(matches!(rx.recv().await, Some(ConversationEvent::SessionInitialized)));
        assert!(
            matches!(rx.recv().await, Some(ConversationEvent::ConversationId(id)) if id == "conv_1")
        );

        handle_server_event(
            r#"{"type":"conversation.item.input_audio_transcription.updated","transcript":"hello world"}"#,
            &tx,
            &mut init,
        )
        .await
        .unwrap();
        assert!(
            matches!(rx.recv().await, Some(ConversationEvent::TranscriptionText(t)) if t == "hello world")
        );

        handle_server_event(
            r#"{"type":"response.function_call_arguments.done","call_id":"c1","name":"os","arguments":"{\"action\":\"read\"}"}"#,
            &tx,
            &mut init,
        )
        .await
        .unwrap();
        match rx.recv().await {
            Some(ConversationEvent::ToolCall { call_id, name, arguments }) => {
                assert_eq!(call_id, "c1");
                assert_eq!(name, "os");
                assert!(arguments.contains("read"));
            }
            other => panic!("expected ToolCall, got {other:?}"),
        }
    }
}
