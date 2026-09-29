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
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::Message;
use tracing::{debug, info, warn};

use crate::VoiceError;
use crate::conversation::ConversationEvent;

/// Wire audio format for both directions of a realtime session.
///
/// The desktop and loop clients capture at 24 kHz PCM; telephony carries
/// G.711 μ-law at 8 kHz. Naming is provider-specific — xAI calls μ-law
/// `audio/pcmu` (OpenAI calls the same codec `g711_ulaw`), verified against
/// `wss://api.x.ai/v1/realtime`, which accepts `audio/pcm`, `audio/pcmu`,
/// `audio/pcma` and `audio/opus`.
///
/// Carrying μ-law end to end means a phone call transcodes NOWHERE: Twilio's
/// 8 kHz μ-law rides untouched all the way to the model and back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AudioFormat {
    /// 24 kHz signed 16-bit PCM — desktop, loop, anything with a real mic.
    #[default]
    Pcm24k,
    /// 8 kHz G.711 μ-law — telephony.
    G711Ulaw,
}

impl AudioFormat {
    /// The `session.audio.{input,output}.format` object for this format.
    fn as_json(self) -> Value {
        match self {
            Self::Pcm24k => json!({ "type": "audio/pcm", "rate": 24000 }),
            Self::G711Ulaw => json!({ "type": "audio/pcmu", "rate": 8000 }),
        }
    }

    /// Bytes of audio per millisecond: 24 kHz PCM16 mono is 48, 8 kHz μ-law
    /// is 8. Turns forwarded audio into played time.
    fn bytes_per_ms(self) -> usize {
        match self {
            Self::Pcm24k => 48,
            Self::G711Ulaw => 8,
        }
    }

    /// The `session.turn_detection` object for this format.
    ///
    /// A device mic (desktop, phone app) waits 1 s of silence before the
    /// turn ends. On the provider's default a 0.9 s thinking pause split one
    /// request into several turns, and the model spoke over the rest of it;
    /// at 1000 ms the same request stayed one turn (probed 2026-09-27). The
    /// VAD threshold stays on the default: 0.5–0.85 made no measurable
    /// difference to barge-in onset. Telephony keeps the provider defaults.
    fn turn_detection(self) -> Value {
        match self {
            Self::Pcm24k => json!({ "type": "server_vad", "silence_duration_ms": 1000 }),
            Self::G711Ulaw => json!({ "type": "server_vad" }),
        }
    }
}

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
    /// Wire audio format. Defaults to 24 kHz PCM, so every existing caller is
    /// byte-identical; telephony opts into μ-law.
    pub audio_format: AudioFormat,
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
            audio_format: AudioFormat::default(),
        }
    }
}

/// Commands into a live realtime session.
#[derive(Debug)]
pub enum RealtimeCommand {
    /// Raw audio bytes in the session's configured `AudioFormat` (PCM16 LE
    /// mono @ 24 kHz by default; μ-law @ 8 kHz for telephony) — forwarded as
    /// a binary WS frame.
    Audio(Bytes),
    /// Typed user input (no audio): creates a message item and requests a
    /// response.
    Text(String),
    /// Barge-in: cancel the in-flight response and cut the model's memory of
    /// its reply back to what the user heard. `played_ms` is how much of the
    /// reply the client played; None estimates it from the wall clock. The
    /// user's speech in progress is never touched: it is what barged in.
    Interrupt { played_ms: Option<u64> },
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
    // Janus groups usage by purpose; a realtime session is always voice.
    request
        .headers_mut()
        .insert("X-Purpose", HeaderValue::from_static("voice"));

