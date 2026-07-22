//! Tunnel-aware plugin OAuth relay (pod side).
//!
//! Plugins with `auth.type = oauth_cli` (e.g. gws) run a loopback HTTP
//! listener and hand Google a `redirect_uri=http://localhost:<port>`. On a
//! desktop that works — the browser and the listener share a host. On a cloud
//! bot reached through the reverse tunnel (`https://neboai.com/t/<botId>/`)
//! the user's browser is on their phone/laptop while the listener is on the
//! pod, so Google's redirect to `localhost:<port>` dead-ends (ERR_FAILED).
//!
//! The fix is a hub relay over the existing tunnel:
//!
//! 1. When `NEBOAI_PUBLIC_OAUTH=1` (set by the cloud provisioner; never on
//!    desktop), the login flow registers a pending auth here — `{nonce, port,
//!    expiry}` — and passes the ONE public redirect
//!    (`https://api.neboai.com/oauth/plugin/callback`) plus an opaque `state`
//!    (`base64url({bot_id, port, nonce})`) to the plugin via
//!    `NEBO_OAUTH_REDIRECT_URI` / `NEBO_OAUTH_STATE` / `NEBO_OAUTH_PORT`.
//! 2. Google redirects the user's browser to the hub; the hub parses `state`
//!    only to pick the bot's tunnel (same trust as `/t/` routing) and forwards
//!    the query into the tunnel as `GET /api/v1/plugins/oauth/relay?...`.
//! 3. [`relay_request`] verifies the nonce against the pending registry (the
//!    nonce is the auth — the hub route is necessarily unauthenticated) and
//!    relays the querystring verbatim to `127.0.0.1:<port>`, where the
//!    plugin's untouched loopback listener completes the token exchange.
//!
//! The port always comes from the registry, never from `state` alone, so a
//! forwarded request can never pick an arbitrary local port.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use tracing::warn;

/// The ONE fixed public redirect URI carried on the Google OAuth client.
/// The Google Cloud console for the client must list it under
/// "Authorized redirect URIs" or Google rejects the auth request.
pub const PUBLIC_REDIRECT_URI: &str = "https://api.neboai.com/oauth/plugin/callback";

/// How long a pending auth stays valid — the user has this long to finish the
/// Google consent screen.
const PENDING_TTL: Duration = Duration::from_secs(600);

struct Pending {
    port: u16,
    expires_at: Instant,
}

static PENDING: LazyLock<Mutex<HashMap<String, Pending>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Whether logins should use the public hub-relayed redirect. Opt-in via the
/// env the cloud provisioner sets, so a desktop install (whose Google console
/// entry may not exist yet) can never be broken by this pathway.
pub fn public_oauth_enabled() -> bool {
    std::env::var("NEBOAI_PUBLIC_OAUTH").is_ok_and(|v| v == "1" || v == "true")
}

/// A registered pending auth, ready to hand to the plugin's login command.
pub struct StartedRelay {
    /// Opaque state for the Google auth URL: `base64url({bot_id, port, nonce})`.
    pub state: String,
    /// Loopback port the plugin must bind for the relayed callback.
    pub port: u16,
}

/// Allocate a loopback port + nonce, register the pending auth, and build the
/// opaque `state` blob. Called at login start when [`public_oauth_enabled`].
pub fn begin(bot_id: &str) -> std::io::Result<StartedRelay> {
    // Ephemeral bind probe: take a free loopback port, release it, and tell
    // the plugin to bind exactly that port. The tiny reuse race is acceptable —
    // a lost race fails the login cleanly and a retry works.
    let port = std::net::TcpListener::bind(("127.0.0.1", 0))?
        .local_addr()?
        .port();
    let nonce = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    register(nonce.clone(), port, PENDING_TTL);
    let state = encode_state(bot_id, port, &nonce);
    Ok(StartedRelay { state, port })
}

fn register(nonce: String, port: u16, ttl: Duration) {
    let mut pending = PENDING.lock().unwrap();
    let now = Instant::now();
    pending.retain(|_, p| p.expires_at > now);
    pending.insert(nonce, Pending { port, expires_at: now + ttl });
}

/// Consume a pending auth: returns its loopback port if the nonce is known and
/// unexpired. Single-use — a second call with the same nonce returns `None`.
fn take(nonce: &str) -> Option<u16> {
    let mut pending = PENDING.lock().unwrap();
    let entry = pending.remove(nonce)?;
    (entry.expires_at > Instant::now()).then_some(entry.port)
}

fn encode_state(bot_id: &str, port: u16, nonce: &str) -> String {
    use base64::Engine;
    let json = serde_json::json!({ "bot_id": bot_id, "port": port, "nonce": nonce });
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json.to_string())
}

/// Decode the opaque state blob → (bot_id, port, nonce). The hub does the same
/// parse in Go (`internal/api/plugin_oauth.go`) but only reads `bot_id`.
fn decode_state(state: &str) -> Option<(String, u16, String)> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(state)
        .ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let bot_id = v.get("bot_id")?.as_str()?.to_string();
    let port = u16::try_from(v.get("port")?.as_u64()?).ok()?;
    let nonce = v.get("nonce")?.as_str()?.to_string();
    Some((bot_id, port, nonce))
}

