//! Reverse management tunnel — the bot side of the loop-as-interface plan
//! (docs/plans/nebo-cloud-architecture.md, Plane B).
//!
//! The bot dials the hub over one outbound WebSocket authenticated with its
//! NeboAI bot token, then runs a yamux multiplexer over it in server mode:
//! the hub opens one stream per browser request and pipes HTTP/WS through to
//! the local nebo server. Because the bot only ever dials out, the hub is the
//! only peer that can reach the local API.
//!
//! The tunnel is the new localhost, so the bot enforces its own boundary
//! rather than trusting the hub blindly (Phase 3, `nebo-cloud-architecture.md`):
//! it dials only a TLS-authenticated (`wss://`) hub, presents its bot token,
//! and refuses to proxy local-trust surfaces that have no auth of their own
//! (`/ws/extension`, `/api/v1/update/` but its read-only `check`) — see `is_blocked_path`. Everything
//! else (the management REST API + `/ws` chat stream) passes through unchanged.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use futures::{AsyncRead, AsyncWrite, Sink, Stream};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use tokio_util::compat::FuturesAsyncReadCompatExt;
use tracing::{debug, info};

use crate::reconnect::Disconnect;

/// How long the hub may stay silent before the bot declares the tunnel dead.
/// The hub's yamux keepalive writes every 30s, so three missed keepalives.
const HUB_SILENCE: Duration = Duration::from_secs(90);

/// The header the dial sets ("1") to take the hub's drain notice (the hub's
/// `tunnel.DrainNoticeHeader`). A pod going away for a deploy then tells the
/// bot first — [`DRAIN_NOTICE`] in a text frame — and keeps the session open
/// until the requests already in it (a phone's upload) have been answered;
/// the bot opens its next session at once, on another pod, and keeps
/// serving this one meanwhile. Without it the pod's 1012 would end the
/// session, and every request in it, a moment after the drain began.
const DRAIN_NOTICE_HEADER: &str = "X-Nebo-Drain-Notice";

/// The drain notice's text frame.
const DRAIN_NOTICE: &str = "drain";

/// The longest a drained session is kept serving the requests already in
/// it. The hub closes it within its own grace (90 s) once they are done;
/// this only bounds a hub that never does.
const DRAINED_SESSION_LIMIT: Duration = Duration::from_secs(150);

#[derive(Debug, thiserror::Error)]
pub enum TunnelError {
    #[error("tunnel dial failed: {0}")]
    Dial(String),
    #[error("tunnel mux failed: {0}")]
    Mux(String),
    /// The hub ended the tunnel because the bot was removed from NeboAI: it
    /// closed the carrier with 1008 [`REVOKED_CLOSE_REASON`], or refused the
    /// dial with 401 [`crate::REVOKED_REASON`]. Redialing cannot succeed.
    #[error("this bot was removed from NeboAI")]
    Revoked,
}

/// The close reason the hub sends, with 1008 Policy Violation, when it ends a
/// removed bot's tunnel.
pub const REVOKED_CLOSE_REASON: &str = "revoked";

/// Dial the hub and serve tunnel streams until the connection closes.
///
/// Returns how an established session ended — [`Disconnect::Drain`] when
/// the hub closed with 1012, [`Disconnect::Cut`] when it ended with no close
/// frame (a reset, an EOF, a mux error, a silent hub),
/// [`Disconnect::Dropped`] for any other close — and an error when the dial
/// or the handshake failed. A dial tries every address the
/// hub's hostname resolves to, in a random order, and fails only when each
/// has refused or timed out (`tls::connect_ws`). The caller owns reconnect and backoff
/// ([`crate::reconnect::Backoff`]). [`TunnelError::Revoked`] is final: the
/// bot was removed from NeboAI.
///
/// A cell that does not hold the account (a 421 on the dial, or a
/// [`crate::cell::REDIRECT_CLOSE_CODE`] close) names the one that does, and
/// `run` dials it within half a second, at most
/// [`crate::cell::MAX_REDIRECTS`] times in a row; past that, or for an
/// address outside NeboAI, it fails like a refused dial.
///
/// `sleep` resolves when the computer is about to sleep: the session is
/// closed with 1001 "sleep" ([`Disconnect::Sleep`]) and `run` returns it.
pub async fn run(
    hub_url: &str,
    token: &str,
    local_addr: &str,
    online: &std::sync::atomic::AtomicBool,
    sleep: impl Future<Output = ()>,
) -> Result<Disconnect, TunnelError> {
    // `online` is true exactly while the hub holds this session — the
    // switchboard can reach us — and false the moment it does not.
    struct Offline<'a>(&'a std::sync::atomic::AtomicBool);
    impl Drop for Offline<'_> {
        fn drop(&mut self) {
            self.0.store(false, std::sync::atomic::Ordering::Relaxed);
        }
    }
    let _offline = Offline(online);
    verify_hub_url(hub_url)?;
    // A cell the account does not live in names the one it does; follow it
    // quietly, a bounded number of times (crate::cell).
    let mut redirects = crate::cell::Redirects::new();
    let mut url = hub_url.to_string();
    let mut sleep = std::pin::pin!(sleep);
    loop {
        let started = std::time::Instant::now();
        let redirect = match session(&url, token, local_addr, online, sleep.as_mut()).await? {
            Ended::Closed(how) => return Ok(how),
            Ended::Redirected(redirect) => redirect,
        };
        online.store(false, std::sync::atomic::Ordering::Relaxed);
        if started.elapsed() >= crate::reconnect::HEALTHY {
            redirects.session_up();
        }
        let Some(next) = redirects.follow(hub_url, &redirect) else {
            return Err(TunnelError::Dial(format!(
                "not following the hub to cell {}",
                redirect.cell
            )));
        };
        tokio::time::sleep(crate::cell::jitter()).await;
        url = next;
    }
}

/// How one tunnel session ended.
enum Ended {
    /// The hub closed it ([`run`]'s result).
    Closed(Disconnect),
    /// The account lives in another cell: dial there.
    Redirected(crate::cell::Redirect),
}

