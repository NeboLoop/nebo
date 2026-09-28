//! Nebo as an Open Agent Link client (OAL, `spec/oal-0.1.md` in nebo-link):
//! how it reaches the agents of a linked bot, on another computer or on its
//! own.
//!
//! - [`Conn`]: one OAL connection, whole frames both ways, whatever carries
//!   it: an in-process connection to this computer's own host
//!   ([`oal_host::OalHost::connect_local`], nothing to carry and so nothing to
//!   encrypt), or an end-to-end encrypted session to a linked bot's host
//!   ([`Via`]: directly, or through a [`Relay`]).
//! - [`Direct`]: the ways to a linked bot's host with nothing between: on
//!   this computer, where this OS user's nebo-link daemon serves its bot on
//!   loopback (`link_core::machine::direct`), then on the LAN, where a host
//!   the owner turned LAN direct on for is heard by DNS-SD and its
//!   certificate was learned on an earlier connection (spec 4.5). Nebo tries
//!   them first, for every connection and every reconnect, and the relay
//!   only when none answers: a relay that fails never strands a turn with a
//!   host Nebo can reach itself.
//! - [`Relay`]: NeboAI, which reaches a linked bot at `/t/<botId>/oal` with
//!   the Nebo bot's own token, or a self-hosted `oal-relay`.
//! - [`connect`]: Nebo is one device, with one key in one key store, paired
//!   with each linked bot it reaches. The first time (or after the bot forgot
//!   Nebo), it asks the bot for a pairing code through NeboAI's tunnel, which
//!   only the owner's own requests reach, and pairs with it at once
//!   ([`pair`]). The code is the bot's and works once; CPace and Noise turn it
//!   into keys that never leave the two ends, so NeboAI carries only
//!   ciphertext. The same session runs over any way: only what carries its
//!   bytes differs.

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, Poll};
use std::time::Duration;

use futures::future::BoxFuture;
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
/// How long a host reached directly (on this computer or the LAN) may take
/// to open a connection before Nebo goes the next way.
const DIRECT_TIMEOUT: Duration = Duration::from_secs(5);
/// How long Nebo listens on the LAN, once, before a host not heard is taken
/// to be elsewhere.
const LAN_WAIT: Duration = Duration::from_millis(1500);
/// How long `host/info` may take on a new connection through the relay.
const INFO_TIMEOUT: Duration = Duration::from_secs(5);
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

/// How a connection reached the linked bot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    /// Nebo's own host, in this process.
    Local,
    /// The bot's host on this computer (this OS user's nebo-link daemon).
    Direct,
    /// The bot's host on the LAN.
    Lan,
    /// Through the relay.
    Relay,
}

impl fmt::Display for Via {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Via::Local => "local",
            Via::Direct => "direct",
            Via::Lan => "lan",
            Via::Relay => "relay",
        })
    }
}

/// Finds a host on the LAN by its id: where it listens, as last heard.
pub type Finder = Arc<dyn Fn(String) -> BoxFuture<'static, Vec<SocketAddr>> + Send + Sync>;

/// The ways to a linked bot's host with nothing between (spec 4.5), tried
/// before the relay.
#[derive(Clone)]
pub struct Direct {
    /// Where this OS user's nebo-link daemon keeps its state
    /// ([`link_core::machine::daemon_home`]): a bot it serves is reached on
    /// this computer.
    daemon_home: Option<PathBuf>,
    /// Each linked bot's LAN direct certificate, as `host/info` named it on an
    /// earlier connection, by bot id.
    lan_file: PathBuf,
    find: Finder,
}

/// One way to a host with nothing between.
struct Way {
    via: Via,
    addr: SocketAddr,
    fingerprint: String,
}

/// Held while the LAN file is read and written.
static LAN_FILE: Mutex<()> = Mutex::new(());

