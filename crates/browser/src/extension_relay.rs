//! Chrome native-messaging relay — the ONE relay implementation.
//!
//! Chrome launches the Nebo binary as a native messaging host; this module is
//! the stdin/stdout ⟷ WebSocket bridge between the extension and the local
//! server's `/ws/extension` endpoint:
//!
//!   Extension ←stdin/stdout→ this process ←WebSocket→ Nebo server
//!
//! Both entry binaries (nebo-cli and the Tauri app) call [`run`] — never add a
//! second implementation. The native-messaging manifest may point at either
//! binary, so any behavior that lives in only one launch path is a bug
//! (this exact drift once locked the relay out of `/ws/extension` and capped
//! extension messages at 1 MB in one path but not the other).

use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use tokio::io::AsyncReadExt;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

/// Run the native messaging relay until either side disconnects.
/// Never returns on the happy path: exits the process so tokio's blocking
/// stdin thread can't prevent shutdown (Chrome's onDisconnect then fires and
/// the extension reconnects).
pub async fn run() -> anyhow::Result<()> {
    // NOTE: stdout is the native messaging channel — ALL diagnostic logging goes to stderr.
    eprintln!("[nebo-relay] starting native messaging bridge");

    let ws_url = "ws://127.0.0.1:27895/ws/extension";

    // Present the per-install relay secret so the server can tell this relay
    // from any other local WS client (a hostile web page can't read it). We
    // resolve the same default data dir as the server, so the value matches.
    let relay_secret = nebo_config::read_extension_secret().unwrap_or_default();

    // Build the upgrade request fresh each attempt (connect_async consumes it).
    let build_request = || -> anyhow::Result<_> {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut request = ws_url.into_client_request()?;
        let value = relay_secret
            .parse()
            .map_err(|_| anyhow::anyhow!("relay secret is not a valid header value"))?;
        request
            .headers_mut()
            .insert("X-Nebo-Extension-Secret", value);
        Ok(request)
    };

    // Retry WS connection with backoff — server may not be ready yet
    let ws_stream = {
        let mut attempts = 0u32;
        loop {
            let request = build_request()?;
            match connect_async(request).await {
                Ok((stream, _)) => {
                    eprintln!("[nebo-relay] connected to server at {}", ws_url);
                    break stream;
                }
                Err(e) if attempts < 10 => {
                    attempts += 1;
                    let delay = std::cmp::min(500 * 2u64.pow(attempts - 1), 5000);
                    eprintln!(
                        "[nebo-relay] WS connect attempt {}/10 failed ({}), retrying in {}ms",
                        attempts, e, delay
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                }
                Err(e) => {
                    eprintln!("[nebo-relay] giving up after 10 attempts: {}", e);
                    // Give up — exit so Chrome can retry later
                    std::process::exit(1);
                }
            }
        }
    };

    let (mut ws_tx, mut ws_rx) = ws_stream.split();

    // Detect which browser launched this relay (check parent process)
    let browser = detect_parent_browser();
    eprintln!("[nebo-relay] detected browser: {}", browser);

    // Send hello to server with browser identification (must be first message)
    let hello = serde_json::json!({
        "type": "hello",
        "browser": browser,
        "relay": true,
    });
    let _ = ws_tx
        .send(Message::Text(
            serde_json::to_string(&hello).unwrap().into(),
        ))
        .await;

    let mut stdin = tokio::io::stdin();
    let stdout = Arc::new(tokio::sync::Mutex::new(tokio::io::stdout()));

    let stdout_send = stdout.clone();

    // Task 1: Read from Chrome extension (stdin) → forward to server (WS)
    let send_task = tokio::spawn(async move {
        loop {
            // Read 4-byte length prefix
            let mut len_buf = [0u8; 4];
            if stdin.read_exact(&mut len_buf).await.is_err() {
                eprintln!("[nebo-relay] stdin closed — extension disconnected");
                break;
            }
            let len = u32::from_le_bytes(len_buf) as usize;
            // Chrome's 1 MB native-messaging cap applies HOST→EXTENSION only; the
            // extension legitimately sends larger payloads this way (screenshots
            // ~1.4 MB, full-page outerHTML several MB). Anything past this bound
            // means a corrupted length prefix — resync is impossible on a
            // length-prefixed stream, so exit and let Chrome relaunch the relay.
            const MAX_EXT_MSG_BYTES: usize = 64 * 1024 * 1024;
            if len > MAX_EXT_MSG_BYTES {
                eprintln!(
                    "[nebo-relay] implausible message length {} bytes — corrupted stream, exiting",
                    len
                );
                break;
            }

            // Read JSON body
            let mut body = vec![0u8; len];
            if stdin.read_exact(&mut body).await.is_err() {
                eprintln!("[nebo-relay] stdin read error");
                break;
            }

            let msg: serde_json::Value = match serde_json::from_slice(&body) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("[nebo-relay] malformed JSON from extension: {}", e);
                    continue;
                }
            };

            let msg_type = msg["type"].as_str().unwrap_or("");

            // Handle hello and ping locally — respond immediately via stdout
            match msg_type {
                "hello" => {
                    eprintln!(
                        "[nebo-relay] extension hello (v{}, id={})",
                        msg["version"].as_str().unwrap_or("?"),
                        msg["extension_id"].as_str().unwrap_or("?")
                    );
                    let resp = serde_json::json!({"type": "connected"});
                    let _ = write_native_message(&stdout_send, &resp).await;
                    // Also forward to server so it knows extension connected
                    let text = serde_json::to_string(&msg).unwrap();
                    let _ = ws_tx.send(Message::Text(text.into())).await;
                    continue;
                }
                "ping" => {
                    let resp = serde_json::json!({"type": "pong"});
                    let _ = write_native_message(&stdout_send, &resp).await;
                    continue;
                }
                _ => {}
            }

            // Forward everything else to the server
            eprintln!("[nebo-relay] ext→server: type={}", msg_type);
            let text = serde_json::to_string(&msg).unwrap();
            if ws_tx.send(Message::Text(text.into())).await.is_err() {
                eprintln!("[nebo-relay] WS send failed — server disconnected");
                break; // WS broke — exit so Chrome relaunches us
            }
        }
    });

    // Task 2: Read from server (WS) → forward to Chrome extension (stdout)
    let stdout_recv = stdout.clone();
    let recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_rx.next().await {
            match msg {
                Message::Text(text) => {
                    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text) {
                        let msg_type = parsed["type"].as_str().unwrap_or("");
                        eprintln!("[nebo-relay] server→ext: type={}", msg_type);
                        if write_native_message(&stdout_recv, &parsed).await.is_err() {
                            eprintln!("[nebo-relay] stdout write failed — extension disconnected");
                            break;
                        }
                    }
                }
                Message::Close(_) => {
                    eprintln!("[nebo-relay] WS closed by server");
                    break;
                }
                _ => {}
            }
        }
    });

    // Wait for either direction to close
    tokio::select! {
        _ = send_task => { eprintln!("[nebo-relay] send task ended"); }
        _ = recv_task => { eprintln!("[nebo-relay] recv task ended"); }
    }

    eprintln!("[nebo-relay] shutting down");
    // Force exit — tokio's blocking stdin thread prevents clean shutdown.
    // Chrome's onDisconnect will fire and the extension will reconnect.
    std::process::exit(0);
}

