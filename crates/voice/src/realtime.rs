//! xAI Grok realtime (speech-to-speech) client.
//!
//! One WebSocket to Janus's metered realtime relay
//! (`wss://janus.neboai.com/v1/realtime`, Bearer = the owner's NeboAI token),
//! on Nebo's keys or on his own xAI key (`X-Provider-API-Key`, the platform
//! fee only). Janus speaks xAI's protocol, forwards frames verbatim and
//! bills wall-clock minutes.
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
    /// A call's format: telephony carries the carrier's own μ-law end to
    /// end, so nothing transcodes between the phone and the model; every
    /// other call is 24 kHz PCM.
    pub fn for_call(telephony: bool) -> Self {
        if telephony { Self::G711Ulaw } else { Self::Pcm24k }
    }

    /// The `session.audio.{input,output}.format` object for this format.
    fn as_json(self) -> Value {
        match self {
            Self::Pcm24k => json!({ "type": "audio/pcm", "rate": 24000 }),
            Self::G711Ulaw => json!({ "type": "audio/pcmu", "rate": 8000 }),
        }
    }

    /// Bytes of audio per millisecond: 24 kHz PCM16 mono is 48, 8 kHz μ-law
    /// is 8. Turns forwarded audio into played time.
    pub fn bytes_per_ms(self) -> usize {
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
    /// `wss://.../v1/realtime`: Janus's relay, on Nebo's keys or the
    /// owner's own xAI key (`own_key`). Always set by the caller.
    pub endpoint: String,
    /// The model the endpoint is asked for: the call's intelligence pack's
    /// Voice model (`agent::harness::turn::CallVoice`), `nebo-1-voice` or
    /// an xAI realtime model such as `grok-voice-latest` on the owner's
    /// key. Always set by the caller: nothing here picks one.
    pub model: String,
    /// Bearer token: the owner's NeboAI token (Janus's relay).
    pub bearer: String,
    /// Janus attribution header (X-Bot-ID).
    pub bot_id: Option<String>,
    /// The owner's own xAI key, when his pack's voice runs on it: Janus
    /// dials xAI on it and charges the platform fee (`types::own_key`).
    pub own_key: Option<types::own_key::OwnKey>,
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
    /// ASR bias terms, most important first, as [`crate::keyterms`] bounds
    /// them.
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
            // No call dials a provider direct: the caller names Janus's relay.
            endpoint: String::new(),
            model: String::new(),
            bearer: String::new(),
            bot_id: None,
            own_key: None,
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
    /// Typed user input (no audio): creates a message item, and requests a
    /// response when `respond`. Without one the model holds the words for
    /// what comes next and says nothing now (what the owner typed into the
    /// conversation while on the call: a text turn answers it).
    Text { text: String, respond: bool },
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
    let ws = dial(&cfg, None, &mut ::types::balance::ReservedWaits::default()).await?;
    info!(endpoint = %cfg.endpoint, model = %cfg.model, "realtime session connected");

    let (cmd_tx, cmd_rx) = mpsc::channel::<RealtimeCommand>(64);
    let (event_tx, event_rx) = mpsc::channel::<ConversationEvent>(64);

    tokio::spawn(run_session(ws, cfg, cmd_rx, event_tx));

    Ok((cmd_tx, event_rx))
}

pub(crate) type Socket = tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
>;

/// The URL a session dials: the endpoint, asked for the call's model, and
/// the conversation it resumes, when it resumes one.
fn realtime_url(cfg: &RealtimeConfig) -> String {
    let mut url = format!("{}?model={}", cfg.endpoint, cfg.model);
    if let Some(cid) = &cfg.conversation_id {
        url.push_str(&format!("&conversation_id={}", cid));
    }
    url
}

/// Open the socket to the realtime endpoint, each connect bounded by
/// `connect_limit` when given (a redial), waiting out a held balance from
/// `waits`: one budget per connection or per reconnection, however many
/// dials it takes.
async fn dial(
    cfg: &RealtimeConfig,
    connect_limit: Option<std::time::Duration>,
    waits: &mut ::types::balance::ReservedWaits,
) -> Result<Socket, VoiceError> {
    // Janus groups usage by purpose; a realtime session is always voice.
    open(&realtime_url(cfg), &cfg.bearer, cfg.bot_id.as_deref(), cfg.own_key.as_ref(), "voice", connect_limit, waits).await
}

/// Open a socket to a Janus (or direct) audio endpoint: the bearer, the bot
/// it is for, the owner's own key when it is his that pays, and the purpose
/// Janus groups the usage under. The voice call
/// and dictation both dial through here. Janus admits a line at connect; a
/// refusal because running work holds the balance is dialed again after the
/// waits `types::balance::ReservedWaits` gives, and one that outlasts them is
/// [`VoiceError::BalanceReserved`]. `connect_limit` bounds each connect
/// alone, never the waits between them: a redial waits out a held balance
/// as long as Janus asks. `waits` is the caller's one budget for the
/// connection (a reconnection passes the same one to every redial).
pub(crate) async fn open(
    url: &str,
    bearer: &str,
    bot_id: Option<&str>,
    own_key: Option<&types::own_key::OwnKey>,
    purpose: &'static str,
    connect_limit: Option<std::time::Duration>,
    waits: &mut ::types::balance::ReservedWaits,
) -> Result<Socket, VoiceError> {
    loop {
        let dialing = dial_once(url, bearer, bot_id, own_key, purpose);
        let dialed = match connect_limit {
            Some(limit) => tokio::time::timeout(limit, dialing)
                .await
                .unwrap_or_else(|_| Err(Dialed::Failed(VoiceError::Realtime("realtime dial timed out".into())))),
            None => dialing.await,
        };
        match dialed {
            Err(Dialed::Reserved(retry_after)) => match waits.next(retry_after) {
                Some(wait) => {
                    info!(wait_ms = wait.as_millis() as u64, purpose, "balance held by running work; dialing again after the wait");
                    tokio::time::sleep(wait).await;
                }
                None => return Err(VoiceError::BalanceReserved),
            },
            Err(Dialed::Failed(e)) => return Err(e),
            Ok(ws) => return Ok(ws),
        }
    }
}

/// Why one dial did not open: the balance held (with the wait the refusal
/// asked for), or anything else.
enum Dialed {
    Reserved(Option<std::time::Duration>),
    Failed(VoiceError),
}

