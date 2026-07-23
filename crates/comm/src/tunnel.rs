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
//! (`/ws/extension`, `/api/v1/update/`) — see `is_blocked_path`. Everything
//! else (the management REST API + `/ws` chat stream) passes through unchanged.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use futures::{AsyncRead, AsyncWrite, Sink, Stream};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use tokio_util::compat::FuturesAsyncReadCompatExt;
use tracing::{debug, info};

#[derive(Debug, thiserror::Error)]
pub enum TunnelError {
    #[error("tunnel dial failed: {0}")]
    Dial(String),
    #[error("tunnel mux failed: {0}")]
    Mux(String),
}

/// Dial the hub and serve tunnel streams until the connection closes.
///
/// Returns `Ok(())` on a clean close by the hub and an error on dial/auth/mux
/// failure; the caller owns reconnect and backoff (the watcher in
/// `crates/server`, mirroring the comms reconnect watcher).
pub async fn run(hub_url: &str, token: &str, local_addr: &str) -> Result<(), TunnelError> {
    verify_hub_url(hub_url)?;
    let mut request = hub_url
        .into_client_request()
        .map_err(|e| TunnelError::Dial(format!("bad hub url: {e}")))?;
    let auth = format!("Bearer {token}")
        .parse()
        .map_err(|_| TunnelError::Dial("bot token is not a valid header value".into()))?;
    request.headers_mut().insert("Authorization", auth);

    let (ws, _) = tokio_tungstenite::connect_async(request)
        .await
        .map_err(|e| TunnelError::Dial(e.to_string()))?;
    info!(hub = %hub_url, "tunnel: connected to hub");

    let mut conn =
        yamux::Connection::new(WsIo::new(ws), yamux::Config::default(), yamux::Mode::Server);
    loop {
        match futures::future::poll_fn(|cx| conn.poll_next_inbound(cx)).await {
            Some(Ok(stream)) => {
                let addr = local_addr.to_string();
                tokio::spawn(async move {
                    if let Err(e) = proxy_stream(stream, &addr).await {
                        debug!(error = %e, "tunnel: stream closed with error");
                    }
                });
            }
            Some(Err(e)) => return Err(TunnelError::Mux(e.to_string())),
            None => {
                info!("tunnel: hub closed the connection");
                return Ok(());
            }
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
fn is_blocked_path(path: &str) -> bool {
    let p = path.split('?').next().unwrap_or(path);
    p == "/ws/extension" || p.starts_with("/ws/extension/") || p.starts_with("/api/v1/update/")
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
/// the head is forwarded untouched.
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
    if is_blocked_path(path) {
        debug!(path = %path, "tunnel: refused blocked local surface");
        return deny(&mut stream, "not reachable through the tunnel").await;
    }

    let is_upgrade = head_str.split("\r\n").any(|l| {
        let l = l.to_ascii_lowercase();
        l.starts_with("upgrade:") || (l.starts_with("connection:") && l.contains("upgrade"))
    });

    let mut local = TcpStream::connect(local_addr).await?;
    if is_upgrade {
        local.write_all(&head).await?;
    } else {
        local
            .write_all(force_connection_close(&head_str).as_bytes())
            .await?;
    }
    tokio::io::copy_bidirectional(&mut stream, &mut local).await?;
    Ok(())
}

/// Rewrite a request head so the local server closes after one response —
/// replacing any `Connection:` header, or inserting one if absent.
fn force_connection_close(head: &str) -> String {
    let mut out = String::with_capacity(head.len() + 20);
    let mut wrote_conn = false;
    for line in head.split_inclusive("\r\n") {
        let trimmed = line.trim_end_matches("\r\n");
        if trimmed.to_ascii_lowercase().starts_with("connection:") {
            out.push_str("Connection: close\r\n");
            wrote_conn = true;
        } else if trimmed.is_empty() {
            if !wrote_conn {
                out.push_str("Connection: close\r\n");
            }
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

    #[test]
    fn blocks_local_trust_surfaces() {
        assert!(is_blocked_path("/ws/extension"));
        assert!(is_blocked_path("/ws/extension?x=1"));
        assert!(is_blocked_path("/api/v1/update/apply"));
        assert!(is_blocked_path("/api/v1/update/check"));
        // Management UI + chat stream must still pass.
        assert!(!is_blocked_path("/ws"));
        assert!(!is_blocked_path("/api/v1/agents"));
        assert!(!is_blocked_path("/api/v1/chats/message"));
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
        let out = force_connection_close(h);
        assert!(out.contains("Connection: close\r\n"));
        assert!(!out.to_ascii_lowercase().contains("keep-alive"));
        assert!(out.ends_with("\r\n\r\n"));
        // Absent header is inserted before the blank line.
        let h2 = "GET /x HTTP/1.1\r\nHost: a\r\n\r\n";
        let out2 = force_connection_close(h2);
        assert_eq!(out2, "GET /x HTTP/1.1\r\nHost: a\r\nConnection: close\r\n\r\n");
    }
}

/// Adapts the WebSocket to the plain byte stream yamux expects: each write
/// becomes one binary frame, reads drain binary frames, and everything else
/// is skipped (tungstenite answers pings internally).
struct WsIo {
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    buf: tokio_tungstenite::tungstenite::Bytes,
}

impl WsIo {
    fn new(ws: WebSocketStream<MaybeTlsStream<TcpStream>>) -> Self {
        Self { ws, buf: tokio_tungstenite::tungstenite::Bytes::new() }
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
                Poll::Ready(Some(Ok(WsMessage::Binary(data)))) => self.buf = data,
                Poll::Ready(Some(Ok(WsMessage::Close(_)))) | Poll::Ready(None) => {
                    return Poll::Ready(Ok(0));
                }
                Poll::Ready(Some(Ok(_))) => {} // ping/pong/text — not tunnel bytes
                Poll::Ready(Some(Err(e))) => return Poll::Ready(Err(io::Error::other(e))),
                Poll::Pending => return Poll::Pending,
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
        Pin::new(&mut self.ws).poll_close(cx).map_err(io::Error::other)
    }
}
