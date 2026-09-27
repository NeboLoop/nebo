//! Nebo as an Open Agent Link client (OAL, `spec/oal-0.1.md` in nebo-link):
//! how it reaches the agents of a linked bot, on another computer or on its
//! own.
//!
//! - [`Conn`]: one OAL connection, whole frames both ways, whatever carries
//!   it: an in-process connection to this computer's own host
//!   ([`oal_host::OalHost::connect_local`], nothing to carry and so nothing to
//!   encrypt), or an end-to-end encrypted session to another computer's host
//!   through a [`Relay`].
//! - [`Relay`]: NeboAI, which reaches a linked bot at `/t/<botId>/oal` with
//!   the Nebo bot's own token, or a self-hosted `oal-relay`.
//! - [`connect`]: Nebo is one device, with one key in one key store, paired
//!   with each linked bot it reaches. The first time (or after the bot forgot
//!   Nebo), it asks the bot for a pairing code through NeboAI's tunnel, which
//!   only the owner's own requests reach, and pairs with it at once
//!   ([`pair`]). The code is the bot's and works once; CPace and Noise turn it
//!   into keys that never leave the two ends, so NeboAI carries only
//!   ciphertext.

use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use futures::{Sink, Stream};
use link_core::model::{DeviceRef, ErrorObject, code};
use oal_host::OalHost;
use oal_secure::{KeyStore, PairingCode, Peer, PublicKey, Session, Side, Transport};
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::{AUTHORIZATION, SEC_WEBSOCKET_PROTOCOL};
use tracing::info;

/// The OAL version Nebo speaks.
pub const PROTOCOL: &str = "0.1";
/// How long a relay and a host may take to open a connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// How long a request on a connection may take to be answered (a first
/// `initialize` starts the agent's process).
const CALL_TIMEOUT: Duration = Duration::from_secs(60);

/// The Nebo bot's current NeboAI token, resolved per call (the hub rotates
/// it on every comms connect). `None` = not signed in.
pub type TokenSource = Arc<dyn Fn() -> Option<String> + Send + Sync>;

/// How Nebo reaches linked bots on other computers.
#[derive(Clone)]
pub enum Relay {
    /// NeboAI: `{api_url}/t/<botId>/oal`, with the Nebo bot's token, which
    /// the hub admits for a bot of the same owner. A bot Nebo hasn't paired
    /// with gives it a code on `{api_url}/t/<botId>/_link/oal/pair`.
    Hub { api_url: String, token: TokenSource },
    /// A self-hosted `oal-relay` at `url` (its base URL): hosts at
    /// `/oal/hosts/<botId>`, pairings at `/oal/pair/<nameplate>`. Nebo proves
    /// its device key to it on every connection. Pairing needs a code the
    /// owner got on the bot's computer (`nebo-link pair`).
    Oal { url: String },
}

/// Why a linked bot could not be reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unreached {
    /// Nothing answered, or not in a way Nebo can use: the owner reads
    /// "Could not connect to <name>. Try again."
    Offline,
    /// Not signed in to NeboAI.
    SignedOut,
    /// A refusal in its own words.
    Refused(String),
}

/// Who Nebo says it is to a host (`client`).
fn client() -> Value {
    json!({ "name": "nebo", "version": env!("CARGO_PKG_VERSION") })
}

fn protocol() -> Value {
    json!({ "min": PROTOCOL, "max": PROTOCOL })
}

/// One OAL connection: whole frames (JSON) both ways.
pub struct Conn {
    tx: mpsc::UnboundedSender<Vec<u8>>,
    rx: mpsc::UnboundedReceiver<Vec<u8>>,
    next_id: u64,
    /// The device this connection is authenticated as.
    pub device: String,
}

impl Conn {
    /// A connection to this computer's own host, in this process.
    pub fn local(oal: &Arc<OalHost>, device: DeviceRef) -> Self {
        let id = device.device_id.clone();
        let conn = oal.connect_local(device);
        Self {
            tx: conn.tx,
            rx: conn.rx,
            next_id: 0,
            device: id,
        }
    }