/// Dial `hub_url` once and serve the session until it ends.
async fn session(
    hub_url: &str,
    token: &str,
    local_addr: &str,
    online: &std::sync::atomic::AtomicBool,
    mut sleep: Pin<&mut impl Future<Output = ()>>,
) -> Result<Ended, TunnelError> {
    let mut request = hub_url
        .into_client_request()
        .map_err(|e| TunnelError::Dial(format!("bad hub url: {e}")))?;
    let auth = format!("Bearer {token}")
        .parse()
        .map_err(|_| TunnelError::Dial("bot token is not a valid header value".into()))?;
    request.headers_mut().insert("Authorization", auth);
    claim_lease(request.headers_mut(), crate::lease::process());
    request.headers_mut().insert(
        DRAIN_NOTICE_HEADER,
        tokio_tungstenite::tungstenite::http::HeaderValue::from_static("1"),
    );

    // Each address the hostname resolves to is tried in a random order,
    // within its own connect and handshake limits (`tls::connect_ws`).
    let ws = match tls::connect_ws(request).await {
        Ok((ws, _)) => ws,
        Err(e) => match crate::cell::Redirect::from_dial_error(&e) {
            Some(redirect) => return Ok(Ended::Redirected(redirect)),
            None => return Err(dial_error(e)),
        },
    };
    info!(hub = %::types::redact::redact(hub_url), "tunnel: connected to hub");
    online.store(true, std::sync::atomic::Ordering::Relaxed);

    let closed = HubClose::default();
    let drain = DrainNotice::default();
    let mut conn = yamux::Connection::new(
        WsIo::new(ws, HUB_SILENCE, closed.clone(), drain.clone()),
        yamux::Config::default(),
        yamux::Mode::Server,
    );
    loop {
        let ended = tokio::select! {
            ended = futures::future::poll_fn(|cx| conn.poll_next_inbound(cx)) => ended,
            () = drain.0.notified() => {
                // The pod holding this session is going away. The caller
                // dials the next one now; this one keeps serving what is
                // already in it until the hub closes it.
                info!("tunnel: hub is draining this connection; finishing its requests while the next one opens");
                finish_drained(conn, local_addr.to_string());
                return Ok(Ended::Closed(Disconnect::Drain));
            }
            () = sleep.as_mut() => {
                // The computer is about to sleep: close the session (the
                // carrier says 1001 "sleep", `WsIo::poll_close`) so the hub
                // drops it now, not a keepalive window from now.
                info!("tunnel: the computer is going to sleep; closing until it wakes");
                let close = futures::future::poll_fn(|cx| conn.poll_close(cx));
                if tokio::time::timeout(crate::reconnect::SLEEP_CLOSE_LIMIT, close).await.is_err() {
                    debug!("tunnel: the sleep close did not finish in time");
                }
                return Ok(Ended::Closed(Disconnect::Sleep));
            }
        };
        if let Some(Ok(stream)) = ended {
            serve_stream(stream, local_addr);
            continue;
        }
        let close = closed.get();
        if close.as_ref().is_some_and(|(code, reason)| is_revoked(*code, reason)) {
            return Err(TunnelError::Revoked);
        }
        if let Some(redirect) = close
            .as_ref()
            .and_then(|(code, reason)| crate::cell::Redirect::from_close(*code, reason))
        {
            return Ok(Ended::Redirected(redirect));
        }
        // Classified exactly as the comms connection is
        // (`Disconnect::from_close`): 1012 is a drain, another close code a
        // drop, and an end with no close frame — a mux or socket error, an
        // EOF, a hub gone silent — a hard cut.
        let how = Disconnect::from_close(close.as_ref().map(|(code, _)| *code));
        match (&how, ended) {
            (Disconnect::Drain, _) => info!("tunnel: hub is draining this connection"),
            (Disconnect::Cut, Some(Err(e))) => info!(error = %e, "tunnel: connection to the hub was cut"),
            (Disconnect::Cut, _) => info!("tunnel: connection to the hub was cut"),
            _ => info!("tunnel: hub closed the connection"),
        }
        return Ok(Ended::Closed(how));
    }
}

/// Pipe one hub-opened stream to the local server on its own task.
fn serve_stream(stream: yamux::Stream, local_addr: &str) {
    let addr = local_addr.to_string();
    tokio::spawn(async move {
        if let Err(e) = proxy_stream(stream, &addr).await {
            debug!(error = %e, "tunnel: stream closed with error");
        }
    });
}

/// Keep serving a session the hub is draining (it sent [`DRAIN_NOTICE`]):
/// the requests already in it — an upload from the phone — finish here while
/// the bot's next session takes the new ones. The hub closes it once they
/// are done; [`DRAINED_SESSION_LIMIT`] bounds a hub that never does.
fn finish_drained(mut conn: yamux::Connection<WsIo>, local_addr: String) {
    tokio::spawn(async move {
        let serve = async {
            while let Some(Ok(stream)) =
                futures::future::poll_fn(|cx| conn.poll_next_inbound(cx)).await
            {
                serve_stream(stream, &local_addr);
            }
        };
        match tokio::time::timeout(DRAINED_SESSION_LIMIT, serve).await {
            Ok(()) => debug!("tunnel: the drained connection finished its requests"),
            Err(_) => info!("tunnel: the drained connection outlived its limit; closed it"),
        }
    });
}

/// The hub's drain notice ([`DRAIN_NOTICE`]), raised by the carrier that
/// reads it and awaited by `session`.
#[derive(Clone, Default)]
struct DrainNotice(std::sync::Arc<tokio::sync::Notify>);

/// The close frame the hub ended the tunnel with (code, reason), shared
/// between the carrier that reads it and `run`, which tells a revoke and a
/// drain from a routine close.
#[derive(Clone, Default)]
struct HubClose(std::sync::Arc<std::sync::Mutex<Option<(u16, String)>>>);

impl HubClose {
    fn set(&self, code: u16, reason: &str) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Some((code, reason.to_string()));
    }

    fn get(&self) -> Option<(u16, String)> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// 1008 [`REVOKED_CLOSE_REASON`]: the bot was removed from NeboAI.
fn is_revoked(code: u16, reason: &str) -> bool {
    code == u16::from(CloseCode::Policy) && reason == REVOKED_CLOSE_REASON
}

/// A refused dial is [`TunnelError::Revoked`] when the hub says the bot was
/// removed (401 with [`crate::REVOKED_REASON`]); anything else may pass.
fn dial_error(e: tokio_tungstenite::tungstenite::Error) -> TunnelError {
    if let tokio_tungstenite::tungstenite::Error::Http(response) = &e {
        let reason = response
            .body()
            .as_deref()
            .and_then(|body| serde_json::from_slice::<serde_json::Value>(body).ok())
            .and_then(|body| body["error"].as_str().map(str::to_owned));
        if response.status() == 401 && reason.as_deref() == Some(crate::REVOKED_REASON) {
            return TunnelError::Revoked;
        }
    }
    TunnelError::Dial(e.to_string())
}

/// Name this process's lease on the dial (`crate::lease`): the hub lets only
/// the process holding the bot's lease hold its tunnel, and closes a session
/// whose lease has since passed to another process. A process with no lease
/// yet sends epoch 0, which the hub refuses.
fn claim_lease(headers: &mut tokio_tungstenite::tungstenite::http::HeaderMap, lease: &crate::lease::Lease) {
    let pairs = [
        ("X-Nebo-Instance", lease.instance_id().to_string()),
        ("X-Nebo-Lease-Epoch", lease.epoch().to_string()),
    ];
    for (name, value) in pairs {
        if let Ok(value) = value.parse() {
            headers.insert(name, value);
        }
    }
}

