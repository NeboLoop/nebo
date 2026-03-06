//! Chrome Native Messaging Host
//!
//! Implements the Chrome native messaging protocol (4-byte length-prefix + JSON)
//! for communication between the Nebo desktop app and the Chrome extension.
//!
//! The host is bidirectional: it receives messages from the extension (tool results,
//! tab events) and sends messages to the extension (tool requests, indicator commands).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{oneshot, Mutex, RwLock};
use tracing::{debug, info, warn};

use crate::audit;
use crate::native_types::NativeMessage;
use crate::BrowserError;

/// Native messaging host — bridges the Chrome extension to the Nebo agent.
pub struct NativeHost {
    /// Writer for sending messages to the extension via stdout.
    writer: Arc<Mutex<tokio::io::Stdout>>,
    /// Pending tool responses from the extension, keyed by request ID.
    pending: Arc<RwLock<HashMap<i64, oneshot::Sender<Result<serde_json::Value, String>>>>>,
    /// Monotonic ID counter for outgoing tool requests.
    next_id: AtomicI64,
    /// Whether the extension is connected.
    connected: AtomicBool,
    /// Extension version (from handshake).
    extension_version: RwLock<Option<String>>,
    /// Extension ID (from handshake).
    extension_id: RwLock<Option<String>>,
}

impl NativeHost {
    pub fn new() -> Self {
        Self {
            writer: Arc::new(Mutex::new(tokio::io::stdout())),
            pending: Arc::new(RwLock::new(HashMap::new())),
            next_id: AtomicI64::new(1),
            connected: AtomicBool::new(false),
            extension_version: RwLock::new(None),
            extension_id: RwLock::new(None),
        }
    }

    /// Check if the extension is currently connected.
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    /// Run the native messaging host read loop. Reads messages from stdin
    /// and dispatches them. This blocks until stdin is closed (extension disconnects).
    pub async fn run(&self) -> Result<(), BrowserError> {
        let mut stdin = tokio::io::stdin();
        info!("native messaging host started, reading from stdin");

        loop {
            match read_native_message(&mut stdin).await {
                Ok(msg) => {
                    let response = self.handle_message(msg).await;
                    if let Some(resp) = response {
                        if let Err(e) = self.write_message(&resp).await {
                            warn!("failed to write native message: {}", e);
                            break;
                        }
                    }
                }
                Err(e) => {
                    // EOF or read error — extension disconnected
                    info!("native messaging host stdin closed: {}", e);
                    self.connected.store(false, Ordering::SeqCst);

                    // Reject all pending requests
                    let mut pending = self.pending.write().await;
                    for (_, tx) in pending.drain() {
                        let _ = tx.send(Err("Extension disconnected".to_string()));
                    }
                    break;
                }
            }
        }

        Ok(())
    }

    /// Handle an incoming message from the extension.
    async fn handle_message(&self, msg: NativeMessage) -> Option<NativeMessage> {
        match msg.msg_type.as_str() {
            "hello" => {
                info!(
                    version = ?msg.version,
                    extension_id = ?msg.extension_id,
                    "extension connected via native messaging"
                );
                *self.extension_version.write().await = msg.version;
                *self.extension_id.write().await = msg.extension_id;
                self.connected.store(true, Ordering::SeqCst);
                Some(NativeMessage::connected())
            }

            "ping" => Some(NativeMessage::pong()),

            "pong" => {
                debug!("received pong from extension");
                None
            }

            "tool_response" => {
                // Extension completed a tool request we sent
                if let Some(id) = msg.id {
                    let mut pending = self.pending.write().await;
                    if let Some(tx) = pending.remove(&id) {
                        let result = if let Some(err) = msg.error {
                            Err(err)
                        } else {
                            Ok(msg.result.unwrap_or(serde_json::Value::Null))
                        };
                        let _ = tx.send(result);
                    }
                }
                None
            }

            "tab_attached" => {
                debug!(args = ?msg.args, "tab attached");
                None
            }

            "tab_detached" => {
                debug!(args = ?msg.args, "tab detached");
                None
            }

            "stop_agent" => {
                info!("stop agent requested from extension");
                // TODO: Signal the runner to cancel the current session
                None
            }

            _ => {
                debug!(msg_type = msg.msg_type, "unknown message type from extension");
                None
            }
        }
    }