async fn dial_once(
    url: &str,
    bearer: &str,
    bot_id: Option<&str>,
    own_key: Option<&types::own_key::OwnKey>,
    purpose: &'static str,
) -> Result<Socket, Dialed> {
    let failed = |e: String| Dialed::Failed(VoiceError::Realtime(e));
    let mut request = url
        .into_client_request()
        .map_err(|e| failed(format!("bad realtime url: {e}")))?;
    request.headers_mut().insert(
        "Authorization",
        format!("Bearer {}", bearer)
            .parse()
            .map_err(|_| failed("invalid bearer token".into()))?,
    );
    if let Some(bot_id) = bot_id {
        if let Ok(v) = bot_id.parse() {
            request.headers_mut().insert("X-Bot-ID", v);
        }
    }
    for (name, value) in own_key.iter().flat_map(|k| k.headers()) {
        if let Ok(v) = value.parse() {
            request.headers_mut().insert(name, v);
        }
    }
    request
        .headers_mut()
        .insert("X-Purpose", HeaderValue::from_static(purpose));
    request
        .headers_mut()
        .insert("User-Agent", HeaderValue::from_static(types::constants::USER_AGENT));

    match tls::connect_ws(request).await {
        Ok((ws, _resp)) => Ok(ws),
        Err(tokio_tungstenite::tungstenite::Error::Http(resp))
            if ::types::balance::is_balance_reserved(
                resp.status().as_u16(),
                &String::from_utf8_lossy(resp.body().as_deref().unwrap_or_default()),
            ) =>
        {
            let retry_after = ::types::balance::retry_after(resp.headers().get("retry-after").and_then(|v| v.to_str().ok()));
            Err(Dialed::Reserved(retry_after))
        }
        Err(e) => Err(failed(format!("realtime dial failed: {e}"))),
    }
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

/// How long one redial's connect may take before it counts as failed. Each
/// connect alone: a held balance between them is waited out in full.
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
            Redial::BalanceReserved => {
                warn!("realtime line held by running work past the waits; ending the call");
                let _ = event_tx.send(ConversationEvent::BalanceReserved).await;
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
    if let Err(e) = up(&mut sink, state, update, false).await {
        return Ended::Lost(format!("session.update failed: {e}"));
    }
    // What the dropped line owed goes up again, in the order it first went
    // up (see [`Resend`]): the new session has only the call's speech in its
    // recap, never the typed words or tool outputs it was given.
    for resend in std::mem::take(&mut memory.resend) {
        let sent = match resend {
            Resend::Ask(asked) => ask_again(&mut sink, state, asked, true).await,
            Resend::Input(asked) => ask_again(&mut sink, state, asked, false).await,
            Resend::Output(output) => {
                state.outputs.push((state.frames + 1, output.clone()));
                up(&mut sink, state, words_item(&output_words(&output)), false).await.map(drop)
            }
        };
        if let Err(e) = sent {
            return Ended::Lost(format!("realtime send failed: {e}"));
        }
    }
    // What the handler sent while the line was being dialled goes out now.
    for cmd in std::mem::take(deferred) {
        match send_command(&mut sink, cmd, cfg, state, memory, event_tx).await {
            Ok(true) => {}
            Ok(false) => return Ended::Done,
            Err(e) => return Ended::Lost(format!("realtime send failed: {e}")),
        }
    }

    let mut ping = tokio::time::interval_at(tokio::time::Instant::now() + PING_EVERY, PING_EVERY);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_heard = std::time::Instant::now();

    loop {
        let create_due = state.create_due();
        tokio::select! {
            // A reply asked for went unbegun too long with nothing else
            // running: the provider dropped the request. The call stops
            // waiting for it, so the line is never wedged.
            _ = tokio::time::sleep_until(tokio::time::Instant::from_std(create_due.unwrap_or_else(std::time::Instant::now))), if create_due.is_some() => {
                if let Some(dropped) = state.unbegun.pop_front() {
                    warn!(asked = ?dropped.asked, waited_ms = dropped.at.elapsed().as_millis() as u64, "realtime reply asked for never began; no longer waiting for it");
                    if matches!(dropped.asked, Asked::Say(_)) {
                        state.say_pending = None;
                    }
                    state.last_done = Some(std::time::Instant::now());
                    if event_tx.send(response_done(state)).await.is_err() {
                        return Ended::Done;
                    }
                }
            }

            cmd = cmd_rx.recv() => {
                let Some(cmd) = cmd else { return Ended::Done };
                match send_command(&mut sink, cmd, cfg, state, memory, event_tx).await {
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
                        if std::mem::take(&mut state.cancel_now)
                            && let Err(e) = up(&mut sink, state, json!({ "type": "response.cancel" }), false).await
                        {
                            return Ended::Lost(format!("realtime send failed: {e}"));
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
    event_tx: &mpsc::Sender<ConversationEvent>,
) -> Result<bool, tokio_tungstenite::tungstenite::Error>
where
    S: futures::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    match cmd {
        RealtimeCommand::Audio(pcm) => {
            // transport: "binary" — raw codec bytes, no base64.
            sink.send(Message::Binary(pcm.to_vec().into())).await?;
        }
        RealtimeCommand::Text { text, respond } => {
            up(sink, state, words_item(&text), false).await?;
            if respond {
                state.asked += 1;
                up(sink, state, json!({ "type": "response.create" }), false).await?;
                state.ask(Asked::Words(text));
            }
        }
        RealtimeCommand::Interrupt { played_ms } => {
            for frame in interrupt_frames(
                state,
                played_ms,
                std::time::Instant::now(),
                cfg.audio_format,
            ) {
                up(sink, state, frame, false).await?;
            }
        }
        RealtimeCommand::ToolOutput { call_id, output } => {
            // Named by the frame it goes up in (the next one).
            state.outputs.push((state.frames + 1, output.clone()));
            let item = if memory.orphans.remove(&call_id) {
                // The call it answers was made on a line that dropped: the
                // session on this one never saw it, so the result arrives
                // as words, not as that call's output.
                words_item(&output_words(&output))
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
            up(sink, state, item, false).await?;
        }
        RealtimeCommand::Say(text) => {
            state.asked += 1;
            if state.say_refused {
                // Never sent: the response it asked for is over already.
                let _ = event_tx.send(response_done(state)).await;
                return Ok(true);
            }
            let id = up(sink, state, say_frame(&text), true).await?;
            state.say_pending = Some(id);
            state.ask(Asked::Say(text));
        }
        RealtimeCommand::ToolOutputsDone => {
            state.asked += 1;
            up(sink, state, json!({ "type": "response.create" }), false).await?;
            let outputs = std::mem::take(&mut state.outputs).into_iter().map(|(_, o)| o).collect();
            state.ask(Asked::Outputs(outputs));
        }
        RealtimeCommand::Close => {
            let _ = sink.send(Message::Close(None)).await;
            return Ok(false);
        }
    }
    Ok(true)
}

/// Where the call's responses stand ([`ConversationEvent::ResponseDone`]).
fn response_done(state: &SessionState) -> ConversationEvent {
    ConversationEvent::ResponseDone { asked: state.asked, open: state.response_active || state.pending_creates() > 0 }
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
    // The reply under way is asked for again on the next line, with its
    // input, and so is each one asked for and not begun. One the owner cut
    // short or tapped away is never asked for again, but its input goes up
    // again with the input held for those before it, and each tool output
    // with no reply asked for yet: all in the order they went up.
    memory.lost();
    let mut resend: Vec<(u64, Resend)> = Vec::new();
    if let Some(begun) = state.begun.take() {
        let again = if state.response_active { Resend::Ask(begun.asked) } else { Resend::Input(begun.asked) };
        resend.push((begun.frame, again));
    }
    resend.extend(state.held.drain(..).map(|(frame, asked)| (frame, Resend::Input(asked))));
    resend.extend(state.unbegun.drain(..).map(|u| (u.frame, if u.tapped { Resend::Input(u.asked) } else { Resend::Ask(u.asked) })));
    resend.extend(std::mem::take(&mut state.outputs).into_iter().map(|(frame, o)| (frame, Resend::Output(o))));
    // A bare reply or words to say carry no input of their own.
    resend.retain(|(_, r)| !matches!(r, Resend::Input(Asked::Again | Asked::Say(_))));
    resend.sort_by_key(|(frame, _)| *frame);
    memory.resend = resend.into_iter().map(|(_, r)| r).collect();
    *state = SessionState {
        initialized: state.initialized,
        asked: state.asked,
        frames: state.frames,
        say_refused: state.say_refused,
        ..Default::default()
    };
    // Whatever was in flight went with the line; what is asked for again on
    // the next one is still open.
    event_tx
        .send(ConversationEvent::ResponseDone { asked: state.asked, open: memory.resend.iter().any(|r| matches!(r, Resend::Ask(_))) })
        .await
        .map_err(|_| ())?;
    Ok(())
}

enum Redial {
    Connected(Box<Socket>),
    /// The call was closed while the line was being dialled.
    Done,
    /// Every redial failed.
    Failed,
    /// The last redial was refused because running work holds the balance,
    /// past the reconnection's one budget.
    BalanceReserved,
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
    // One budget for waiting out a held balance across this reconnection's
    // dials, never one per dial.
    let mut waits = ::types::balance::ReservedWaits::default();
    let mut held = false;
    while *failures < REDIAL_DELAYS.len() {
        let delay = REDIAL_DELAYS[*failures];
        *failures += 1;
        let attempt = *failures;
        let dialling = async {
            tokio::time::sleep(delay).await;
            dial(cfg, Some(DIAL_TIMEOUT), &mut waits).await
        };
        tokio::pin!(dialling);
        loop {
            tokio::select! {
                dialled = &mut dialling => {
                    match dialled {
                        Ok(socket) => return Redial::Connected(Box::new(socket)),
                        Err(e) => {
                            held = matches!(e, VoiceError::BalanceReserved);
                            warn!(attempt, error = %e, "realtime redial failed");
                        }
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
    if held {
        Redial::BalanceReserved
    } else {
        Redial::Failed
    }
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
    /// What goes up again on the next line, in the order it went up on the
    /// one that dropped.
    resend: Vec<Resend>,
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

    /// The line dropped: the reply being spoken is as far as it got, and the
    /// calls still out can no longer be answered as calls.
    fn lost(&mut self) {
        self.close_reply();
        self.orphans.extend(self.open_calls.drain());
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

/// What a dropped line's next one is sent again (see [`CallMemory::resend`]).
#[derive(Debug, PartialEq)]
enum Resend {
    /// A reply under way, or asked for and not begun, asked for again with
    /// its input.
    Ask(Asked),
    /// The input of a reply the owner cut short or tapped away, sent again
    /// with no reply asked for: typed words, tool outputs as words.
    Input(Asked),
    /// A tool output sent with no reply asked for yet, as words.
    Output(String),
}

/// A reply asked for that the provider has not begun, and what it answers.
struct Unbegun {
    asked: Asked,
    at: std::time::Instant,
    /// The frame that asked for it (see [`SessionState::frames`]).
    frame: u64,
    /// The owner cut the reply short before the provider began this one:
    /// it is cancelled as it begins, its audio never played, and never
    /// asked for again after a drop.
    tapped: bool,
}

/// The reply under way: what it answers and the frame that asked for it.
struct Begun {
    asked: Asked,
    frame: u64,
    /// The owner cut it short (a tap, before or after it began).
    cut: bool,
}

/// What a reply asked for answers, kept so a dropped line can ask again with
/// the same input.
#[derive(Clone, Debug, PartialEq)]
enum Asked {
    /// A bare `response.create`: what it answers is in the call so far.
    Again,
    /// Typed words ([`RealtimeCommand::Text`] with `respond`).
    Words(String),
    /// Words said as they are ([`RealtimeCommand::Say`]).
    Say(String),
    /// The tool outputs a [`RealtimeCommand::ToolOutputsDone`] answers.
    Outputs(Vec<String>),
}

/// How long a reply asked for may go unbegun, with no other reply running,
/// before the call stops waiting for it: a `response.create` the provider
/// dropped without a word never wedges the line.
#[cfg(not(test))]
const CREATE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);
#[cfg(test)]
const CREATE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(400);

impl SessionState {
    /// Replies asked for and not begun.
    fn pending_creates(&self) -> usize {
        self.unbegun.len()
    }

    /// A reply was asked for, answering `asked`.
    fn ask(&mut self, asked: Asked) {
        self.unbegun.push_back(Unbegun { asked, at: std::time::Instant::now(), frame: self.frames, tapped: false });
    }

    /// When the oldest reply asked for has waited too long, if it is still
    /// waiting: from when it was asked for or the last reply ended, never
    /// while one runs.
    fn create_due(&self) -> Option<std::time::Instant> {
        if self.response_active {
            return None;
        }
        let oldest = self.unbegun.front()?.at;
        Some(self.last_done.map_or(oldest, |d| d.max(oldest)) + CREATE_TIMEOUT)
    }
}

/// Send one frame upstream, named: it carries an `event_id`
/// (`<kind>_<n>`, an identifier Janus passes back, `^[a-z0-9_.]{1,64}$`), so
/// an error about it names it. Returns the name.
async fn up<S>(sink: &mut S, state: &mut SessionState, mut frame: Value, say: bool) -> Result<String, tokio_tungstenite::tungstenite::Error>
where
    S: futures::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    let kind = if say {
        "say"
    } else {
        match frame.get("type").and_then(|t| t.as_str()) {
            Some("conversation.item.create") => "item",
            Some("response.create") => "create",
            Some("response.cancel") => "cancel",
            Some("conversation.item.truncate") => "truncate",
            Some("session.update") => "update",
            _ => "frame",
        }
    };
    state.frames += 1;
    let id = format!("{kind}_{}", state.frames);
    frame["event_id"] = json!(id);
    state.say_alone = say;
    sink.send(Message::Text(frame.to_string().into())).await?;
    Ok(id)
}

/// A user message item: typed words, or what a tool returned on a line
/// that no longer has its call.
fn words_item(text: &str) -> Value {
    json!({
        "type": "conversation.item.create",
        "item": {
            "type": "message",
            "role": "user",
            "content": [{ "type": "input_text", "text": text }],
        },
    })
}

/// A tool's output said as words, for a session that never saw its call.
fn output_words(output: &str) -> String {
    format!("(The result of the task you started earlier: {output})")
}

/// Ask again, on a new line, for a reply the last one owed: its input first,
/// as words where the new session cannot hold it as it was. Without
/// `respond`, only the input goes up (held for the line after, should this
/// one drop too), and words to say are not said.
async fn ask_again<S>(sink: &mut S, state: &mut SessionState, asked: Asked, respond: bool) -> Result<(), tokio_tungstenite::tungstenite::Error>
where
    S: futures::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    let frame = state.frames + 1;
    match &asked {
        Asked::Again => {}
        Asked::Words(text) => {
            up(sink, state, words_item(text), false).await?;
        }
        Asked::Say(text) => {
            if !respond || state.say_refused {
                return Ok(());
            }
            let id = up(sink, state, say_frame(text), true).await?;
            state.say_pending = Some(id);
            state.ask(asked);
            return Ok(());
        }
        Asked::Outputs(outputs) => {
            for output in outputs {
                up(sink, state, words_item(&output_words(output)), false).await?;
            }
        }
    }
    if !respond {
        state.held.push((frame, asked));
        return Ok(());
    }
    up(sink, state, json!({ "type": "response.create" }), false).await?;
    state.ask(asked);
    Ok(())
}

/// Turn-taking state of one realtime session, owned by the session task and
/// updated by `handle_server_event`.
#[derive(Default)]
struct SessionState {
    /// `SessionInitialized` has been sent.
    initialized: bool,
    /// Responses the handler's commands asked for, all call long (a dropped
    /// line keeps the count).
    asked: u64,
    /// Responses asked for on this line (the handler's, and a re-answer
    /// after a redial) that the provider has not begun yet, oldest first,
    /// with what each answers: a dropped line asks for them again, input
    /// and all, on the next.
    unbegun: std::collections::VecDeque<Unbegun>,
    /// The reply under way, from its `response.created` to its
    /// `response.done`: a dropped line asks for it again with its input.
    begun: Option<Begun>,
    /// Input re-sent on this line for replies the owner cut short or tapped
    /// away, with the frame each went up in: the next line needs it too.
    held: Vec<(u64, Asked)>,
    /// Tool outputs sent since the last response asked for, each with the
    /// frame it went up in: what the next `ToolOutputsDone` answers.
    outputs: Vec<(u64, String)>,
    /// When the last response ended: a reply asked for while another ran
    /// waits from here, not from when it was asked for.
    last_done: Option<std::time::Instant>,
    /// Frames sent upstream, each named `<kind>_<n>` (its `event_id`).
    frames: u64,
    /// The last frame sent was the waiting `Say`: an error naming no frame
    /// can only be about it.
    say_alone: bool,
    /// Set when a begun response must be cancelled now; the socket loop
    /// sends the cancel (the event handler has no way upstream).
    cancel_now: bool,
    /// A [`RealtimeCommand::Say`] went out and its response has not started,
    /// with the `event_id` its frame carried: an `error` about that event is
    /// its refusal, not the call's.
    say_pending: Option<String>,
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
        if let Some(begun) = &mut state.begun {
            begun.cut = true;
        }
        frames.push(json!({ "type": "response.cancel" }));
    } else {
        // Cut short before the provider began them: each reply waiting now
        // is cancelled as it begins.
        for waiting in &mut state.unbegun {
            waiting.tapped = true;
        }
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

/// Error codes that are the call's whatever was sent last: the account,
/// the key, the limits, the provider. Never a refusal of one `Say`.
const CALL_ERROR_CODES: &[&str] = &[
    "insufficient_quota",
    "usage_limit_exceeded",
    "rate_limit_exceeded",
    "token_limit_exceeded",
    "invalid_api_key",
    "authentication_failed",
    "insufficient_permissions",
    "balance_reserved",
    "provider_error",
    "provider_unavailable",
    "upstream_error",
    "server_error",
];

/// Whether `error` refuses the `Say` waiting on its response (its frame's
/// `event_id` is `pending`). Every frame the call sends is named, and Janus
/// passes back a client event id an error carries in `error.event_id`
/// (with the error's type and code; its message is Janus's own words). An
/// error naming that event is the refusal and one naming another is not.
/// xAI does not echo the client's id: measured 2026-10-21 through Janus,
/// its `error.event_id` is its own UUID, which Janus drops, so its errors
/// name no frame. One naming none is the refusal only when the `Say` is the
/// last frame sent (`say_alone`: nothing else it could be about) and its
/// code or type is not the call's ([`CALL_ERROR_CODES`]).
fn say_refusal(error: &Value, pending: Option<&str>, say_alone: bool) -> bool {
    let Some(pending) = pending else { return false };
    let field = |k: &str| error.pointer(k).and_then(|v| v.as_str()).unwrap_or_default();
    match error.pointer("/error/event_id").and_then(|v| v.as_str()) {
        Some(id) => id == pending,
        None => say_alone && !CALL_ERROR_CODES.contains(&field("/error/code")) && field("/error/type") != "server_error",
    }
}

/// Translate one xAI server event into `ConversationEvent`s. Returns Err when
/// the event channel is closed (downstream gone).
async fn handle_server_event(
    text: &str,
    event_tx: &mpsc::Sender<ConversationEvent>,
    state: &mut SessionState,
) -> Result<(), ()> {
    let Ok(ev) = serde_json::from_str::<Value>(text) else {
        warn!(frame = %::types::redact::redact(text), "unparseable realtime event");
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
                debug!(frame = %::types::redact::redact(text), "transcript for a closed utterance (dropped)");
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
                debug!(frame = %::types::redact::redact(text), "transcript for a closed utterance (dropped)");
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
            state.say_pending = None;
            let next = state.unbegun.pop_front();
            state.response_active = true;
            state.discard_audio = false;
            // A reply begun while the owner is speaking is the provider's
            // answer to him, never one the tap was about.
            let cut = next.as_ref().is_some_and(|u| u.tapped) && state.utterance == Utterance::Closed;
            if cut {
                // The owner cut it short before it began: none of it plays,
                // and its `response.done` still ends it.
                state.response_active = false;
                state.discard_audio = true;
                state.cancel_now = true;
            }
            // One the provider began on its own answers the call so far.
            state.begun = Some(match next {
                Some(u) => Begun { asked: u.asked, frame: u.frame, cut },
                None => Begun { asked: Asked::Again, frame: state.frames, cut },
            });
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
                warn!(frame = %::types::redact::redact(text), "function call event missing call_id/name");
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
            // A reply cut short leaves its input with the session alone (none
            // of it was said): a dropped line sends it again.
            if let Some(begun) = state.begun.take()
                && begun.cut
            {
                state.held.push((begun.frame, begun.asked));
            }
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
            // Every response ends here, heard or silent, done or cancelled.
            state.last_done = Some(std::time::Instant::now());
            send(response_done(state)).await?;
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
                warn!(frame = %::types::redact::redact(text), "benign realtime cancel race (ignored)");
            } else if say_refusal(&ev, state.say_pending.as_deref(), state.say_alone) {
                // Words said on the session's behalf were refused. The call
                // is fine; it just stops saying them.
                state.say_pending = None;
                state.say_refused = true;
                if let Some(at) = state.unbegun.iter().position(|u| matches!(u.asked, Asked::Say(_))) {
                    state.unbegun.remove(at);
                }
                warn!(frame = %::types::redact::redact(text), "realtime refused a spoken update; no more this call");
                send(ConversationEvent::SayRefused).await?;
                send(response_done(state)).await?;
            } else {
                // The provider's words stay in the log; the client says the
                // call ended, plainly.
                warn!(frame = %::types::redact::redact(text), "realtime upstream error");
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

    /// The dial names this Nebo and its version (`User-Agent: nebo/<v>`):
    /// Janus logs it per session, to count the clients still on old versions.
    #[tokio::test]
    async fn the_dial_names_nebo_and_its_version() {
        use tokio::io::AsyncReadExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 8192];
            let mut got = Vec::new();
            while !got.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = sock.read(&mut buf).await.unwrap();
                if n == 0 {
                    break;
                }
                got.extend_from_slice(&buf[..n]);
            }
            String::from_utf8_lossy(&got).to_ascii_lowercase()
        });
        let cfg = RealtimeConfig {
            endpoint: format!("ws://127.0.0.1:{port}/v1/realtime"),
            model: "nebo-1-voice".into(),
            bearer: "jwt".into(),
            ..Default::default()
        };
        // The far end closes once it has read the request, so the dial ends
        // then; the bound only catches a dial that never does. (A 2 s bound
        // here once raced the connect on a busy machine and left the far end
        // waiting for good.)
        let _ = tokio::time::timeout(std::time::Duration::from_secs(30), dial(&cfg, None, &mut ::types::balance::ReservedWaits::default())).await;
        let request = tokio::time::timeout(std::time::Duration::from_secs(30), seen)
            .await
            .expect("the dial never reached the far end")
            .unwrap();
        let want = format!("user-agent: nebo/{}", env!("CARGO_PKG_VERSION"));
        assert!(request.contains(&want), "the dial sent no {want}: {request}");
        assert!(
            request.starts_with("get /v1/realtime?model=nebo-1-voice http/1.1"),
            "the dial asks the relay for the call's model: {request}"
        );
    }

    /// The URL asks the endpoint for the call's model (the pack's Voice) and
    /// carries the conversation a redial resumes; nothing here picks a model.
    #[test]
    fn the_realtime_url_carries_the_calls_model() {
        let mut cfg = RealtimeConfig {
            endpoint: "wss://janus.example/v1/realtime".into(),
            model: "nebo-1-voice".into(),
            ..Default::default()
        };
        assert_eq!(realtime_url(&cfg), "wss://janus.example/v1/realtime?model=nebo-1-voice");
        cfg.model = "grok-voice-latest".into();
        cfg.conversation_id = Some("c1".into());
        assert_eq!(realtime_url(&cfg), "wss://janus.example/v1/realtime?model=grok-voice-latest&conversation_id=c1");
        let default = RealtimeConfig::default();
        assert!(default.model.is_empty(), "no model is hard-coded");
        assert!(default.endpoint.is_empty(), "no provider is dialed direct");
    }

    /// The session.update frame must pin the invariants the relay depends on:
    /// binary transport + 24kHz PCM both directions, server VAD, resumption.
    #[test]
    fn session_update_pins_audio_contract() {
        let mut cfg = RealtimeConfig::default();
        cfg.keyterms = crate::keyterms(["Nebo", "Tymick", "nebo"]);
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
        assert_eq!(s["audio"]["input"]["transcription"]["keyterms"], json!(["Nebo", "Tymick"]));
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
    /// order, with the state they left — all but the end of each response,
    /// which `every_response_end_is_told_with_the_count_asked` proves.
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
            if !matches!(e, ConversationEvent::ResponseDone { .. }) {
                events.push(e);
            }
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
        assert!(matches!(rx.recv().await, Some(ConversationEvent::ResponseDone { asked: 0, open: false })));

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
        let (redialled_tx, redialled_rx) = tokio::sync::oneshot::channel::<()>();
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
            let _ = redialled_tx.send(());
            seen.push(next_json(&mut second).await); // the task's result
            seen.push(next_json(&mut second).await); // response.create
            // The session pings a quiet line (every 100 ms here), so a ping
            // can come first when the machine is busy.
            loop {
                match second.next().await {
                    Some(Ok(WsMessage::Binary(b))) => break assert_eq!(&b[..], &[1u8, 2, 3]),
                    Some(Ok(WsMessage::Ping(_) | WsMessage::Pong(_))) => continue,
                    other => panic!("expected the caller's audio on the new line, got {other:?}"),
                }
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
        // The task finishes after the line dropped and the call is on the
        // new line: told by the far end, never a guessed wait (a busy
        // machine ran the guess before the drop was even noticed).
        tokio::time::timeout(std::time::Duration::from_secs(30), redialled_rx)
            .await
            .expect("the line was never dialled again")
            .unwrap();
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

    /// The `ResponseDone`s a call's events carry, as (asked, open).
    async fn ends_until(rx: &mut mpsc::Receiver<ConversationEvent>, n: usize, events: &mut Vec<ConversationEvent>) -> Vec<(u64, bool)> {
        let mut ends = Vec::new();
        while ends.len() < n {
            let e = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await.expect("no response end").unwrap();
            if let ConversationEvent::ResponseDone { asked, open } = e {
                ends.push((asked, open));
            }
            events.push(e);
        }
        ends
    }

    /// A tap before the provider begins the reply asked for cancels it as it
    /// begins: none of its audio plays, and its `response.done` still ends
    /// it, the line free. A reply tapped once begun is cancelled at once.
    #[tokio::test]
    async fn a_tap_before_a_reply_begins_cancels_it_as_it_begins() {
        let (l, cfg) = listener().await;
        let server = tokio::spawn(async move {
            let mut ws = accept(&l).await;
            next_json(&mut ws).await; // session.update
            say(&mut ws, json!({"type": "session.created"})).await;
            let mut sent = Vec::new();
            for _ in 0..3 {
                sent.push(next_json(&mut ws).await); // item, create, marker
            }
            say(&mut ws, json!({"type": "response.created", "response": {"id": "r1"}})).await;
            sent.push(next_json(&mut ws).await); // the cancel, as it begins
            ws.send(WsMessage::Binary(vec![0u8; 480].into())).await.unwrap();
            say(&mut ws, json!({"type": "response.done", "response": {"id": "r1", "status": "cancelled"}})).await;
            for _ in 0..2 {
                sent.push(next_json(&mut ws).await); // item, create
            }
            say(&mut ws, json!({"type": "response.created", "response": {"id": "r2"}})).await;
            sent.push(next_json(&mut ws).await); // the cancel of a begun reply
            say(&mut ws, json!({"type": "response.done", "response": {"id": "r2", "status": "cancelled"}})).await;
            (sent, ws, l)
        });
        let (tx, mut rx) = connect(cfg).await.unwrap();
        let mut events = Vec::new();
        tx.send(RealtimeCommand::Text { text: "first".into(), respond: true }).await.unwrap();
        tx.send(RealtimeCommand::Interrupt { played_ms: None }).await.unwrap();
        tx.send(RealtimeCommand::Text { text: "marker".into(), respond: false }).await.unwrap();
        assert_eq!(ends_until(&mut rx, 1, &mut events).await, [(1, false)]);
        assert!(
            !events.iter().any(|e| matches!(e, ConversationEvent::PlaybackStart | ConversationEvent::AudioChunk(_))),
            "a reply tapped before it began played: {events:?}"
        );
        tx.send(RealtimeCommand::Text { text: "second".into(), respond: true }).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        tx.send(RealtimeCommand::Interrupt { played_ms: None }).await.unwrap();
        assert_eq!(ends_until(&mut rx, 1, &mut events).await, [(2, false)]);
        let (sent, _ws, _l) = tokio::time::timeout(std::time::Duration::from_secs(5), server).await.unwrap().unwrap();
        assert_eq!(sent[1]["type"], "response.create");
        assert_eq!(sent[2]["item"]["content"][0]["text"], "marker", "the tap sent something before the reply began: {}", sent[2]);
        assert_eq!(sent[3]["type"], "response.cancel", "a reply tapped before it began was not cancelled: {}", sent[3]);
        assert_eq!(sent[5]["type"], "response.create");
        assert_eq!(sent[6]["type"], "response.cancel", "a begun reply was not cancelled: {}", sent[6]);
        drop(tx);
    }

    /// Two replies asked for before either begins: the first one's end says
    /// one is still to come, the second's frees the line.
    #[tokio::test]
    async fn every_reply_asked_for_is_open_until_it_ends() {
        let (l, cfg) = listener().await;
        let server = tokio::spawn(async move {
            let mut ws = accept(&l).await;
            next_json(&mut ws).await; // session.update
            say(&mut ws, json!({"type": "session.created"})).await;
            for _ in 0..3 {
                next_json(&mut ws).await; // item, create, create
            }
            for id in ["r1", "r2"] {
                say(&mut ws, json!({"type": "response.created", "response": {"id": id}})).await;
                say(&mut ws, json!({"type": "response.done", "response": {"id": id}})).await;
            }
            (ws, l)
        });
        let (tx, mut rx) = connect(cfg).await.unwrap();
        tx.send(RealtimeCommand::Text { text: "one".into(), respond: true }).await.unwrap();
        tx.send(RealtimeCommand::ToolOutputsDone).await.unwrap();
        let mut events = Vec::new();
        assert_eq!(ends_until(&mut rx, 2, &mut events).await, [(2, true), (2, false)]);
        let _ = server.await;
        drop(tx);
    }

    /// A reply under way when the line drops is still open: the dropped
    /// line's report says so, and the new line's re-answer ends it.
    #[tokio::test]
    async fn a_reply_asked_again_after_a_redial_stays_open_until_it_ends() {
        let (l, cfg) = listener().await;
        let server = tokio::spawn(async move {
            let mut first = accept(&l).await;
            next_json(&mut first).await; // session.update
            say(&mut first, json!({"type": "session.created"})).await;
            say(&mut first, json!({"type": "response.created", "response": {"id": "r1"}})).await;
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            drop(first);
            let mut second = accept(&l).await;
            next_json(&mut second).await; // session.update
            let again = next_json(&mut second).await;
            say(&mut second, json!({"type": "response.created", "response": {"id": "r2"}})).await;
            say(&mut second, json!({"type": "response.done", "response": {"id": "r2"}})).await;
            (again, second, l)
        });
        let (tx, mut rx) = connect(cfg).await.unwrap();
        let mut events = Vec::new();
        assert_eq!(ends_until(&mut rx, 2, &mut events).await, [(0, true), (0, false)], "{events:?}");
        let (again, _ws, _l) = tokio::time::timeout(std::time::Duration::from_secs(10), server).await.unwrap().unwrap();
        assert_eq!(again["type"], "response.create");
        drop(tx);
    }

    /// An error frame as Janus relays it: its own message, the upstream's
    /// type and code, and the client event it is about when there is one.
    fn janus_error(code: &str, about: Option<&str>) -> String {
        let mut e = json!({"type": "invalid_request_error", "code": code, "message": "This request couldn't be completed. Try again."});
        if let Some(id) = about {
            e["event_id"] = json!(id);
        }
        json!({"type": "error", "event_id": "event_9", "error": e}).to_string()
    }

    /// Only an error about the `Say` waiting is its refusal, as Janus
    /// relays it: one naming that event; one naming none unless its code is
    /// the call's. A held balance or an error about another event is the
    /// call's error, even while a `Say` waits. `Say` ids pass Janus's
    /// identifier check.
    #[tokio::test]
    async fn only_an_error_about_the_say_is_its_refusal() {
        let parsed = |f: String| serde_json::from_str::<Value>(&f).unwrap();
        let pending = Some("say_3");
        assert!(say_refusal(&parsed(janus_error("invalid_value", Some("say_3"))), pending, false));
        assert!(!say_refusal(&parsed(janus_error("invalid_value", Some("truncate_4"))), pending, true));
        // Naming no frame: the Say's only when it is the last frame sent.
        assert!(say_refusal(&parsed(janus_error("invalid_value", None)), pending, true));
        assert!(!say_refusal(&parsed(janus_error("invalid_value", None)), pending, false));
        assert!(!say_refusal(&parsed(janus_error("insufficient_quota", None)), pending, true));
        assert!(!say_refusal(&parsed(janus_error("invalid_value", Some("say_3"))), None, true));

        for (frame, refused) in [(janus_error("invalid_value", Some("say_1")), true), (janus_error("insufficient_quota", None), false)] {
            let (tx, mut rx) = mpsc::channel(8);
            let mut state = SessionState { initialized: true, say_pending: Some("say_1".into()), unbegun: waiting(1), say_alone: true, ..Default::default() };
            handle_server_event(&frame, &tx, &mut state).await.unwrap();
            match rx.recv().await {
                Some(ConversationEvent::SayRefused) => assert!(refused, "a balance error was taken for the refusal"),
                Some(ConversationEvent::Error(m)) => assert!(!refused && m == CALL_ENDED, "the refusal ended the call"),
                other => panic!("unexpected {other:?}"),
            }
        }
    }

    /// A tap cancels the replies waiting at that moment as they begin, and
    /// no other: one refused takes its cancel with it, a reply asked for
    /// after the tap plays, and a reply begun while the owner speaks (the
    /// provider's answer to him) is never cancelled.
    #[tokio::test]
    async fn a_tap_cancels_only_the_replies_it_came_before() {
        let (tx, _rx) = mpsc::channel(64);
        let created = r#"{"type":"response.created","response":{"id":"r"}}"#;
        let fresh = || {
            let mut state = SessionState { initialized: true, say_pending: Some("say_1".into()), ..Default::default() };
            state.ask(Asked::Say("Now editing.".into()));
            state
        };

        // The Say tapped before it began is refused: nothing is left to cancel.
        let mut state = fresh();
        interrupt_frames(&mut state, None, std::time::Instant::now(), AudioFormat::Pcm24k);
        assert!(state.unbegun.iter().all(|u| u.tapped));
        handle_server_event(&janus_error("invalid_value", Some("say_1")), &tx, &mut state).await.unwrap();
        state.ask(Asked::Again); // a reply asked for after the tap
        handle_server_event(created, &tx, &mut state).await.unwrap();
        assert!(!state.cancel_now && state.response_active, "a reply asked for after the tap was cancelled");

        // A reply begun while he speaks is his answer, not the tapped one.
        let mut state = fresh();
        state.unbegun = waiting(2);
        interrupt_frames(&mut state, None, std::time::Instant::now(), AudioFormat::Pcm24k);
        state.utterance = Utterance::Open { transcribed: false };
        handle_server_event(created, &tx, &mut state).await.unwrap();
        assert!(!state.cancel_now, "the provider's answer to him was cancelled");
        state.utterance = Utterance::Closed;
        state.response_active = false;
        handle_server_event(created, &tx, &mut state).await.unwrap();
        assert!(state.cancel_now, "the tapped reply was not cancelled as it began");
    }

    /// `n` replies asked for with nothing kept to resend.
    fn waiting(n: usize) -> std::collections::VecDeque<Unbegun> {
        (0..n).map(|_| Unbegun { asked: Asked::Again, at: std::time::Instant::now(), frame: 0, tapped: false }).collect()
    }

    /// A question put just before the line drops, its reply not begun, is
    /// kept, words and all, to be asked for again on the new line.
    #[tokio::test]
    async fn a_reply_asked_for_but_not_begun_is_asked_again_after_a_drop() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut state = SessionState { initialized: true, asked: 1, ..Default::default() };
        state.ask(Asked::Words("Book the flight.".into()));
        let mut memory = CallMemory::default();
        settle_lost_socket(&tx, &mut state, &mut memory).await.unwrap();
        assert_eq!(memory.resend, [Resend::Ask(Asked::Words("Book the flight.".into()))], "a reply asked for just before the drop was dropped with it");
        assert!(matches!(rx.recv().await, Some(ConversationEvent::ResponseDone { asked: 1, open: true })));
    }

    /// A reply the owner tapped away before it began, the line then
    /// dropping, is never asked for on the new line. (Review repro.)
    #[tokio::test]
    async fn a_reply_tapped_away_is_not_asked_again_after_a_drop() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut state = SessionState { initialized: true, asked: 1, unbegun: waiting(1), ..Default::default() };
        interrupt_frames(&mut state, None, std::time::Instant::now(), AudioFormat::Pcm24k);
        assert!(state.unbegun.iter().all(|u| u.tapped));
        let mut memory = CallMemory::default();
        settle_lost_socket(&tx, &mut state, &mut memory).await.unwrap();
        let _ = rx.recv().await;
        assert!(memory.resend.is_empty(), "the reply the owner tapped away is re-asked on the new line");
    }

    /// An error naming no frame, while a `Say` waits but another frame (the
    /// tap's truncate) went after it, is not the `Say`'s refusal. (Review
    /// repro.)
    #[tokio::test]
    async fn an_error_about_another_frame_is_not_the_says_refusal() {
        let frame = json!({"type":"error","error":{"type":"invalid_request_error","code":"invalid_value","message":"This request couldn't be completed. Try again."}}).to_string();
        let (tx, mut rx) = mpsc::channel(8);
        let mut state = SessionState { initialized: true, say_pending: Some("say_1".into()), unbegun: waiting(1), ..Default::default() };
        handle_server_event(&frame, &tx, &mut state).await.unwrap();
        assert!(!matches!(rx.recv().await, Some(ConversationEvent::SayRefused)), "an error about another frame refused the Say for the call");
    }

    /// Every frame the call sends is named, and a reply asked for and never
    /// begun on a line that drops is asked for again on the next with its
    /// input: typed words as they were, tool outputs as words.
    #[tokio::test]
    async fn a_dropped_line_asks_again_with_the_input() {
        let (l, cfg) = listener().await;
        let server = tokio::spawn(async move {
            let mut first = accept(&l).await;
            let mut seen = vec![next_json(&mut first).await]; // session.update
            say(&mut first, json!({"type": "session.created"})).await;
            for _ in 0..4 {
                seen.push(next_json(&mut first).await); // item, create, output, create
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            drop(first);
            let mut second = accept(&l).await;
            let mut again = Vec::new();
            for _ in 0..5 {
                again.push(next_json(&mut second).await); // update, item, create, item, create
            }
            (seen, again, second, l)
        });
        let (tx, _rx) = connect(cfg).await.unwrap();
        tx.send(RealtimeCommand::Text { text: "Book the flight.".into(), respond: true }).await.unwrap();
        tx.send(RealtimeCommand::ToolOutput { call_id: "c1".into(), output: "Seat 12A.".into() }).await.unwrap();
        tx.send(RealtimeCommand::ToolOutputsDone).await.unwrap();
        let (seen, again, _ws, _l) = tokio::time::timeout(std::time::Duration::from_secs(10), server).await.unwrap().unwrap();
        for frame in seen.iter().chain(&again) {
            let id = frame["event_id"].as_str().unwrap_or_default();
            assert!(!id.is_empty() && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'), "unnamed frame: {frame}");
        }
        assert_eq!(again[0]["type"], "session.update");
        assert_eq!(again[1]["item"]["content"][0]["text"], "Book the flight.");
        assert_eq!(again[2]["type"], "response.create");
        assert_eq!(again[3]["item"]["content"][0]["text"], "(The result of the task you started earlier: Seat 12A.)");
        assert_eq!(again[4]["type"], "response.create");
        drop(tx);
    }

    /// A tap lets go of only the replies waiting at that moment: the oldest,
    /// let go of unbegun, takes its cancel with it, and a reply asked for
    /// after the tap plays when it begins. (Review repro: Text A, tap,
    /// Text B, A never begun, B begins.)
    #[tokio::test]
    async fn a_reply_asked_after_the_tap_plays_when_the_tapped_one_is_let_go() {
        let (l, cfg) = listener().await;
        let (go_tx, go_rx) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            let mut ws = accept(&l).await;
            next_json(&mut ws).await; // session.update
            say(&mut ws, json!({"type": "session.created"})).await;
            for _ in 0..4 {
                next_json(&mut ws).await; // item A, create A, item B, create B
            }
            // A is let go; the line stays up (pings answered) meanwhile.
            tokio::pin!(go_rx);
            loop {
                tokio::select! {
                    _ = &mut go_rx => break,
                    msg = ws.next() => assert!(matches!(msg, Some(Ok(WsMessage::Ping(_) | WsMessage::Pong(_)))), "unexpected {msg:?}"),
                }
            }
            say(&mut ws, json!({"type": "response.created", "response": {"id": "rb"}})).await;
            ws.send(WsMessage::Binary(vec![0u8; 480].into())).await.unwrap();
            say(&mut ws, json!({"type": "response.done", "response": {"id": "rb"}})).await;
            let after = next_json(&mut ws).await;
            (after, ws, l)
        });
        let (tx, mut rx) = connect(cfg).await.unwrap();
        let mut events = Vec::new();
        tx.send(RealtimeCommand::Text { text: "A".into(), respond: true }).await.unwrap();
        tx.send(RealtimeCommand::Interrupt { played_ms: None }).await.unwrap();
        tx.send(RealtimeCommand::Text { text: "B".into(), respond: true }).await.unwrap();
        assert_eq!(ends_until(&mut rx, 1, &mut events).await, [(2, true)], "A was not let go: {events:?}");
        go_tx.send(()).unwrap();
        assert_eq!(ends_until(&mut rx, 1, &mut events).await, [(2, false)]);
        assert!(events.iter().any(|e| matches!(e, ConversationEvent::PlaybackStart)), "the reply asked after the tap did not play: {events:?}");
        tx.send(RealtimeCommand::Text { text: "marker".into(), respond: false }).await.unwrap();
        let (after, _ws, _l) = tokio::time::timeout(std::time::Duration::from_secs(5), server).await.unwrap().unwrap();
        assert_eq!(after["item"]["content"][0]["text"], "marker", "the reply asked after the tap was cancelled: {after}");
        drop(tx);
    }

    /// A tapped `Say` refused takes its cancel with it: a reply asked for
    /// after the tap plays when it begins.
    #[tokio::test]
    async fn a_refused_say_takes_its_tap_with_it() {
        let (l, cfg) = listener().await;
        let server = tokio::spawn(async move {
            let mut ws = accept(&l).await;
            next_json(&mut ws).await; // session.update
            say(&mut ws, json!({"type": "session.created"})).await;
            let said = next_json(&mut ws).await;
            for _ in 0..2 {
                next_json(&mut ws).await; // item B, create B
            }
            let about = said["event_id"].as_str().unwrap().to_string();
            ws.send(WsMessage::Text(janus_error("invalid_value", Some(&about)).into())).await.unwrap();
            say(&mut ws, json!({"type": "response.created", "response": {"id": "rb"}})).await;
            say(&mut ws, json!({"type": "response.done", "response": {"id": "rb"}})).await;
            let after = next_json(&mut ws).await;
            (after, ws, l)
        });
        let (tx, mut rx) = connect(cfg).await.unwrap();
        let mut events = Vec::new();
        tx.send(RealtimeCommand::Say("Now editing.".into())).await.unwrap();
        tx.send(RealtimeCommand::Interrupt { played_ms: None }).await.unwrap();
        tx.send(RealtimeCommand::Text { text: "B".into(), respond: true }).await.unwrap();
        assert_eq!(ends_until(&mut rx, 2, &mut events).await, [(2, true), (2, false)], "{events:?}");
        assert!(events.iter().any(|e| matches!(e, ConversationEvent::SayRefused)), "{events:?}");
        tx.send(RealtimeCommand::Text { text: "marker".into(), respond: false }).await.unwrap();
        let (after, _ws, _l) = tokio::time::timeout(std::time::Duration::from_secs(5), server).await.unwrap().unwrap();
        assert_eq!(after["item"]["content"][0]["text"], "marker", "the reply asked after the tap was cancelled: {after}");
        drop(tx);
    }

    /// What goes up again after a drop goes in the order it first went up,
    /// and a tool output still waiting on its `ToolOutputsDone` survives a
    /// second drop too.
    #[tokio::test]
    async fn a_tool_output_waiting_survives_two_drops_in_order() {
        let (l, cfg) = listener().await;
        let server = tokio::spawn(async move {
            let mut first = accept(&l).await;
            next_json(&mut first).await; // session.update
            say(&mut first, json!({"type": "session.created"})).await;
            for _ in 0..3 {
                next_json(&mut first).await; // item, create, output
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            drop(first);
            let mut lines = Vec::new();
            for last in [false, true] {
                let mut ws = accept(&l).await;
                let mut again = Vec::new();
                for _ in 0..4 {
                    again.push(next_json(&mut ws).await); // update, item, create, output words
                }
                if last {
                    again.push(next_json(&mut ws).await); // the ToolOutputsDone's create
                    lines.push(again);
                    return (lines, ws, l);
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                drop(ws);
                lines.push(again);
            }
            unreachable!()
        });
        let (tx, _rx) = connect(cfg).await.unwrap();
        tx.send(RealtimeCommand::Text { text: "Book the flight.".into(), respond: true }).await.unwrap();
        tx.send(RealtimeCommand::ToolOutput { call_id: "c1".into(), output: "Seat 12A.".into() }).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(1_500)).await;
        tx.send(RealtimeCommand::ToolOutputsDone).await.unwrap();
        let (lines, _ws, _l) = tokio::time::timeout(std::time::Duration::from_secs(10), server).await.unwrap().unwrap();
        for again in &lines {
            assert_eq!(again[0]["type"], "session.update");
            assert_eq!(again[1]["item"]["content"][0]["text"], "Book the flight.");
            assert_eq!(again[2]["type"], "response.create");
            assert_eq!(again[3]["item"]["content"][0]["text"], "(The result of the task you started earlier: Seat 12A.)", "{again:?}");
        }
        assert_eq!(lines[1][4]["type"], "response.create");
        drop(tx);
    }

    /// A reply tapped away is never asked for again after a drop, but what
    /// it answered is: the tool's result goes up as words with no reply
    /// asked for, on the next line and the one after.
    #[tokio::test]
    async fn a_tapped_replys_tool_result_goes_up_again_with_no_reply() {
        let (l, cfg) = listener().await;
        let server = tokio::spawn(async move {
            let mut first = accept(&l).await;
            next_json(&mut first).await; // session.update
            say(&mut first, json!({"type": "session.created"})).await;
            for _ in 0..2 {
                next_json(&mut first).await; // output, create
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            drop(first);
            let mut lines = Vec::new();
            for last in [false, true] {
                let mut ws = accept(&l).await;
                let mut again = Vec::new();
                for _ in 0..3 {
                    again.push(next_json(&mut ws).await); // update, output words, marker
                    if again.len() == 2 && !last {
                        break;
                    }
                }
                lines.push(again);
                if last {
                    return (lines, ws, l);
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                drop(ws);
            }
            unreachable!()
        });
        let (tx, _rx) = connect(cfg).await.unwrap();
        tx.send(RealtimeCommand::ToolOutput { call_id: "c1".into(), output: "Seat 12A.".into() }).await.unwrap();
        tx.send(RealtimeCommand::ToolOutputsDone).await.unwrap();
        tx.send(RealtimeCommand::Interrupt { played_ms: None }).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(1_000)).await;
        tx.send(RealtimeCommand::Text { text: "marker".into(), respond: false }).await.unwrap();
        let (lines, _ws, _l) = tokio::time::timeout(std::time::Duration::from_secs(10), server).await.unwrap().unwrap();
        for again in &lines {
            assert_eq!(again[0]["type"], "session.update");
            assert_eq!(again[1]["item"]["content"][0]["text"], "(The result of the task you started earlier: Seat 12A.)", "{again:?}");
        }
        assert_eq!(lines[1][2]["item"]["content"][0]["text"], "marker", "a reply was asked for the tapped one: {}", lines[1][2]);
        drop(tx);
    }

    /// A reply under way when the line drops is asked for again with what
    /// it answers, after the tool output that went up before it.
    #[tokio::test]
    async fn a_reply_under_way_is_asked_again_with_its_input_in_order() {
        let (l, cfg) = listener().await;
        let server = tokio::spawn(async move {
            let mut first = accept(&l).await;
            next_json(&mut first).await; // session.update
            say(&mut first, json!({"type": "session.created"})).await;
            for _ in 0..3 {
                next_json(&mut first).await; // output, item, create
            }
            say(&mut first, json!({"type": "response.created", "response": {"id": "r1"}})).await;
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            drop(first);
            let mut second = accept(&l).await;
            let mut again = Vec::new();
            for _ in 0..4 {
                again.push(next_json(&mut second).await); // update, output words, item, create
            }
            (again, second, l)
        });
        let (tx, _rx) = connect(cfg).await.unwrap();
        tx.send(RealtimeCommand::ToolOutput { call_id: "c1".into(), output: "Seat 12A.".into() }).await.unwrap();
        tx.send(RealtimeCommand::Text { text: "Book the flight.".into(), respond: true }).await.unwrap();
        let (again, _ws, _l) = tokio::time::timeout(std::time::Duration::from_secs(10), server).await.unwrap().unwrap();
        assert_eq!(again[0]["type"], "session.update");
        assert_eq!(again[1]["item"]["content"][0]["text"], "(The result of the task you started earlier: Seat 12A.)", "{again:?}");
        assert_eq!(again[2]["item"]["content"][0]["text"], "Book the flight.", "{again:?}");
        assert_eq!(again[3]["type"], "response.create");
        drop(tx);
    }

    /// A `response.create` the provider drops without a word never wedges
    /// the line: past the wait, the call stops waiting for it.
    #[tokio::test]
    async fn a_reply_never_begun_is_let_go() {
        let (l, cfg) = listener().await;
        let server = tokio::spawn(async move {
            let mut ws = accept(&l).await;
            next_json(&mut ws).await; // session.update
            say(&mut ws, json!({"type": "session.created"})).await;
            next_json(&mut ws).await; // item
            next_json(&mut ws).await; // create, never answered
            // The line stays up (pings answered) while nothing begins.
            let _ = tokio::time::timeout(std::time::Duration::from_secs(3), async { while ws.next().await.is_some() {} }).await;
            (ws, l)
        });
        let (tx, mut rx) = connect(cfg).await.unwrap();
        tx.send(RealtimeCommand::Text { text: "Hello?".into(), respond: true }).await.unwrap();
        let mut events = Vec::new();
        let started = std::time::Instant::now();
        assert_eq!(ends_until(&mut rx, 1, &mut events).await, [(1, false)], "the line stayed wedged");
        assert!(started.elapsed() >= CREATE_TIMEOUT, "let go before the wait was up");
        drop(tx);
        let _ = server.await;
    }

    /// Typed words held for later go up whole as one item with no
    /// `response.create`: the model says nothing for them. Words to answer
    /// get their response.
    #[tokio::test]
    async fn typed_words_held_for_later_ask_for_no_response() {
        let (l, cfg) = listener().await;
        let server = tokio::spawn(async move {
            let mut ws = accept(&l).await;
            next_json(&mut ws).await; // session.update
            say(&mut ws, json!({"type": "session.created"})).await;
            let held = next_json(&mut ws).await;
            let after = next_json(&mut ws).await;
            let respond = next_json(&mut ws).await;
            (held, after, respond, ws)
        });
        let (tx, _rx) = connect(cfg).await.unwrap();
        let post = "word ".repeat(1_200);
        tx.send(RealtimeCommand::Text { text: post.clone(), respond: false }).await.unwrap();
        tx.send(RealtimeCommand::Text { text: "read it".into(), respond: true }).await.unwrap();
        let (held, after, respond, _ws) = tokio::time::timeout(std::time::Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(held["item"]["content"][0]["text"], post.as_str(), "the words went up cut");
        assert_eq!(after["item"]["content"][0]["text"], "read it", "held words asked for a response: {after}");
        assert_eq!(respond["type"], "response.create");
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
            // A frame the session answers with an event, after the refusal:
            // frames are read in order, so once the client sees it the
            // refusal has been taken in.
            say(&mut ws, json!({"type": "conversation.created", "conversation": {"id": "after-refusal"}})).await;
            // The next frame is the typed text, not a second update.
            let next = next_json(&mut ws).await;
            // The line stays up while the call's events are read.
            (said, next, ws, l)
        });
        let (tx, mut rx) = connect(cfg).await.unwrap();
        tx.send(RealtimeCommand::Say("Now editing.".into())).await.unwrap();
        // The second update is asked for only once the refusal of the first
        // has been taken in, never after a guessed wait.
        let mut events = Vec::new();
        loop {
            let e = tokio::time::timeout(std::time::Duration::from_secs(30), rx.recv())
                .await
                .expect("the refusal was never taken in")
                .expect("session ended");
            let taken_in = matches!(&e, ConversationEvent::ConversationId(id) if id == "after-refusal");
            events.push(e);
            if taken_in {
                break;
            }
        }
        tx.send(RealtimeCommand::Say("Now checking.".into())).await.unwrap();
        tx.send(RealtimeCommand::Text { text: "hello".into(), respond: true }).await.unwrap();
        let (said, next, _line, _l) = tokio::time::timeout(std::time::Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(said["type"], "conversation.item.create");
        assert_eq!(said["item"]["type"], "force_message");
        assert_eq!(said["item"]["role"], "assistant");
        assert_eq!(said["item"]["content"][0]["text"], "Now editing.");
        assert!(said["event_id"].as_str().is_some_and(|id| id.starts_with("say_")), "a Say carries an id Janus passes back: {said}");
        assert_eq!(next["item"]["type"], "message", "a refused update was sent again: {next}");
        while let Ok(Some(e)) = tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await {
            events.push(e);
        }
        assert!(errors(&events).is_empty(), "a refused update ended the call");
        assert_eq!(
            events.iter().filter(|e| matches!(e, ConversationEvent::SayRefused)).count(),
            1,
            "the refusal was not told once: {events:?}"
        );
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

    const RESERVED: &str = r#"{"error":{"code":"BALANCE_RESERVED","message":"balance reserved by running work"}}"#;

    /// Refuses the next upgrade the way Janus does while running work holds
    /// the balance.
    async fn refuse_reserved(l: &TcpListener, retry_after: u64) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut sock, _) = l.accept().await.unwrap();
        let mut buf = vec![0u8; 8192];
        let _ = sock.read(&mut buf).await;
        let reply = format!(
            "HTTP/1.1 429 Too Many Requests\r\ncontent-type: application/json\r\nretry-after: {retry_after}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{RESERVED}",
            RESERVED.len()
        );
        let _ = sock.write_all(reply.as_bytes()).await;
    }

    /// Janus refused the line twice while running work held the balance; the
    /// third dial opens it.
    #[tokio::test]
    async fn a_held_balance_is_waited_out_and_the_line_opens() {
        let (l, cfg) = listener().await;
        let far = tokio::spawn(async move {
            refuse_reserved(&l, 0).await;
            refuse_reserved(&l, 0).await;
            let _ws = accept(&l).await;
        });
        let opened = open(&cfg.endpoint, "tok", None, None, "voice", None, &mut ::types::balance::ReservedWaits::default()).await;
        assert!(opened.is_ok(), "{:?}", opened.err());
        far.await.unwrap();
    }

    /// Still held after every wait: the busy words, never the call ending
    /// and never out of balance.
    #[tokio::test]
    async fn a_balance_held_past_the_waits_is_busy() {
        let (l, cfg) = listener().await;
        let far = tokio::spawn(async move {
            for _ in 0..=::types::balance::RESERVED_RETRIES {
                refuse_reserved(&l, 0).await;
            }
        });
        let err = open(&cfg.endpoint, "tok", None, None, "voice", None, &mut ::types::balance::ReservedWaits::default()).await.err().expect("refused");
        assert!(matches!(err, VoiceError::BalanceReserved), "{err:?}");
        assert_eq!(err.to_string(), ::types::balance::BALANCE_BUSY);
        far.await.unwrap();
    }

    /// The call's line drops and the redial is refused while running work
    /// holds the balance, asking for 5 s (Janus's Retry-After): longer than
    /// one connect may take. The wait is not cut off by the connect deadline:
    /// the redial waits it out, connects, and the call carries on, with no
    /// error for the client.
    #[tokio::test]
    async fn a_redial_waits_out_a_held_balance_and_the_call_carries_on() {
        let (l, cfg) = listener().await;
        let server = tokio::spawn(async move {
            let mut first = accept(&l).await;
            next_json(&mut first).await; // session.update
            say(&mut first, json!({"type": "session.created"})).await;
            drop(first);
            refuse_reserved(&l, 5).await;
            let mut second = accept(&l).await;
            let update = next_json(&mut second).await;
            say(&mut second, json!({"type": "conversation.created", "conversation": {"id": "back"}})).await;
            // Hold the line until the client closes it.
            while let Some(Ok(m)) = second.next().await {
                if matches!(m, WsMessage::Close(_)) {
                    break;
                }
            }
            update
        });
        let (tx, mut rx) = connect(cfg).await.unwrap();
        let started = tokio::time::Instant::now();
        let mut events = Vec::new();
        loop {
            let e = tokio::time::timeout(std::time::Duration::from_secs(30), rx.recv())
                .await
                .expect("the call never came back")
                .expect("the call ended");
            let back = matches!(&e, ConversationEvent::ConversationId(id) if id == "back");
            events.push(e);
            if back {
                break;
            }
        }
        assert!(started.elapsed() >= std::time::Duration::from_secs(5), "the wait Janus asked for was taken");
        assert!(errors(&events).is_empty(), "the client saw {:?}", errors(&events));
        tx.send(RealtimeCommand::Close).await.unwrap();
        let update = tokio::time::timeout(std::time::Duration::from_secs(10), server).await.unwrap().unwrap();
        assert_eq!(update["type"], "session.update");
    }

    /// The line drops and every redial is refused while running work holds
    /// the balance. The reconnection has ONE budget for waiting it out, not
    /// one per dial, and when it runs out the client hears the balance is
    /// busy, never that the call ended.
    #[tokio::test]
    async fn a_reconnection_held_past_its_one_budget_ends_busy() {
        let (l, cfg) = listener().await;
        let refused = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = refused.clone();
        let server = tokio::spawn(async move {
            let mut first = accept(&l).await;
            next_json(&mut first).await; // session.update
            say(&mut first, json!({"type": "session.created"})).await;
            drop(first);
            // Refuse every redial until the client stops dialling.
            while tokio::time::timeout(std::time::Duration::from_secs(5), async {
                refuse_reserved(&l, 1).await;
            })
            .await
            .is_ok()
            {
                counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        });
        let (_tx, mut rx) = connect(cfg).await.unwrap();
        let mut events = Vec::new();
        loop {
            match tokio::time::timeout(std::time::Duration::from_secs(60), rx.recv()).await.expect("the call never ended") {
                Some(ConversationEvent::BalanceReserved) => break,
                Some(e) => events.push(e),
                None => panic!("the call ended without saying the balance is busy: {events:?}"),
            }
        }
        assert!(errors(&events).is_empty(), "never the call-ended words: {:?}", errors(&events));
        server.await.unwrap();
        let dials = refused.load(std::sync::atomic::Ordering::SeqCst);
        // The first redial waits the whole budget (1 + RESERVED_RETRIES
        // dials); the rest have no waits left and dial once each.
        assert_eq!(dials, 1 + ::types::balance::RESERVED_RETRIES + (REDIAL_DELAYS.len() - 1), "one budget per reconnection");
    }
}