/// Verify the tunnel targets a TLS-authenticated hub ("verify hub identity",
/// Phase 3). `wss://` means the hub's certificate authenticates it end-to-end;
/// a plaintext `ws://` hub could be impersonated by any on-path attacker who
/// would then drive the local API, so it is rejected — except for loopback,
/// which the spike example / tests use.
fn verify_hub_url(hub_url: &str) -> Result<(), TunnelError> {
    if hub_url.starts_with("wss://") {
        return Ok(());
    }
    if let Some(rest) = hub_url.strip_prefix("ws://") {
        let host = rest.split(['/', ':']).next().unwrap_or("");
        if matches!(host, "127.0.0.1" | "localhost" | "[::1]") {
            return Ok(());
        }
        return Err(TunnelError::Dial(format!(
            "refusing plaintext ws:// hub {hub_url} — use wss:// so TLS authenticates the hub"
        )));
    }
    Err(TunnelError::Dial(format!(
        "hub url must be wss://, got {hub_url}"
    )))
}

/// Local surfaces that must never be reachable through the tunnel: they assume
/// "localhost = trusted" and carry no auth of their own. The bot refuses them
/// itself, so this holds even if the hub is compromised or its owner check has
/// a gap.
///
/// - `/ws/extension` drives the user's REAL browser (arbitrary in-page JS via
///   `evaluate`, local-file read via `file_upload`) — inherently local, never
///   legitimate from a remote UI.
/// - `/api/v1/update/` manages and swaps the local binary — a local-only op.
///   Except `/api/v1/update/check`: it only reports the running and newest
///   versions, is authenticated like the rest of the API, and changes
///   nothing, so the owner's phone can ask it. Applying stays local.
/// - `/api/v1/import/` reads arbitrary local install directories and copies
///   their credentials — an owner-machine operation, never legitimate remotely.
fn is_blocked_path(path: &str) -> bool {
    let p = path.split('?').next().unwrap_or(path);
    p == "/ws/extension"
        || p.starts_with("/ws/extension/")
        || (p.starts_with("/api/v1/update/") && p != "/api/v1/update/check")
        || p.starts_with("/api/v1/import/")
}

/// Pipe one hub-opened stream to the local server, gating the request first.
///
/// The full request head is read and the path checked before any byte reaches
/// the local server. Crucially, a non-upgrade request is rewritten to
/// `Connection: close` so the local server answers exactly once and closes —
/// which means a hostile hub cannot pipeline a blocked request behind an
/// allowed one on the same stream (HTTP keep-alive reuse would otherwise slip
/// the second request past a first-line-only check). WebSocket upgrades switch
/// protocols and own the stream, so there is no second request to smuggle and
/// their `Connection` header is left as it is. Every head, upgrade or not,
/// carries the tunnel stamp (see [`tunnel_auth_secret`]).
async fn proxy_stream(stream: yamux::Stream, local_addr: &str) -> io::Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = stream.compat();

    // Read through the blank line that ends the request head. Byte-at-a-time so
    // we never consume the body/frames (spliced below); capped so a stream that
    // never terminates the head can't grow the buffer without bound.
    const MAX_HEAD: usize = 64 * 1024;
    let mut head = Vec::with_capacity(1024);
    let mut byte = [0u8; 1];
    loop {
        if stream.read(&mut byte).await? == 0 {
            return Ok(()); // closed before a full head
        }
        head.push(byte[0]);
        if head.ends_with(b"\r\n\r\n") {
            break;
        }
        if head.len() >= MAX_HEAD {
            return deny(&mut stream, "request head too large").await;
        }
    }

    let head_str = String::from_utf8_lossy(&head);
    let path = head_str
        .split("\r\n")
        .next()
        .unwrap_or("")
        .split(' ')
        .nth(1)
        .unwrap_or("");
    if is_blocked_path(path.strip_prefix(UNVERIFIED_PREFIX).unwrap_or(path)) {
        debug!(path = %::types::redact::redact(path), "tunnel: refused blocked local surface");
        return deny(&mut stream, "not reachable through the tunnel").await;
    }

    let is_upgrade = head_str.split("\r\n").any(|l| {
        let l = l.to_ascii_lowercase();
        l.starts_with("upgrade:") || (l.starts_with("connection:") && l.contains("upgrade"))
    });

    let mut local = TcpStream::connect(local_addr).await?;
    local
        .write_all(forward_head(&head_str, is_upgrade).as_bytes())
        .await?;
    tokio::io::copy_bidirectional(&mut stream, &mut local).await?;
    Ok(())
}

/// The header the tunnel stamps on a request the hub checked the owner on.
pub const TUNNEL_AUTH_HEADER: &str = "x-nebo-tunnel-auth";

/// The header the tunnel stamps, instead of [`TUNNEL_AUTH_HEADER`], on a
/// request the hub forwarded without checking anyone ([`UNVERIFIED_PREFIX`]).
pub const TUNNEL_UNVERIFIED_HEADER: &str = "x-nebo-tunnel-unverified";

/// The path prefix the hub puts on a request it forwards WITHOUT checking
/// the owner: an app page's own calls (`/k/<app pass>/…`), which run in an
/// origin of their own and carry no hub sign-in. The tunnel takes it off and
/// stamps [`TUNNEL_UNVERIFIED_HEADER`], so the local server asks the request
/// for the app's pass and admits nothing else. A Nebo that predates this
/// sees the prefix as an unknown page and serves nothing of the owner's.
pub const UNVERIFIED_PREFIX: &str = "/tunnel-unverified";

/// Per-boot secret the tunnel stamps onto requests it forwards to the local
/// server (`X-Nebo-Tunnel-Auth`). A request carrying it was owner-authenticated
/// by the hub before entering the tunnel, so local handlers may treat it as the
/// owner acting remotely (the server's Host check and the app-UI invoke gate
/// do). A drive-by web page can
/// never learn the value — it exists only in this process — and any incoming
/// copy of the header is stripped before the stamp, so it cannot be smuggled
/// through the tunnel either.
pub fn tunnel_auth_secret() -> &'static str {
    static SECRET: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SECRET.get_or_init(|| {
        let mut bytes = [0u8; 32];
        getrandom::getrandom(&mut bytes).expect("os rng");
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    })
}