    /// A connection over an open encrypted session, authenticated as
    /// `device`.
    fn session<T: Transport + 'static>(session: Session<T>, device: String) -> Self {
        let (in_tx, rx) = mpsc::unbounded_channel();
        let (tx, mut out_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let (mut reader, mut writer) = session.split();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    frame = reader.recv() => match frame {
                        Ok(Some(frame)) => {
                            if in_tx.send(frame).is_err() {
                                break;
                            }
                        }
                        Ok(None) => break,
                        Err(e) => {
                            info!(error = %e, "linked: the connection to the linked bot ended");
                            break;
                        }
                    },
                    out = out_rx.recv() => match out {
                        Some(frame) => {
                            if writer.send(&frame).await.is_err() {
                                break;
                            }
                        }
                        None => {
                            let _ = writer.close().await;
                            break;
                        }
                    },
                }
            }
        });
        Self {
            tx,
            rx,
            next_id: 0,
            device,
        }
    }

    /// A fresh request id.
    pub fn id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// Sends one frame; `false` once the connection is gone.
    pub fn send(&self, frame: &Value) -> bool {
        self.tx.send(frame.to_string().into_bytes()).is_ok()
    }

    /// Sends a request on `agent`'s channel, or the host channel.
    pub fn request(&mut self, agent: Option<&str>, method: &str, params: Value) -> Option<u64> {
        let id = self.id();
        let msg = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        self.send(&frame(agent, msg)).then_some(id)
    }

    /// Answers a request the host sent on `agent`'s channel.
    pub fn respond(&self, agent: &str, id: &Value, result: Value) -> bool {
        self.send(&frame(Some(agent), json!({ "jsonrpc": "2.0", "id": id, "result": result })))
    }

    /// The next frame; `None` once the connection is gone. Anything not JSON
    /// is dropped (spec 4.2).
    pub async fn next(&mut self) -> Option<Value> {
        loop {
            let bytes = self.rx.recv().await?;
            if let Ok(frame) = serde_json::from_slice(&bytes) {
                return Some(frame);
            }
        }
    }

    /// Sends a request and waits for its answer. Every other frame read
    /// meanwhile comes back with it, in order. `None` when the connection is
    /// gone or the answer took too long.
    pub async fn call(&mut self, agent: Option<&str>, method: &str, params: Value) -> Option<(Result<Value, ErrorObject>, Vec<Value>)> {
        let id = self.request(agent, method, params)?;
        let mut others = Vec::new();
        let deadline = tokio::time::Instant::now() + CALL_TIMEOUT;
        loop {
            let frame = tokio::time::timeout_at(deadline, self.next()).await.ok()??;
            let msg = match agent {
                Some(agent) if frame["agent"] == agent => &frame["acp"],
                None if frame.get("agent").is_none() => &frame,
                _ => {
                    others.push(frame);
                    continue;
                }
            };
            if msg.get("method").is_none() && msg["id"] == json!(id) {
                return Some((answer(msg), others));
            }
            others.push(frame);
        }
    }
}

/// A message on `agent`'s channel, or on the host channel.
fn frame(agent: Option<&str>, msg: Value) -> Value {
    match agent {
        Some(agent) => json!({ "agent": agent, "acp": msg }),
        None => msg,
    }
}

/// A JSON-RPC response's result or error.
pub fn answer(msg: &Value) -> Result<Value, ErrorObject> {
    match msg.get("error") {
        Some(error) => Err(serde_json::from_value(error.clone())
            .unwrap_or_else(|_| ErrorObject::new(code::INTERNAL, "The linked bot answered with an error it didn't explain."))),
        None => Ok(msg["result"].clone()),
    }
}