    /// Send a tool execution request to the extension and wait for the result.
    pub async fn execute_tool(
        &self,
        tool: &str,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, BrowserError> {
        if !self.is_connected() {
            return Err(BrowserError::Other(
                "Chrome extension not connected. Install the Nebo extension and ensure native messaging is configured.".to_string(),
            ));
        }

        audit::log_tool_request(tool, args);

        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();

        self.pending.write().await.insert(id, tx);

        let msg = NativeMessage::tool_request(id, tool, args);
        if let Err(e) = self.write_message(&msg).await {
            self.pending.write().await.remove(&id);
            return Err(BrowserError::Other(format!(
                "Failed to send tool request to extension: {}",
                e
            )));
        }

        // Wait for response with timeout
        let result = tokio::time::timeout(std::time::Duration::from_secs(30), rx)
            .await
            .map_err(|_| {
                // Clean up pending on timeout
                let pending = self.pending.clone();
                tokio::spawn(async move {
                    pending.write().await.remove(&id);
                });
                BrowserError::Timeout(format!("Tool '{}' timed out after 30s", tool))
            })?
            .map_err(|_| {
                BrowserError::Other("Extension disconnected while waiting for tool result".to_string())
            })?;

        result.map_err(|e| BrowserError::Other(e))
    }

    /// Send a show_indicators command to the extension.
    pub async fn show_indicators(&self) -> Result<(), BrowserError> {
        self.write_message(&NativeMessage::show_indicators()).await
    }

    /// Send a hide_indicators command to the extension.
    pub async fn hide_indicators(&self) -> Result<(), BrowserError> {
        self.write_message(&NativeMessage::hide_indicators()).await
    }

    /// Write a message to stdout using the native messaging protocol.
    async fn write_message(&self, msg: &NativeMessage) -> Result<(), BrowserError> {
        let json = serde_json::to_vec(msg)?;
        let len = json.len() as u32;

        let mut writer = self.writer.lock().await;
        writer.write_all(&len.to_le_bytes()).await?;
        writer.write_all(&json).await?;
        writer.flush().await?;

        Ok(())
    }
}

/// Read a single native message from the reader.
/// Chrome native messaging protocol: 4-byte little-endian length prefix + JSON body.
async fn read_native_message(
    reader: &mut tokio::io::Stdin,
) -> Result<NativeMessage, BrowserError> {
    // Read 4-byte length prefix
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf).await?;
    let len = u32::from_le_bytes(len_buf) as usize;

    // Sanity check — Chrome limits to 1MB
    if len > 1_048_576 {
        return Err(BrowserError::Other(format!(
            "Native message too large: {} bytes",
            len
        )));
    }

    // Read JSON body
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).await?;

    let msg: NativeMessage = serde_json::from_slice(&body)?;
    Ok(msg)
}

/// Production Chrome Web Store extension ID.
const PRODUCTION_EXTENSION_ID: &str = "heaeiepdllbncnnlfniglgmbfmmemkcg";

/// Development extension ID — the unpacked extension loaded from chrome-extension/ in the repo.
/// Chrome assigns a stable ID per unpacked extension path, so this is deterministic for dev.
const DEV_EXTENSION_ID: &str = "bmkkjdcmjiebhegfibdnbimjpkmaickm";

/// Install the native messaging host manifest for all supported Chromium browsers
/// (Chrome, Brave, Edge, Chromium). Each browser has its own NativeMessagingHosts
/// directory, so we write the same manifest to all of them.
///
/// The manifest points directly at the `nebo` binary. Chrome passes
/// `chrome-extension://EXTENSION_ID/` as an argument when launching it.
/// The binary detects this and runs as a lightweight stdin/stdout bridge
/// instead of starting the full app.
///
/// Accepts optional local_extension_id for additional unpacked development builds.
pub fn install_manifest(nebo_binary_path: &str, local_extension_id: &str) -> Result<(), BrowserError> {
    let mut origins = vec![
        format!("chrome-extension://{}/", PRODUCTION_EXTENSION_ID),
        format!("chrome-extension://{}/", DEV_EXTENSION_ID),
    ];
    if !local_extension_id.is_empty()
        && local_extension_id != PRODUCTION_EXTENSION_ID
        && local_extension_id != DEV_EXTENSION_ID
    {
        origins.push(format!("chrome-extension://{}/", local_extension_id));
    }

    let manifest = serde_json::json!({
        "name": "dev.neboloop.nebo",
        "description": "Nebo Browser Automation Host",
        "path": nebo_binary_path,
        "type": "stdio",
        "allowed_origins": origins
    });

    let manifest_json = serde_json::to_string_pretty(&manifest)?;
    let dirs = all_native_messaging_dirs();

    let mut installed = 0;
    for dir in &dirs {
        if let Err(e) = std::fs::create_dir_all(dir) {
            debug!(dir = %dir, error = %e, "skipping native messaging dir");
            continue;
        }
        let path = std::path::Path::new(dir).join("dev.neboloop.nebo.json");
        if let Err(e) = std::fs::write(&path, &manifest_json) {
            debug!(path = %path.display(), error = %e, "failed to write manifest");
            continue;
        }
        info!(path = %path.display(), "installed native messaging host manifest");
        installed += 1;
    }

    if installed == 0 {
        return Err(BrowserError::Other(
            "Failed to install native messaging manifest to any browser".to_string(),
        ));
    }

    Ok(())
}

