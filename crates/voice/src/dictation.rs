//! Dictation: the phone's microphone, streamed to NeboAI's transcription and
//! back as text for the message box.
//!
//! One socket to a streaming speech-to-text endpoint: Janus's metered
//! `/v1/stt` (Bearer = the owner's NeboAI token, billed as `stt_minute` on
//! the wall clock like a voice call) or the provider direct with an
//! owner-held key. Janus forwards frames verbatim, so this module speaks the
//! provider's streaming shape and hands the server a small vocabulary of its
//! own ([`DictationEvent`]); nothing past here knows whose recognizer it is.
//!
//! Audio goes up as raw PCM16 LE mono binary frames from the moment the
//! phone starts talking. Frames that arrive before the far end says it is
//! ready are held and sent in order once it is, so the first words are never
//! lost to the dial. Text comes back as live partials for the span being
//! heard and finals (punctuated) that replace them; each carries where in
//! the audio it starts and ends, which is how the phone stitches spans
//! without losing or repeating a word.

use std::time::Duration;

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::protocol::Message;
use tracing::{debug, info, warn};

use crate::realtime::{Socket, open};

/// The phone's microphone rate when it does not say: 16 kHz is what the
/// recognizer takes without resampling.
pub const DEFAULT_SAMPLE_RATE: u32 = 16_000;

/// After the phone says it has stopped, how long the last words may take to
/// come back before dictation ends with what it has.
const STOP_GRACE: Duration = Duration::from_secs(5);

/// Audio held while the far end is not ready yet: about 30 s at 16 kHz. Past
/// that the dial has failed in all but name.
const HOLD_LIMIT: usize = 1_000_000;

/// One dictation's line.
#[derive(Debug, Clone)]
pub struct DictationConfig {
    /// `wss://.../v1/stt` (Janus relay or the provider direct).
    pub endpoint: String,
    /// Janus JWT for the relay leg, the owner's key for direct.
    pub bearer: String,
    /// Janus attribution header (X-Bot-ID).
    pub bot_id: Option<String>,
    /// The phone's PCM16 mono rate.
    pub sample_rate: u32,
    /// BCP-47 language of the speaker ("en", "pt-BR"); none = the
    /// recognizer's own choice.
    pub language: Option<String>,
    /// Words the speaker is likely to say (names, products), most important
    /// first, as [`crate::keyterms`] bounds them.
    pub keyterms: Vec<String>,
}

/// What the phone sends.
#[derive(Debug)]
pub enum DictationInput {
    /// PCM16 LE mono at [`DictationConfig::sample_rate`].
    Audio(Bytes),
    /// The owner stopped: finish the words already said, then end.
    Stop,
}

/// What dictation tells the phone.
#[derive(Debug, Clone, PartialEq)]
pub enum DictationEvent {
    /// The recognizer is listening (audio held until now has gone up).
    Ready,
    /// Words for the span of audio from `start` to `end` seconds. A partial
    /// is replaced by the next event; a final is the punctuated text for its
    /// span and stays.
    Text { text: String, start: f64, end: f64, is_final: bool },
    /// Every word said before the stop has been delivered. `text` is the
    /// recognizer's whole transcript when it sends one (may be empty).
    Done { text: String },
    /// Dictation could not go on. Plain words for logs; the phone has its own.
    Error(String),
    /// The line was refused because running work holds the balance, past
    /// the waits. Sent with the code `balance_reserved`, which the phone
    /// words in the owner's language; never out of balance.
    BalanceReserved,
}

impl DictationEvent {
    /// The frame the phone reads.
    pub fn to_json(&self) -> Value {
        match self {
            Self::Ready => json!({"type": "ready"}),
            Self::Text { text, start, end, is_final } => {
                json!({"type": "text", "text": text, "start": start, "end": end, "final": is_final})
            }
            Self::Done { text } => json!({"type": "done", "text": text}),
            Self::Error(m) => json!({"type": "error", "message": m}),
            // The message stays for a phone that does not know the code.
            Self::BalanceReserved => json!({
                "type": "error",
                "code": "balance_reserved",
                "message": ::types::balance::BALANCE_BUSY,
            }),
        }
    }
}

/// The provider URL for this dictation: PCM at the phone's rate, live
/// partials, the end of a thought heard at a pause (not every breath), the
/// speaker's language, and the words to expect. Words stay as said
/// ("first", "nine thirty"): written-form numbers are for transcripts, not a
/// message being typed by voice.
pub fn stt_url(cfg: &DictationConfig) -> String {
    let mut url = format!(
        "{}?encoding=pcm&sample_rate={}&interim_results=true&smart_turn=0.7",
        cfg.endpoint, cfg.sample_rate
    );
    if let Some(lang) = cfg.language.as_deref().and_then(clean_language) {
        url.push_str("&language=");
        url.push_str(&lang);
    }
    for term in &cfg.keyterms {
        url.push_str("&keyterm=");
        url.push_str(&urlencoding::encode(term));
    }
    url
}

