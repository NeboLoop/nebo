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
//!
//! The line is kept for the whole call. It is pinged while the call is quiet
//! (a muted owner listening to his employee work sends no audio for minutes),
//! and a line that drops mid-call anyway is dialled again at once: a new
//! session, the same configuration, and the call so far in its instructions.
//! The client hears the next reply, not the drop; only when every redial fails
//! does it get [`ConversationEvent::Error`], in plain words.

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
use crate::conversation::{CALL_ENDED, ConversationEvent};

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
    /// Speak these words as they are, outside the model: the provider's
    /// `force_message` (text to speech, its own response lifecycle, no
    /// `response.create`). What a long hand-off says while the owner waits
    /// ("Now editing."). The owner can talk over it. A provider that refuses
    /// it is not a failed call: the refusal is logged, and the session says
    /// nothing more this way.
    Say(String),
    /// All tool outputs submitted — request the model's continuation.
    /// Callers must wait for current audio playback to finish first, or the
    /// next response overlaps the tail of the current one.
    ToolOutputsDone,
    /// Close the session.
    Close,
}

/// Start a realtime session. Returns the command sender and the event
/// receiver; the socket task runs until `Close`, the client going away, or a
/// dropped line that could not be brought back.
pub async fn connect(
    cfg: RealtimeConfig,
) -> Result<(mpsc::Sender<RealtimeCommand>, mpsc::Receiver<ConversationEvent>), VoiceError> {
    let ws = dial(&cfg).await?;
    info!(endpoint = %cfg.endpoint, model = %cfg.model, "realtime session connected");

    let (cmd_tx, cmd_rx) = mpsc::channel::<RealtimeCommand>(64);
    let (event_tx, event_rx) = mpsc::channel::<ConversationEvent>(64);

    tokio::spawn(run_session(ws, cfg, cmd_rx, event_tx));

    Ok((cmd_tx, event_rx))
}

type Socket = tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
>;

