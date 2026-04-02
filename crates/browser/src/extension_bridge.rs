//! Extension Bridge — connects the agent's web_tool to the Chrome extension
//! via a WebSocket relay to the native messaging bridge process.
//!
//! Flow:
//!   Agent (web_tool) → ExtensionBridge::execute() → pending queue
//!   WS handler reads from queue → sends to bridge process → extension executes
//!   Extension result → bridge process → WS handler → resolves pending → web_tool gets result
//!
//! Multiple browsers can connect simultaneously. Tool requests are routed
//! only to the system default browser's connection.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::{debug, info, warn};

/// A tool request from the agent to the extension.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ToolRequest {
    pub id: i64,
    pub tool: String,
    pub args: serde_json::Value,
    /// If true, this is a batch request — `tool` is "__batch__" and `args` contains
    /// the actions array and options.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_batch: bool,
}

/// A single action in a batch request.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BatchAction {
    pub tool: String,
    pub args: serde_json::Value,
}

/// Options for batch execution.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BatchOptions {
    /// If true, stop executing after the first error. Default: false.
    #[serde(default)]
    pub stop_on_error: bool,
}

/// Per-browser connection: each relay gets its own request channel.
struct BrowserConnection {
    tx: mpsc::Sender<ToolRequest>,
    browser: String,
}

/// Cached read_page result with timestamp.
struct PageCacheEntry {
    result: serde_json::Value,
    timestamp: Instant,
}

/// Tools that mutate page state — read_page cache is invalidated before these.
const MUTATION_TOOLS: &[&str] = &[
    "click", "double_click", "triple_click", "right_click",
    "form_input", "type", "select", "press", "navigate",
    "go_back", "go_forward", "evaluate", "drag",
    "new_tab", "close_tab",
];

/// The extension bridge — shared via AppState and Manager.
#[derive(Clone)]
pub struct ExtensionBridge {
    /// Active browser connections keyed by connection ID.
    connections: Arc<Mutex<HashMap<i64, BrowserConnection>>>,
    /// Pending responses keyed by request ID.
    pending: Arc<Mutex<HashMap<i64, oneshot::Sender<Result<serde_json::Value, String>>>>>,
    /// Monotonic connection/request ID counter.
    next_id: Arc<AtomicI64>,
    /// The system default browser bundle ID (detected at startup).
    default_browser: Arc<Mutex<Option<String>>>,
    /// Timestamp of last active connection (for grace period on reconnect).
    last_connected: Arc<Mutex<Option<Instant>>>,
    /// Single-slot cache for read_page results (2.5s TTL).
    page_cache: Arc<Mutex<Option<PageCacheEntry>>>,
}

impl ExtensionBridge {
    pub fn new() -> Self {
        let bridge = Self {
            connections: Arc::new(Mutex::new(HashMap::new())),
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(AtomicI64::new(1)),
            default_browser: Arc::new(Mutex::new(None)),
            last_connected: Arc::new(Mutex::new(None)),
            page_cache: Arc::new(Mutex::new(None)),
        };
        // Detect default browser in background
        let db = bridge.default_browser.clone();
        tokio::spawn(async move {
            let detected = detect_default_browser().await;
            info!(browser = %detected, "detected default browser");
            *db.lock().await = Some(detected);
        });
        bridge
    }

    /// Check if any extension is connected.
    pub fn is_connected(&self) -> bool {
        // Use try_lock to avoid blocking — if locked, someone is modifying connections
        match self.connections.try_lock() {
            Ok(conns) => !conns.is_empty(),
            Err(_) => true, // Assume connected if we can't check
        }
    }

    /// Register a new browser connection. Returns (conn_id, request_receiver).
    /// The WS handler reads from the receiver to get tool requests for this browser.
    pub async fn connect(&self, browser: String) -> (i64, mpsc::Receiver<ToolRequest>) {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel(64);
        let mut conns = self.connections.lock().await;
        conns.insert(id, BrowserConnection { tx, browser: browser.clone() });
        info!(conn_id = id, browser = %browser, active = conns.len(), "extension connected");
        *self.last_connected.lock().await = Some(Instant::now());
        (id, rx)
    }

    /// Remove a browser connection.
    pub async fn disconnect(&self, conn_id: i64) {
        let mut conns = self.connections.lock().await;
        let browser = conns.remove(&conn_id).map(|c| c.browser).unwrap_or_default();
        let remaining = conns.len();
        drop(conns);
        info!(conn_id = conn_id, browser = %browser, remaining = remaining, "extension disconnected");
        if remaining == 0 {
            *self.last_connected.lock().await = Some(Instant::now());
            // Don't reject pending — extension reconnects in ~2s,
            // individual 30s timeouts handle truly dead connections.
        }
    }