/// Check if the native messaging host manifest is installed in at least one browser.
pub fn is_manifest_installed() -> bool {
    all_native_messaging_dirs().iter().any(|dir| {
        std::path::Path::new(dir)
            .join("dev.neboloop.nebo.json")
            .exists()
    })
}

/// Check if any installed manifest needs updating — binary path changed or
/// allowed_origins is missing the local extension ID.
pub fn needs_manifest_update(nebo_binary_path: &str, local_extension_id: &str) -> bool {
    let dirs = all_native_messaging_dirs();
    let mut found_any = false;

    for dir in &dirs {
        let path = std::path::Path::new(dir).join("dev.neboloop.nebo.json");
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let manifest: serde_json::Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(_) => return true, // Corrupt manifest — reinstall
        };
        found_any = true;

        // Check binary path matches
        if manifest["path"].as_str() != Some(nebo_binary_path) {
            info!(
                expected = nebo_binary_path,
                actual = ?manifest["path"].as_str(),
                "native messaging manifest has stale binary path"
            );
            return true;
        }

        // Check allowed_origins includes both production and dev extension IDs
        let origins = manifest["allowed_origins"].as_array();
        let has_origin = |id: &str| -> bool {
            let expected = format!("chrome-extension://{}/", id);
            origins
                .map(|arr| arr.iter().any(|o| o.as_str() == Some(&expected)))
                .unwrap_or(false)
        };
        if !has_origin(DEV_EXTENSION_ID) || !has_origin(PRODUCTION_EXTENSION_ID) {
            info!("native messaging manifest missing required extension origins");
            return true;
        }

        // Check additional local extension ID (if configured beyond the built-in ones)
        if !local_extension_id.is_empty()
            && local_extension_id != PRODUCTION_EXTENSION_ID
            && local_extension_id != DEV_EXTENSION_ID
            && !has_origin(local_extension_id)
        {
            info!(
                extension_id = local_extension_id,
                "native messaging manifest missing configured extension origin"
            );
            return true;
        }
    }

    !found_any // No manifests at all — need to install
}

/// Returns NativeMessagingHosts directories for all supported Chromium browsers.
fn all_native_messaging_dirs() -> Vec<String> {
    let mut dirs = Vec::new();

    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = std::env::var("HOME") {
            let base = format!("{}/Library/Application Support", home);
            dirs.push(format!("{}/Google/Chrome/NativeMessagingHosts", base));
            dirs.push(format!("{}/BraveSoftware/Brave-Browser/NativeMessagingHosts", base));
            dirs.push(format!("{}/Microsoft Edge/NativeMessagingHosts", base));
            dirs.push(format!("{}/Chromium/NativeMessagingHosts", base));
        }
    }

    #[cfg(target_os = "linux")]
    {
        if let Ok(home) = std::env::var("HOME") {
            dirs.push(format!("{}/.config/google-chrome/NativeMessagingHosts", home));
            dirs.push(format!("{}/.config/BraveSoftware/Brave-Browser/NativeMessagingHosts", home));
            dirs.push(format!("{}/.config/microsoft-edge/NativeMessagingHosts", home));
            dirs.push(format!("{}/.config/chromium/NativeMessagingHosts", home));
        }
    }

    #[cfg(target_os = "windows")]
    {
        if let Ok(appdata) = std::env::var("LOCALAPPDATA") {
            dirs.push(format!("{}\\Google\\Chrome\\User Data\\NativeMessagingHosts", appdata));
            dirs.push(format!("{}\\BraveSoftware\\Brave-Browser\\User Data\\NativeMessagingHosts", appdata));
            dirs.push(format!("{}\\Microsoft\\Edge\\User Data\\NativeMessagingHosts", appdata));
        }
    }

    dirs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_native_message_serialization() {
        let msg = NativeMessage::tool_request(
            1,
            "read_page",
            &serde_json::json!({"filter": "all"}),
        );
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("execute_tool"));
        assert!(json.contains("read_page"));

        let back: NativeMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(back.msg_type, "execute_tool");
        assert_eq!(back.id, Some(1));
        assert_eq!(back.tool, "read_page");
    }

    #[test]
    fn test_native_message_pong() {
        let msg = NativeMessage::pong();
        let json = serde_json::to_string(&msg).unwrap();
        let back: NativeMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(back.msg_type, "pong");
    }

    #[test]
    fn test_tool_response_ok() {
        let msg = NativeMessage::tool_response(42, Ok(serde_json::json!("hello")));
        assert_eq!(msg.id, Some(42));
        assert!(msg.error.is_none());
        assert_eq!(msg.result, Some(serde_json::json!("hello")));
    }

    #[test]
    fn test_tool_response_err() {
        let msg = NativeMessage::tool_response(42, Err("boom".to_string()));
        assert_eq!(msg.id, Some(42));
        assert_eq!(msg.error, Some("boom".to_string()));
        assert!(msg.result.is_none());
    }
}