/// Open the socket to the realtime endpoint.
async fn dial(cfg: &RealtimeConfig) -> Result<Socket, VoiceError> {
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
    Ok(ws)
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

/// A ping goes to the realtime endpoint this often. A quiet line is not an
/// idle one: the owner can sit muted for minutes while his employee works,
/// and a socket that carries nothing for that long is closed by whatever
/// counts silence on the way (on 2026-09-30 the relay's socket closed a live
/// call after two minutes with no audio from the phone). Every ping proves
/// the call is still there, and its pong proves the far end is.
#[cfg(not(test))]
const PING_EVERY: std::time::Duration = std::time::Duration::from_secs(15);
#[cfg(test)]
const PING_EVERY: std::time::Duration = std::time::Duration::from_millis(100);

/// Nothing at all heard from the far end for this long, pongs included: the
/// line is dead even though no error said so, and it is dialled again.
#[cfg(not(test))]
const DEAD_AFTER: std::time::Duration = std::time::Duration::from_secs(40);
#[cfg(test)]
const DEAD_AFTER: std::time::Duration = std::time::Duration::from_millis(350);

/// A line that drops mid-call is dialled again after each of these in turn,
/// all within a couple of seconds; after the last one fails the call ends.
#[cfg(not(test))]
const REDIAL_DELAYS: [std::time::Duration; 3] = [
    std::time::Duration::ZERO,
    std::time::Duration::from_millis(500),
    std::time::Duration::from_millis(1500),
];
#[cfg(test)]
const REDIAL_DELAYS: [std::time::Duration; 3] = [
    std::time::Duration::ZERO,
    std::time::Duration::from_millis(10),
    std::time::Duration::from_millis(20),
];

/// How long one redial may take before it counts as failed.
const DIAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(4);

/// A line that stayed up this long was a good one: the next drop gets the
/// full set of redials again. One that drops sooner shares the count, so a
/// far end that takes the call and drops it at once cannot loop forever.
const STABLE_AFTER: std::time::Duration = std::time::Duration::from_secs(10);

/// How much of the call so far a redialled session is given, at most.
const RECAP_TURNS: usize = 16;
const RECAP_CHARS: usize = 4_000;

/// How one socket's part of the call ended.
enum Ended {
    /// The call is over: `Close`, the command channel gone, or the client
    /// gone (the event channel closed).
    Done,
    /// The line dropped under a live call. The reason is for the log.
    Lost(String),
}

/// The session task: one call, over as many sockets as it takes. A line that
/// drops mid-call is dialled again (a new session, the same configuration,
/// and the call so far) without the client seeing anything but the next
/// reply; only when the redials run out does the client hear that the call
/// ended, in plain words, with the reason in the log.
async fn run_session(
    ws: Socket,
    cfg: RealtimeConfig,
    mut cmd_rx: mpsc::Receiver<RealtimeCommand>,
    event_tx: mpsc::Sender<ConversationEvent>,
) {
    let mut state = SessionState::default();
    let mut memory = CallMemory::default();
    let mut deferred: Vec<RealtimeCommand> = Vec::new();
    let mut ws = Some(ws);
    let mut failures = 0usize;

    while let Some(socket) = ws.take() {
        let up_since = std::time::Instant::now();
        let ended = drive_socket(
            socket,
            &cfg,
            &mut cmd_rx,
            &event_tx,
            &mut state,
            &mut memory,
            &mut deferred,
        )
        .await;
        let Ended::Lost(reason) = ended else { break };
        if up_since.elapsed() >= STABLE_AFTER {
            failures = 0;
        }
        warn!(reason = %reason, "realtime line dropped mid-call; dialling again");

        // What the dropped socket owed the client is settled now: the
        // utterance in progress ends with the words heard so far, and a
        // reply that was playing stops. The client stays on the call.
        if settle_lost_socket(&event_tx, &mut state, &mut memory).await.is_err() {
            break;
        }

        match redial(&cfg, &mut cmd_rx, &mut deferred, &mut failures).await {
            Redial::Connected(socket) => {
                info!(endpoint = %cfg.endpoint, "realtime line re-established; the call carries on");
                memory.resumed = true;
                ws = Some(*socket);
            }
            Redial::Done => break,
            Redial::Failed => {
                warn!("realtime line could not be re-established; ending the call");
                let _ = event_tx
                    .send(ConversationEvent::Error(CALL_ENDED.to_string()))
                    .await;
                break;
            }
        }
    }

    // The session ended mid-utterance: the words so far are all there will be.
    if state.utterance != Utterance::Closed {
        let _ = event_tx.send(ConversationEvent::TranscriptionEnd).await;
    }

    debug!("realtime session task ended");
}

/// One socket's part of the call: configure it (on a redialled socket, with
/// the call so far), then relay until the call ends or the line drops.
async fn drive_socket(
    socket: Socket,
    cfg: &RealtimeConfig,
    cmd_rx: &mut mpsc::Receiver<RealtimeCommand>,
    event_tx: &mpsc::Sender<ConversationEvent>,
    state: &mut SessionState,
    memory: &mut CallMemory,
    deferred: &mut Vec<RealtimeCommand>,
) -> Ended {
    let (mut sink, mut stream) = socket.split();

    // Configure the session before any audio flows.
    let update = if memory.resumed {
        let mut resumed = cfg.clone();
        resumed.instructions.push_str(&memory.recap());
        session_update(&resumed)
    } else {
        session_update(cfg)
    };
    if let Err(e) = sink.send(Message::Text(update.to_string().into())).await {
        return Ended::Lost(format!("session.update failed: {e}"));
    }
    // A reply that was owed when the line dropped is asked for again: the
    // new session has the question in the call so far.
    if std::mem::take(&mut memory.reanswer)
        && let Err(e) = sink
            .send(Message::Text(json!({ "type": "response.create" }).to_string().into()))
            .await
    {
        return Ended::Lost(format!("realtime send failed: {e}"));
    }
    // What the handler sent while the line was being dialled goes out now.
    for cmd in std::mem::take(deferred) {
        match send_command(&mut sink, cmd, cfg, state, memory).await {
            Ok(true) => {}
            Ok(false) => return Ended::Done,
            Err(e) => return Ended::Lost(format!("realtime send failed: {e}")),
        }
    }

    let mut ping = tokio::time::interval_at(tokio::time::Instant::now() + PING_EVERY, PING_EVERY);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_heard = std::time::Instant::now();

    loop {
        tokio::select! {
            cmd = cmd_rx.recv() => {
                let Some(cmd) = cmd else { return Ended::Done };
                match send_command(&mut sink, cmd, cfg, state, memory).await {
                    Ok(true) => {}
                    Ok(false) => return Ended::Done,
                    Err(e) => return Ended::Lost(format!("realtime send failed: {e}")),
                }
            }

            msg = stream.next() => {
                last_heard = std::time::Instant::now();
                match msg {
                    Some(Ok(Message::Binary(data))) => {
                        // transport: "binary" — model audio in the session's
                        // format, raw.
                        if reply_audio(
                            Bytes::from(data),
                            std::time::Instant::now(),
                            event_tx,
                            state,
                        )
                        .await
                        .is_err()
                        {
                            return Ended::Done;
                        }
                    }
                    Some(Ok(Message::Text(text))) => {
                        memory.observe(&text);
                        if handle_server_event(&text, event_tx, state).await.is_err() {
                            return Ended::Done;
                        }
                    }
                    Some(Ok(Message::Close(frame))) => {
                        return Ended::Lost(format!("closed by the far end: {frame:?}"));
                    }
                    Some(Ok(_)) => {} // ping/pong handled by tungstenite
                    Some(Err(e)) => return Ended::Lost(format!("realtime stream error: {e}")),
                    None => return Ended::Lost("realtime stream ended".into()),
                }
            }

            _ = ping.tick() => {
                if last_heard.elapsed() > DEAD_AFTER {
                    return Ended::Lost(format!(
                        "nothing heard for {} ms, pongs included",
                        last_heard.elapsed().as_millis()
                    ));
                }
                if let Err(e) = sink.send(Message::Ping(Vec::new().into())).await {
                    return Ended::Lost(format!("realtime ping failed: {e}"));
                }
            }
        }
    }
}

/// Send one command upstream. `Ok(false)`: the call is closed.
async fn send_command<S>(
    sink: &mut S,
    cmd: RealtimeCommand,
    cfg: &RealtimeConfig,
    state: &mut SessionState,
    memory: &mut CallMemory,
) -> Result<bool, tokio_tungstenite::tungstenite::Error>
where
    S: futures::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    match cmd {
        RealtimeCommand::Audio(pcm) => {
            // transport: "binary" — raw codec bytes, no base64.
            sink.send(Message::Binary(pcm.to_vec().into())).await?;
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
            sink.send(Message::Text(item.to_string().into())).await?;
            sink.send(Message::Text(
                json!({ "type": "response.create" }).to_string().into(),
            ))
            .await?;
        }
        RealtimeCommand::Interrupt { played_ms } => {
            for frame in interrupt_frames(
                state,
                played_ms,
                std::time::Instant::now(),
                cfg.audio_format,
            ) {
                sink.send(Message::Text(frame.to_string().into())).await?;
            }
        }
        RealtimeCommand::ToolOutput { call_id, output } => {
            let item = if memory.orphans.remove(&call_id) {
                // The call it answers was made on a line that dropped: the
                // session on this one never saw it, so the result arrives
                // as words, not as that call's output.
                json!({
                    "type": "conversation.item.create",
                    "item": {
                        "type": "message",
                        "role": "user",
                        "content": [{
                            "type": "input_text",
                            "text": format!("(The result of the task you started earlier: {output})"),
                        }],
                    },
                })
            } else {
                memory.open_calls.remove(&call_id);
                json!({
                    "type": "conversation.item.create",
                    "item": {
                        "type": "function_call_output",
                        "call_id": call_id,
                        "output": output,
                    },
                })
            };
            sink.send(Message::Text(item.to_string().into())).await?;
        }
        RealtimeCommand::Say(text) => {
            if state.say_refused {
                return Ok(true);
            }
            state.say_pending = true;
            sink.send(Message::Text(say_frame(&text).to_string().into())).await?;
        }
        RealtimeCommand::ToolOutputsDone => {
            sink.send(Message::Text(
                json!({ "type": "response.create" }).to_string().into(),
            ))
            .await?;
        }
        RealtimeCommand::Close => {
            let _ = sink.send(Message::Close(None)).await;
            return Ok(false);
        }
    }
    Ok(true)
}