    /// Check if the extension was recently connected (within the given duration).
    /// Returns true if currently connected OR if last connection was within the window.
    pub async fn was_recently_connected(&self, within: Duration) -> bool {
        if self.is_connected() {
            return true;
        }
        match *self.last_connected.lock().await {
            Some(t) => t.elapsed() < within,
            None => false,
        }
    }

    /// Wait for the extension to reconnect, polling every 100ms.
    /// Returns true if connected within the timeout, false otherwise.
    pub async fn wait_for_connection(&self, timeout: Duration) -> bool {
        if self.is_connected() {
            return true;
        }
        let start = Instant::now();
        while start.elapsed() < timeout {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if self.is_connected() {
                return true;
            }
        }
        false
    }

    /// Execute a browser tool via the extension. Routes to the default browser.
    /// read_page results are cached for 2.5s; mutation tools invalidate the cache.
    pub async fn execute(
        &self,
        tool: &str,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        // Check read_page cache (snapshot-then-release)
        if tool == "read_page" {
            let cached = {
                let guard = self.page_cache.lock().await;
                guard.as_ref()
                    .filter(|e| e.timestamp.elapsed() < Duration::from_millis(2500))
                    .map(|e| e.result.clone())
            };
            if let Some(result) = cached {
                debug!("read_page cache hit");
                return Ok(result);
            }
        }

        // Invalidate cache before mutation tools
        if MUTATION_TOOLS.contains(&tool) {
            *self.page_cache.lock().await = None;
        }

        let conns = self.connections.lock().await;
        if conns.is_empty() {
            return Err("Chrome extension not connected".to_string());
        }

        // Find the connection matching the default browser, or fall back to any
        let default = self.default_browser.lock().await.clone().unwrap_or_default();
        let target = conns.values()
            .find(|c| !default.is_empty() && c.browser.contains(&default))
            .or_else(|| conns.values().next());

        let tx = match target {
            Some(conn) => conn.tx.clone(),
            None => return Err("No browser connection available".to_string()),
        };
        drop(conns);

        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (resp_tx, resp_rx) = oneshot::channel();
        self.pending.lock().await.insert(id, resp_tx);

        let request = ToolRequest {
            id,
            tool: tool.to_string(),
            args: args.clone(),
            is_batch: false,
        };

        if tx.send(request).await.is_err() {
            self.pending.lock().await.remove(&id);
            return Err("Failed to send tool request to browser".to_string());
        }

        // Wait with timeout
        let result = match tokio::time::timeout(std::time::Duration::from_secs(30), resp_rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => {
                self.pending.lock().await.remove(&id);
                Err("Extension disconnected while waiting".to_string())
            }
            Err(_) => {
                let mut map = self.pending.lock().await;
                map.remove(&id);
                let pending_count = map.len();
                drop(map);
                warn!(
                    tool = tool,
                    pending = pending_count,
                    "browser tool timed out after 30s"
                );
                Err(format!(
                    "Tool '{}' timed out after 30s (pending: {})",
                    tool, pending_count
                ))
            }
        };

        // Populate read_page cache on success
        if tool == "read_page" {
            if let Ok(ref value) = result {
                *self.page_cache.lock().await = Some(PageCacheEntry {
                    result: value.clone(),
                    timestamp: Instant::now(),
                });
            }
        }

        result
    }