/// Rewrite a request head for the local server: stamp the tunnel-auth secret
/// (stripping any inbound copy first; see [`tunnel_auth_secret`]) and, unless
/// the request is a WebSocket upgrade, make the server close after one
/// response — replacing any `Connection:` header, or inserting one if absent.
///
/// A request the hub marked unchecked ([`UNVERIFIED_PREFIX`] on its path)
/// goes on without the prefix and stamped [`TUNNEL_UNVERIFIED_HEADER`]
/// instead: it never carries the owner's stamp.
fn forward_head(head: &str, upgrade: bool) -> String {
    let mut out = String::with_capacity(head.len() + 80);
    let mut wrote_conn = false;
    let mut stamp = "X-Nebo-Tunnel-Auth";
    for (i, line) in head.split_inclusive("\r\n").enumerate() {
        let trimmed = line.trim_end_matches("\r\n");
        let lower = trimmed.to_ascii_lowercase();
        if i == 0 {
            // The request line: `<METHOD> <target> <version>`.
            let mut parts = trimmed.splitn(3, ' ');
            let (method, target, version) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""), parts.next().unwrap_or(""));
            if let Some(rest) = target.strip_prefix(UNVERIFIED_PREFIX)
                && (rest.starts_with('/') || rest.is_empty())
            {
                stamp = "X-Nebo-Tunnel-Unverified";
                let rest = if rest.is_empty() { "/" } else { rest };
                out.push_str(&format!("{method} {rest} {version}\r\n"));
                continue;
            }
            out.push_str(line);
            continue;
        }
        if lower.starts_with(&format!("{TUNNEL_AUTH_HEADER}:")) || lower.starts_with(&format!("{TUNNEL_UNVERIFIED_HEADER}:")) {
            continue; // never trust an inbound copy
        }
        if lower.starts_with("connection:") && !upgrade {
            out.push_str("Connection: close\r\n");
            wrote_conn = true;
        } else if trimmed.is_empty() {
            if !wrote_conn && !upgrade {
                out.push_str("Connection: close\r\n");
            }
            out.push_str(&format!("{stamp}: {}\r\n", tunnel_auth_secret()));
            out.push_str(line); // the terminating \r\n
        } else {
            out.push_str(line);
        }
    }
    out
}