/// The frame that speaks `text` verbatim (see [`RealtimeCommand::Say`]).
fn say_frame(text: &str) -> Value {
    json!({
        "type": "conversation.item.create",
        "item": {
            "type": "force_message",
            "role": "assistant",
            "interruptible": true,
            "content": [{ "type": "output_text", "text": text }],
        },
    })
}

/// Close out what a dropped socket owed the client, and start the next
/// socket's turn-taking fresh (the session is already initialized as far as
/// the client knows).
async fn settle_lost_socket(
    event_tx: &mpsc::Sender<ConversationEvent>,
    state: &mut SessionState,
    memory: &mut CallMemory,
) -> Result<(), ()> {
    if state.utterance != Utterance::Closed {
        event_tx
            .send(ConversationEvent::TranscriptionEnd)
            .await
            .map_err(|_| ())?;
    }
    if state.playing {
        event_tx
            .send(ConversationEvent::PlaybackEnd)
            .await
            .map_err(|_| ())?;
    }
    memory.lost(state.response_active);
    *state = SessionState {
        initialized: state.initialized,
        ..Default::default()
    };
    Ok(())
}

enum Redial {
    Connected(Box<Socket>),
    /// The call was closed while the line was being dialled.
    Done,
    /// Every redial failed.
    Failed,
}