/// A connection to the linked bot `bot` through `relay`, as the device
/// Nebo's key store names for it: a session with the keys pinned at pairing,
/// or, the first time (or once the bot forgot Nebo), a pairing with a code
/// the bot gives through NeboAI. `device_name` is what the bot will call
/// Nebo.
pub async fn connect(relay: &Relay, keys: &KeyStore, bot: &str, device_name: &str) -> Result<Conn, Unreached> {
    let paired = |keys: &KeyStore| keys.peers().into_iter().find(|p| p.side == Side::Host && p.id == bot);
    if let Some(peer) = paired(keys) {
        match session(relay, keys, &peer).await {
            Ok(conn) => return Ok(conn),
            Err(Opened::Forgotten) => {
                info!(bot, "linked: the linked bot no longer knows Nebo; pairing again");
                let _ = keys.revoke(&peer.public_key);
            }
            Err(Opened::Failed(why)) => return Err(why),
        }
    }
    // One pairing at a time: turns that all found a bot unpaired pair once
    // (a second code would replace the first one's).
    let _one = PAIRING.lock().await;
    if let Some(peer) = paired(keys) {
        return session(relay, keys, &peer).await.map_err(|e| match e {
            Opened::Failed(why) => why,
            Opened::Forgotten => Unreached::Offline,
        });
    }
    let code = relay.code(bot).await?;
    pair(relay, keys, bot, &code, device_name).await
}

/// Held while Nebo pairs with a bot.
static PAIRING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Pairs Nebo with the linked bot `bot` using `code` (spec 6, 17.5): CPace
/// and Noise with the code, `host/pair` inside, both static keys checked
/// against the handshake's. The pairing connection stays open as Nebo's
/// first connection to the bot.
pub async fn pair(relay: &Relay, keys: &KeyStore, bot: &str, code: &PairingCode, device_name: &str) -> Result<Conn, Unreached> {
    let (transport, _) = relay.socket(keys, bot, Some(code.nameplate())).await?;
    let mut pairing = match tokio::time::timeout(CONNECT_TIMEOUT, oal_secure::pair(transport, code, keys, Side::Client)).await {
        Ok(Ok(pairing)) => pairing,
        Ok(Err(e)) => {
            info!(bot, error = %e, "linked: pairing with the linked bot failed");
            return Err(Unreached::Offline);
        }
        Err(_) => return Err(Unreached::Offline),
    };
    let request = json!({ "jsonrpc": "2.0", "id": 1, "method": "host/pair", "params": {
        "protocol": protocol(),
        "client": client(),
        "code": code.to_string(),
        "device": { "name": device_name, "publicKey": keys.public_key().to_string() },
    } });
    if pairing.send(request.to_string().as_bytes()).await.is_err() {
        return Err(Unreached::Offline);
    }
    let answered: Value = match tokio::time::timeout(CONNECT_TIMEOUT, pairing.recv()).await {
        Ok(Ok(Some(bytes))) => serde_json::from_slice(&bytes).unwrap_or_default(),
        _ => return Err(Unreached::Offline),
    };
    let result = match answer(&answered) {
        Ok(result) => result,
        Err(e) => {
            info!(bot, code = e.code, message = %e.message, "linked: the linked bot refused the pairing");
            return Err(Unreached::Offline);
        }
    };
    let host = &result["info"]["host"];
    let key: Option<PublicKey> = host["publicKey"].as_str().and_then(|k| k.parse().ok());
    let (Some(key), Some(device)) = (key, result["device"]["id"].as_str()) else {
        return Err(Unreached::Offline);
    };
    if host["id"] != bot {
        info!(bot, host = %host["id"], "linked: a different host answered the pairing");
        return Err(Unreached::Offline);
    }
    let name = host["name"].as_str().unwrap_or(bot);
    let session = pairing.finish(&key, bot, name, device).map_err(|e| {
        info!(bot, error = %e, "linked: the pairing's keys did not match");
        Unreached::Offline
    })?;
    info!(bot, device, "linked: Nebo paired with the linked bot");
    Ok(Conn::session(session, device.to_owned()))
}

/// Why a session didn't open.
enum Opened {
    /// The host doesn't know Nebo's key (unpaired, or its key changed): pair
    /// again.
    Forgotten,
    Failed(Unreached),
}

