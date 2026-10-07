//! Shared HTTP client builders for AI providers.
//!
//! Default `reqwest::Client::new()` has no timeouts. Cloud providers behind
//! load balancers and NAT silently drop idle keep-alive connections, causing
//! the next request to hang forever on `read()`. That manifests as the chat
//! bot showing "Thinking..." indefinitely until the user sends another message
//! (which forces a fresh connection).
//!
//! These builders apply sensible defaults so a dead connection turns into a
//! quick error that the runner can recover from, instead of a permanent hang.
//!
//! `streaming_client()` is the right default for SSE chat-completion calls.
//! `request_client()` is for short JSON request/response (e.g. embeddings,
//! version checks) where we want a hard total cap.

use std::time::Duration;

/// A response's `Retry-After`, in seconds (the delta-seconds form; the
/// HTTP-date form is not sent by any provider we call).
pub fn retry_after_secs(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse().ok())
}

/// HTTP client for streaming chat completions (Anthropic, OpenAI, Janus,
/// DeepSeek, Gemini). Long-running SSE — no total request timeout, but we
/// detect dead connections via TCP keepalive and proactively recycle idle
/// pool entries so we never hand out a stale half-closed socket.
pub fn streaming_client() -> reqwest::Client {
    tls::http_client()
        .user_agent(types::constants::USER_AGENT)
        .connect_timeout(Duration::from_secs(10))
        // Close idle keep-alives well before typical LB idle reap (~60-120s).
        .pool_idle_timeout(Duration::from_secs(30))
        // OS-level keepalive probes. After ~3 failed probes (~45s), reads
        // return ECONNRESET instead of blocking forever.
        .tcp_keepalive(Duration::from_secs(15))
        // Per-read deadline on the streaming body. Generous on purpose:
        // providers BUFFER tool-call arguments, so a model emitting a large
        // call (a 30KB document spec) goes wire-silent for minutes while the
        // JSON generates upstream. At 60s this deadline guillotined every
        // long generation at the same mark — the retry restarted the same
        // emission and died identically, an infinite loop. Dead sockets are
        // the keepalive probes' job (~45s); silence is not death.
        .read_timeout(Duration::from_secs(300))
        .build()
        .expect("reqwest streaming client builder is infallible with these options")
}

/// HTTP client for short request/response calls (Janus decisions, embeddings,
/// model listing, version pings). Has a hard total timeout since these
/// complete quickly.
///
/// These calls come in bursts separated by quiet minutes, and a cold
/// connection to Janus costs a full TLS handshake (0.8-1.8 s against 0.37 s
/// warm). So idle connections live until just under edgelb's 120 s idle
/// reap, and HTTP/2 pings keep a quiet connection open and prove it alive:
/// a dead one fails its ping and leaves the pool instead of being handed out.
pub fn request_client() -> reqwest::Client {
    tls::http_client()
        .user_agent(types::constants::USER_AGENT)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .pool_idle_timeout(Duration::from_secs(110))
        .tcp_keepalive(Duration::from_secs(15))
        .http2_keep_alive_interval(Duration::from_secs(30))
        .http2_keep_alive_timeout(Duration::from_secs(10))
        .http2_keep_alive_while_idle(true)
        .build()
        .expect("reqwest request client builder is infallible with these options")
}

#[cfg(test)]
mod tests {
    /// The request line and headers one call sends to a local listener.
    async fn headers_sent_by(client: reqwest::Client) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1/models", listener.local_addr().unwrap());
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
            let _ = sock.write_all(b"HTTP/1.1 204 No Content\r\ncontent-length: 0\r\n\r\n").await;
            String::from_utf8_lossy(&got).to_ascii_lowercase()
        });
        let _ = client.get(&url).send().await;
        seen.await.unwrap()
    }

    /// Every call to Janus through these clients names this Nebo and its
    /// version, which Janus logs per session.
    #[tokio::test]
    async fn both_clients_name_nebo_and_its_version() {
        let want = format!("user-agent: nebo/{}", env!("CARGO_PKG_VERSION"));
        for client in [super::request_client(), super::streaming_client()] {
            let sent = headers_sent_by(client).await;
            assert!(sent.contains(&want), "no {want} in: {sent}");
        }
    }
}