/// Write a minimal `403` and close — for surfaces the bot refuses to tunnel.
async fn deny<S>(stream: &mut S, reason: &str) -> io::Result<()>
where
    S: tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::AsyncWriteExt;
    let resp = format!(
        "HTTP/1.1 403 Forbidden\r\nContent-Length: {}\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\n{}",
        reason.len(),
        reason
    );
    stream.write_all(resp.as_bytes()).await?;
    // Clean shutdown (FIN, not reset) so the hub reads the 403 as a real
    // response instead of a dropped connection.
    stream.shutdown().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The dial names the process and the epoch it holds; before any grant
    /// that epoch is 0, which the hub never accepts.
    #[test]
    fn the_dial_claims_the_lease() {
        let lease = crate::lease::Lease::new();
        let mut headers = tokio_tungstenite::tungstenite::http::HeaderMap::new();
        claim_lease(&mut headers, &lease);
        assert_eq!(headers["X-Nebo-Instance"], lease.instance_id());
        assert_eq!(headers["X-Nebo-Lease-Epoch"], "0");

        lease.granted(9, std::time::Duration::from_secs(60), std::time::Instant::now());
        claim_lease(&mut headers, &lease);
        assert_eq!(headers["X-Nebo-Lease-Epoch"], "9");
    }

    #[test]
    fn blocks_local_trust_surfaces() {
        assert!(is_blocked_path("/ws/extension"));
        assert!(is_blocked_path("/ws/extension?x=1"));
        assert!(is_blocked_path("/api/v1/update/apply"));
        assert!(is_blocked_path("/api/v1/update/apply?x=1"));
        assert!(is_blocked_path("/api/v1/update/check/../apply"));
        // Reading the versions is not a local-trust surface: it passes.
        assert!(!is_blocked_path("/api/v1/update/check"));
        assert!(!is_blocked_path("/api/v1/update/check?x=1"));
        assert!(is_blocked_path("/api/v1/import/scan"));
        assert!(is_blocked_path("/api/v1/import/apply?x=1"));
        // Management UI + chat stream must still pass.
        assert!(!is_blocked_path("/ws"));
        assert!(!is_blocked_path("/api/v1/agents"));
        assert!(!is_blocked_path("/api/v1/chats/message"));
    }

    #[test]
    fn stamps_tunnel_auth_and_strips_inbound_copies() {
        let head = format!(
            "POST /api/v1/apps/x/agents/invoke HTTP/1.1\r\nHost: h\r\nX-Nebo-Tunnel-Auth: forged\r\nConnection: keep-alive\r\n\r\n"
        );
        let out = forward_head(&head, false);
        // The forged inbound copy is gone; exactly one stamp with OUR secret.
        assert!(!out.contains("forged"));
        assert_eq!(out.matches("X-Nebo-Tunnel-Auth:").count(), 1);
        assert!(out.contains(&format!("X-Nebo-Tunnel-Auth: {}", tunnel_auth_secret())));
        assert!(out.contains("Connection: close\r\n"));
        assert!(out.ends_with("\r\n\r\n"));
    }

    // The server's Host check admits tunnel traffic by this stamp (the Host
    // is the browser's), so the chat stream's upgrade must carry it too —
    // and stay an upgrade.
    #[test]
    fn stamps_websocket_upgrades_and_keeps_them_open() {
        let h = "GET /ws HTTP/1.1\r\nHost: neboai.com\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nX-Nebo-Tunnel-Auth: forged\r\n\r\n";
        assert_eq!(
            forward_head(h, true),
            format!(
                "GET /ws HTTP/1.1\r\nHost: neboai.com\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nX-Nebo-Tunnel-Auth: {}\r\n\r\n",
                tunnel_auth_secret()
            )
        );
    }

    /// A request the hub forwarded unchecked (an app page's own call) goes
    /// on without the marker and never with the owner's stamp, whatever it
    /// carried in; an inbound unverified stamp is stripped like the other.
    #[test]
    fn an_unchecked_request_never_carries_the_owners_stamp() {
        let h = "GET /tunnel-unverified/k/av1/api/v1/apps/a1/storage HTTP/1.1\r\nHost: neboai.com\r\nX-Nebo-Tunnel-Auth: forged\r\nX-Nebo-Tunnel-Unverified: forged\r\n\r\n";
        assert_eq!(
            forward_head(h, false),
            format!(
                "GET /k/av1/api/v1/apps/a1/storage HTTP/1.1\r\nHost: neboai.com\r\nConnection: close\r\nX-Nebo-Tunnel-Unverified: {}\r\n\r\n",
                tunnel_auth_secret()
            )
        );
        // Only the whole prefix counts.
        let out = forward_head("GET /tunnel-unverifiedx HTTP/1.1\r\nHost: a\r\n\r\n", false);
        assert!(out.starts_with("GET /tunnel-unverifiedx ") && out.contains("X-Nebo-Tunnel-Auth: "));
        // A blocked surface stays blocked behind the prefix.
        assert!(is_blocked_path("/tunnel-unverified/ws/extension".strip_prefix(UNVERIFIED_PREFIX).unwrap()));
    }

    #[test]
    fn requires_tls_hub_except_loopback() {
        assert!(verify_hub_url("wss://api.neboai.com/tunnel/connect").is_ok());
        assert!(verify_hub_url("ws://127.0.0.1:18899/tunnel/connect").is_ok());
        assert!(verify_hub_url("ws://localhost/tunnel").is_ok());
        assert!(verify_hub_url("ws://api.neboai.com/tunnel").is_err());
        assert!(verify_hub_url("http://api.neboai.com").is_err());
    }

    #[test]
    fn forces_connection_close() {
        // Existing keep-alive header is replaced.
        let h = "GET /x HTTP/1.1\r\nHost: a\r\nConnection: keep-alive\r\n\r\n";
        let out = forward_head(h, false);
        assert!(out.contains("Connection: close\r\n"));
        assert!(!out.to_ascii_lowercase().contains("keep-alive"));
        assert!(out.ends_with("\r\n\r\n"));
        // Absent header is inserted before the blank line (plus the tunnel
        // auth stamp — see stamps_tunnel_auth_and_strips_inbound_copies).
        let h2 = "GET /x HTTP/1.1\r\nHost: a\r\n\r\n";
        let out2 = forward_head(h2, false);
        assert_eq!(
            out2,
            format!(
                "GET /x HTTP/1.1\r\nHost: a\r\nConnection: close\r\nX-Nebo-Tunnel-Auth: {}\r\n\r\n",
                tunnel_auth_secret()
            )
        );
    }

    /// The hub's revoke signals end the tunnel for good; a routine close
    /// (a draining pod, 1012) or another refusal does not.
    #[tokio::test]
    async fn revoked_close_and_refusal_are_final_and_a_drain_is_not() {
        use futures::SinkExt;
        use tokio::io::AsyncWriteExt;
        use tokio_tungstenite::tungstenite::protocol::CloseFrame;

        // A hub that accepts the tunnel, then closes it with `code, reason`.
        async fn closed_with(code: CloseCode, reason: &'static str) -> Result<Disconnect, TunnelError> {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(async move {
                let (sock, _) = listener.accept().await.unwrap();
                let mut ws = tokio_tungstenite::accept_async(sock).await.unwrap();
                ws.send(WsMessage::Close(Some(CloseFrame {
                    code,
                    reason: reason.into(),
                })))
                .await
                .unwrap();
                let _ = ws.close(None).await;
            });
            let online = std::sync::atomic::AtomicBool::new(false);
            run(&format!("ws://{addr}/tunnel"), "t", "127.0.0.1:9", &online, std::future::pending()).await
        }
        assert!(matches!(
            closed_with(CloseCode::Policy, REVOKED_CLOSE_REASON).await,
            Err(TunnelError::Revoked)
        ));
        assert!(matches!(
            closed_with(CloseCode::Restart, "drain").await,
            Ok(Disconnect::Drain)
        ));
        // Any other close is a routine drop, not a drain.
        assert!(matches!(
            closed_with(CloseCode::Normal, "").await,
            Ok(Disconnect::Dropped)
        ));
        assert!(matches!(
            closed_with(CloseCode::Away, "going away").await,
            Ok(Disconnect::Dropped)
        ));

        // A hub that refuses the dial with `status` and `body`.
        async fn refused_with(status: &'static str, body: &'static str) -> Result<Disconnect, TunnelError> {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(async move {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut head = [0u8; 4096];
                let _ = tokio::io::AsyncReadExt::read(&mut sock, &mut head).await;
                let reply = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                sock.write_all(reply.as_bytes()).await.unwrap();
            });
            let online = std::sync::atomic::AtomicBool::new(false);
            run(&format!("ws://{addr}/tunnel"), "t", "127.0.0.1:9", &online, std::future::pending()).await
        }
        assert!(matches!(
            refused_with("401 Unauthorized", r#"{"error":"bot has been revoked"}"#).await,
            Err(TunnelError::Revoked)
        ));
        assert!(matches!(
            refused_with("401 Unauthorized", r#"{"error":"stale token"}"#).await,
            Err(TunnelError::Dial(_))
        ));
        assert!(matches!(
            refused_with("503 Service Unavailable", r#"{"error":"bot has been revoked"}"#).await,
            Err(TunnelError::Dial(_))
        ));
    }

    /// How a fake cell answers each tunnel dial.
    #[derive(Clone)]
    enum FakeCell {
        /// Accept, then close with `code` and `reason`.
        Closes(CloseCode, String),
        /// Refuse the upgrade with a 421 naming `url`.
        Refuses(String),
    }

    async fn bind_cell() -> (tokio::net::TcpListener, String) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/tunnel/connect", listener.local_addr().unwrap());
        (listener, url)
    }

    fn moved_to(url: &str) -> String {
        serde_json::json!({"cell": "2", "url": url}).to_string()
    }

    /// Serve every dial as `cell`, counting the dials that carried the
    /// bot's token.
    fn serve_cell(listener: tokio::net::TcpListener, cell: FakeCell) -> std::sync::Arc<std::sync::atomic::AtomicUsize> {
        use futures::SinkExt;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio_tungstenite::tungstenite::protocol::CloseFrame;
        let dials = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = dials.clone();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = listener.accept().await.unwrap();
                let counted = counted.clone();
                let cell = cell.clone();
                tokio::spawn(async move {
                    match cell {
                        FakeCell::Refuses(url) => {
                            let mut head = [0u8; 4096];
                            let n = sock.read(&mut head).await.unwrap_or(0);
                            if String::from_utf8_lossy(&head[..n]).contains("Bearer t\r\n") {
                                counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            }
                            let body = serde_json::json!({"error": "wrong_cell", "cell": "2", "url": url}).to_string();
                            let reply = format!(
                                "HTTP/1.1 421 Misdirected Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                                body.len()
                            );
                            let _ = sock.write_all(reply.as_bytes()).await;
                        }
                        FakeCell::Closes(code, reason) => {
                            let check = |req: &tokio_tungstenite::tungstenite::handshake::server::Request,
                                         resp| {
                                if req.headers().get("Authorization").is_some_and(|v| v == "Bearer t") {
                                    counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                                }
                                Ok(resp)
                            };
                            let mut ws = tokio_tungstenite::accept_hdr_async(sock, check).await.unwrap();
                            let _ = ws.send(WsMessage::Close(Some(CloseFrame { code, reason: reason.into() }))).await;
                            let _ = ws.close(None).await;
                        }
                    }
                });
            }
        });
        dials
    }

    async fn run_from(url: &str) -> Result<Disconnect, TunnelError> {
        let online = std::sync::atomic::AtomicBool::new(false);
        tokio::time::timeout(Duration::from_secs(10), run(url, "t", "127.0.0.1:9", &online, std::future::pending()))
            .await
            .expect("run ends")
    }

    /// A cell the account does not live in names the one it does, before or
    /// after the upgrade; the tunnel dials there with the same token, and the
    /// session it gets there decides the outcome.
    #[tokio::test]
    async fn a_cell_redirect_is_followed() {
        let redirect = CloseCode::from(crate::cell::REDIRECT_CLOSE_CODE);
        for refuse_on_dial in [true, false] {
            let (home_listener, home_url) = bind_cell().await;
            let (wrong_listener, wrong_url) = bind_cell().await;
            let home = serve_cell(home_listener, FakeCell::Closes(CloseCode::Restart, "drain".into()));
            let wrong = serve_cell(
                wrong_listener,
                if refuse_on_dial {
                    FakeCell::Refuses(home_url.clone())
                } else {
                    FakeCell::Closes(redirect, moved_to(&home_url))
                },
            );
            assert!(matches!(run_from(&wrong_url).await, Ok(Disconnect::Drain)));
            assert_eq!(wrong.load(std::sync::atomic::Ordering::SeqCst), 1);
            assert_eq!(home.load(std::sync::atomic::Ordering::SeqCst), 1, "the home cell got the token");
        }
    }

    /// Cells that keep pointing at each other are followed twice; the third
    /// redirect fails the run like a refused dial, and the watcher backs off.
    #[tokio::test]
    async fn a_redirect_loop_is_capped() {
        let redirect = CloseCode::from(crate::cell::REDIRECT_CLOSE_CODE);
        let (a_listener, a_url) = bind_cell().await;
        let (b_listener, b_url) = bind_cell().await;
        let (c_listener, c_url) = bind_cell().await;
        let a = serve_cell(a_listener, FakeCell::Refuses(b_url));
        let b = serve_cell(b_listener, FakeCell::Closes(redirect, moved_to(&c_url)));
        let c = serve_cell(c_listener, FakeCell::Refuses(a_url.clone()));
        let ended = run_from(&a_url).await;
        assert!(matches!(ended, Err(TunnelError::Dial(ref m)) if m.starts_with("not following")), "{ended:?}");
        let dials = [&a, &b, &c].map(|n| n.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(dials, [1, 1, 1]);
    }

    /// A redirect outside NeboAI is refused: the token never leaves for it.
    #[tokio::test]
    async fn a_redirect_to_a_foreign_host_is_refused() {
        let redirect = CloseCode::from(crate::cell::REDIRECT_CLOSE_CODE);
        for cell in [
            FakeCell::Refuses("wss://evil.example/tunnel/connect".into()),
            FakeCell::Closes(redirect, moved_to("wss://neboai.com.evil.example/tunnel/connect")),
            FakeCell::Closes(redirect, moved_to("ws://10.0.0.1/tunnel/connect")),
        ] {
            let (listener, url) = bind_cell().await;
            let dials = serve_cell(listener, cell);
            let ended = run_from(&url).await;
            assert!(matches!(ended, Err(TunnelError::Dial(ref m)) if m.starts_with("not following")), "{ended:?}");
            assert_eq!(dials.load(std::sync::atomic::Ordering::SeqCst), 1);
        }
    }

    /// A hub that takes the TCP connection but never answers the upgrade
    /// fails the dial after the dial timeout instead of hanging the watcher.
    #[tokio::test]
    async fn a_dial_the_hub_never_answers_times_out() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (_sock, _) = listener.accept().await.unwrap();
            futures::future::pending::<()>().await;
        });
        let online = std::sync::atomic::AtomicBool::new(false);
        let started = std::time::Instant::now();
        let ended = run(&format!("ws://{addr}/tunnel"), "t", "127.0.0.1:9", &online, std::future::pending()).await;
        assert!(matches!(ended, Err(TunnelError::Dial(_))), "{ended:?}");
        let took = started.elapsed();
        assert!(took >= crate::reconnect::DIAL_TIMEOUT && took < crate::reconnect::DIAL_TIMEOUT * 2, "{took:?}");
    }

    /// How a fake hub ends an established tunnel.
    #[derive(Clone, Copy)]
    enum HubEnd {
        /// A 1012 close frame (a planned restart).
        Restart,
        /// The TCP connection dropped with no close frame (a node dying).
        HardCut,
    }

    /// A hub that upgrades every tunnel dial, ends the first session as
    /// `end` says, and keeps every later one open. Returns the instant each
    /// dial arrived.
    fn serve_hub(
        listener: tokio::net::TcpListener,
        end: HubEnd,
    ) -> std::sync::Arc<std::sync::Mutex<Vec<std::time::Instant>>> {
        use futures::SinkExt;
        use tokio_tungstenite::tungstenite::protocol::CloseFrame;
        let dials = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = dials.clone();
        tokio::spawn(async move {
            loop {
                let (sock, _) = listener.accept().await.unwrap();
                let first = {
                    let mut seen = seen.lock().unwrap();
                    seen.push(std::time::Instant::now());
                    seen.len() == 1
                };
                tokio::spawn(async move {
                    let mut ws = tokio_tungstenite::accept_async(sock).await.unwrap();
                    if !first {
                        futures::future::pending::<()>().await;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    match end {
                        HubEnd::Restart => {
                            let frame = CloseFrame { code: CloseCode::Restart, reason: "drain".into() };
                            let _ = ws.send(WsMessage::Close(Some(frame))).await;
                            let _ = ws.close(None).await;
                        }
                        HubEnd::HardCut => drop(ws),
                    }
                });
            }
        });
        dials
    }

    /// The watcher's loop (Nebo's and Nebo Link's): run the tunnel, wait
    /// what the shared backoff says, run it again. Returns how the first
    /// session ended and the gap between its end and the redial reaching
    /// the hub.
    async fn redial_after(end: HubEnd) -> (Disconnect, Duration) {
        let (listener, url) = bind_cell().await;
        let dials = serve_hub(listener, end);
        let online = std::sync::atomic::AtomicBool::new(false);
        let mut backoff = crate::reconnect::Backoff::new();
        let started = std::time::Instant::now();
        let how = run(&url, "t", "127.0.0.1:9", &online, std::future::pending()).await.expect("the session came up");
        let ended = std::time::Instant::now();
        tokio::time::sleep(backoff.wait(how, started.elapsed())).await;
        let online2 = std::sync::atomic::AtomicBool::new(false);
        let _ = tokio::time::timeout(Duration::from_millis(300), run(&url, "t", "127.0.0.1:9", &online2, std::future::pending())).await;
        let dials = dials.lock().unwrap().clone();
        assert_eq!(dials.len(), 2, "one redial");
        (how, dials[1].saturating_duration_since(ended))
    }

    /// A 1012 is redialed within half a second and a hard cut within five,
    /// both straight away rather than after a backoff step, and both
    /// classified exactly as the comms connection classifies them.
    #[tokio::test]
    async fn an_established_tunnel_that_ends_is_redialed_promptly() {
        for _ in 0..3 {
            let (how, gap) = redial_after(HubEnd::Restart).await;
            assert_eq!(how, Disconnect::Drain);
            assert!(gap < Duration::from_millis(500) + Duration::from_millis(250), "{gap:?}");
        }

        let (how, gap) = redial_after(HubEnd::HardCut).await;
        assert_eq!(how, Disconnect::Cut);
        assert!(gap < Duration::from_secs(5) + Duration::from_millis(250), "{gap:?}");
    }

    /// The computer going to sleep ends an established session with 1001
    /// "sleep" on the carrier, promptly, and `run` says so; the bot is no
    /// longer online.
    #[tokio::test]
    async fn going_to_sleep_closes_the_tunnel_with_1001() {
        use futures::StreamExt;

        let (listener, url) = bind_cell().await;
        let (seen_tx, seen) = tokio::sync::oneshot::channel::<Option<(u16, String)>>();
        tokio::spawn(async move {
            let (sock, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(sock).await.unwrap();
            while let Some(Ok(msg)) = ws.next().await {
                if let WsMessage::Close(frame) = msg {
                    let _ = seen_tx.send(frame.map(|f| (u16::from(f.code), f.reason.to_string())));
                    return;
                }
            }
        });
        let online = std::sync::atomic::AtomicBool::new(false);
        let (asleep_tx, asleep) = tokio::sync::oneshot::channel::<()>();
        let session = run(&url, "t", "127.0.0.1:9", &online, async {
            let _ = asleep.await;
        });
        let watch = async {
            // The session comes up, then the computer is about to sleep.
            while !online.load(std::sync::atomic::Ordering::Relaxed) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            asleep_tx.send(()).unwrap();
        };
        let (how, ()) = tokio::time::timeout(Duration::from_secs(5), futures::future::join(session, watch))
            .await
            .expect("the sleep close is prompt");
        assert_eq!(how.expect("the session came up"), Disconnect::Sleep);
        assert!(!online.load(std::sync::atomic::Ordering::Relaxed));
        let close = tokio::time::timeout(Duration::from_secs(1), seen).await.expect("the hub got a close").unwrap();
        assert_eq!(close, Some((1001, "sleep".to_string())));
    }

    /// The hub's end of a carrier in these tests: yamux over binary frames,
    /// with the drain notice sent ahead of its first write when `notice`.
    struct HubSide {
        ws: WebSocketStream<TcpStream>,
        buf: tokio_tungstenite::tungstenite::Bytes,
        notice: bool,
    }

    impl AsyncRead for HubSide {
        fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, out: &mut [u8]) -> Poll<io::Result<usize>> {
            loop {
                if !self.buf.is_empty() {
                    let n = out.len().min(self.buf.len());
                    let chunk = self.buf.split_to(n);
                    out[..n].copy_from_slice(&chunk);
                    return Poll::Ready(Ok(n));
                }
                match futures::ready!(Pin::new(&mut self.ws).poll_next(cx)) {
                    Some(Ok(WsMessage::Binary(data))) => self.buf = data,
                    Some(Ok(WsMessage::Close(_))) | None => return Poll::Ready(Ok(0)),
                    Some(Ok(_)) => {}
                    Some(Err(e)) => return Poll::Ready(Err(io::Error::other(e))),
                }
            }
        }
    }

    impl AsyncWrite for HubSide {
        fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, data: &[u8]) -> Poll<io::Result<usize>> {
            futures::ready!(Pin::new(&mut self.ws).poll_ready(cx)).map_err(io::Error::other)?;
            if self.notice {
                self.notice = false;
                Pin::new(&mut self.ws)
                    .start_send(WsMessage::Text(DRAIN_NOTICE.into()))
                    .map_err(io::Error::other)?;
                futures::ready!(Pin::new(&mut self.ws).poll_ready(cx)).map_err(io::Error::other)?;
            }
            Pin::new(&mut self.ws)
                .start_send(WsMessage::Binary(tokio_tungstenite::tungstenite::Bytes::copy_from_slice(data)))
                .map_err(io::Error::other)?;
            Poll::Ready(Ok(data.len()))
        }

        fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut self.ws).poll_flush(cx).map_err(io::Error::other)
        }

        fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut self.ws).poll_close(cx).map_err(io::Error::other)
        }
    }

    /// The upload a hub rollout cut on 2026-10-11: the phone's request was
    /// inside the tunnel when the pod holding it drained. With the drain
    /// notice the bot hands back to its caller at once — which dials the
    /// next pod — while the old session keeps serving, and the request in
    /// it is answered whole after `run` has returned. The dial asks for the
    /// notice.
    #[tokio::test]
    async fn a_drain_notice_redials_and_finishes_the_request_in_flight() {
        use futures::{AsyncReadExt as _, AsyncWriteExt as _};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        // The bot's local server: answers once the body is in and the test
        // releases it.
        let local = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = local.local_addr().unwrap().to_string();
        let (release, released) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(async move {
            let (mut sock, _) = local.accept().await.unwrap();
            let mut got = Vec::new();
            let mut buf = [0u8; 1024];
            while !got.ends_with(b"video") {
                let n = sock.read(&mut buf).await.unwrap();
                assert!(n > 0, "the request ended before its body");
                got.extend_from_slice(&buf[..n]);
            }
            released.await.unwrap();
            sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nConnection: close\r\n\r\ndone:video")
                .await
                .unwrap();
            sock.shutdown().await.unwrap();
        });

        // The hub: takes the dial, sends the notice ahead of its first
        // frame, and carries one upload into the tunnel.
        let (listener, url) = bind_cell().await;
        let (asked_tx, asked) = tokio::sync::oneshot::channel::<Option<String>>();
        let (answer_tx, answer) = tokio::sync::oneshot::channel::<Vec<u8>>();
        tokio::spawn(async move {
            use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
            let (sock, _) = listener.accept().await.unwrap();
            let ws = tokio_tungstenite::accept_hdr_async(sock, |req: &Request, resp: Response| {
                let header = req.headers().get(DRAIN_NOTICE_HEADER).and_then(|v| v.to_str().ok()).map(str::to_owned);
                let _ = asked_tx.send(header);
                Ok(resp)
            })
            .await
            .unwrap();
            let hub_side = HubSide { ws, buf: Default::default(), notice: true };
            let mut conn = yamux::Connection::new(hub_side, yamux::Config::default(), yamux::Mode::Client);
            let mut stream = futures::future::poll_fn(|cx| conn.poll_new_outbound(cx)).await.unwrap();
            tokio::spawn(async move {
                while futures::future::poll_fn(|cx| conn.poll_next_inbound(cx)).await.is_some() {}
            });
            stream
                .write_all(b"POST /api/v1/files/upload HTTP/1.1\r\nHost: bot\r\nContent-Length: 5\r\n\r\nvideo")
                .await
                .unwrap();
            stream.flush().await.unwrap();
            let mut got = Vec::new();
            let _ = stream.read_to_end(&mut got).await;
            let _ = answer_tx.send(got);
        });

        let online = std::sync::atomic::AtomicBool::new(false);
        let how = tokio::time::timeout(Duration::from_secs(5), run(&url, "t", &local_addr, &online, std::future::pending()))
            .await
            .expect("run returned on the notice")
            .expect("the session came up");
        assert_eq!(how, Disconnect::Drain);
        assert_eq!(asked.await.unwrap().as_deref(), Some("1"), "the dial asks for the drain notice");

        // `run` has returned and the caller is free to redial; the request
        // already in the old session is still answered, whole.
        release.send(()).unwrap();
        let got = tokio::time::timeout(Duration::from_secs(5), answer).await.expect("answered").unwrap();
        assert!(
            String::from_utf8_lossy(&got).ends_with("done:video"),
            "answer = {:?}",
            String::from_utf8_lossy(&got)
        );
    }

    /// A hub that goes silent must fail the read rather than hang forever —
    /// that failure is what makes the watcher redial.
    #[tokio::test]
    async fn silent_hub_times_out_the_read() {
        use futures::AsyncReadExt;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (sock, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(sock).await.unwrap();
            use futures::SinkExt;
            ws.send(WsMessage::Binary(vec![7u8; 4].into())).await.unwrap();
            // Then say nothing at all, holding the socket open.
            futures::future::pending::<()>().await;
        });

        let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/"))
            .await
            .unwrap();
        let mut io = WsIo::new(ws, Duration::from_millis(200), Default::default(), Default::default());

        let mut buf = [0u8; 4];
        assert_eq!(io.read(&mut buf).await.unwrap(), 4);
        let err = io.read(&mut buf).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
    }
}