/// Dial the line again, at most `REDIAL_DELAYS.len()` times counting the
/// failures already spent on this stretch of the call. Commands keep coming
/// while it dials: audio and barge-ins belong to the dead line and are
/// dropped, `Close` ends the call, everything else waits for the new line.
async fn redial(
    cfg: &RealtimeConfig,
    cmd_rx: &mut mpsc::Receiver<RealtimeCommand>,
    deferred: &mut Vec<RealtimeCommand>,
    failures: &mut usize,
) -> Redial {
    while *failures < REDIAL_DELAYS.len() {
        let delay = REDIAL_DELAYS[*failures];
        *failures += 1;
        let attempt = *failures;
        let dialling = async {
            tokio::time::sleep(delay).await;
            tokio::time::timeout(DIAL_TIMEOUT, dial(cfg)).await
        };
        tokio::pin!(dialling);
        loop {
            tokio::select! {
                dialled = &mut dialling => {
                    match dialled {
                        Ok(Ok(socket)) => return Redial::Connected(Box::new(socket)),
                        Ok(Err(e)) => warn!(attempt, error = %e, "realtime redial failed"),
                        Err(_) => warn!(attempt, "realtime redial timed out"),
                    }
                    break;
                }
                cmd = cmd_rx.recv() => match cmd {
                    None | Some(RealtimeCommand::Close) => return Redial::Done,
                    Some(RealtimeCommand::Audio(_)) | Some(RealtimeCommand::Interrupt { .. }) => {}
                    Some(other) => deferred.push(other),
                },
            }
        }
    }
    Redial::Failed
}

/// What a redialled session needs to carry the call on: the call so far, the
/// tool calls still out, and whether a reply was owed.
#[derive(Default)]
struct CallMemory {
    /// Finished turns, oldest first: (is_user, text, the user item's id).
    turns: std::collections::VecDeque<(bool, String, Option<String>)>,
    /// The assistant's reply being spoken: its words so far.
    speaking: Option<String>,
    /// Tool calls handed to the client whose output has not gone out yet.
    open_calls: std::collections::HashSet<String>,
    /// Calls made on a line that dropped: their outputs go out as words.
    orphans: std::collections::HashSet<String>,
    /// A reply was in flight when the line dropped: ask for it again.
    reanswer: bool,
    /// This call has been redialled: its session.update carries the recap.
    resumed: bool,
}

