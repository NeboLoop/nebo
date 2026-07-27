//! TLS handshake smoke tests — the gate the rustls migration shipped without.
//!
//! The workspace switched reqwest AND tokio-tungstenite from native-tls to
//! rustls (`e60c548d`) for platform reach: native-tls means openssl-sys (a C
//! library) on Linux/Android and security-framework/securityd on macOS. That
//! change was verified by watching live traffic — real evidence, but manual and
//! unrepeatable. Nothing in the suite would notice if a feature flag flipped a
//! stack back to native-tls (dragging openssl-sys into the tree and quietly
//! losing the platform goal) or left it with no TLS backend at all (every
//! https:// / wss:// URL fails at runtime while the build stays green).
//!
//! These tests need NO network: a local rustls server with a generated
//! certificate, and both client stacks must complete a real handshake against
//! it. If either stack loses its rustls backend, these fail in CI.

use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

/// The test binary links rustls with BOTH `ring` (our dev-dep) and `aws-lc-rs`
/// (reqwest's default) enabled, so rustls cannot auto-select a process-level
/// CryptoProvider and its builders panic. Pin ring, once. The shipping app does
/// not need this — each stack selects internally (proven by live connections).
fn install_crypto_provider() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// Generated cert for "localhost" + the rustls server config using it.
fn tls_server_parts() -> (rustls::pki_types::CertificateDer<'static>, Arc<rustls::ServerConfig>) {
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()])
        .expect("generate self-signed cert");
    let cert_der = rustls::pki_types::CertificateDer::from(cert.cert.der().to_vec());
    let key_der = rustls::pki_types::PrivateKeyDer::try_from(
        cert.key_pair.serialize_der(),
    )
    .expect("private key");

    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert_der.clone()], key_der)
        .expect("server config");
    (cert_der, Arc::new(config))
}

/// reqwest (HTTP stack) completes a rustls handshake.
///
/// `add_root_certificate` + a live GET against a local TLS server exercises the
/// full client path: root store construction, SNI, verification, HTTP over the
/// encrypted stream. Fails if reqwest is built without a working TLS backend.
#[tokio::test]
async fn reqwest_completes_rustls_handshake() {
    install_crypto_provider();
    let (cert_der, config) = tls_server_parts();
    let acceptor = TlsAcceptor::from(config);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut tls = acceptor.accept(stream).await.expect("server-side handshake");
        // Minimal HTTP: read the request, answer 200, close.
        let mut buf = [0u8; 1024];
        let _ = tls.read(&mut buf).await.unwrap();
        tls.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok")
            .await
            .unwrap();
        tls.shutdown().await.ok();
    });

    let client = reqwest::Client::builder()
        .add_root_certificate(reqwest::Certificate::from_der(&cert_der).unwrap())
        .build()
        .expect("client with TLS backend — fails here if no TLS feature is compiled in");

    let resp = client
        .get(format!("https://localhost:{}/", addr.port()))
        .send()
        .await
        .expect("HTTPS request over rustls");
    assert!(resp.status().is_success());
    assert_eq!(resp.text().await.unwrap(), "ok");

    server.await.unwrap();
}

/// tokio-tungstenite (websocket stack) completes a rustls handshake.
///
/// The loop connection, the tunnel, and voice realtime all ride wss:// through
/// tungstenite — a stack switched in the SAME commit as reqwest but with its own
/// TLS features. This is the half the original migration never live-verified;
/// leaving it on native-tls would have pulled openssl-sys straight back in while
/// every build stayed green.
#[tokio::test]
async fn tungstenite_completes_rustls_handshake() {
    install_crypto_provider();
    let (cert_der, config) = tls_server_parts();
    let acceptor = TlsAcceptor::from(config);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let tls = acceptor.accept(stream).await.expect("server-side handshake");
        let mut ws = tokio_tungstenite::accept_async(tls)
            .await
            .expect("server websocket handshake");
        use futures::{SinkExt, StreamExt};
        if let Some(Ok(msg)) = ws.next().await {
            ws.send(msg).await.unwrap(); // echo
        }
        ws.close(None).await.ok();
    });

    // Client trusts exactly the generated cert.
    let mut roots = rustls::RootCertStore::empty();
    roots.add(cert_der).unwrap();
    let client_config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();

    let (mut ws, _) = tokio_tungstenite::connect_async_tls_with_config(
        format!("wss://localhost:{}/", addr.port()),
        None,
        false,
        Some(tokio_tungstenite::Connector::Rustls(Arc::new(client_config))),
    )
    .await
    .expect("wss connect over rustls — fails here if tungstenite lost its rustls backend");

    use futures::{SinkExt, StreamExt};
    ws.send(tokio_tungstenite::tungstenite::Message::Text("ping".into()))
        .await
        .unwrap();
    let echoed = ws.next().await.expect("echo frame").expect("frame ok");
    assert_eq!(echoed.into_text().unwrap(), "ping");

    server.await.unwrap();
}
