//! Outbound TLS trust: the ONE root policy for every HTTPS and WSS client.
//!
//! Every outbound connection — the hub gateway, the management tunnel, the
//! linked-employee chat socket, Janus and hub REST, voice realtime, OAuth,
//! downloads — gets its roots here. [`http_client`] starts every reqwest
//! client and [`connect_ws`] opens every WebSocket. There is no other way.
//!
//! Why this exists: reqwest and tokio-tungstenite each read the OS trust
//! store on their own — tungstenite on every connect, reqwest on every client
//! it builds — and treat an empty read as the answer. On macOS a read costs
//! ~140 ms, trustd serializes them, and in bursts every trust domain (user,
//! admin and system) fails with an I/O error and returns nothing. Every new
//! connection in such a burst failed with "no native root CA certificates
//! found": the hub tunnel dropped and linked employees went unreachable until
//! a later retry happened to land.
//!
//! The policy:
//! - Read the OS store once (system roots plus enterprise and user CAs) and
//!   keep it for the life of the process. Connections never re-read it.
//! - Keep every root the OS returned, even when some sources errored.
//! - Only when the OS gave no usable root, use the bundled public roots
//!   (`webpki-roots`), so every public endpoint stays reachable, and ask the
//!   OS again at most once per [`RETRY_EMPTY_READ`] until it answers.
//! - The first degraded read is logged once, plainly.
//!
//! Both constructors also spread their connections across every address a
//! hostname resolves to ([`spread`]): our edge publishes one A record per
//! node, and the client, not the OS resolver's ordering, decides where it
//! lands and moves on when a node does not answer. The TLS name, the SNI and
//! the `Host` header stay the hostname, and the certificate is verified
//! against it, whichever address carries the connection.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rustls::RootCertStore;
use rustls::pki_types::CertificateDer;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::client::Response;
use tokio_tungstenite::{Connector, MaybeTlsStream, WebSocketStream};
use tracing::{debug, info, warn};

/// Start an outbound HTTP client. Every reqwest client in Nebo is built from
/// this builder, so every one trusts the same roots.
pub fn http_client() -> reqwest::ClientBuilder {
    http_client_over(roots())
}

fn http_client_over(roots: Arc<RootCertStore>) -> reqwest::ClientBuilder {
    // reqwest's own builder offers h2 and http/1.1 over ALPN; a preconfigured
    // config carries its own list, so it must say the same.
    reqwest::Client::builder()
        .use_preconfigured_tls(client_config(
            roots,
            vec![b"h2".to_vec(), b"http/1.1".to_vec()],
        ))
        // Every address in a random order, tried in turn. reqwest splits the
        // connect budget evenly across the addresses it tries: a three-node
        // edge gives each node CONNECT_TIMEOUT. A caller may set its own.
        .dns_resolver(Arc::new(SpreadResolver))
        .connect_timeout(CONNECT_TIMEOUT * SPREAD_BUDGET_ADDRESSES)
}

/// The addresses [`http_client`]'s connect budget is sized for.
const SPREAD_BUDGET_ADDRESSES: u32 = 3;

/// reqwest's resolver: the system lookup, put in [`spread`] order.
struct SpreadResolver;