    /// Execute multiple browser tools in a single WS round-trip.
    /// The extension executes them sequentially and returns all results.
    pub async fn batch_execute(
        &self,
        actions: Vec<BatchAction>,
        opts: BatchOptions,
    ) -> Result<Vec<Result<serde_json::Value, String>>, String> {
        if actions.is_empty() {
            return Ok(vec![]);
        }

        // Invalidate page cache if any mutation tool is in the batch
        let has_mutation = actions.iter().any(|a| MUTATION_TOOLS.contains(&a.tool.as_str()));
        if has_mutation {
            *self.page_cache.lock().await = None;
        }

        let conns = self.connections.lock().await;
        if conns.is_empty() {
            return Err("Chrome extension not connected".to_string());
        }

        let default = self.default_browser.lock().await.clone().unwrap_or_default();
        let target = conns.values()
            .find(|c| !default.is_empty() && c.browser.contains(&default))
            .or_else(|| conns.values().next());

        let tx = match target {
            Some(conn) => conn.tx.clone(),
            None => return Err("No browser connection available".to_string()),
        };
        drop(conns);

        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (resp_tx, resp_rx) = oneshot::channel();
        self.pending.lock().await.insert(id, resp_tx);

        let request = ToolRequest {
            id,
            tool: String::new(),
            args: serde_json::json!({
                "actions": actions,
                "stop_on_error": opts.stop_on_error,
            }),
            is_batch: true,
        };

        if tx.send(request).await.is_err() {
            self.pending.lock().await.remove(&id);
            return Err("Failed to send batch request to browser".to_string());
        }

        // Wait for batch response
        let raw_result = match tokio::time::timeout(Duration::from_secs(60), resp_rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => {
                self.pending.lock().await.remove(&id);
                return Err("Extension disconnected during batch".to_string());
            }
            Err(_) => {
                self.pending.lock().await.remove(&id);
                return Err("Batch request timed out after 60s".to_string());
            }
        };

        // Parse batch response: expect array of { result?, error? } objects
        match raw_result {
            Ok(value) => {
                let results_arr = value.as_array()
                    .ok_or_else(|| "Batch response is not an array".to_string())?;
                let results = results_arr.iter().map(|item| {
                    if let Some(err) = item["error"].as_str() {
                        Err(err.to_string())
                    } else {
                        Ok(item["result"].clone())
                    }
                }).collect();

                // Populate read_page cache from last successful read_page in batch
                if let Some(last_read) = actions.iter().rposition(|a| a.tool == "read_page") {
                    if let Some(Ok(ref value)) = results_arr.get(last_read).map(|item| {
                        if item["error"].is_null() { Ok(item["result"].clone()) } else { Err(()) }
                    }) {
                        *self.page_cache.lock().await = Some(PageCacheEntry {
                            result: value.clone(),
                            timestamp: Instant::now(),
                        });
                    }
                }

                Ok(results)
            }
            Err(e) => Err(e),
        }
    }

    /// Deliver a tool result from the extension (called by WS handler).
    pub async fn deliver_result(&self, id: i64, result: Result<serde_json::Value, String>) {
        let mut map = self.pending.lock().await;
        if let Some(tx) = map.remove(&id) {
            let _ = tx.send(result);
        } else {
            debug!(id, "no pending request for tool result");
        }
    }
}

/// Detect the system default browser on macOS.
/// Returns a short name like "chrome", "brave", "firefox", "safari".
async fn detect_default_browser() -> String {
    #[cfg(target_os = "macos")]
    {
        // Use macOS defaults command to read the HTTPS handler
        let output = tokio::process::Command::new("defaults")
            .args(["read", "com.apple.LaunchServices/com.apple.launchservices.secure", "LSHandlers"])
            .output()
            .await;

        if let Ok(output) = output {
            let text = String::from_utf8_lossy(&output.stdout);
            // Parse the plist-style output to find the https handler
            // Format: { LSHandlerRoleAll = "com.brave.Browser"; LSHandlerURLScheme = https; }
            let mut in_https_block = false;
            for line in text.lines() {
                let trimmed = line.trim();
                if trimmed.contains("LSHandlerURLScheme") && trimmed.to_lowercase().contains("https") {
                    in_https_block = true;
                }
                if in_https_block && trimmed.contains("LSHandlerRoleAll") {
                    // Extract the bundle ID
                    if let Some(start) = trimmed.find('"') {
                        if let Some(end) = trimmed[start+1..].find('"') {
                            let bundle_id = &trimmed[start+1..start+1+end];
                            return bundle_id_to_name(bundle_id);
                        }
                    }
                    // Try without quotes (some plist formats)
                    if let Some(eq) = trimmed.find('=') {
                        let val = trimmed[eq+1..].trim().trim_end_matches(';').trim().trim_matches('"');
                        return bundle_id_to_name(val);
                    }
                }
                // Reset if we hit a new block without finding the handler
                if trimmed.contains('}') {
                    in_https_block = false;
                }
            }
        }
    }

    // Default fallback
    "unknown".to_string()
}

fn bundle_id_to_name(bundle_id: &str) -> String {
    let lower = bundle_id.to_lowercase();
    if lower.contains("chrome") { "chrome".to_string() }
    else if lower.contains("brave") { "brave".to_string() }
    else if lower.contains("firefox") { "firefox".to_string() }
    else if lower.contains("safari") { "safari".to_string() }
    else if lower.contains("edge") { "edge".to_string() }
    else if lower.contains("arc") { "arc".to_string() }
    else { bundle_id.to_string() }
}