impl Direct {
    /// The ways to a linked bot's host with nothing between: this OS user's
    /// daemon in `daemon_home`, and the LAN, whose hosts' certificates are
    /// kept in `lan_file`.
    pub fn new(daemon_home: Option<PathBuf>, lan_file: PathBuf) -> Self {
        Self {
            daemon_home,
            lan_file,
            find: Arc::new(|host| Box::pin(async move { browse(&host).await })),
        }
    }

    /// The ways to the bot's host, looked for as they are needed.
    fn ways<'a>(&'a self, bot: &'a str) -> Ways<'a> {
        Ways {
            direct: self,
            bot,
            pending: VecDeque::new(),
            looked: 0,
        }
    }

    /// The bot's host on this computer, while this OS user's daemon serves it.
    fn machine(&self, bot: &str) -> Option<Way> {
        let direct = link_core::machine::direct(self.daemon_home.as_deref()?, bot)?;
        Some(Way {
            via: Via::Direct,
            addr: direct.addr,
            fingerprint: direct.fingerprint,
        })
    }

    /// The bot's host on the LAN: where it is heard, when its certificate is
    /// known. A host whose certificate Nebo never learned is not looked for.
    async fn lan(&self, bot: &str) -> Vec<Way> {
        let Some(fingerprint) = self.certificates().remove(bot) else {
            return Vec::new();
        };
        (self.find)(bot.to_owned())
            .await
            .into_iter()
            .map(|addr| Way {
                via: Via::Lan,
                addr,
                fingerprint: fingerprint.clone(),
            })
            .collect()
    }

    fn certificates(&self) -> BTreeMap<String, String> {
        let _held = LAN_FILE.lock().expect("lan file");
        std::fs::read_to_string(&self.lan_file)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Learns, on a connection through the relay, whether the bot serves LAN
    /// direct and with which certificate (`host/info`, spec 4.5), for the
    /// next connection to try the LAN first.
    async fn learn(&self, bot: &str, conn: &mut Conn) {
        let asked = tokio::time::timeout(INFO_TIMEOUT, conn.call(None, "host/info", json!({}))).await;
        let Ok(Some((Ok(info), _))) = asked else {
            return;
        };
        let fingerprint = info["host"]["tlsFingerprint"].as_str();
        let _held = LAN_FILE.lock().expect("lan file");
        let mut known: BTreeMap<String, String> = std::fs::read_to_string(&self.lan_file)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        if known.get(bot).map(String::as_str) == fingerprint {
            return;
        }
        match fingerprint {
            Some(fingerprint) => known.insert(bot.to_owned(), fingerprint.to_owned()),
            None => known.remove(bot),
        };
        let written = self
            .lan_file
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&self.lan_file, serde_json::to_vec_pretty(&known).expect("serializes")));
        match written {
            Ok(()) => info!(bot, lan = fingerprint.is_some(), "linked: learned whether the linked bot serves the LAN"),
            Err(e) => info!(bot, error = %e, "linked: could not keep the linked bot's LAN certificate"),
        }
    }
}

/// The ways to a bot's host with nothing between, in the order Nebo tries
/// them: this computer, then the LAN (listened for only once this computer
/// has no way).
struct Ways<'a> {
    direct: &'a Direct,
    bot: &'a str,
    pending: VecDeque<Way>,
    /// 0: nothing looked at yet; 1: this computer; 2: the LAN too.
    looked: u8,
}

impl Ways<'_> {
    async fn next(&mut self) -> Option<Way> {
        loop {
            if let Some(way) = self.pending.pop_front() {
                return Some(way);
            }
            self.looked += 1;
            match self.looked {
                1 => self.pending.extend(self.direct.machine(self.bot)),
                2 => self.pending.extend(self.direct.lan(self.bot).await),
                _ => return None,
            }
        }
    }
}

impl Way {
    /// A WebSocket to the host this way, its certificate pinned.
    async fn dial(&self, bot: &str) -> Option<(Binary, Closed)> {
        match tokio::time::timeout(DIRECT_TIMEOUT, oal_host::lan::dial(self.addr, &self.fingerprint)).await {
            Ok(Ok(ws)) => Some(Binary::new(ws)),
            Ok(Err(e)) => {
                info!(bot, via = %self.via, addr = %self.addr, error = %e, "linked: the linked bot did not answer this way");
                None
            }
            Err(_) => {
                info!(bot, via = %self.via, addr = %self.addr, "linked: the linked bot did not answer this way in time");
                None
            }
        }
    }
}