/// A language tag as the query may carry it: letters, digits and hyphens
/// (`en`, `pt-BR`, `zh-Hant`), else nothing.
fn clean_language(tag: &str) -> Option<String> {
    let tag = tag.trim().replace('_', "-");
    let ok = !tag.is_empty()
        && tag.len() <= 16
        && tag.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && tag.chars().next().is_some_and(|c| c.is_ascii_alphabetic());
    ok.then_some(tag)
}

/// One provider frame, in dictation's words. `None` for frames that say
/// nothing the phone needs.
pub fn parse_event(frame: &str) -> Option<DictationEvent> {
    let v: Value = serde_json::from_str(frame).ok()?;
    let num = |k: &str| v.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    let text = || v.get("text").and_then(Value::as_str).unwrap_or("").to_string();
    match v.get("type").and_then(Value::as_str)? {
        "transcript.created" => Some(DictationEvent::Ready),
        "transcript.partial" => {
            let start = num("start");
            Some(DictationEvent::Text {
                text: text(),
                start,
                end: start + num("duration"),
                is_final: v.get("is_final").and_then(Value::as_bool).unwrap_or(false),
            })
        }
        "transcript.done" => Some(DictationEvent::Done { text: text() }),
        "error" => Some(DictationEvent::Error(
            v.get("message")
                .or_else(|| v.pointer("/error/message"))
                .and_then(Value::as_str)
                .unwrap_or("transcription error")
                .to_string(),
        )),
        _ => None,
    }
}

