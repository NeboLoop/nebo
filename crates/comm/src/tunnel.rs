//! Reverse management tunnel — the bot side of the loop-as-interface plan
//! (docs/plans/nebo-cloud-architecture.md, Plane B).
//!
//! The bot dials the hub over one outbound WebSocket authenticated with its
//! NeboAI bot token, then runs a yamux multiplexer over it in server mode:
//! the hub opens one stream per browser request and pipes raw HTTP/WS
//! through. Each inbound stream is proxied byte-for-byte to the local nebo
//! server, so the full REST + `/ws` surface works remotely unchanged — the
//! tunnel is the new localhost. Because the bot only ever dials out, the hub
//! is the only peer that can reach the local API.

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

/// Pipe one hub-opened stream to the local server, both directions, until
/// either side closes.
async fn proxy_stream(stream: yamux::Stream, local_addr: &str) -> io::Result<()> {
    let mut local = TcpStream::connect(local_addr).await?;
    let mut stream = stream.compat();
    tokio::io::copy_bidirectional(&mut stream, &mut local).await?;
    Ok(())
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