/// Handle a relayed OAuth callback: verify the nonce, then pass the raw
/// querystring untouched to the plugin's loopback listener. Returns
/// `(http_status, message)` for the handler to serialize — 200 only when the
/// listener answered 2xx (i.e. it received the authorization code).
pub async fn relay_request(raw_query: &str) -> (u16, &'static str) {
    let state = match query_param(raw_query, "state") {
        Some(s) => s,
        None => return (400, "missing state"),
    };
    let Some((_bot_id, _state_port, nonce)) = decode_state(&state) else {
        return (400, "malformed state");
    };
    // The nonce is the auth; the port ALWAYS comes from the registry so a
    // forged state cannot steer the relay at an arbitrary local port.
    let Some(port) = take(&nonce) else {
        return (403, "unknown or expired auth attempt");
    };
    match relay_to_listener(port, raw_query).await {
        Ok(status) if (200..300).contains(&status) => (200, "ok"),
        Ok(_) => (502, "plugin listener rejected the callback"),
        Err(e) => {
            warn!(port, error = %e, "plugin oauth relay: loopback listener unreachable");
            (502, "plugin listener unreachable")
        }
    }
}

/// Extract one percent-decoded query parameter from a raw querystring.
fn query_param(raw_query: &str, key: &str) -> Option<String> {
    url::form_urlencoded::parse(raw_query.as_bytes())
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
}

/// Forward the callback query to the plugin's loopback listener as a minimal
/// `GET /?<query>` and report the listener's response status. The querystring
/// is passed byte-for-byte so the plugin sees exactly what Google sent.
async fn relay_to_listener(port: u16, raw_query: &str) -> std::io::Result<u16> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
    let request = format!(
        "GET /?{raw_query} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).await?;
    let mut response = Vec::new();
    // The plugin listener writes one small response and closes.
    let _ = tokio::time::timeout(
        Duration::from_secs(10),
        stream.read_to_end(&mut response),
    )
    .await
    .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "listener response timeout"))??;
    let status_line = std::str::from_utf8(&response)
        .unwrap_or("")
        .lines()
        .next()
        .unwrap_or("");
    status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "bad status line"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_round_trip() {
        let state = encode_state("bot-123", 37567, "abc123");
        let (bot_id, port, nonce) = decode_state(&state).expect("decodes");
        assert_eq!(bot_id, "bot-123");
        assert_eq!(port, 37567);
        assert_eq!(nonce, "abc123");
    }

    #[test]
    fn decode_state_rejects_garbage() {
        assert!(decode_state("not-base64!!!").is_none());
        use base64::Engine;
        let not_json = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode("hello");
        assert!(decode_state(&not_json).is_none());
        let missing_fields =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"bot_id":"x"}"#);
        assert!(decode_state(&missing_fields).is_none());
    }

    #[test]
    fn take_is_single_use_and_expires() {
        register("nonce-live".into(), 4242, Duration::from_secs(60));
        assert_eq!(take("nonce-live"), Some(4242));
        assert_eq!(take("nonce-live"), None, "second take must fail");
        assert_eq!(take("never-registered"), None);

        register("nonce-expired".into(), 4243, Duration::from_secs(0));
        assert_eq!(take("nonce-expired"), None, "expired nonce must fail");
    }

    #[tokio::test]
    async fn relay_passes_query_to_listener() {
        // Stub loopback listener standing in for the plugin's OAuth server.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            use std::io::{Read, Write};
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 2048];
            let n = stream.read(&mut buf).unwrap();
            let head = String::from_utf8_lossy(&buf[..n]).into_owned();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .unwrap();
            head
        });

        let status = relay_to_listener(port, "code=4%2Fabc&state=xyz").await.unwrap();
        assert_eq!(status, 200);
        let head = handle.join().unwrap();
        // Query must arrive byte-for-byte (still percent-encoded).
        assert!(head.starts_with("GET /?code=4%2Fabc&state=xyz HTTP/1.1\r\n"), "head: {head}");
    }

    #[tokio::test]
    async fn relay_request_end_to_end() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf).unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .unwrap();
        });

        register("e2e-nonce".into(), port, Duration::from_secs(60));
        let state = encode_state("bot-1", port, "e2e-nonce");
        let raw_query = format!("code=4%2Fabc&state={state}");

        let (status, msg) = relay_request(&raw_query).await;
        assert_eq!((status, msg), (200, "ok"));

        // Nonce consumed — a replay of the same callback is refused.
        let (status, _) = relay_request(&raw_query).await;
        assert_eq!(status, 403);
    }

    #[tokio::test]
    async fn relay_request_rejects_bad_state() {
        let (status, _) = relay_request("code=abc").await;
        assert_eq!(status, 400);
        let (status, _) = relay_request("code=abc&state=%21%21garbage").await;
        assert_eq!(status, 400);
        // Well-formed state but unknown nonce.
        let state = encode_state("bot-1", 1, "no-such-nonce");
        let (status, _) = relay_request(&format!("state={state}")).await;
        assert_eq!(status, 403);
    }
}