/// Adapts the WebSocket to the plain byte stream yamux expects: each write
/// becomes one binary frame, reads drain binary frames, and everything else
/// is skipped (tungstenite answers pings internally).
///
/// Reads also carry the liveness check. When the hub disappears without a TCP
/// FIN — a pod rolls, a load balancer forgets the flow — an idle tunnel would
/// otherwise sit on a dead socket forever: our yamux only pings while the
/// connection is active, so an idle bot never writes and never learns the peer
/// is gone. The hub's yamux keepalive puts a frame on the wire every 30s, so
/// silence past `hub_silence` means the hub is gone; failing the read unwinds
/// the session and the watcher redials.
///
/// The hub's close frame is kept in `closed`, so `run` can tell a revoke
/// (1008 [`REVOKED_CLOSE_REASON`]) and a drain (1012) from a routine close,
/// and its drain notice ([`DRAIN_NOTICE`], a text frame) is raised on
/// `drain`.
///
/// The bot closes the carrier itself only when the computer is about to
/// sleep, so its close frame is always 1001 "sleep" ([`Disconnect::Sleep`]).
struct WsIo {
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    buf: tokio_tungstenite::tungstenite::Bytes,
    hub_silence: Duration,
    deadline: Pin<Box<tokio::time::Sleep>>,
    closed: HubClose,
    drain: DrainNotice,
    /// The bot's close frame has been queued.
    farewell_sent: bool,
}