/// The LAN, listened to from the first time Nebo looks for a host there.
async fn browse(host: &str) -> Vec<SocketAddr> {
    static BROWSER: OnceLock<Option<oal_host::lan::Browser>> = OnceLock::new();
    let browser = BROWSER.get_or_init(|| {
        oal_host::lan::Browser::start()
            .inspect_err(|e| info!(error = %e, "linked: DNS-SD is not available; linked bots are not looked for on the LAN"))
            .ok()
    });
    match browser {
        Some(browser) => browser.find(host, LAN_WAIT).await,
        None => Vec::new(),
    }
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
    /// How it reached the bot.
    pub via: Via,
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
            via: Via::Local,
        }
    }

    /// A connection over an open encrypted session, authenticated as
    /// `device`, that reached the bot `via`.
    fn session<T: Transport + 'static>(session: Session<T>, device: String, via: Via) -> Self {
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
            via,
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

/// A connection to the linked bot `bot`, directly when it can ([`Direct`]),
/// else through `relay`, as the device Nebo's key store names for it: a
/// session with the keys pinned at pairing, or, the first time (or once the
/// bot forgot Nebo), a pairing with a code the bot gives through NeboAI.
/// `device_name` is what the bot will call Nebo.
pub async fn connect(relay: &Relay, direct: &Direct, keys: &KeyStore, bot: &str, device_name: &str) -> Result<Conn, Unreached> {
    let paired = |keys: &KeyStore| keys.peers().into_iter().find(|p| p.side == Side::Host && p.id == bot);
    if let Some(peer) = paired(keys) {
        match session(relay, direct, keys, &peer).await {
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
        return session(relay, direct, keys, &peer).await.map_err(|e| match e {
            Opened::Failed(why) => why,
            Opened::Forgotten => Unreached::Offline,
        });
    }
    let code = relay.code(bot).await?;
    pair(relay, direct, keys, bot, &code, device_name).await
}

/// Held while Nebo pairs with a bot.
static PAIRING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Pairs Nebo with the linked bot `bot` using `code` (spec 6, 17.5): CPace
/// and Noise with the code, `host/pair` inside, both static keys checked
/// against the handshake's, on the first way to the bot that answers (the
/// relay last). The pairing connection stays open as Nebo's first
/// connection to the bot.
pub async fn pair(relay: &Relay, direct: &Direct, keys: &KeyStore, bot: &str, code: &PairingCode, device_name: &str) -> Result<Conn, Unreached> {
    let mut ways = direct.ways(bot);
    let mut opened = None;
    while let Some(way) = ways.next().await {
        if let Some((transport, _)) = way.dial(bot).await {
            opened = Some((transport, way.via));
            break;
        }
    }
    let (transport, via) = match opened {
        Some(opened) => opened,
        None => (relay.socket(keys, bot, Some(code.nameplate())).await?.0, Via::Relay),
    };
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
    info!(bot, device, %via, "linked: Nebo paired with the linked bot");
    info!(bot, %via, "linked: connected {via}");
    let mut conn = Conn::session(session, device.to_owned(), via);
    if via == Via::Relay {
        direct.learn(bot, &mut conn).await;
    }
    Ok(conn)
}

/// Why a session didn't open.
enum Opened {
    /// The host doesn't know Nebo's key (unpaired, or its key changed): pair
    /// again.
    Forgotten,
    Failed(Unreached),
}

/// A session with the host `peer`, on the first way to it that opens one:
/// this computer, the LAN, then the relay. A host that no longer knows Nebo
/// says so the same whichever way it is reached, so that ends the search.
async fn session(relay: &Relay, direct: &Direct, keys: &KeyStore, peer: &Peer) -> Result<Conn, Opened> {
    let mut ways = direct.ways(&peer.id);
    while let Some(way) = ways.next().await {
        let Some((transport, closed)) = way.dial(&peer.id).await else {
            continue;
        };
        match handshake(transport, closed, keys, peer, way.via, DIRECT_TIMEOUT).await {
            Err(Opened::Failed(_)) => continue,
            opened => return opened,
        }
    }
    let (transport, closed) = relay.socket(keys, &peer.id, None).await.map_err(Opened::Failed)?;
    let mut conn = handshake(transport, closed, keys, peer, Via::Relay, CONNECT_TIMEOUT).await?;
    direct.learn(&peer.id, &mut conn).await;
    Ok(conn)
}

/// The session's handshake (spec 17.2) on a socket that reached the host
/// `via`: Noise IK with the pinned keys, the hello's version checked in the
/// answer.
async fn handshake(transport: Binary, closed: Closed, keys: &KeyStore, peer: &Peer, via: Via, timeout: Duration) -> Result<Conn, Opened> {
    let hello = json!({ "protocol": protocol(), "client": client() });
    let opened = tokio::time::timeout(timeout, oal_secure::connect(transport, keys, peer, hello.to_string().as_bytes())).await;
    let (session, reply) = match opened {
        Ok(Ok(opened)) => opened,
        Ok(Err(e)) => {
            let code = *closed.lock().expect("close code");
            info!(bot = %peer.id, %via, error = %e, ?code, "linked: the session did not open");
            return Err(match (&e, code) {
                (oal_secure::Error::Authentication | oal_secure::Error::UnknownPeer | oal_secure::Error::KeyRetired, _) => Opened::Forgotten,
                (_, Some(4001 | 4003)) => Opened::Forgotten,
                _ => Opened::Failed(Unreached::Offline),
            });
        }
        Err(_) => {
            info!(bot = %peer.id, %via, "linked: the session did not open in time");
            return Err(Opened::Failed(Unreached::Offline));
        }
    };
    let reply: Value = serde_json::from_slice(&reply).unwrap_or_default();
    if let Some(error) = reply.get("error") {
        info!(bot = %peer.id, %via, %error, "linked: the linked bot refused the session");
        return Err(Opened::Failed(Unreached::Offline));
    }
    let device = reply["device"]["id"].as_str().unwrap_or(&peer.local_id).to_owned();
    info!(bot = %peer.id, %via, "linked: connected {via}");
    Ok(Conn::session(session, device, via))
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

    use std::sync::atomic::{AtomicUsize, Ordering};

    use oal_host::lan::{self, Reach};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    use super::super::local_host::LocalHost;

    const BOT: &str = "b-linked";

    /// A linked bot's host, as its nebo-link daemon runs it, its state in
    /// `dir`: opened again on the same state, it is the same host (a daemon
    /// that restarted).
    fn bot_host(dir: &std::path::Path) -> Arc<OalHost> {
        let host = LocalHost::open(Arc::new(|| Some(BOT.to_owned())), dir.join("link"), dir.join("home"), None).unwrap();
        host.oal().unwrap()
    }

    /// What the relay does with a connection to the bot.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Mode {
        Up,
        /// Answers 502, as NeboAI did when its relay blipped.
        Fails,
        /// Takes the connection and never answers.
        Hangs,
    }

    /// NeboAI's side, as a test sees it: `/t/<bot>/oal` carried to the bot's
    /// host (or failing, as `mode` says), the bot's pairing codes, and how
    /// many sockets it was asked for.
    struct Hub {
        url: String,
        host: Arc<Mutex<Arc<OalHost>>>,
        mode: Arc<Mutex<Mode>>,
        sockets: Arc<AtomicUsize>,
    }

    impl Hub {
        async fn start(host: Arc<OalHost>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let hub = Self {
                url: format!("http://{}", listener.local_addr().unwrap()),
                host: Arc::new(Mutex::new(host)),
                mode: Arc::new(Mutex::new(Mode::Up)),
                sockets: Arc::default(),
            };
            let (host, mode, sockets) = (hub.host.clone(), hub.mode.clone(), hub.sockets.clone());
            tokio::spawn(async move {
                loop {
                    let (stream, _) = listener.accept().await.unwrap();
                    let (host, mode, sockets) = (host.lock().unwrap().clone(), *mode.lock().unwrap(), sockets.clone());
                    tokio::spawn(serve_hub(stream, host, mode, sockets));
                }
            });
            hub
        }

        fn relay(&self) -> Relay {
            Relay::Hub { api_url: self.url.clone(), token: Arc::new(|| Some("bot-jwt".to_owned())) }
        }

        fn set(&self, mode: Mode) {
            *self.mode.lock().unwrap() = mode;
        }

        fn reach(&self, host: Arc<OalHost>) {
            *self.host.lock().unwrap() = host;
        }

        fn sockets(&self) -> usize {
            self.sockets.load(Ordering::SeqCst)
        }
    }

    async fn serve_hub(mut stream: TcpStream, oal: Arc<OalHost>, mode: Mode, sockets: Arc<AtomicUsize>) {
        let mut head = [0u8; 2048];
        let n = stream.peek(&mut head).await.unwrap();
        let head = String::from_utf8_lossy(&head[..n]).into_owned();
        let target = head.split_whitespace().nth(1).unwrap_or("").to_owned();
        if target == format!("/t/{BOT}/oal") {
            sockets.fetch_add(1, Ordering::SeqCst);
            match mode {
                Mode::Up => {
                    let oal_protocol = |_: &tokio_tungstenite::tungstenite::handshake::server::Request,
                                        mut resp: tokio_tungstenite::tungstenite::handshake::server::Response| {
                        resp.headers_mut().insert(SEC_WEBSOCKET_PROTOCOL, "oal".parse().unwrap());
                        Ok(resp)
                    };
                    let ws = tokio_tungstenite::accept_hdr_async(stream, oal_protocol).await.unwrap();
                    oal.serve(oal_host::wire::websocket(ws), oal_host::Via::Tunnel).await;
                }
                Mode::Fails => {
                    let _ = stream.read(&mut [0u8; 2048]).await;
                    let _ = stream.write_all(b"HTTP/1.1 502 Bad Gateway\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").await;
                }
                Mode::Hangs => tokio::time::sleep(Duration::from_secs(3600)).await,
            }
            return;
        }
        let _ = stream.read(&mut [0u8; 4096]).await;
        let body = match target == format!("/t/{BOT}/_link/oal/pair") {
            true => json!({ "code": oal.pairing_code().await.unwrap().to_string(), "hostId": BOT }).to_string(),
            false => "{}".to_owned(),
        };
        let status = if body == "{}" { "404 Not Found" } else { "200 OK" };
        let response = format!("HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
        let _ = stream.write_all(response.as_bytes()).await;
    }

    /// Nebo's side: its keys, and the ways it knows besides the relay (the
    /// daemon's state in `dir/nebo-link`, the LAN through `finder`).
    struct Nebo {
        keys: KeyStore,
        direct: Direct,
        daemon_home: PathBuf,
        /// How many times it looked for the bot on the LAN.
        looked: Arc<AtomicUsize>,
    }

    fn nebo(dir: &std::path::Path, lan: Vec<SocketAddr>) -> Nebo {
        let looked = Arc::new(AtomicUsize::new(0));
        let counted = looked.clone();
        let daemon_home = dir.join("nebo-link");
        Nebo {
            keys: KeyStore::open(dir.join("oal")).unwrap(),
            direct: Direct {
                daemon_home: Some(daemon_home.clone()),
                lan_file: dir.join("oal-lan.json"),
                find: Arc::new(move |host| {
                    assert_eq!(host, BOT);
                    counted.fetch_add(1, Ordering::SeqCst);
                    let lan = lan.clone();
                    Box::pin(async move { lan })
                }),
            },
            daemon_home,
            looked,
        }
    }

    impl Nebo {
        async fn connect(&self, hub: &Hub) -> Result<Conn, Unreached> {
            let connected = tokio::time::timeout(Duration::from_secs(15), connect(&hub.relay(), &self.direct, &self.keys, BOT, "Nebo on test")).await;
            connected.expect("connected without waiting on the relay")
        }

        /// The bot's daemon serves its host on this computer, as `oal`.
        async fn daemon_serves(&self, oal: &Arc<OalHost>, dir: &std::path::Path) -> link_core::machine::Direct {
            let serving = lan::serve(oal.clone(), "127.0.0.1:0".parse().unwrap(), &dir.join("oal"), Reach::Machine).await.unwrap();
            let direct = link_core::machine::Direct { addr: serving.addr, fingerprint: serving.fingerprint };
            link_core::machine::record_direct(&self.daemon_home.join(BOT), &direct, true).unwrap();
            direct
        }
    }

    /// The connection answers: the host's agents, asked on it.
    async fn answers(conn: &mut Conn) {
        let (answer, _) = conn.call(None, "host/agents", json!({})).await.expect("an answer");
        assert!(answer.unwrap()["agents"].is_array());
    }

    /// The bot's nebo-link daemon runs on this computer: Nebo pairs with it
    /// and opens every session on this computer, never through the relay,
    /// even while the relay fails or hangs.
    #[tokio::test]
    async fn a_bot_on_this_computer_is_reached_directly_never_through_the_relay() {
        let root = tempfile::tempdir().unwrap();
        let bot_dir = root.path().join("bot");
        let oal = bot_host(&bot_dir);
        let hub = Hub::start(oal.clone()).await;
        let nebo = nebo(&root.path().join("nebo"), Vec::new());
        nebo.daemon_serves(&oal, &bot_dir).await;

        // The pairing code comes through NeboAI; the pairing itself doesn't.
        hub.set(Mode::Fails);
        let mut conn = nebo.connect(&hub).await.unwrap();
        assert_eq!(conn.via, Via::Direct);
        answers(&mut conn).await;
        assert_eq!(oal.devices().len(), 1, "paired");

        hub.set(Mode::Hangs);
        let mut again = nebo.connect(&hub).await.unwrap();
        assert_eq!(again.via, Via::Direct);
        answers(&mut again).await;
        assert_eq!(hub.sockets(), 0, "the relay was never asked for a socket");
        assert_eq!(nebo.looked.load(Ordering::SeqCst), 0, "nor the LAN looked at");
    }

    /// A bot on another computer that serves LAN direct: Nebo learns its
    /// certificate on a connection through the relay, then reaches it on the
    /// LAN, the relay failing or not.
    #[tokio::test]
    async fn a_bot_on_the_lan_is_reached_there_once_its_certificate_is_known() {
        let root = tempfile::tempdir().unwrap();
        let bot_dir = root.path().join("bot");
        let oal = bot_host(&bot_dir);
        let on_lan = lan::serve(oal.clone(), "127.0.0.1:0".parse().unwrap(), &bot_dir.join("oal"), Reach::Lan { advertise: false })
            .await
            .unwrap();
        let hub = Hub::start(oal.clone()).await;
        let nebo = nebo(&root.path().join("nebo"), vec![on_lan.addr]);

        // Nothing is known of it yet: the relay, which names its certificate.
        let mut first = nebo.connect(&hub).await.unwrap();
        assert_eq!(first.via, Via::Relay);
        answers(&mut first).await;
        assert_eq!(nebo.looked.load(Ordering::SeqCst), 0, "a bot whose certificate isn't known is not looked for");
        assert_eq!(nebo.direct.certificates().get(BOT), Some(&on_lan.fingerprint));

        hub.set(Mode::Fails);
        let before = hub.sockets();
        let mut conn = nebo.connect(&hub).await.unwrap();
        assert_eq!(conn.via, Via::Lan);
        answers(&mut conn).await;
        assert_eq!(hub.sockets(), before, "the relay was not asked");
        assert_eq!(nebo.looked.load(Ordering::SeqCst), 1);
    }

    /// Neither this computer nor the LAN answers (a daemon that stopped left
    /// its record; the LAN address is gone): Nebo goes through the relay, and
    /// when the relay fails too, says the bot is offline.
    #[tokio::test]
    async fn the_relay_when_no_direct_way_answers() {
        let root = tempfile::tempdir().unwrap();
        let bot_dir = root.path().join("bot");
        let oal = bot_host(&bot_dir);
        let on_lan = lan::serve(oal.clone(), "127.0.0.1:0".parse().unwrap(), &bot_dir.join("oal"), Reach::Lan { advertise: false })
            .await
            .unwrap();
        let hub = Hub::start(oal.clone()).await;
        let gone = TcpListener::bind("127.0.0.1:0").await.unwrap().local_addr().unwrap();
        let nebo = nebo(&root.path().join("nebo"), vec![gone]);
        // Paired, and the LAN certificate learned, through the relay.
        assert_eq!(nebo.connect(&hub).await.unwrap().via, Via::Relay);
        let stale = link_core::machine::Direct { addr: gone, fingerprint: on_lan.fingerprint.clone() };
        link_core::machine::record_direct(&nebo.daemon_home.join(BOT), &stale, true).unwrap();

        let mut conn = nebo.connect(&hub).await.unwrap();
        assert_eq!(conn.via, Via::Relay);
        answers(&mut conn).await;
        assert_eq!(nebo.looked.load(Ordering::SeqCst), 1, "the LAN was looked at after this computer");

        hub.set(Mode::Fails);
        assert_eq!(nebo.connect(&hub).await.err(), Some(Unreached::Offline));
    }

    /// Every reconnect goes the same way first: a daemon that restarted is
    /// reached on this computer again at once, the relay failing; while it
    /// serves no one directly, through the relay; and directly again as soon
    /// as it does, never kept on the relay.
    #[tokio::test]
    async fn reconnects_try_this_computer_first_every_time() {
        let root = tempfile::tempdir().unwrap();
        let bot_dir = root.path().join("bot");
        let first = bot_host(&bot_dir);
        let hub = Hub::start(first.clone()).await;
        let nebo = nebo(&root.path().join("nebo"), Vec::new());
        nebo.daemon_serves(&first, &bot_dir).await;
        hub.set(Mode::Hangs);
        let mut conn = nebo.connect(&hub).await.unwrap();
        assert_eq!(conn.via, Via::Direct);

        // The daemon restarts: the connection ends, and the next one is on
        // this computer again, at its new address.
        first.shutdown();
        assert!(tokio::time::timeout(Duration::from_secs(10), conn.next()).await.unwrap().is_none(), "the connection ended");
        let second = bot_host(&bot_dir);
        let moved = nebo.daemon_serves(&second, &bot_dir).await;
        let mut conn = nebo.connect(&hub).await.unwrap();
        assert_eq!(conn.via, Via::Direct);
        answers(&mut conn).await;
        assert_eq!(hub.sockets(), 0);

        // Serving no one directly (its record left behind), it is reached
        // through the relay...
        second.shutdown();
        let third = bot_host(&bot_dir);
        hub.reach(third.clone());
        hub.set(Mode::Up);
        let mut conn = nebo.connect(&hub).await.unwrap();
        assert_eq!(conn.via, Via::Relay);
        answers(&mut conn).await;
        assert!(link_core::machine::direct(&nebo.daemon_home, BOT).is_some_and(|d| d == moved));

        // ...and on this computer again as soon as it serves there.
        nebo.daemon_serves(&third, &bot_dir).await;
        hub.set(Mode::Hangs);
        let sockets = hub.sockets();
        let mut conn = nebo.connect(&hub).await.unwrap();
        assert_eq!(conn.via, Via::Direct);
        answers(&mut conn).await;
        assert_eq!(hub.sockets(), sockets);
    }

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