impl reqwest::dns::Resolve for SpreadResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let addrs = system_resolve(host, 0).await?;
            Ok(Box::new(spread(addrs).into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// Open an outbound WebSocket (`ws://` or `wss://`). Every WebSocket client in
/// Nebo connects through here, so every one trusts the same roots.
pub async fn connect_ws<R>(
    request: R,
) -> Result<
    (WebSocketStream<MaybeTlsStream<TcpStream>>, Response),
    tokio_tungstenite::tungstenite::Error,
>
where
    R: IntoClientRequest + Unpin,
{
    connect_ws_over(request, roots(), system_resolve, Timeouts::DIAL).await
}

/// How long one address may take to accept the TCP connection before the
/// next address is tried.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// How long a connected address may take to finish TLS and the WebSocket
/// upgrade before the next address is tried.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// The per-address limits of one WebSocket dial.
#[derive(Clone, Copy)]
struct Timeouts {
    connect: Duration,
    handshake: Duration,
}

impl Timeouts {
    const DIAL: Timeouts = Timeouts {
        connect: CONNECT_TIMEOUT,
        handshake: HANDSHAKE_TIMEOUT,
    };
}

type WsError = tokio_tungstenite::tungstenite::Error;

/// Dial `request` over the addresses `resolve` gives for its host, in
/// [`spread`] order. An address that refuses, resets or does not answer in
/// time gives way to the next; an answer (any HTTP response, including a
/// refusal) ends the dial. The error is the last address's when all fail.
async fn connect_ws_over<R, F, Fut>(
    request: R,
    roots: Arc<RootCertStore>,
    resolve: F,
    timeouts: Timeouts,
) -> Result<(WebSocketStream<MaybeTlsStream<TcpStream>>, Response), WsError>
where
    R: IntoClientRequest + Unpin,
    F: FnOnce(String, u16) -> Fut,
    Fut: std::future::Future<Output = std::io::Result<Vec<SocketAddr>>>,
{
    use tokio_tungstenite::tungstenite::error::UrlError;

    let request = request.into_client_request()?;
    let uri = request.uri();
    let host = uri
        .host()
        .ok_or(WsError::Url(UrlError::NoHostName))?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    let port = uri
        .port_u16()
        .or(match uri.scheme_str() {
            Some("wss") => Some(443),
            Some("ws") => Some(80),
            _ => None,
        })
        .ok_or(WsError::Url(UrlError::UnsupportedUrlScheme))?;

    let addrs = spread(resolve(host.clone(), port).await.map_err(WsError::Io)?);
    // No ALPN: a WebSocket upgrade is HTTP/1.1 only.
    let config = Arc::new(client_config(roots, Vec::new()));
    let mut last = WsError::Io(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("{host} resolved to no address"),
    ));
    for addr in addrs {
        let socket = match tokio::time::timeout(timeouts.connect, TcpStream::connect(addr)).await {
            Ok(Ok(socket)) => socket,
            Ok(Err(e)) => {
                debug!(%host, %addr, error = %e, "ws dial: address refused; trying the next");
                last = WsError::Io(e);
                continue;
            }
            Err(_) => {
                debug!(%host, %addr, "ws dial: address did not answer; trying the next");
                last = timed_out(&host, addr);
                continue;
            }
        };
        // The request still names the hostname: TLS sends it as the SNI and
        // verifies the certificate against it, and the upgrade carries it
        // as the Host header.
        let handshake = tokio_tungstenite::client_async_tls_with_config(
            request.clone(),
            socket,
            None,
            Some(Connector::Rustls(config.clone())),
        );
        match tokio::time::timeout(timeouts.handshake, handshake).await {
            Ok(Err(WsError::Io(e))) => {
                debug!(%host, %addr, error = %e, "ws dial: handshake failed; trying the next");
                last = WsError::Io(e);
            }
            Ok(answered) => return answered,
            Err(_) => {
                debug!(%host, %addr, "ws dial: handshake did not finish; trying the next");
                last = timed_out(&host, addr);
            }
        }
    }
    Err(last)
}

fn timed_out(host: &str, addr: SocketAddr) -> WsError {
    WsError::Io(std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        format!("{host} at {addr} timed out"),
    ))
}

/// The system resolver: every address the OS returns for `host`, fresh on
/// every call, so each dial sees the records as they are now.
async fn system_resolve(host: String, port: u16) -> std::io::Result<Vec<SocketAddr>> {
    Ok(tokio::net::lookup_host((host.as_str(), port)).await?.collect())
}

/// The order to try `addrs` in: each address family shuffled, the families
/// in the order the OS put them (so a preference for IPv6 or IPv4 stands).
/// Many clients dialing one hostname land evenly across its addresses.
pub fn spread(addrs: Vec<SocketAddr>) -> Vec<SocketAddr> {
    let first_v6 = addrs.first().is_some_and(SocketAddr::is_ipv6);
    let (mut v6, mut v4): (Vec<_>, Vec<_>) = addrs.into_iter().partition(SocketAddr::is_ipv6);
    shuffle(&mut v6);
    shuffle(&mut v4);
    if first_v6 {
        v6.extend(v4);
        v6
    } else {
        v4.extend(v6);
        v4
    }
}

/// Fisher-Yates over a per-call random seed.
fn shuffle<T>(items: &mut [T]) {
    for i in (1..items.len()).rev() {
        items.swap(i, random_below(i + 1));
    }
}