impl WsIo {
    fn new(
        ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
        hub_silence: Duration,
        closed: HubClose,
        drain: DrainNotice,
    ) -> Self {
        Self {
            ws,
            buf: tokio_tungstenite::tungstenite::Bytes::new(),
            hub_silence,
            deadline: Box::pin(tokio::time::sleep(hub_silence)),
            closed,
            drain,
            farewell_sent: false,
        }
    }
}

impl WsIo {
    fn reset_deadline(&mut self) {
        let next = tokio::time::Instant::now() + self.hub_silence;
        self.deadline.as_mut().reset(next);
    }
}

impl AsyncRead for WsIo {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        out: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        loop {
            if !self.buf.is_empty() {
                let n = out.len().min(self.buf.len());
                let chunk = self.buf.split_to(n);
                out[..n].copy_from_slice(&chunk);
                return Poll::Ready(Ok(n));
            }
            match Pin::new(&mut self.ws).poll_next(cx) {
                Poll::Ready(Some(Ok(WsMessage::Binary(data)))) => {
                    self.reset_deadline();
                    self.buf = data;
                }
                Poll::Ready(Some(Ok(WsMessage::Close(frame)))) => {
                    if let Some(f) = frame {
                        self.closed.set(u16::from(f.code), f.reason.as_str());
                    }
                    return Poll::Ready(Ok(0));
                }
                Poll::Ready(None) => return Poll::Ready(Ok(0)),
                Poll::Ready(Some(Ok(WsMessage::Text(text)))) => {
                    self.reset_deadline();
                    if text.as_str() == DRAIN_NOTICE {
                        self.drain.0.notify_one();
                    }
                }
                Poll::Ready(Some(Ok(_))) => self.reset_deadline(), // ping/pong — liveness, not bytes
                Poll::Ready(Some(Err(e))) => return Poll::Ready(Err(io::Error::other(e))),
                Poll::Pending => {
                    return match self.deadline.as_mut().poll(cx) {
                        Poll::Ready(()) => Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "hub sent nothing for the keepalive window — treating the tunnel as dead",
                        ))),
                        Poll::Pending => Poll::Pending,
                    };
                }
            }
        }
    }
}