impl CallMemory {
    /// Note what one server event says about the call.
    fn observe(&mut self, text: &str) {
        let Ok(ev) = serde_json::from_str::<Value>(text) else {
            return;
        };
        match ev.get("type").and_then(|t| t.as_str()).unwrap_or("") {
            "conversation.item.input_audio_transcription.completed" => {
                let Some(t) = ev.get("transcript").and_then(|v| v.as_str()) else {
                    return;
                };
                if t.is_empty() {
                    return;
                }
                let item = ev.get("item_id").and_then(|v| v.as_str()).map(str::to_string);
                // A longer finished transcript for the same item replaces
                // the shorter one: it is the same utterance going on.
                if let Some(turn) = self
                    .turns
                    .iter_mut()
                    .rev()
                    .find(|(user, _, id)| *user && item.is_some() && *id == item)
                {
                    turn.1 = t.to_string();
                } else {
                    self.push((true, t.to_string(), item));
                }
            }
            "response.output_audio_transcript.delta" | "response.text.delta" => {
                if let Some(d) = ev.get("delta").and_then(|v| v.as_str()) {
                    self.speaking.get_or_insert_with(String::new).push_str(d);
                }
            }
            "response.done" => self.close_reply(),
            "response.function_call_arguments.done" => {
                if let Some(id) = ev.get("call_id").and_then(|v| v.as_str())
                    && !id.is_empty()
                {
                    self.open_calls.insert(id.to_string());
                }
            }
            _ => {}
        }
    }

    fn close_reply(&mut self) {
        if let Some(words) = self.speaking.take()
            && !words.trim().is_empty()
        {
            self.push((false, words.trim().to_string(), None));
        }
    }

    fn push(&mut self, turn: (bool, String, Option<String>)) {
        self.turns.push_back(turn);
        while self.turns.len() > RECAP_TURNS {
            self.turns.pop_front();
        }
    }

    /// The line dropped: the reply being spoken is as far as it got, the
    /// calls still out can no longer be answered as calls, and a reply in
    /// flight is owed again.
    fn lost(&mut self, response_active: bool) {
        self.close_reply();
        self.orphans.extend(self.open_calls.drain());
        self.reanswer = response_active;
    }

    /// The instructions' tail for a redialled session: the call so far,
    /// newest last, at most `RECAP_CHARS`.
    fn recap(&self) -> String {
        let mut lines: Vec<String> = Vec::new();
        let mut used = 0;
        for (user, text, _) in self.turns.iter().rev() {
            let line = format!("{}: {}", if *user { "They" } else { "You" }, text);
            if used + line.len() > RECAP_CHARS && !lines.is_empty() {
                break;
            }
            used += line.len();
            lines.push(line);
        }
        lines.reverse();
        let mut out = String::from(
            "\n\n---\n\nThis call is already under way. Carry it on from where it is, \
             as the same conversation: do not greet again, and do not mention any \
             interruption.",
        );
        if !self.orphans.is_empty() {
            out.push_str(
                " A task you started is still running; its result will arrive as a \
                 message, and you relay it then.",
            );
        }
        if !lines.is_empty() {
            out.push_str("\n\nThe call so far, oldest first:\n");
            out.push_str(&lines.join("\n"));
        }
        out
    }
}

