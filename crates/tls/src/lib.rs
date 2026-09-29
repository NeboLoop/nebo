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

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rustls::RootCertStore;
use rustls::pki_types::CertificateDer;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::client::Response;
use tokio_tungstenite::{Connector, MaybeTlsStream, WebSocketStream};
use tracing::{info, warn};

/// Start an outbound HTTP client. Every reqwest client in Nebo is built from
/// this builder, so every one trusts the same roots.
pub fn http_client() -> reqwest::ClientBuilder {
    http_client_over(roots())
}

fn http_client_over(roots: Arc<RootCertStore>) -> reqwest::ClientBuilder {
    // reqwest's own builder offers h2 and http/1.1 over ALPN; a preconfigured
    // config carries its own list, so it must say the same.
    reqwest::Client::builder().use_preconfigured_tls(client_config(
        roots,
        vec![b"h2".to_vec(), b"http/1.1".to_vec()],
    ))
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
    connect_ws_over(request, roots()).await
}

async fn connect_ws_over<R>(
    request: R,
    roots: Arc<RootCertStore>,
) -> Result<
    (WebSocketStream<MaybeTlsStream<TcpStream>>, Response),
    tokio_tungstenite::tungstenite::Error,
>
where
    R: IntoClientRequest + Unpin,
{
    // No ALPN: a WebSocket upgrade is HTTP/1.1 only.
    let config = client_config(roots, Vec::new());
    tokio_tungstenite::connect_async_tls_with_config(
        request,
        None,
        false,
        Some(Connector::Rustls(Arc::new(config))),
    )
    .await
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

        let (mut ws, _) = connect_ws_over(format!("wss://localhost:{port}/"), trusting(cert))
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
        )
        .await;
        assert!(result.is_err());
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