impl AsyncWrite for WsIo {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        match Pin::new(&mut self.ws).poll_ready(cx) {
            Poll::Ready(Ok(())) => {}
            Poll::Ready(Err(e)) => return Poll::Ready(Err(io::Error::other(e))),
            Poll::Pending => return Poll::Pending,
        }
        Pin::new(&mut self.ws)
            .start_send(WsMessage::Binary(
                tokio_tungstenite::tungstenite::Bytes::copy_from_slice(data),
            ))
            .map_err(io::Error::other)?;
        Poll::Ready(Ok(data.len()))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.ws).poll_flush(cx).map_err(io::Error::other)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if !self.farewell_sent {
            futures::ready!(Pin::new(&mut self.ws).poll_ready(cx)).map_err(io::Error::other)?;
            let frame = tokio_tungstenite::tungstenite::protocol::CloseFrame {
                code: crate::reconnect::SLEEP_CLOSE_CODE.into(),
                reason: crate::reconnect::SLEEP_CLOSE_REASON.into(),
            };
            Pin::new(&mut self.ws)
                .start_send(WsMessage::Close(Some(frame)))
                .map_err(io::Error::other)?;
            self.farewell_sent = true;
        }
        Pin::new(&mut self.ws).poll_close(cx).map_err(io::Error::other)
    }
}