/// A random index in `0..n`, from std's randomly keyed hasher (no extra
/// dependency; ample for spreading load).
fn random_below(n: usize) -> usize {
    use std::hash::{BuildHasher, Hasher};
    static CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(CALLS.fetch_add(1, Ordering::Relaxed));
    (h.finish() % n as u64) as usize
}

/// How long the bundled roots stand in after the OS returned none before the
/// OS store is read again.
const RETRY_EMPTY_READ: Duration = Duration::from_secs(30);

/// Where a root store's certificates came from.
#[derive(Debug, PartialEq, Eq)]
enum Source {
    /// The OS store, read without error.
    System,
    /// The certificates the OS returned, though some sources errored.
    Partial,
    /// The OS gave no usable root; the bundled public roots.
    Bundled,
}

/// What the OS loader returned: certificates, and the errors it hit.
type Loaded = (Vec<CertificateDer<'static>>, Vec<String>);

/// The process's roots: read once, reused by every connection.
struct Roots {
    held: Mutex<Option<Held>>,
    /// Set once the first degraded read has been logged.
    degraded_logged: AtomicBool,
}

enum Held {
    /// Roots the OS returned. Kept for the life of the process.
    Os(Arc<RootCertStore>),
    /// The bundled roots, standing in since `since` because the OS returned none.
    Bundled {
        store: Arc<RootCertStore>,
        since: Instant,
    },
}

impl Roots {
    const fn new() -> Self {
        Self {
            held: Mutex::new(None),
            degraded_logged: AtomicBool::new(false),
        }
    }

    /// The roots to use at `now`, reading the OS store through `load` only
    /// when nothing usable is held yet.
    fn get(&self, now: Instant, load: impl FnOnce() -> Loaded) -> Arc<RootCertStore> {
        match &*self.held.lock().unwrap_or_else(|e| e.into_inner()) {
            Some(Held::Os(store)) => return store.clone(),
            Some(Held::Bundled { store, since })
                if now.duration_since(*since) < RETRY_EMPTY_READ =>
            {
                return store.clone();
            }
            _ => {}
        }

        // Read outside the lock: a slow read must not stall callers that are
        // already served by a held store.
        let (certs, errors) = load();
        let (store, source) = root_store(certs, &errors);
        let store = Arc::new(store);
        self.log(&source, store.len(), &errors);

        let mut held = self.held.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(Held::Os(existing)) = &*held {
            // Another caller's read landed first; keep one store.
            return existing.clone();
        }
        *held = Some(match source {
            Source::System | Source::Partial => Held::Os(store.clone()),
            Source::Bundled => Held::Bundled {
                store: store.clone(),
                since: now,
            },
        });
        store
    }

    fn log(&self, source: &Source, roots: usize, errors: &[String]) {
        match source {
            Source::System => {
                if self.degraded_logged.load(Ordering::Relaxed) {
                    info!(roots, "tls: the system trust store was read in full");
                }
            }
            Source::Partial => {
                if !self.degraded_logged.swap(true, Ordering::Relaxed) {
                    warn!(
                        roots,
                        ?errors,
                        "tls: the system trust store could only be read in part; using the roots it returned"
                    );
                }
            }
            Source::Bundled => {
                if !self.degraded_logged.swap(true, Ordering::Relaxed) {
                    warn!(
                        roots,
                        ?errors,
                        "tls: the system trust store returned no roots; using the bundled public roots until it does"
                    );
                }
            }
        }
    }
}

static ROOTS: Roots = Roots::new();

/// The roots every outbound client trusts.
fn roots() -> Arc<RootCertStore> {
    ROOTS.get(Instant::now(), || {
        let loaded = rustls_native_certs::load_native_certs();
        (
            loaded.certs,
            loaded.errors.iter().map(ToString::to_string).collect(),
        )
    })
}

/// Build the root store from what the OS loader returned: every usable
/// certificate it gave, or the bundled public roots when it gave none.
fn root_store(certs: Vec<CertificateDer<'static>>, errors: &[String]) -> (RootCertStore, Source) {
    let mut store = RootCertStore::empty();
    store.add_parsable_certificates(certs);
    if store.is_empty() {
        store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        return (store, Source::Bundled);
    }
    let source = if errors.is_empty() {
        Source::System
    } else {
        Source::Partial
    };
    (store, source)
}

/// A client config over `roots` offering `alpn`.
fn client_config(roots: Arc<RootCertStore>, alpn: Vec<Vec<u8>>) -> rustls::ClientConfig {
    // A process-wide provider wins if one was installed; otherwise ring, the
    // one this workspace compiles — the same choice reqwest makes.
    let provider = rustls::crypto::CryptoProvider::get_default()
        .cloned()
        .unwrap_or_else(|| Arc::new(rustls::crypto::ring::default_provider()));
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("the ring provider supports the default TLS versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = alpn;
    config
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generated_cert() -> CertificateDer<'static> {
        let cert = rcgen::generate_simple_self_signed(vec!["example.com".into()])
            .expect("generate self-signed cert");
        CertificateDer::from(cert.cert.der().to_vec())
    }

    /// The locked-Mac read: the user domain errors, the other domains come
    /// back empty. The store must still hold roots.
    #[test]
    fn an_empty_read_with_errors_falls_back_to_the_bundled_roots() {
        let errors =
            vec!["failed to load user trust settings: interaction not allowed".to_string()];
        let (store, source) = root_store(Vec::new(), &errors);
        assert_eq!(source, Source::Bundled);
        assert!(!store.is_empty());
        assert_eq!(store.len(), webpki_roots::TLS_SERVER_ROOTS.len());
    }

    /// Certificates the OS did return are kept, and nothing is added to them.
    #[test]
    fn a_partial_read_keeps_the_roots_it_returned() {
        let errors =
            vec!["failed to load user trust settings: interaction not allowed".to_string()];
        let (store, source) = root_store(vec![generated_cert()], &errors);
        assert_eq!(source, Source::Partial);
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn a_clean_read_is_the_system_store() {
        let (store, source) = root_store(vec![generated_cert()], &[]);
        assert_eq!(source, Source::System);
        assert_eq!(store.len(), 1);
    }

    /// Certificates rustls cannot parse are no roots at all.
    #[test]
    fn a_read_of_only_unparsable_certificates_falls_back() {
        let junk = CertificateDer::from(vec![0u8; 16]);
        let (store, source) = root_store(vec![junk], &[]);
        assert_eq!(source, Source::Bundled);
        assert_eq!(store.len(), webpki_roots::TLS_SERVER_ROOTS.len());
    }

    /// A generated "localhost" certificate and a TLS acceptor serving it.
    fn local_tls_server() -> (CertificateDer<'static>, tokio_rustls::TlsAcceptor) {
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()])
            .expect("generate self-signed cert");
        let cert_der = CertificateDer::from(cert.cert.der().to_vec());
        let key_der = rustls::pki_types::PrivateKeyDer::try_from(cert.key_pair.serialize_der())
            .expect("private key");
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("versions")
            .with_no_client_auth()
            .with_single_cert(vec![cert_der.clone()], key_der)
            .expect("server config");
        (cert_der, tokio_rustls::TlsAcceptor::from(Arc::new(config)))
    }

    fn trusting(cert: CertificateDer<'static>) -> Arc<RootCertStore> {
        let mut store = RootCertStore::empty();
        store.add(cert).expect("add root");
        Arc::new(store)
    }

    /// reqwest accepts the shared config (a wrong type is a build error) and
    /// completes a verified handshake with it.
    #[tokio::test]
    async fn the_http_client_completes_a_verified_handshake() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (cert, acceptor) = local_tls_server();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut tls = acceptor.accept(stream).await.expect("server handshake");
            let mut buf = [0u8; 1024];
            let _ = tls.read(&mut buf).await.unwrap();
            tls.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok")
                .await
                .unwrap();
            tls.shutdown().await.ok();
        });

        let client = http_client_over(trusting(cert)).build().expect("client");
        let resp = client
            .get(format!("https://localhost:{port}/"))
            .send()
            .await
            .expect("HTTPS request");
        assert_eq!(resp.text().await.unwrap(), "ok");
        server.await.unwrap();
    }

    /// The WebSocket path completes a verified handshake with the shared config.
    #[tokio::test]
    async fn the_websocket_client_completes_a_verified_handshake() {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;
        let (cert, acceptor) = local_tls_server();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let tls = acceptor.accept(stream).await.expect("server handshake");
            let mut ws = tokio_tungstenite::accept_async(tls)
                .await
                .expect("server ws");
            if let Some(Ok(msg)) = ws.next().await {
                ws.send(msg).await.unwrap();
            }
            ws.close(None).await.ok();
        });

        let (mut ws, _) = connect_ws_over(format!("wss://localhost:{port}/"), trusting(cert), system_resolve, Timeouts::DIAL)
            .await
            .expect("wss connect");
        ws.send(Message::Text("ping".into())).await.unwrap();
        let echoed = ws.next().await.expect("echo").expect("frame");
        assert_eq!(echoed.into_text().unwrap(), "ping");
        server.await.unwrap();
    }

    /// A server the roots do not vouch for is refused.
    #[tokio::test]
    async fn an_untrusted_server_is_refused() {
        let (_, acceptor) = local_tls_server();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let _ = acceptor.accept(stream).await;
        });
        let result = connect_ws_over(
            format!("wss://localhost:{port}/"),
            trusting(generated_cert()),
            system_resolve,
            Timeouts::DIAL,
        )
        .await;
        assert!(result.is_err());
    }

    // ---- Spreading across a hostname's addresses -------------------------

    /// A fake resolver: `host` resolves to `addrs`, whatever the system says.
    fn resolving_to(
        addrs: Vec<SocketAddr>,
    ) -> impl FnOnce(String, u16) -> std::future::Ready<std::io::Result<Vec<SocketAddr>>> {
        move |_, _| std::future::ready(Ok(addrs))
    }

    /// A plain WebSocket server that counts the upgrades it accepts, then
    /// closes each with `close` (a code), or keeps it open when `None`.
    async fn counting_ws_server(
        close: Option<u16>,
    ) -> (SocketAddr, Arc<std::sync::atomic::AtomicUsize>) {
        use futures_util::SinkExt;
        use tokio_tungstenite::tungstenite::Message;
        use tokio_tungstenite::tungstenite::protocol::CloseFrame;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = count.clone();
        tokio::spawn(async move {
            loop {
                let (sock, _) = listener.accept().await.unwrap();
                let counted = counted.clone();
                tokio::spawn(async move {
                    let Ok(mut ws) = tokio_tungstenite::accept_async(sock).await else {
                        return;
                    };
                    counted.fetch_add(1, Ordering::SeqCst);
                    if let Some(code) = close {
                        let frame = CloseFrame { code: code.into(), reason: "drain".into() };
                        let _ = ws.send(Message::Close(Some(frame))).await;
                    }
                    futures_util::future::pending::<()>().await;
                });
            }
        });
        (addr, count)
    }

    /// An address nothing listens on: a dial there is refused at once.
    /// (Port 1 on loopback, not a freed ephemeral port: the OS may hand that
    /// port to the dial's own socket, and a SYN to it is dropped, not
    /// refused.)
    async fn refusing_addr() -> SocketAddr {
        let addr: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let refused = TcpStream::connect(addr).await.expect_err("port 1 must be closed");
        assert_eq!(refused.kind(), std::io::ErrorKind::ConnectionRefused);
        addr
    }

    /// An address that takes the TCP connection and never says a word.
    async fn silent_addr() -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let mut held = Vec::new();
            loop {
                let (sock, _) = listener.accept().await.unwrap();
                held.push(sock);
            }
        });
        addr
    }

    /// Roots for the plain `ws://` dials: none needed, and the OS store is
    /// not read (a slow read would count against the timings).
    fn plain() -> Arc<RootCertStore> {
        Arc::new(RootCertStore::empty())
    }

    const FAST: Timeouts = Timeouts {
        connect: Duration::from_millis(300),
        handshake: Duration::from_millis(300),
    };

    /// Many dials to one hostname land on every address it resolves to,
    /// not all on the first one the resolver listed.
    #[tokio::test]
    async fn dials_spread_across_every_address() {
        let (a, at_a) = counting_ws_server(None).await;
        let (b, at_b) = counting_ws_server(None).await;
        let (c, at_c) = counting_ws_server(None).await;
        let mut held = Vec::new();
        for _ in 0..90 {
            let (ws, _) = connect_ws_over(
                "ws://edge.test/ws",
                plain(),
                resolving_to(vec![a, b, c]),
                FAST,
            )
            .await
            .expect("dial");
            held.push(ws);
        }
        let counts = [&at_a, &at_b, &at_c].map(|n| n.load(Ordering::SeqCst));
        assert_eq!(counts.iter().sum::<usize>(), 90);
        for n in counts {
            assert!(n >= 10, "uneven spread {counts:?}");
        }
    }

    /// An address that refuses, or takes the connection and never answers,
    /// gives way to the next address within the same dial: every dial
    /// succeeds, none waits on a backoff.
    #[tokio::test]
    async fn a_dead_address_fails_over_to_the_next_within_the_dial() {
        let (live, at_live) = counting_ws_server(None).await;
        let refused = refusing_addr().await;
        let silent = silent_addr().await;
        let mut held = Vec::new();
        for _ in 0..30 {
            let started = Instant::now();
            let (ws, _) = connect_ws_over(
                "ws://edge.test/ws",
                plain(),
                resolving_to(vec![refused, silent, live]),
                FAST,
            )
            .await
            .expect("a live address remains");
            // At worst: a refusal, the silent address's handshake limit, and
            // the live dial.
            assert!(started.elapsed() < FAST.handshake * 3, "{:?}", started.elapsed());
            held.push(ws);
        }
        assert_eq!(at_live.load(Ordering::SeqCst), 30);
    }

    /// When every address fails the dial fails, promptly, with an error.
    #[tokio::test]
    async fn every_address_failing_fails_the_dial() {
        let addrs = vec![refusing_addr().await, silent_addr().await, silent_addr().await];
        let started = Instant::now();
        let result = connect_ws_over("ws://edge.test/ws", plain(), resolving_to(addrs), FAST).await;
        assert!(result.is_err());
        assert!(started.elapsed() < FAST.handshake * 3, "{:?}", started.elapsed());
    }

    /// An HTTP answer is final: a refusal (here a 421 naming another cell)
    /// comes back to the caller instead of being retried on the next node.
    #[tokio::test]
    async fn an_http_refusal_is_returned_not_retried() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let refusing = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut head = [0u8; 2048];
                let _ = sock.read(&mut head).await;
                let body = r#"{"error":"wrong_cell","cell":"2","url":"wss://cell-2.neboai.com/ws"}"#;
                let reply = format!(
                    "HTTP/1.1 421 Misdirected Request\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(reply.as_bytes()).await;
            }
        });
        let (live, at_live) = counting_ws_server(None).await;
        let result = connect_ws_over(
            "ws://edge.test/ws",
            plain(),
            // The refusing node first, always.
            |_, _| std::future::ready(Ok(vec![refusing])),
            FAST,
        )
        .await;
        match result {
            Err(WsError::Http(resp)) => assert_eq!(resp.status().as_u16(), 421),
            other => panic!("expected the 421, got {:?}", other.map(|_| ())),
        }
        assert_eq!(at_live.load(Ordering::SeqCst), 0);
        let _ = live;
    }

    /// Every dial asks the resolver again: after a node closes with 1012
    /// (service restart) and leaves DNS, the redial goes where the records
    /// point now.
    #[tokio::test]
    async fn every_dial_resolves_afresh_after_a_restart_close() {
        use futures_util::StreamExt;
        let (draining, at_draining) = counting_ws_server(Some(1012)).await;
        let (fresh, at_fresh) = counting_ws_server(None).await;
        let records = Arc::new(Mutex::new(vec![draining]));
        let lookups = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let resolver = || {
            let records = records.clone();
            let lookups = lookups.clone();
            move |_: String, _: u16| {
                lookups.fetch_add(1, Ordering::SeqCst);
                std::future::ready(Ok(records.lock().unwrap().clone()))
            }
        };

        let (mut ws, _) = connect_ws_over("ws://edge.test/ws", plain(), resolver(), FAST)
            .await
            .expect("first dial");
        let closed = loop {
            match ws.next().await {
                Some(Ok(tokio_tungstenite::tungstenite::Message::Close(frame))) => break frame,
                Some(Ok(_)) => continue,
                other => panic!("expected a close, got {other:?}"),
            }
        };
        assert_eq!(u16::from(closed.expect("close frame").code), 1012);

        // The node left DNS; the next dial must not reuse the old answer.
        *records.lock().unwrap() = vec![fresh];
        let (_ws, _) = connect_ws_over("ws://edge.test/ws", plain(), resolver(), FAST)
            .await
            .expect("redial");
        assert_eq!(lookups.load(Ordering::SeqCst), 2);
        assert_eq!(at_draining.load(Ordering::SeqCst), 1);
        assert_eq!(at_fresh.load(Ordering::SeqCst), 1);
    }

    /// The connection goes to an address, but TLS still names the hostname
    /// and checks the certificate against it: a certificate for the
    /// hostname passes, and the same server reached under another name is
    /// refused.
    #[tokio::test]
    async fn tls_verifies_the_hostname_not_the_address() {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;
        let (cert, acceptor) = local_tls_server();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let host_seen = Arc::new(Mutex::new(String::new()));
        let seen = host_seen.clone();
        tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let acceptor = acceptor.clone();
                let seen = seen.clone();
                tokio::spawn(async move {
                    let Ok(tls) = acceptor.accept(stream).await else { return };
                    let sni = tls.get_ref().1.server_name().unwrap_or_default().to_string();
                    let check = move |req: &tokio_tungstenite::tungstenite::handshake::server::Request,
                                      resp| {
                        let host = req.headers().get("host").and_then(|h| h.to_str().ok()).unwrap_or("");
                        *seen.lock().unwrap() = format!("{sni} {host}");
                        Ok(resp)
                    };
                    let Ok(mut ws) = tokio_tungstenite::accept_hdr_async(tls, check).await else { return };
                    if let Some(Ok(msg)) = ws.next().await {
                        let _ = ws.send(msg).await;
                    }
                });
            }
        });

        let refused = refusing_addr().await;
        let (mut ws, _) = connect_ws_over(
            "wss://localhost/ws",
            trusting(cert.clone()),
            resolving_to(vec![refused, addr]),
            FAST,
        )
        .await
        .expect("verified by hostname over the address");
        ws.send(Message::Text("ping".into())).await.unwrap();
        assert_eq!(ws.next().await.unwrap().unwrap().into_text().unwrap(), "ping");
        assert_eq!(*host_seen.lock().unwrap(), "localhost localhost");

        let wrong_name = connect_ws_over(
            "wss://edge.test/ws",
            trusting(cert),
            resolving_to(vec![addr]),
            FAST,
        )
        .await;
        assert!(wrong_name.is_err(), "a certificate for another name must be refused");
    }

    /// The spread keeps the OS's family preference and shuffles within it.
    #[test]
    fn spread_keeps_the_family_order_and_shuffles_within_it() {
        let v4: Vec<SocketAddr> = (1..=3).map(|n| format!("192.0.2.{n}:443").parse().unwrap()).collect();
        let v6: Vec<SocketAddr> = (1..=3).map(|n| format!("[2001:db8::{n}]:443").parse().unwrap()).collect();
        let mut firsts = std::collections::HashSet::new();
        for _ in 0..200 {
            let mut os = v6.clone();
            os.extend(v4.clone());
            let order = spread(os);
            assert_eq!(order.len(), 6);
            assert!(order[..3].iter().all(SocketAddr::is_ipv6), "{order:?}");
            assert!(order[3..].iter().all(SocketAddr::is_ipv4), "{order:?}");
            firsts.insert(order[0]);

            let mut os = v4.clone();
            os.extend(v6.clone());
            assert!(spread(os)[0].is_ipv4());
        }
        assert_eq!(firsts.len(), 3, "every address comes first sometimes");
    }

    /// The HTTP client resolves through the spread too.
    #[tokio::test]
    async fn the_http_resolver_returns_every_address() {
        use reqwest::dns::Resolve;
        let name: reqwest::dns::Name = "localhost".parse().unwrap();
        let addrs: Vec<SocketAddr> = SpreadResolver.resolve(name).await.unwrap().collect();
        assert!(addrs.iter().all(|a| a.ip().is_loopback()) && !addrs.is_empty(), "{addrs:?}");
    }

    /// Once the OS store has been read, a failing OS changes nothing: new
    /// connections reuse the roots already held and the OS is not asked again.
    #[test]
    fn a_failing_os_after_a_good_read_does_not_touch_new_connections() {
        let roots = Roots::new();
        let start = Instant::now();
        let first = roots.get(start, || (vec![generated_cert()], Vec::new()));
        assert_eq!(first.len(), 1);

        let later = start + RETRY_EMPTY_READ * 10;
        let again = roots.get(later, || panic!("the OS store was read again"));
        assert!(Arc::ptr_eq(&first, &again));
    }

    /// A partial read counts as a read: its roots are kept, not re-read.
    #[test]
    fn a_partial_read_is_kept_for_the_process() {
        let roots = Roots::new();
        let start = Instant::now();
        let first = roots.get(start, || {
            (
                vec![generated_cert()],
                vec!["failed to load user trust settings".into()],
            )
        });
        let again = roots.get(start + RETRY_EMPTY_READ * 10, || {
            panic!("the OS store was read again")
        });
        assert!(Arc::ptr_eq(&first, &again));
    }

    /// An empty read stands in the bundled roots, is not re-read on every
    /// connection, and gives way to the OS store once the OS answers.
    #[test]
    fn an_empty_read_is_retried_on_an_interval_not_per_connection() {
        let roots = Roots::new();
        let start = Instant::now();
        let failed = || {
            (
                Vec::new(),
                vec!["failed to load system trust settings: ioErr".to_string()],
            )
        };

        let bundled = roots.get(start, failed);
        assert_eq!(bundled.len(), webpki_roots::TLS_SERVER_ROOTS.len());

        let soon = roots.get(start + Duration::from_secs(1), || {
            panic!("re-read before the interval")
        });
        assert!(Arc::ptr_eq(&bundled, &soon));

        let after = start + RETRY_EMPTY_READ + Duration::from_secs(1);
        let recovered = roots.get(after, || (vec![generated_cert()], Vec::new()));
        assert_eq!(recovered.len(), 1);

        let kept = roots.get(after + RETRY_EMPTY_READ * 10, || {
            panic!("the OS store was read again")
        });
        assert!(Arc::ptr_eq(&recovered, &kept));
    }

    /// Whatever state the OS store is in on the machine running the tests,
    /// the roots handed to clients are never empty.
    #[test]
    fn the_live_roots_are_never_empty() {
        assert!(!roots().is_empty());
    }

    /// Every outbound client is built by this crate. A reqwest client or a
    /// WebSocket opened anywhere else reads the OS store on its own and fails
    /// every connection whenever that read comes back empty.
    #[test]
    fn no_outbound_client_is_built_outside_this_crate() {
        let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut roots = vec![workspace.join("src-tauri/src")];
        for entry in std::fs::read_dir(workspace.join("crates")).expect("crates dir") {
            let dir = entry.expect("crate entry").path();
            if dir.file_name().is_some_and(|n| n == "tls") {
                continue;
            }
            roots.push(dir.join("src"));
            // Nested crates (crates/a2ui/*).
            if let Ok(nested) = std::fs::read_dir(&dir) {
                for sub in nested.flatten() {
                    roots.push(sub.path().join("src"));
                }
            }
        }

        const FORBIDDEN: &[&str] = &[
            "reqwest::Client::new(",
            "reqwest::Client::builder(",
            "reqwest::Client::default(",
            "reqwest::ClientBuilder::new(",
            "reqwest::get(",
            "connect_async(",
            "connect_async_with_config(",
            "connect_async_tls_with_config(",
            "client_async_tls(",
            "client_async_tls_with_config(",
        ];
        // Files that import reqwest's Client call it by its short name.
        const FORBIDDEN_SHORT: &[&str] = &["Client::new(", "Client::builder(", "Client::default("];

        let mut offenders = Vec::new();
        let mut files = Vec::new();
        for root in roots {
            collect_rs(&root, &mut files);
        }
        for file in files {
            let text = std::fs::read_to_string(&file).expect("read source");
            // Production code only: stop at the unit-test module.
            let code = text.split("#[cfg(test)]\nmod ").next().unwrap_or("");
            let imports_client = code.contains("use reqwest::Client;");
            for (n, line) in code.lines().enumerate() {
                if line.trim_start().starts_with("//") {
                    continue;
                }
                let hit = FORBIDDEN.iter().any(|p| line.contains(p))
                    || imports_client
                        && FORBIDDEN_SHORT.iter().any(|p| {
                            line.match_indices(p).any(|(at, _)| {
                                !line[..at]
                                    .chars()
                                    .last()
                                    .is_some_and(|c| c.is_alphanumeric() || c == '_')
                            })
                        });
                if hit {
                    offenders.push(format!("{}:{}: {}", file.display(), n + 1, line.trim()));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "outbound clients must come from tls::http_client() / tls::connect_ws():\n{}",
            offenders.join("\n")
        );
    }

    fn collect_rs(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_rs(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
}