/// Run one dictation: dial, relay the phone's audio up and the text down,
/// and end on the phone's stop (after its last words), the phone going away,
/// or the line failing. Every way it ends tells `events` (except the phone
/// going away, when there is nobody to tell).
pub async fn run(
    cfg: DictationConfig,
    mut input: mpsc::Receiver<DictationInput>,
    events: mpsc::Sender<DictationEvent>,
) {
    let url = stt_url(&cfg);
    let mut held: Vec<Bytes> = Vec::new();
    let mut held_bytes = 0usize;
    let mut stop_asked = false;

    // Dial while the phone already talks: its audio is held, not dropped.
    // Janus files the minutes under Transcription, with every other
    // transcription the bot asks for.
    let mut waits = ::types::balance::ReservedWaits::default();
    let dial = open(&url, &cfg.bearer, cfg.bot_id.as_deref(), "transcribe", None, &mut waits);
    tokio::pin!(dial);
    let ws: Socket = loop {
        tokio::select! {
            dialed = &mut dial => match dialed {
                Ok(ws) => break ws,
                Err(e) => {
                    warn!(error = %e, "dictation dial failed");
                    let ended = match e {
                        crate::VoiceError::BalanceReserved => DictationEvent::BalanceReserved,
                        e => DictationEvent::Error(e.to_string()),
                    };
                    let _ = events.send(ended).await;
                    return;
                }
            },
            got = input.recv() => match got {
                Some(DictationInput::Audio(b)) => hold(&mut held, &mut held_bytes, b),
                Some(DictationInput::Stop) => stop_asked = true,
                None => return,
            },
        }
    };
    info!(sample_rate = cfg.sample_rate, "dictation line open");
    let (mut up, mut down) = ws.split();

    let mut ready = false;
    let mut stopping: Option<tokio::time::Instant> = None;
    loop {
        let grace = async {
            match stopping {
                Some(at) => tokio::time::sleep_until(at).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            frame = down.next() => match frame {
                Some(Ok(Message::Text(t))) => match parse_event(&t) {
                    Some(DictationEvent::Ready) if !ready => {
                        ready = true;
                        for b in held.drain(..) {
                            if up.send(Message::Binary(b)).await.is_err() {
                                break;
                            }
                        }
                        held_bytes = 0;
                        let _ = events.send(DictationEvent::Ready).await;
                        if stop_asked && stopping.is_none() {
                            stopping = Some(finish(&mut up).await);
                        }
                    }
                    Some(DictationEvent::Ready) => {}
                    Some(done @ DictationEvent::Done { .. }) => {
                        let _ = events.send(done).await;
                        break;
                    }
                    Some(err @ DictationEvent::Error(_)) => {
                        warn!(?err, "dictation upstream error");
                        let _ = events.send(err).await;
                        break;
                    }
                    Some(text) => {
                        if events.send(text).await.is_err() {
                            break;
                        }
                    }
                    None => debug!(frame = %t, "dictation frame ignored"),
                },
                Some(Ok(Message::Close(_))) | None => {
                    // After the stop, the far end closing is the end of the
                    // words; before it, the line failed.
                    let _ = events
                        .send(if stopping.is_some() {
                            DictationEvent::Done { text: String::new() }
                        } else {
                            DictationEvent::Error("transcription line closed".into())
                        })
                        .await;
                    break;
                }
                Some(Ok(_)) => {}
                Some(Err(e)) => {
                    warn!(error = %e, "dictation upstream read failed");
                    let _ = events.send(DictationEvent::Error(e.to_string())).await;
                    break;
                }
            },
            got = input.recv(), if stopping.is_none() => match got {
                Some(DictationInput::Audio(b)) => {
                    if ready {
                        if up.send(Message::Binary(b)).await.is_err() {
                            let _ = events.send(DictationEvent::Error("transcription line closed".into())).await;
                            break;
                        }
                    } else {
                        hold(&mut held, &mut held_bytes, b);
                    }
                }
                Some(DictationInput::Stop) => {
                    if ready {
                        stopping = Some(finish(&mut up).await);
                    } else {
                        stop_asked = true;
                    }
                }
                // The phone went away: nobody to finish for.
                None => break,
            },
            _ = grace => {
                info!("dictation stop grace ran out");
                let _ = events.send(DictationEvent::Done { text: String::new() }).await;
                break;
            }
        }
    }
    let _ = up.send(Message::Close(None)).await;
}

fn hold(held: &mut Vec<Bytes>, held_bytes: &mut usize, b: Bytes) {
    if *held_bytes + b.len() > HOLD_LIMIT {
        return;
    }
    *held_bytes += b.len();
    held.push(b);
}

/// Tell the far end the audio is over: end the words being said now without
/// waiting for a pause (`finalize`), then close the audio (`audio.done`); the
/// last words follow. Returns when to give up waiting for them.
async fn finish<S>(up: &mut S) -> tokio::time::Instant
where
    S: futures::Sink<Message> + Unpin,
{
    for kind in ["finalize", "audio.done"] {
        if up.send(Message::Text(json!({"type": kind}).to_string().into())).await.is_err() {
            break;
        }
    }
    tokio::time::Instant::now() + STOP_GRACE
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    type Served = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;

    fn cfg(endpoint: String) -> DictationConfig {
        DictationConfig {
            endpoint,
            bearer: "token".into(),
            bot_id: Some("bot-1".into()),
            sample_rate: 16_000,
            language: Some("en".into()),
            keyterms: Vec::new(),
        }
    }

    #[test]
    fn the_url_asks_for_pcm_partials_and_the_language() {
        let c = cfg("wss://janus.example/v1/stt".into());
        assert_eq!(
            stt_url(&c),
            "wss://janus.example/v1/stt?encoding=pcm&sample_rate=16000&interim_results=true&smart_turn=0.7&language=en"
        );
        assert!(!stt_url(&c).contains("format"), "words stay as said");
        let mut c = c;
        c.language = Some("pt_BR".into());
        assert!(stt_url(&c).ends_with("&language=pt-BR"));
        c.language = Some("en&x=1".into());
        assert!(!stt_url(&c).contains("language"));
        c.language = None;
        assert!(!stt_url(&c).contains("language"));
    }

    #[test]
    fn the_url_carries_each_word_to_expect_as_its_own_keyterm() {
        let mut c = cfg("wss://janus.example/v1/stt".into());
        c.keyterms = vec!["NeboLoop".into(), "Nebo Media".into(), "Tymick & Co".into()];
        assert!(
            stt_url(&c).ends_with("&language=en&keyterm=NeboLoop&keyterm=Nebo%20Media&keyterm=Tymick%20%26%20Co"),
            "{}",
            stt_url(&c)
        );
        c.keyterms = crate::keyterms((0..150).map(|i| format!("Term {i}")));
        assert_eq!(stt_url(&c).matches("&keyterm=").count(), crate::MAX_KEYTERMS);
    }

    #[test]
    fn provider_frames_read_as_dictation_events() {
        assert_eq!(parse_event(r#"{"type":"transcript.created"}"#), Some(DictationEvent::Ready));
        assert_eq!(
            parse_event(r#"{"type":"transcript.partial","text":"Hello there.","is_final":true,"speech_final":false,"start":1.5,"duration":2.0}"#),
            Some(DictationEvent::Text { text: "Hello there.".into(), start: 1.5, end: 3.5, is_final: true })
        );
        assert_eq!(
            parse_event(r#"{"type":"transcript.partial","text":"hel","is_final":false,"start":0,"duration":0.5}"#),
            Some(DictationEvent::Text { text: "hel".into(), start: 0.0, end: 0.5, is_final: false })
        );
        assert_eq!(
            parse_event(r#"{"type":"transcript.done","text":"All of it.","duration":9.0}"#),
            Some(DictationEvent::Done { text: "All of it.".into() })
        );
        assert_eq!(
            parse_event(r#"{"type":"error","message":"bad audio"}"#),
            Some(DictationEvent::Error("bad audio".into()))
        );
        assert_eq!(parse_event(r#"{"type":"something.else"}"#), None);
        assert_eq!(parse_event("not json"), None);
    }

    #[test]
    fn the_phone_reads_its_own_vocabulary() {
        let t = DictationEvent::Text { text: "Hi.".into(), start: 0.0, end: 1.0, is_final: true };
        assert_eq!(t.to_json(), json!({"type":"text","text":"Hi.","start":0.0,"end":1.0,"final":true}));
        assert_eq!(DictationEvent::Ready.to_json(), json!({"type":"ready"}));
        assert_eq!(DictationEvent::Done { text: String::new() }.to_json(), json!({"type":"done","text":""}));
    }

    async fn served() -> (TcpListener, DictationConfig) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let c = cfg(format!("ws://{}/v1/stt", l.local_addr().unwrap()));
        (l, c)
    }

    async fn accept(l: &TcpListener) -> (Served, String, Vec<(String, String)>) {
        use std::sync::{Arc, Mutex};
        let (stream, _) = l.accept().await.unwrap();
        let seen = Arc::new(Mutex::new((String::new(), Vec::new())));
        let keep = seen.clone();
        let ws = tokio_tungstenite::accept_hdr_async(
            stream,
            move |req: &tokio_tungstenite::tungstenite::handshake::server::Request, resp| {
                let mut s = keep.lock().unwrap();
                s.0 = req.uri().to_string();
                s.1 = req
                    .headers()
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
                    .collect();
                Ok(resp)
            },
        )
        .await
        .unwrap();
        let (uri, headers) = seen.lock().unwrap().clone();
        (ws, uri, headers)
    }

    async fn say(ws: &mut Served, frame: Value) {
        ws.send(Message::Text(frame.to_string().into())).await.unwrap();
    }

    #[tokio::test]
    async fn audio_said_before_the_line_is_ready_goes_up_first_and_in_order() {
        let (l, c) = served().await;
        let (in_tx, in_rx) = mpsc::channel(64);
        let (ev_tx, mut ev_rx) = mpsc::channel(64);
        // The phone talks before the line is even dialled.
        in_tx.send(DictationInput::Audio(Bytes::from_static(b"one"))).await.unwrap();
        in_tx.send(DictationInput::Audio(Bytes::from_static(b"two"))).await.unwrap();
        let task = tokio::spawn(run(c, in_rx, ev_tx));
        let (mut far, uri, headers) = accept(&l).await;
        assert!(uri.contains("sample_rate=16000") && uri.contains("interim_results=true"), "{uri}");
        let header = |k: &str| headers.iter().find(|(h, _)| h == k).map(|(_, v)| v.clone());
        assert_eq!(header("authorization").as_deref(), Some("Bearer token"));
        assert_eq!(header("x-purpose").as_deref(), Some("transcribe"));
        assert_eq!(header("x-bot-id").as_deref(), Some("bot-1"));
        in_tx.send(DictationInput::Audio(Bytes::from_static(b"three"))).await.unwrap();
        // Nothing goes up before the far end is ready.
        say(&mut far, json!({"type":"transcript.created"})).await;
        assert_eq!(ev_rx.recv().await, Some(DictationEvent::Ready));
        let mut got = Vec::new();
        while got.len() < 3 {
            match far.next().await {
                Some(Ok(Message::Binary(b))) => got.push(b.to_vec()),
                Some(Ok(_)) => {}
                other => panic!("expected audio, got {other:?}"),
            }
        }
        assert_eq!(got, vec![b"one".to_vec(), b"two".to_vec(), b"three".to_vec()]);
        drop(in_tx);
        task.await.unwrap();
    }

    #[tokio::test]
    async fn partials_and_finals_come_down_and_the_stop_finishes_the_last_words() {
        let (l, c) = served().await;
        let (in_tx, in_rx) = mpsc::channel(64);
        let (ev_tx, mut ev_rx) = mpsc::channel(64);
        let task = tokio::spawn(run(c, in_rx, ev_tx));
        let (mut far, _, _) = accept(&l).await;
        say(&mut far, json!({"type":"transcript.created"})).await;
        assert_eq!(ev_rx.recv().await, Some(DictationEvent::Ready));
        say(&mut far, json!({"type":"transcript.partial","text":"hello","is_final":false,"start":0.0,"duration":0.6})).await;
        say(&mut far, json!({"type":"transcript.partial","text":"Hello there.","is_final":true,"start":0.0,"duration":1.2})).await;
        assert_eq!(ev_rx.recv().await, Some(DictationEvent::Text { text: "hello".into(), start: 0.0, end: 0.6, is_final: false }));
        assert_eq!(ev_rx.recv().await, Some(DictationEvent::Text { text: "Hello there.".into(), start: 0.0, end: 1.2, is_final: true }));
        in_tx.send(DictationInput::Stop).await.unwrap();
        // The far end ends the words being said, then hears the audio is over.
        let mut told = Vec::new();
        while told.len() < 2 {
            match far.next().await {
                Some(Ok(Message::Text(t))) => told.push(serde_json::from_str::<Value>(&t).unwrap()),
                Some(Ok(_)) => {}
                other => panic!("expected finalize then audio.done, got {other:?}"),
            }
        }
        assert_eq!(told, vec![json!({"type":"finalize"}), json!({"type":"audio.done"})]);
        say(&mut far, json!({"type":"transcript.partial","text":"Bye.","is_final":true,"start":1.2,"duration":0.5})).await;
        say(&mut far, json!({"type":"transcript.done","text":"Hello there. Bye.","duration":1.7})).await;
        assert_eq!(ev_rx.recv().await, Some(DictationEvent::Text { text: "Bye.".into(), start: 1.2, end: 1.7, is_final: true }));
        assert_eq!(ev_rx.recv().await, Some(DictationEvent::Done { text: "Hello there. Bye.".into() }));
        task.await.unwrap();
    }

    #[tokio::test]
    async fn a_line_that_drops_mid_dictation_says_so() {
        let (l, c) = served().await;
        let (_in_tx, in_rx) = mpsc::channel(64);
        let (ev_tx, mut ev_rx) = mpsc::channel(64);
        let task = tokio::spawn(run(c, in_rx, ev_tx));
        let (mut far, _, _) = accept(&l).await;
        say(&mut far, json!({"type":"transcript.created"})).await;
        assert_eq!(ev_rx.recv().await, Some(DictationEvent::Ready));
        drop(far);
        assert!(matches!(ev_rx.recv().await, Some(DictationEvent::Error(_))));
        task.await.unwrap();
    }

    #[tokio::test]
    async fn a_line_that_cannot_be_dialled_says_so() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let c = cfg(format!("ws://{}/v1/stt", l.local_addr().unwrap()));
        drop(l);
        let (_in_tx, in_rx) = mpsc::channel(64);
        let (ev_tx, mut ev_rx) = mpsc::channel(64);
        run(c, in_rx, ev_tx).await;
        assert!(matches!(ev_rx.recv().await, Some(DictationEvent::Error(_))));
    }

    /// A line refused while running work holds the balance, past the waits,
    /// tells the phone with the code it words in the owner's language and
    /// the English sentence for a phone that does not know it.
    #[tokio::test]
    async fn a_line_refused_for_a_held_balance_says_busy_with_its_code() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        const RESERVED: &str = r#"{"error":{"code":"BALANCE_RESERVED","message":"reserved"}}"#;
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let c = cfg(format!("ws://{}/v1/stt", l.local_addr().unwrap()));
        let far = tokio::spawn(async move {
            for _ in 0..=::types::balance::RESERVED_RETRIES {
                let (mut sock, _) = l.accept().await.unwrap();
                let mut buf = vec![0u8; 8192];
                let _ = sock.read(&mut buf).await;
                let reply = format!(
                    "HTTP/1.1 429 Too Many Requests\r\nretry-after: 0\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{RESERVED}",
                    RESERVED.len()
                );
                let _ = sock.write_all(reply.as_bytes()).await;
            }
        });
        let (_in_tx, in_rx) = mpsc::channel(64);
        let (ev_tx, mut ev_rx) = mpsc::channel(64);
        run(c, in_rx, ev_tx).await;
        let ended = ev_rx.recv().await;
        assert_eq!(ended, Some(DictationEvent::BalanceReserved));
        assert_eq!(
            ended.unwrap().to_json(),
            json!({"type": "error", "code": "balance_reserved", "message": ::types::balance::BALANCE_BUSY})
        );
        far.await.unwrap();
    }
}