/// A session with the host `peer` (spec 17.2): Noise IK with the pinned
/// keys, the hello's version checked in the answer.
async fn session(relay: &Relay, keys: &KeyStore, peer: &Peer) -> Result<Conn, Opened> {
    let (transport, closed) = relay.socket(keys, &peer.id, None).await.map_err(Opened::Failed)?;
    let hello = json!({ "protocol": protocol(), "client": client() });
    let opened = tokio::time::timeout(CONNECT_TIMEOUT, oal_secure::connect(transport, keys, peer, hello.to_string().as_bytes())).await;
    let (session, reply) = match opened {
        Ok(Ok(opened)) => opened,
        Ok(Err(e)) => {
            let code = *closed.lock().expect("close code");
            info!(bot = %peer.id, error = %e, ?code, "linked: the session did not open");
            return Err(match (&e, code) {
                (oal_secure::Error::Authentication | oal_secure::Error::UnknownPeer | oal_secure::Error::KeyRetired, _) => Opened::Forgotten,
                (_, Some(4001 | 4003)) => Opened::Forgotten,
                _ => Opened::Failed(Unreached::Offline),
            });
        }
        Err(_) => return Err(Opened::Failed(Unreached::Offline)),
    };
    let reply: Value = serde_json::from_slice(&reply).unwrap_or_default();
    if let Some(error) = reply.get("error") {
        info!(bot = %peer.id, %error, "linked: the linked bot refused the session");
        return Err(Opened::Failed(Unreached::Offline));
    }
    let device = reply["device"]["id"].as_str().unwrap_or(&peer.local_id).to_owned();
    Ok(Conn::session(session, device))
}

impl Relay {
    /// A WebSocket to the host `bot` (a pairing one through `nameplate`),
    /// as `oal_secure`'s transport, with the close code it ends with.
    async fn socket(&self, keys: &KeyStore, bot: &str, nameplate: Option<&str>) -> Result<(Binary, Closed), Unreached> {
        match self {
            Relay::Hub { api_url, token } => {
                let token = token().ok_or(Unreached::SignedOut)?;
                let url = format!("{}/t/{bot}/oal", ws_base(api_url));
                let mut request = url.as_str().into_client_request().map_err(|e| {
                    info!(bot, error = %e, "linked: the linked bot's address is not valid");
                    Unreached::Offline
                })?;
                let bearer = format!("Bearer {token}").parse().map_err(|_| Unreached::Offline)?;
                request.headers_mut().insert(AUTHORIZATION, bearer);
                request.headers_mut().insert(SEC_WEBSOCKET_PROTOCOL, "oal".parse().expect("header"));
                match tokio::time::timeout(CONNECT_TIMEOUT, tls::connect_ws(request)).await {
                    Ok(Ok((ws, _))) => Ok(Binary::new(ws)),
                    Ok(Err(e)) => {
                        info!(bot, error = %e, "linked: the linked bot's socket did not connect");
                        Err(Unreached::Offline)
                    }
                    Err(_) => Err(Unreached::Offline),
                }
            }
            Relay::Oal { url } => {
                let relay = oal_relay::RelayClient::new(url, oal_relay::Keypair::from_secret(*keys.secret())).map_err(|e| {
                    info!(bot, error = %e, "linked: the relay's address is not valid");
                    Unreached::Offline
                })?;
                let dialed = match nameplate {
                    Some(nameplate) => tokio::time::timeout(CONNECT_TIMEOUT, async { relay.pair(nameplate).await.map(|(ws, _)| ws) }).await,
                    None => tokio::time::timeout(CONNECT_TIMEOUT, relay.connect(bot)).await,
                };
                match dialed {
                    Ok(Ok(ws)) => Ok(Binary::new(ws)),
                    Ok(Err(e)) => {
                        info!(bot, error = %e, "linked: the relay did not reach the linked bot");
                        Err(Unreached::Offline)
                    }
                    Err(_) => Err(Unreached::Offline),
                }
            }
        }
    }