/// Write a native messaging response (4-byte length prefix + JSON) to stdout.
async fn write_native_message(
    stdout: &tokio::sync::Mutex<tokio::io::Stdout>,
    msg: &serde_json::Value,
) -> Result<(), std::io::Error> {
    use tokio::io::AsyncWriteExt;
    let json_bytes = serde_json::to_vec(msg).unwrap();
    let len = (json_bytes.len() as u32).to_le_bytes();
    let mut out = stdout.lock().await;
    out.write_all(&len).await?;
    out.write_all(&json_bytes).await?;
    out.flush().await?;
    Ok(())
}

/// Detect which browser launched this relay by checking the parent process name.
fn detect_parent_browser() -> String {
    #[cfg(unix)]
    {
        let ppid = std::os::unix::process::parent_id();
        if let Ok(output) = std::process::Command::new("ps")
            .args(["-p", &ppid.to_string(), "-o", "comm="])
            .output()
        {
            let parent = String::from_utf8_lossy(&output.stdout)
                .trim()
                .to_string()
                .to_lowercase();
            if parent.contains("brave") {
                return "brave".to_string();
            }
            if parent.contains("chrome") {
                return "chrome".to_string();
            }
            if parent.contains("firefox") {
                return "firefox".to_string();
            }
            if parent.contains("safari") {
                return "safari".to_string();
            }
            if parent.contains("edge") {
                return "edge".to_string();
            }
            if parent.contains("arc") {
                return "arc".to_string();
            }
            // Return the raw parent name if unrecognized
            if !parent.is_empty() {
                return parent;
            }
        }
    }
    "unknown".to_string()
}