    let (ws, _resp) = tls::connect_ws(request)
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
        "turn_detection": cfg.audio_format.turn_detection(),
        // Opt in to resumption so a dropped connection can reconnect with
        // ?conversation_id= and replay history (both sides must opt in).
        "resumption": { "enabled": true },
        "audio": {
            "input": {
                "format": cfg.audio_format.as_json(),
                "transport": "binary",
            },
            "output": {
                "format": cfg.audio_format.as_json(),
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

    let mut state = SessionState::default();

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
                    RealtimeCommand::Interrupt { played_ms } => {
                        let mut sent = Ok(());
                        for frame in interrupt_frames(
                            &mut state,
                            played_ms,
                            std::time::Instant::now(),
                            cfg.audio_format,
                        ) {
                            sent = sink.send(Message::Text(frame.to_string().into())).await;
                            if sent.is_err() {
                                break;
                            }
                        }
                        sent
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
                        // transport: "binary" — model audio in the session's
                        // format, raw.
                        if reply_audio(
                            Bytes::from(data),
                            std::time::Instant::now(),
                            &event_tx,
                            &mut state,
                        )
                        .await
                        .is_err()
                        {
                            break;
                        }
                    }
                    Some(Ok(Message::Text(text))) => {
                        if handle_server_event(&text, &event_tx, &mut state).await.is_err()
                        {
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

    // The session ended mid-utterance: the words so far are all there will be.
    if state.utterance != Utterance::Closed {
        let _ = event_tx.send(ConversationEvent::TranscriptionEnd).await;
    }

    debug!("realtime session task ended");
}

/// Turn-taking state of one realtime session, owned by the session task and
/// updated by `handle_server_event`.
#[derive(Default)]
struct SessionState {
    /// `SessionInitialized` has been sent.
    initialized: bool,
    /// A model response is in flight (response.created seen, no response.done
    /// yet). Barge-in near the end of a response otherwise races the cancel:
    /// the client still hears buffered audio, sends Interrupt, and upstream
    /// rejects the cancel with "no active response found". A response xAI
    /// drops because the user kept talking never gets its `response.done`,
    /// so this can stay set until the next one ends; the cancel it then
    /// allows is the benign race above.
    response_active: bool,
    /// Where the user's current utterance is. Consumers commit a user turn
    /// on every `TranscriptionEnd`, so each utterance gets exactly one.
    utterance: Utterance,
    /// xAI's item id for the open utterance. A pause and more words before
    /// any reply audio reuse the item: its `speech_started` is the same
    /// utterance going on, not a new one.
    utterance_item: Option<String>,
    /// The item of the last utterance that ended; its late transcripts are
    /// dropped (a second end would commit the same speech twice).
    closed_item: Option<String>,
    /// The response created while the open utterance was open. Its
    /// `response.done` ends the utterance even with no audio (a reply that
    /// only calls a tool). xAI creates a response at every pause in the
    /// user's speech and drops it when they go on, so the latest one counts.
    answering: Option<String>,
    /// The current response's audio has started: `PlaybackStart` is sent and
    /// its `PlaybackEnd` is owed at `response.done`.
    playing: bool,
    /// The response's assistant item, announced before its audio.
    pending_item: Option<String>,
    /// The assistant item whose audio is going to the client: what an
    /// interrupt truncates.
    heard_item: Option<HeardItem>,
    /// The client barged in on the response in flight: its remaining audio
    /// is dropped, never played after the user cut it off.
    discard_audio: bool,
}

/// An assistant item's audio as forwarded to the client.
struct HeardItem {
    id: String,
    /// Audio bytes forwarded so far.
    bytes: usize,
    /// When its first audio was forwarded.
    first_at: std::time::Instant,
}

/// The user's utterance, from its first sound to its one `TranscriptionEnd`.
#[derive(Debug, Default, PartialEq)]
enum Utterance {
    /// No utterance in progress; its end has been sent.
    #[default]
    Closed,
    /// Speech started and no reply audio yet. `transcribed`: a finished
    /// transcript has arrived. It is not the end — the user can pause and go
    /// on, and xAI then sends a longer finished transcript for the same item.
    /// The end is the reply's first audio.
    Open { transcribed: bool },
    /// Reply audio started before the finished transcript arrived: the end
    /// is that transcript, or, failing that, the reply's `response.done`.
    Answered,
}

/// The utterance is over: its one `TranscriptionEnd`.
async fn end_utterance(
    event_tx: &mpsc::Sender<ConversationEvent>,
    state: &mut SessionState,
) -> Result<(), ()> {
    state.utterance = Utterance::Closed;
    state.closed_item = state.utterance_item.take();
    state.answering = None;
    event_tx
        .send(ConversationEvent::TranscriptionEnd)
        .await
        .map_err(|_| ())
}

/// One chunk of reply audio. The response's first audio is where the model
/// is heard: an utterance with its finished transcript ends here (before the
/// reply, so rows land in spoken order), and `PlaybackStart` goes out once.
async fn reply_audio(
    audio: Bytes,
    now: std::time::Instant,
    event_tx: &mpsc::Sender<ConversationEvent>,
    state: &mut SessionState,
) -> Result<(), ()> {
    if state.discard_audio {
        return Ok(());
    }
    if !state.playing {
        state.playing = true;
        // Only a reply created while the utterance was open answers it; the
        // tail of an earlier one does not.
        if state.answering.is_some() {
            match state.utterance {
                Utterance::Open { transcribed: true } => end_utterance(event_tx, state).await?,
                Utterance::Open { transcribed: false } => state.utterance = Utterance::Answered,
                _ => {}
            }
        }
        event_tx
            .send(ConversationEvent::PlaybackStart)
            .await
            .map_err(|_| ())?;
    }
    if let Some(id) = state.pending_item.take() {
        state.heard_item = Some(HeardItem {
            id,
            bytes: 0,
            first_at: now,
        });
    }
    if let Some(item) = state.heard_item.as_mut() {
        item.bytes += audio.len();
    }
    event_tx
        .send(ConversationEvent::AudioChunk(audio))
        .await
        .map_err(|_| ())
}

/// The frames a barge-in sends upstream. Never `input_audio_buffer.clear`:
/// that deleted the user's words in progress, the very words barging in.
/// `response.cancel` only while a response is in flight. Then the reply the
/// client was playing is truncated to what was played — `played_ms` from the
/// client, else the wall-clock time since its first audio went out — never
/// past the audio actually forwarded, so the model remembers saying only
/// what the user heard. A reply already played to its end is left alone.
fn interrupt_frames(
    state: &mut SessionState,
    played_ms: Option<u64>,
    now: std::time::Instant,
    format: AudioFormat,
) -> Vec<Value> {
    let mut frames = Vec::new();
    let cancelling = state.response_active;
    if cancelling {
        state.response_active = false;
        state.discard_audio = true;
        frames.push(json!({ "type": "response.cancel" }));
    }
    if let Some(item) = state.heard_item.take() {
        let forwarded_ms = (item.bytes / format.bytes_per_ms()) as u64;
        let played = played_ms
            .unwrap_or_else(|| now.saturating_duration_since(item.first_at).as_millis() as u64)
            .min(forwarded_ms);
        if cancelling || played < forwarded_ms {
            frames.push(json!({
                "type": "conversation.item.truncate",
                "item_id": item.id,
                "content_index": 0,
                "audio_end_ms": played,
            }));
        }
    }
    frames
}

/// Translate one xAI server event into `ConversationEvent`s. Returns Err when
/// the event channel is closed (downstream gone).
async fn handle_server_event(
    text: &str,
    event_tx: &mpsc::Sender<ConversationEvent>,
    state: &mut SessionState,
) -> Result<(), ()> {
    let Ok(ev) = serde_json::from_str::<Value>(text) else {
        warn!(frame = %text, "unparseable realtime event");
        return Ok(());
    };
    let typ = ev.get("type").and_then(|t| t.as_str()).unwrap_or("");

    let send = |e: ConversationEvent| async move { event_tx.send(e).await.map_err(|_| ()) };

    match typ {
        "session.created" | "session.updated" => {
            if !state.initialized {
                state.initialized = true;
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
            let item = ev.get("item_id").and_then(|v| v.as_str());
            // The user paused and went on before any reply audio: xAI keeps
            // the same item, and it is the same utterance — no end, and no
            // start either (clients open a new bubble and barge in on a
            // start), only more cumulative words.
            let continues = matches!(state.utterance, Utterance::Open { .. })
                && item.is_some()
                && item == state.utterance_item.as_deref();
            if !continues {
                // A new utterance: a previous one still open will get no
                // more words, so it ends here, before this one starts.
                if state.utterance != Utterance::Closed {
                    end_utterance(event_tx, state).await?;
                }
                if item.is_some() && item == state.closed_item.as_deref() {
                    // Speech over the reply on the item that just ended:
                    // its transcripts belong to this utterance now.
                    state.closed_item = None;
                }
                state.utterance = Utterance::Open { transcribed: false };
                state.utterance_item = item.map(str::to_string);
                send(ConversationEvent::TranscriptionStart).await?;
            }
        }
        // xAI-specific rename of OpenAI's `...transcription.delta` — the
        // transcript is CUMULATIVE (includes corrections). Consumers replace,
        // never append.
        "conversation.item.input_audio_transcription.updated" => {
            let item = ev.get("item_id").and_then(|v| v.as_str());
            if item.is_some() && item == state.closed_item.as_deref() {
                debug!(frame = %text, "transcript for a closed utterance (dropped)");
            } else if let Some(t) = ev.get("transcript").and_then(|v| v.as_str()) {
                if state.utterance == Utterance::Closed {
                    state.utterance = Utterance::Open { transcribed: false };
                    state.utterance_item = item.map(str::to_string);
                }
                send(ConversationEvent::TranscriptionText(t.to_string())).await?;
            }
        }
        // What xAI sends (probed against grok-voice-latest, 2026-09-17 and
        // 2026-09-27): a finished transcript after `speech_stopped`, and when
        // the user pauses and goes on before any reply audio, ANOTHER one for
        // the same item carrying everything said so far. So it is the
        // utterance's words, not its end: the end is the reply's first audio
        // (or, when this lands after that audio, right here). A transcript
        // for an utterance already closed (by the next `speech_started`, the
        // reply's `response.done`, or session end) is dropped — a second end
        // would commit the same speech as a second user turn.
        "conversation.item.input_audio_transcription.completed" => {
            let item = ev.get("item_id").and_then(|v| v.as_str());
            if state.utterance == Utterance::Closed
                || (item.is_some() && item == state.closed_item.as_deref())
            {
                debug!(frame = %text, "transcript for a closed utterance (dropped)");
            } else if let Some(t) = ev.get("transcript").and_then(|v| v.as_str())
                && !t.is_empty()
            {
                send(ConversationEvent::TranscriptionText(t.to_string())).await?;
                if state.utterance == Utterance::Answered {
                    end_utterance(event_tx, state).await?;
                } else {
                    state.utterance = Utterance::Open { transcribed: true };
                }
            }
        }
        // Not the utterance's end: the finished transcript follows it, and
        // ending here committed the words heard so far as a turn of their own.
        "input_audio_buffer.speech_stopped" => {}
        // Not playback: xAI creates a response at every pause in the user's
        // speech and drops it, silent, when they go on. Playback starts with
        // the response's first audio (`reply_audio`).
        "response.created" => {
            state.response_active = true;
            state.discard_audio = false;
            state.pending_item = None;
            if state.utterance != Utterance::Closed {
                state.answering = Some(
                    ev.pointer("/response/id")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                );
            }
        }
        "response.output_item.added" => {
            if ev.pointer("/item/role").and_then(|v| v.as_str()) == Some("assistant")
                && let Some(id) = ev.pointer("/item/id").and_then(|v| v.as_str())
            {
                state.pending_item = Some(id.to_string());
            }
        }
        // JSON-transport fallback (binary transport makes these unnecessary,
        // but the server is allowed to ignore the hint).
        "response.output_audio.delta" | "response.audio.delta" => {
            if let Some(b64) = ev.get("delta").and_then(|v| v.as_str())
                && let Ok(pcm) = base64::engine::general_purpose::STANDARD.decode(b64)
            {
                reply_audio(Bytes::from(pcm), std::time::Instant::now(), event_tx, state).await?;
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
            state.response_active = false;
            state.discard_audio = false;
            // The reply to the utterance is over — spoken with no finished
            // transcript yet, or silent (a tool call): the words already sent
            // are its final words, and the runs it asked for can start. An
            // utterance the reply did not answer (a barge-in) stays open.
            let id = ev
                .pointer("/response/id")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            if state.utterance != Utterance::Closed && state.answering.as_deref() == Some(id) {
                end_utterance(event_tx, state).await?;
            }
            // Only a response that was heard ends playback: a silent one
            // never started it.
            if state.playing {
                state.playing = false;
                send(ConversationEvent::PlaybackEnd).await?;
            }
        }
        "error" => {
            let msg = ev
                .pointer("/error/message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown realtime error");
            // A cancel that lost the race to response.done is normal duplex
            // turn-taking, not a session failure — surfacing it as Error made
            // the client tear the whole call down on every tail barge-in.
            if msg.contains("Cancellation failed") {
                warn!(frame = %text, "benign realtime cancel race (ignored)");
            } else {
                warn!(frame = %text, "realtime upstream error");
                send(ConversationEvent::Error(msg.to_string())).await?;
            }
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
        // A thinking pause does not end a device mic's turn.
        assert_eq!(s["turn_detection"]["silence_duration_ms"], 1000);
        assert!(s["turn_detection"].get("threshold").is_none());
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

    /// Telephony pins the other half of the contract: μ-law at 8kHz, under
    /// xAI's name for it (`audio/pcmu`, NOT OpenAI's `g711_ulaw` — xAI
    /// rejects that string). Getting this wrong is silent: the session opens
    /// and every frame is noise.
    #[test]
    fn session_update_pins_telephony_audio_contract() {
        let cfg = RealtimeConfig {
            audio_format: AudioFormat::G711Ulaw,
            ..Default::default()
        };

        let v = session_update(&cfg);
        let s = &v["session"];
        assert_eq!(s["turn_detection"], json!({ "type": "server_vad" }));
        assert_eq!(s["resumption"]["enabled"], true);
        for dir in ["input", "output"] {
            assert_eq!(s["audio"][dir]["format"]["type"], "audio/pcmu");
            assert_eq!(s["audio"][dir]["format"]["rate"], 8000);
            assert_eq!(s["audio"][dir]["transport"], "binary");
        }
    }

    /// Cumulative transcription events must map to TranscriptionText with the
    /// full transcript (consumers replace), and function calls must surface
    /// call_id + name + raw argument JSON.
    #[tokio::test]
    async fn server_events_translate() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut state = SessionState::default();

        handle_server_event(
            r#"{"type":"session.created","conversation":{"id":"conv_1"}}"#,
            &tx,
            &mut state,
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
            &mut state,
        )
        .await
        .unwrap();
        assert!(
            matches!(rx.recv().await, Some(ConversationEvent::TranscriptionText(t)) if t == "hello world")
        );

        handle_server_event(
            r#"{"type":"conversation.item.input_audio_transcription.completed","transcript":"hello world","status":"completed"}"#,
            &tx,
            &mut state,
        )
        .await
        .unwrap();
        assert!(
            matches!(rx.recv().await, Some(ConversationEvent::TranscriptionText(t)) if t == "hello world")
        );
        // The finished transcript is not the end (the user may go on): the
        // next event is the tool call, not a TranscriptionEnd.

        handle_server_event(
            r#"{"type":"response.function_call_arguments.done","call_id":"c1","name":"os","arguments":"{\"action\":\"read\"}"}"#,
            &tx,
            &mut state,
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


    /// Stands for one binary frame of reply audio in a replay: 480 bytes of
    /// 24 kHz PCM16, 10 ms.
    const AUDIO: &str = "AUDIO";

    /// Drives `frames` through one session state (`AUDIO` = a reply audio
    /// frame, forwarded at `now`) and returns every event they produced, in
    /// order, with the state they left.
    async fn drive(
        frames: &[&str],
        now: std::time::Instant,
    ) -> (Vec<ConversationEvent>, SessionState) {
        let (tx, mut rx) = mpsc::channel(1024);
        let mut state = SessionState {
            initialized: true,
            ..Default::default()
        };
        for frame in frames {
            if *frame == AUDIO {
                reply_audio(Bytes::from_static(&[0u8; 480]), now, &tx, &mut state)
                    .await
                    .unwrap();
            } else {
                handle_server_event(frame, &tx, &mut state).await.unwrap();
            }
        }
        drop(tx);
        let mut events = Vec::new();
        while let Some(e) = rx.recv().await {
            events.push(e);
        }
        (events, state)
    }

    async fn replay(frames: &[&str]) -> Vec<ConversationEvent> {
        drive(frames, std::time::Instant::now()).await.0
    }

    fn count(events: &[ConversationEvent], want: fn(&ConversationEvent) -> bool) -> usize {
        events.iter().filter(|e| want(e)).count()
    }

    /// The order xAI sends one utterance in: the finished transcript lands
    /// after `speech_stopped`, before the reply's audio. Exactly one end,
    /// after the final words and before the reply is heard.
    #[tokio::test]
    async fn one_end_per_utterance_carrying_final_transcript() {
        let events = replay(&[
            r#"{"type":"input_audio_buffer.speech_started","item_id":"u1"}"#,
            r#"{"type":"conversation.item.input_audio_transcription.updated","item_id":"u1","transcript":"hello"}"#,
            r#"{"type":"input_audio_buffer.speech_stopped"}"#,
            r#"{"type":"response.created","response":{"id":"r1"}}"#,
            r#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"u1","transcript":"hello world","status":"completed"}"#,
            r#"{"type":"response.output_item.added","item":{"id":"a1","type":"message","role":"assistant"}}"#,
            AUDIO,
        ])
        .await;
        assert!(
            matches!(
                events.as_slice(),
                [
                    ConversationEvent::TranscriptionStart,
                    ConversationEvent::TranscriptionText(partial),
                    ConversationEvent::TranscriptionText(full),
                    ConversationEvent::TranscriptionEnd,
                    ConversationEvent::PlaybackStart,
                    ConversationEvent::AudioChunk(_),
                ] if partial == "hello" && full == "hello world"
            ),
            "{events:?}"
        );
    }

    /// xAI creates a response at every pause in the user's speech and drops
    /// it, silent and with no `response.done`, when they go on. That is not
    /// the model speaking: no PlaybackStart, and the utterance stays open.
    #[tokio::test]
    async fn phantom_response_is_not_playback() {
        let events = replay(&[
            r#"{"type":"input_audio_buffer.speech_started","item_id":"u1"}"#,
            r#"{"type":"input_audio_buffer.speech_stopped"}"#,
            r#"{"type":"response.created","response":{"id":"p1"}}"#,
            r#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"u1","transcript":"Okay, so what I would like you to do.","status":"completed"}"#,
            r#"{"type":"response.output_item.added","item":{"id":"a1","type":"message","role":"assistant"}}"#,
        ])
        .await;
        assert!(
            matches!(
                events.as_slice(),
                [ConversationEvent::TranscriptionStart, ConversationEvent::TranscriptionText(_)]
            ),
            "{events:?}"
        );
    }

    /// Playback starts with the response's first audio, once, and ends at
    /// its `response.done`.
    #[tokio::test]
    async fn first_audio_starts_playback_once() {
        let events = replay(&[
            r#"{"type":"response.created","response":{"id":"r1"}}"#,
            AUDIO,
            AUDIO,
            AUDIO,
            r#"{"type":"response.done","response":{"id":"r1","status":"completed"}}"#,
        ])
        .await;
        assert!(
            matches!(
                events.as_slice(),
                [
                    ConversationEvent::PlaybackStart,
                    ConversationEvent::AudioChunk(_),
                    ConversationEvent::AudioChunk(_),
                    ConversationEvent::AudioChunk(_),
                    ConversationEvent::PlaybackEnd,
                ]
            ),
            "{events:?}"
        );
    }

    /// The owner's duplicate rows ("I'm listening." then "I'm listening.
    /// Okay, uh, what I'd like…"): the user pauses and goes on before any
    /// reply audio, xAI reuses the SAME item and sends a second, cumulative
    /// finished transcript. One utterance: one start, the final cumulative
    /// words, one end — at the reply's first audio. Frames as probed.
    #[tokio::test]
    async fn same_item_continuation_is_one_utterance() {
        let events = replay(&[
            r#"{"type":"input_audio_buffer.speech_started","item_id":"c405"}"#,
            r#"{"type":"input_audio_buffer.speech_stopped"}"#,
            r#"{"type":"response.created","response":{"id":"p1"}}"#,
            r#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"c405","transcript":"Okay, so what I would like you to do.","status":"completed"}"#,
            r#"{"type":"response.output_item.added","item":{"id":"a1","type":"message","role":"assistant"}}"#,
            r#"{"type":"input_audio_buffer.speech_started","item_id":"c405"}"#,
            r#"{"type":"input_audio_buffer.speech_stopped"}"#,
            r#"{"type":"response.created","response":{"id":"r2"}}"#,
            r#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"c405","transcript":"Okay, so what I would like you to do is figure out a couple of things for me.","status":"completed"}"#,
            r#"{"type":"response.output_item.added","item":{"id":"a2","type":"message","role":"assistant"}}"#,
            AUDIO,
            AUDIO,
            r#"{"type":"response.done","response":{"id":"r2","status":"completed"}}"#,
        ])
        .await;
        assert_eq!(count(&events, |e| matches!(e, ConversationEvent::TranscriptionStart)), 1, "{events:?}");
        assert_eq!(count(&events, |e| matches!(e, ConversationEvent::TranscriptionEnd)), 1, "{events:?}");
        assert_eq!(count(&events, |e| matches!(e, ConversationEvent::PlaybackStart)), 1, "{events:?}");
        let end = events
            .iter()
            .position(|e| matches!(e, ConversationEvent::TranscriptionEnd))
            .unwrap();
        assert!(
            matches!(&events[end - 1], ConversationEvent::TranscriptionText(t)
                if t == "Okay, so what I would like you to do is figure out a couple of things for me."),
            "the end carries the cumulative words: {events:?}"
        );
        assert!(
            matches!(events[end + 1], ConversationEvent::PlaybackStart),
            "the end comes before the reply is heard: {events:?}"
        );
    }

    /// Once the reply is heard, speech is a new utterance even on the same
    /// item: the open one ends, and a start goes out (the clients' barge-in
    /// signal and new bubble). Only speech before any reply audio continues.
    #[tokio::test]
    async fn same_item_speech_over_the_reply_is_a_new_utterance() {
        let events = replay(&[
            r#"{"type":"input_audio_buffer.speech_started","item_id":"u1"}"#,
            r#"{"type":"conversation.item.input_audio_transcription.updated","item_id":"u1","transcript":"tell me"}"#,
            r#"{"type":"response.created","response":{"id":"r1"}}"#,
            AUDIO,
            r#"{"type":"input_audio_buffer.speech_started","item_id":"u1"}"#,
            r#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"u1","transcript":"tell me a story","status":"completed"}"#,
        ])
        .await;
        assert!(
            matches!(
                events.as_slice(),
                [
                    ConversationEvent::TranscriptionStart,
                    ConversationEvent::TranscriptionText(_),
                    ConversationEvent::PlaybackStart,
                    ConversationEvent::AudioChunk(_),
                    ConversationEvent::TranscriptionEnd,
                    ConversationEvent::TranscriptionStart,
                    ConversationEvent::TranscriptionText(t),
                ] if t == "tell me a story"
            ),
            "{events:?}"
        );
    }

    /// A new item is a new utterance: the open one ends before it starts.
    #[tokio::test]
    async fn different_item_ends_the_previous_utterance() {
        let events = replay(&[
            r#"{"type":"input_audio_buffer.speech_started","item_id":"u1"}"#,
            r#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"u1","transcript":"one","status":"completed"}"#,
            r#"{"type":"input_audio_buffer.speech_started","item_id":"u2"}"#,
        ])
        .await;
        assert!(
            matches!(
                events.as_slice(),
                [
                    ConversationEvent::TranscriptionStart,
                    ConversationEvent::TranscriptionText(t),
                    ConversationEvent::TranscriptionEnd,
                    ConversationEvent::TranscriptionStart,
                ] if t == "one"
            ),
            "{events:?}"
        );
    }

    /// A reply that only calls a tool is never heard: no playback events,
    /// and its `response.done` ends the utterance, so the runs waiting for
    /// the user's row start.
    #[tokio::test]
    async fn silent_tool_reply_ends_the_utterance_at_response_done() {
        let events = replay(&[
            r#"{"type":"input_audio_buffer.speech_started","item_id":"u1"}"#,
            r#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"u1","transcript":"make the repo","status":"completed"}"#,
            r#"{"type":"response.created","response":{"id":"r1"}}"#,
            r#"{"type":"response.function_call_arguments.done","call_id":"c1","name":"nebo","arguments":"{}"}"#,
            r#"{"type":"response.done","response":{"id":"r1","status":"completed"}}"#,
        ])
        .await;
        assert!(
            matches!(
                events.as_slice(),
                [
                    ConversationEvent::TranscriptionStart,
                    ConversationEvent::TranscriptionText(_),
                    ConversationEvent::ToolCall { .. },
                    ConversationEvent::TranscriptionEnd,
                ]
            ),
            "{events:?}"
        );
    }

    /// Reply audio before the finished transcript: the utterance stays open
    /// through the first audio; the late transcript is its final words and
    /// its one end.
    #[tokio::test]
    async fn late_transcript_after_reply_starts_is_the_end() {
        let events = replay(&[
            r#"{"type":"input_audio_buffer.speech_started","item_id":"u1"}"#,
            r#"{"type":"conversation.item.input_audio_transcription.updated","item_id":"u1","transcript":"what I want is just"}"#,
            r#"{"type":"input_audio_buffer.speech_stopped"}"#,
            r#"{"type":"response.created","response":{"id":"r1"}}"#,
            AUDIO,
            r#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"u1","transcript":"what I want is just a list","status":"completed"}"#,
            r#"{"type":"response.done","response":{"id":"r1","status":"completed"}}"#,
        ])
        .await;
        assert!(
            matches!(
                events.as_slice(),
                [
                    ConversationEvent::TranscriptionStart,
                    ConversationEvent::TranscriptionText(partial),
                    ConversationEvent::PlaybackStart,
                    ConversationEvent::AudioChunk(_),
                    ConversationEvent::TranscriptionText(full),
                    ConversationEvent::TranscriptionEnd,
                    ConversationEvent::PlaybackEnd,
                ] if partial == "what I want is just" && full == "what I want is just a list"
            ),
            "{events:?}"
        );
    }

    /// A provider that never sends the finished transcript: the utterance
    /// ends when the reply to it is done, with the words already sent, and a
    /// transcript arriving after that is dropped rather than ending it twice.
    #[tokio::test]
    async fn utterance_ends_on_response_done_without_completed() {
        let events = replay(&[
            r#"{"type":"input_audio_buffer.speech_started","item_id":"u1"}"#,
            r#"{"type":"conversation.item.input_audio_transcription.updated","item_id":"u1","transcript":"hello"}"#,
            r#"{"type":"input_audio_buffer.speech_stopped"}"#,
            r#"{"type":"response.created","response":{"id":"r1"}}"#,
            AUDIO,
            r#"{"type":"response.done","response":{"id":"r1","status":"completed"}}"#,
            r#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"u1","transcript":"hello world","status":"completed"}"#,
            r#"{"type":"conversation.item.input_audio_transcription.updated","item_id":"u1","transcript":"hello world"}"#,
        ])
        .await;
        assert!(
            matches!(
                events.as_slice(),
                [
                    ConversationEvent::TranscriptionStart,
                    ConversationEvent::TranscriptionText(t),
                    ConversationEvent::PlaybackStart,
                    ConversationEvent::AudioChunk(_),
                    ConversationEvent::TranscriptionEnd,
                    ConversationEvent::PlaybackEnd,
                ] if t == "hello"
            ),
            "{events:?}"
        );
    }

    /// Barge-in: the user speaks over a reply. The earlier utterance ended
    /// at the reply's first audio; the interrupted reply's `response.done`
    /// does not end the new one — its own reply's first audio does.
    #[tokio::test]
    async fn barge_in_ends_the_previous_utterance_not_the_new_one() {
        let events = replay(&[
            r#"{"type":"input_audio_buffer.speech_started","item_id":"u1"}"#,
            r#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"u1","transcript":"first","status":"completed"}"#,
            r#"{"type":"response.created","response":{"id":"r1"}}"#,
            AUDIO,
            r#"{"type":"input_audio_buffer.speech_started","item_id":"u2"}"#,
            r#"{"type":"response.done","response":{"id":"r1","status":"completed"}}"#,
            r#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"u2","transcript":"second","status":"completed"}"#,
            r#"{"type":"response.created","response":{"id":"r2"}}"#,
            AUDIO,
        ])
        .await;
        assert!(
            matches!(
                events.as_slice(),
                [
                    ConversationEvent::TranscriptionStart,
                    ConversationEvent::TranscriptionText(first),
                    ConversationEvent::TranscriptionEnd,
                    ConversationEvent::PlaybackStart,
                    ConversationEvent::AudioChunk(_),
                    ConversationEvent::TranscriptionStart,
                    ConversationEvent::PlaybackEnd,
                    ConversationEvent::TranscriptionText(second),
                    ConversationEvent::TranscriptionEnd,
                    ConversationEvent::PlaybackStart,
                    ConversationEvent::AudioChunk(_),
                ] if first == "first" && second == "second"
            ),
            "{events:?}"
        );
    }

    fn types(frames: &[Value]) -> Vec<&str> {
        frames.iter().map(|f| f["type"].as_str().unwrap_or_default()).collect()
    }

    /// The reply the user heard: 300 frames of 10 ms = 3000 ms forwarded,
    /// first sent at `first_at`, response still in flight.
    async fn hearing_a_reply(first_at: std::time::Instant) -> SessionState {
        let mut frames = vec![
            r#"{"type":"response.created","response":{"id":"r1"}}"#,
            r#"{"type":"response.output_item.added","item":{"id":"a1","type":"message","role":"assistant"}}"#,
        ];
        frames.extend(std::iter::repeat_n(AUDIO, 300));
        drive(&frames, first_at).await.1
    }

    /// Barge-in mid-response: cancel, then truncate the heard item at the
    /// client's played position. Never a buffer clear — that deleted the
    /// user's words in progress. The cancelled reply's late audio is dropped.
    #[tokio::test]
    async fn interrupt_cancels_and_truncates_at_the_played_position() {
        let now = std::time::Instant::now();
        let mut state = hearing_a_reply(now).await;
        let frames = interrupt_frames(&mut state, Some(1200), now, AudioFormat::Pcm24k);
        assert_eq!(types(&frames), ["response.cancel", "conversation.item.truncate"]);
        assert_eq!(frames[1]["item_id"], "a1");
        assert_eq!(frames[1]["content_index"], 0);
        assert_eq!(frames[1]["audio_end_ms"], 1200);

        let (tx, mut rx) = mpsc::channel(4);
        reply_audio(Bytes::from_static(&[0u8; 480]), now, &tx, &mut state)
            .await
            .unwrap();
        drop(tx);
        assert!(rx.recv().await.is_none(), "audio after the barge-in is not played");

        // Nothing left to cut: a second barge-in sends nothing.
        assert!(interrupt_frames(&mut state, Some(1500), now, AudioFormat::Pcm24k).is_empty());
    }

    /// The common case: generation outran playback, the response is done,
    /// the client is still playing it. No cancel (nothing in flight), but
    /// the truncate still cuts the model's memory to what was heard; with no
    /// played position from the client, the wall clock since the first
    /// audio estimates it.
    #[tokio::test]
    async fn interrupt_after_generation_truncates_by_the_wall_clock() {
        let now = std::time::Instant::now();
        let first_at = now - std::time::Duration::from_millis(800);
        let mut state = hearing_a_reply(first_at).await;
        let (tx, _rx) = mpsc::channel(4);
        handle_server_event(r#"{"type":"response.done","response":{"id":"r1"}}"#, &tx, &mut state)
            .await
            .unwrap();
        let frames = interrupt_frames(&mut state, None, now, AudioFormat::Pcm24k);
        assert_eq!(types(&frames), ["conversation.item.truncate"]);
        assert_eq!(frames[0]["audio_end_ms"], 800);
    }

    /// The played position never passes the audio actually forwarded, and a
    /// reply the user heard to its end is left alone.
    #[tokio::test]
    async fn interrupt_clamps_to_the_audio_forwarded() {
        let now = std::time::Instant::now();
        let long_ago = now - std::time::Duration::from_secs(10);

        let mut state = hearing_a_reply(long_ago).await;
        let frames = interrupt_frames(&mut state, None, now, AudioFormat::Pcm24k);
        assert_eq!(types(&frames), ["response.cancel", "conversation.item.truncate"]);
        assert_eq!(frames[1]["audio_end_ms"], 3000);

        let mut state = hearing_a_reply(now).await;
        let frames = interrupt_frames(&mut state, Some(9000), now, AudioFormat::Pcm24k);
        assert_eq!(frames[1]["audio_end_ms"], 3000);

        let mut state = hearing_a_reply(long_ago).await;
        state.response_active = false;
        assert!(
            interrupt_frames(&mut state, None, now, AudioFormat::Pcm24k).is_empty(),
            "fully heard, nothing in flight: nothing to send"
        );

        // μ-law is 8 bytes a millisecond: the same 144000 bytes are 18 s.
        let mut state = hearing_a_reply(long_ago).await;
        let frames = interrupt_frames(&mut state, None, now, AudioFormat::G711Ulaw);
        assert_eq!(frames[1]["audio_end_ms"], 10_000);
    }

    /// With nothing in flight and nothing heard, a barge-in sends nothing at
    /// all — least of all a buffer clear.
    #[tokio::test]
    async fn interrupt_with_nothing_playing_sends_nothing() {
        let mut state = SessionState::default();
        assert!(
            interrupt_frames(&mut state, Some(100), std::time::Instant::now(), AudioFormat::Pcm24k)
                .is_empty()
        );
    }

    /// Barge-in duplex contract: response lifecycle events track the in-flight
    /// flag, and the benign cancel-race error is swallowed while real errors
    /// still surface as fatal.
    #[tokio::test]
    async fn barge_in_cancel_race_is_benign() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut state = SessionState {
            initialized: true,
            ..Default::default()
        };

        handle_server_event(r#"{"type":"response.created"}"#, &tx, &mut state)
            .await
            .unwrap();
        assert!(state.response_active);
        reply_audio(Bytes::from_static(&[0u8; 480]), std::time::Instant::now(), &tx, &mut state)
            .await
            .unwrap();
        assert!(matches!(rx.recv().await, Some(ConversationEvent::PlaybackStart)));
        assert!(matches!(rx.recv().await, Some(ConversationEvent::AudioChunk(_))));

        handle_server_event(r#"{"type":"response.done"}"#, &tx, &mut state)
            .await
            .unwrap();
        assert!(!state.response_active);
        assert!(matches!(rx.recv().await, Some(ConversationEvent::PlaybackEnd)));

        // The cancel race must NOT surface as a client-facing Error.
        handle_server_event(
            r#"{"type":"error","error":{"message":"Cancellation failed: no active response found","type":"invalid_request_error"}}"#,
            &tx,
            &mut state,
        )
        .await
        .unwrap();
        // A real error still must.
        handle_server_event(
            r#"{"type":"error","error":{"message":"insufficient balance","type":"payment_error"}}"#,
            &tx,
            &mut state,
        )
        .await
        .unwrap();
        match rx.recv().await {
            Some(ConversationEvent::Error(m)) => assert!(m.contains("insufficient balance")),
            other => panic!("expected only the real error, got {other:?}"),
        }
    }
}