/// Turn-taking state of one realtime session, owned by the session task and
/// updated by `handle_server_event`.
#[derive(Default)]
struct SessionState {
    /// `SessionInitialized` has been sent.
    initialized: bool,
    /// A [`RealtimeCommand::Say`] went out and its response has not started:
    /// an `error` now is its refusal, not the call's.
    say_pending: bool,
    /// The provider refused a `Say`: no more are sent this call.
    say_refused: bool,
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
            state.say_pending = false;
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
            } else if std::mem::take(&mut state.say_pending) {
                // Words said on the session's behalf were refused. The call
                // is fine; it just stops saying them.
                state.say_refused = true;
                warn!(frame = %text, "realtime refused a spoken update; no more this call");
            } else {
                // The provider's words stay in the log; the client says the
                // call ended, plainly.
                warn!(frame = %text, "realtime upstream error");
                send(ConversationEvent::Error(CALL_ENDED.to_string())).await?;
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
            // Plain words for the client; the provider's message is logged.
            Some(ConversationEvent::Error(m)) => assert_eq!(m, CALL_ENDED),
            other => panic!("expected only the real error, got {other:?}"),
        }
    }

    // ── The line under a live call ──────────────────────────────────────

    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    type Served = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;

    async fn listener() -> (TcpListener, RealtimeConfig) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let cfg = RealtimeConfig {
            endpoint: format!("ws://{}/v1/realtime", l.local_addr().unwrap()),
            instructions: "You are the owner's assistant.".into(),
            ..Default::default()
        };
        (l, cfg)
    }

    async fn accept(l: &TcpListener) -> Served {
        let (stream, _) = l.accept().await.unwrap();
        tokio_tungstenite::accept_async(stream).await.unwrap()
    }

    /// The next text frame the far end receives, parsed (pings skipped).
    async fn next_json(ws: &mut Served) -> Value {
        loop {
            match ws.next().await {
                Some(Ok(WsMessage::Text(t))) => return serde_json::from_str(&t).unwrap(),
                Some(Ok(WsMessage::Ping(_) | WsMessage::Pong(_))) => continue,
                other => panic!("expected a text frame, got {other:?}"),
            }
        }
    }

    async fn say(ws: &mut Served, frame: Value) {
        ws.send(WsMessage::Text(frame.to_string().into())).await.unwrap();
    }

    fn errors(events: &[ConversationEvent]) -> Vec<String> {
        events
            .iter()
            .filter_map(|e| match e {
                ConversationEvent::Error(m) => Some(m.clone()),
                _ => None,
            })
            .collect()
    }

    /// The 2026-09-30 drop, on the desktop's side: the line to the voice
    /// service is cut with no closing handshake, mid-call, with a task the
    /// employee started still running. The session dials again at once, gives
    /// the new session its instructions and the call so far, turns the
    /// running task's result into words the new session can use, and carries
    /// on: the client sees no error and no second start.
    #[tokio::test]
    async fn a_dropped_line_is_redialled_and_the_call_carries_on() {
        let (l, cfg) = listener().await;
        let (seen_tx, seen_rx) = tokio::sync::oneshot::channel::<Vec<Value>>();
        let server = tokio::spawn(async move {
            let mut first = accept(&l).await;
            let update = next_json(&mut first).await;
            assert_eq!(update["type"], "session.update");
            assert!(!update["session"]["instructions"].as_str().unwrap().contains("under way"));
            say(&mut first, json!({"type": "session.created"})).await;
            say(&mut first, json!({
                "type": "conversation.item.input_audio_transcription.completed",
                "item_id": "i1", "transcript": "Book the flight to Denver."
            })).await;
            say(&mut first, json!({"type": "response.output_audio_transcript.delta", "delta": "On it."})).await;
            say(&mut first, json!({"type": "response.done", "response": {"id": "r1"}})).await;
            say(&mut first, json!({
                "type": "response.function_call_arguments.done",
                "call_id": "c1", "name": "nebo", "arguments": "{\"task\":\"book\"}"
            })).await;
            // Cut: no close frame, the socket just goes.
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            drop(first);

            let mut second = accept(&l).await;
            let mut seen = vec![next_json(&mut second).await];
            say(&mut second, json!({"type": "session.created"})).await;
            seen.push(next_json(&mut second).await); // the task's result
            seen.push(next_json(&mut second).await); // response.create
            match second.next().await {
                Some(Ok(WsMessage::Binary(b))) => assert_eq!(&b[..], &[1u8, 2, 3]),
                other => panic!("expected the caller's audio on the new line, got {other:?}"),
            }
            let _ = seen_tx.send(seen);
            // Hold the line until the client closes it.
            while let Some(Ok(m)) = second.next().await {
                if matches!(m, WsMessage::Close(_)) {
                    break;
                }
            }
        });

        let (tx, mut rx) = connect(cfg).await.unwrap();
        let mut events = Vec::new();
        // Wait for the tool call, as the handler does, before answering it.
        loop {
            let e = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
                .await
                .expect("events stopped")
                .expect("session ended");
            let is_call = matches!(e, ConversationEvent::ToolCall { .. });
            events.push(e);
            if is_call {
                break;
            }
        }
        // The task finishes after the line dropped.
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        tx.send(RealtimeCommand::ToolOutput { call_id: "c1".into(), output: "Booked, seat 12A.".into() })
            .await
            .unwrap();
        tx.send(RealtimeCommand::ToolOutputsDone).await.unwrap();
        tx.send(RealtimeCommand::Audio(Bytes::from_static(&[1, 2, 3]))).await.unwrap();

        let seen = tokio::time::timeout(std::time::Duration::from_secs(5), seen_rx)
            .await
            .expect("the call never reached the new line")
            .unwrap();
        let instructions = seen[0]["session"]["instructions"].as_str().unwrap();
        assert!(instructions.starts_with("You are the owner's assistant."), "{instructions}");
        assert!(instructions.contains("already under way"), "{instructions}");
        assert!(instructions.contains("They: Book the flight to Denver."), "{instructions}");
        assert!(instructions.contains("You: On it."), "{instructions}");
        assert!(instructions.contains("still running"), "{instructions}");
        assert_eq!(seen[1]["item"]["type"], "message");
        assert!(seen[1]["item"]["content"][0]["text"].as_str().unwrap().contains("Booked, seat 12A."));
        assert_eq!(seen[2]["type"], "response.create");

        tx.send(RealtimeCommand::Close).await.unwrap();
        while let Ok(Some(e)) = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await {
            events.push(e);
        }
        server.await.unwrap();
        assert!(errors(&events).is_empty(), "the client saw {:?}", errors(&events));
        assert_eq!(count(&events, |e| matches!(e, ConversationEvent::SessionInitialized)), 1);
    }

    /// When the line cannot be brought back, the call ends once, in plain
    /// words — never the socket's own error text.
    #[tokio::test]
    async fn the_call_ends_plainly_when_the_line_cannot_come_back() {
        let (l, cfg) = listener().await;
        let server = tokio::spawn(async move {
            let mut ws = accept(&l).await;
            next_json(&mut ws).await;
            say(&mut ws, json!({"type": "session.created"})).await;
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            // The service is gone: the socket drops and nothing answers again.
            drop(ws);
            drop(l);
        });

        let (_tx, mut rx) = connect(cfg).await.unwrap();
        let mut events = Vec::new();
        while let Ok(Some(e)) = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv()).await {
            events.push(e);
        }
        server.await.unwrap();
        assert_eq!(errors(&events), vec![CALL_ENDED.to_string()]);
        assert!(!CALL_ENDED.contains("WebSocket") && !CALL_ENDED.contains("error"));
    }

    /// A quiet call is not an idle one: with nothing said either way, the
    /// session still pings the far end, so nothing that counts silence on
    /// the way closes it.
    #[tokio::test]
    async fn a_quiet_line_is_pinged() {
        let (l, cfg) = listener().await;
        let server = tokio::spawn(async move {
            let mut ws = accept(&l).await;
            next_json(&mut ws).await;
            let mut pings = 0;
            let until = tokio::time::Instant::now() + PING_EVERY * 5;
            while let Ok(Some(Ok(m))) = tokio::time::timeout_at(until, ws.next()).await {
                if matches!(m, WsMessage::Ping(_)) {
                    pings += 1;
                }
            }
            pings
        });
        let (tx, _rx) = connect(cfg).await.unwrap();
        let pings = server.await.unwrap();
        assert!(pings >= 3, "only {pings} pings on a quiet line");
        drop(tx);
    }

    /// A far end that stops answering, with no error to say so (no pongs,
    /// no frames), is taken for dead and dialled again.
    #[tokio::test]
    async fn a_far_end_that_stops_answering_is_redialled() {
        let (l, cfg) = listener().await;
        let server = tokio::spawn(async move {
            let mut first = accept(&l).await;
            next_json(&mut first).await;
            // Never read again: the pings go unanswered.
            let started = tokio::time::Instant::now();
            let mut second = accept(&l).await;
            let update = next_json(&mut second).await;
            drop(first);
            (started.elapsed(), update)
        });
        let (tx, _rx) = connect(cfg).await.unwrap();
        let (after, update) = tokio::time::timeout(std::time::Duration::from_secs(5), server)
            .await
            .expect("a dead far end was never redialled")
            .unwrap();
        assert_eq!(update["type"], "session.update");
        assert!(after >= DEAD_AFTER, "redialled after {after:?}, before the line was dead");
        drop(tx);
    }

    /// A spoken update goes out as the provider's force message, verbatim,
    /// with no `response.create` (the force message is its own turn). A
    /// refusal of it is logged, never the end of the call, and no further
    /// update is sent; the call carries on.
    #[tokio::test]
    async fn a_spoken_update_is_said_verbatim_and_a_refusal_is_not_the_call() {
        let (l, cfg) = listener().await;
        let server = tokio::spawn(async move {
            let mut ws = accept(&l).await;
            next_json(&mut ws).await; // session.update
            say(&mut ws, json!({"type": "session.created"})).await;
            let said = next_json(&mut ws).await;
            say(&mut ws, json!({"type": "error", "error": {"message": "unknown item type"}})).await;
            // The next frame is the typed text, not a second update.
            let next = next_json(&mut ws).await;
            // The line stays up while the call's events are read.
            (said, next, ws, l)
        });
        let (tx, mut rx) = connect(cfg).await.unwrap();
        tx.send(RealtimeCommand::Say("Now editing.".into())).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        tx.send(RealtimeCommand::Say("Now checking.".into())).await.unwrap();
        tx.send(RealtimeCommand::Text("hello".into())).await.unwrap();
        let (said, next, _line, _l) = tokio::time::timeout(std::time::Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(said["type"], "conversation.item.create");
        assert_eq!(said["item"]["type"], "force_message");
        assert_eq!(said["item"]["role"], "assistant");
        assert_eq!(said["item"]["content"][0]["text"], "Now editing.");
        assert_eq!(next["item"]["type"], "message", "a refused update was sent again: {next}");
        let mut events = Vec::new();
        while let Ok(Some(e)) = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await {
            events.push(e);
        }
        assert!(errors(&events).is_empty(), "a refused update ended the call");
        drop(tx);
    }

    /// The recap a redialled session gets stays inside its budget and keeps
    /// the newest turns; one utterance's longer finished transcript replaces
    /// the shorter one.
    #[test]
    fn recap_keeps_the_newest_turns_within_budget() {
        let mut m = CallMemory::default();
        m.observe(r#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"a","transcript":"Hi"}"#);
        m.observe(r#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"a","transcript":"Hi there"}"#);
        assert_eq!(m.turns.len(), 1);
        assert_eq!(m.turns[0].1, "Hi there");
        for i in 0..40 {
            m.observe(&json!({"type": "response.text.delta", "delta": format!("reply {i} {}", "x".repeat(300))}).to_string());
            m.observe(r#"{"type":"response.done"}"#);
        }
        let recap = m.recap();
        assert!(recap.len() < RECAP_CHARS + 400, "recap is {} chars", recap.len());
        assert!(recap.contains("reply 39"));
        assert!(!recap.contains("reply 0 "));
        assert!(!recap.contains("still running"));
    }
}