    /// A pairing code from the bot, for Nebo to pair with at once. Through
    /// NeboAI, the bot gives one to the owner's own requests; a self-hosted
    /// relay carries no such request, so the owner gets one on the bot's
    /// computer.
    async fn code(&self, bot: &str) -> Result<PairingCode, Unreached> {
        let (api_url, token) = match self {
            Relay::Hub { api_url, token } => (api_url, token().ok_or(Unreached::SignedOut)?),
            Relay::Oal { .. } => {
                info!(bot, "linked: Nebo isn't paired with this bot through its relay");
                return Err(Unreached::Offline);
            }
        };
        let response = crate::http::request_client()
            .post(format!("{api_url}/t/{bot}/_link/oal/pair"))
            .bearer_auth(token)
            .timeout(CONNECT_TIMEOUT)
            .send()
            .await
            .map_err(|e| {
                info!(bot, error = %e, "linked: asking the linked bot for a pairing code did not connect");
                Unreached::Offline
            })?;
        let status = response.status().as_u16();
        let body: Value = response.json().await.unwrap_or(Value::Null);
        if status == 404 {
            info!(bot, "linked: the linked bot's nebo-link does not speak Open Agent Link yet; it updates itself");
            return Err(Unreached::Offline);
        }
        match body["code"].as_str().map(PairingCode::parse) {
            Some(Ok(code)) if (200..300).contains(&status) => Ok(code),
            _ => {
                info!(bot, status, %body, "linked: the linked bot gave no pairing code");
                Err(Unreached::Offline)
            }
        }
    }
}

/// The socket base for an API base: `https://…` → `wss://…`.
pub fn ws_base(api_url: &str) -> String {
    if let Some(rest) = api_url.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = api_url.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        api_url.to_owned()
    }
}

/// The code a WebSocket closed with.
type Closed = Arc<Mutex<Option<u16>>>;

/// A WebSocket's binary messages as `oal_secure`'s transport (spec 17.3:
/// each Noise message is one binary message), keeping the close code.
struct Binary {
    socket: Pin<Box<dyn WsLike>>,
    closed: Closed,
}

trait WsLike: Stream<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>> + Sink<WsMessage, Error = tokio_tungstenite::tungstenite::Error> + Send {}

impl<T> WsLike for T where T: Stream<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>> + Sink<WsMessage, Error = tokio_tungstenite::tungstenite::Error> + Send {}

impl Binary {
    fn new<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(socket: WebSocketStream<S>) -> (Self, Closed) {
        let closed = Closed::default();
        (
            Self {
                socket: Box::pin(socket),
                closed: closed.clone(),
            },
            closed,
        )
    }
}

impl Stream for Binary {
    type Item = io::Result<Vec<u8>>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            return Poll::Ready(match futures::ready!(self.socket.as_mut().poll_next(cx)) {
                Some(Ok(WsMessage::Binary(bytes))) => Some(Ok(bytes.to_vec())),
                Some(Ok(WsMessage::Close(frame))) => {
                    *self.closed.lock().expect("close code") = frame.map(|f| u16::from(f.code));
                    None
                }
                Some(Ok(_)) => continue,
                Some(Err(e)) => Some(Err(io::Error::other(e))),
                None => None,
            });
        }
    }
}

impl Sink<Vec<u8>> for Binary {
    type Error = io::Error;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.socket.as_mut().poll_ready(cx).map_err(io::Error::other)
    }

    fn start_send(mut self: Pin<&mut Self>, item: Vec<u8>) -> io::Result<()> {
        self.socket.as_mut().start_send(WsMessage::Binary(item.into())).map_err(io::Error::other)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.socket.as_mut().poll_flush(cx).map_err(io::Error::other)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.socket.as_mut().poll_close(cx).map_err(io::Error::other)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_bases_follow_the_api() {
        assert_eq!(ws_base("https://api.neboai.com"), "wss://api.neboai.com");
        assert_eq!(ws_base("http://127.0.0.1:1"), "ws://127.0.0.1:1");
    }

    #[test]
    fn answers_are_results_or_errors() {
        assert_eq!(answer(&json!({ "id": 1, "result": { "ok": true } })), Ok(json!({ "ok": true })));
        let refused = answer(&json!({ "id": 1, "error": { "code": -33006, "message": "still working" } })).unwrap_err();
        assert_eq!((refused.code, refused.message.as_str()), (code::TURN_IN_PROGRESS, "still working"));
    }
}
