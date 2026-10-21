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

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::{Mutex, mpsc, oneshot};
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
    /// If true, this is a fire-and-forget command (show_indicators, hide_indicators,
    /// teach_record_start/stop). No response is expected — the WS handler serializes
    /// it as `{ "type": tool, ...args }`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_command: bool,
    /// Session/agent identifier — extension uses this to scope tab groups per agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
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
    "click",
    "double_click",
    "triple_click",
    "right_click",
    "form_input",
    "type",
    "select",
    "press",
    "navigate",
    "go_back",
    "go_forward",
    "evaluate",
    "drag",
    "new_tab",
    "close_tab",
];

/// The extension bridge — shared via AppState and Manager.
#[derive(Clone)]
pub struct ExtensionBridge {
    /// Active browser connections keyed by connection ID.
    connections: Arc<Mutex<HashMap<i64, BrowserConnection>>>,
    /// Pending responses keyed by request ID, bound to the connection the
    /// request was routed to — only that connection may resolve it.
    #[allow(clippy::type_complexity)]
    pending: Arc<Mutex<HashMap<i64, (i64, oneshot::Sender<Result<serde_json::Value, String>>)>>>,
    /// Monotonic connection/request ID counter.
    next_id: Arc<AtomicI64>,
    /// The system default browser bundle ID (detected at startup).
    default_browser: Arc<Mutex<Option<String>>>,
    /// Timestamp of last active connection (for grace period on reconnect).
    last_connected: Arc<Mutex<Option<Instant>>>,
    /// Per-session cache for read_page results (2.5s TTL).
    page_cache: Arc<Mutex<HashMap<String, PageCacheEntry>>>,
    /// The teach session recording right now, if any: every browser records
    /// into it, including one that connects mid-recording.
    teach: Arc<Mutex<Option<crate::teach::TeachCapture>>>,
    /// Signalled when an extension has flushed its last teach events.
    teach_flushed: Arc<tokio::sync::Notify>,
    /// The tabs Nebo opened, by the run that opened them: every request
    /// goes out under its run's key ([`crate::tab_ledger`]).
    tabs: Arc<crate::tab_ledger::TabLedger>,
    /// The ids of the tabs each run opened (`new_tab`'s results), by the
    /// run's tab key: the only tabs that run's close may name by id. Every
    /// other tab is the owner's, or another run's. Dropped when the run ends
    /// or hands its tab to the owner, and when a browser connection closes
    /// (a restarted Chrome numbers its tabs anew).
    opened_tab_ids: Arc<Mutex<HashMap<String, HashSet<i64>>>>,
}

/// How long stop waits for the extension to hand over what it still holds
/// (a half-typed field, the last after-state).
const TEACH_FLUSH_WAIT: Duration = Duration::from_secs(3);

impl ExtensionBridge {
    /// `tabs_file` keeps the keys of the tabs Nebo has open, so a Nebo that
    /// stopped mid-run closes them on the next connect; `None` keeps them in
    /// memory only.
    pub fn new(tabs_file: Option<std::path::PathBuf>) -> Self {
        let bridge = Self {
            connections: Arc::new(Mutex::new(HashMap::new())),
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(AtomicI64::new(1)),
            default_browser: Arc::new(Mutex::new(None)),
            last_connected: Arc::new(Mutex::new(None)),
            page_cache: Arc::new(Mutex::new(HashMap::new())),
            teach: Arc::new(Mutex::new(None)),
            teach_flushed: Arc::new(tokio::sync::Notify::new()),
            tabs: Arc::new(crate::tab_ledger::TabLedger::new(tabs_file)),
            opened_tab_ids: Arc::new(Mutex::new(HashMap::new())),
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
        // A browser that connects mid-recording records too.
        if let Some(cap) = self.teach.lock().await.as_ref() {
            let _ = tx.try_send(teach_command("teach_record_start", &cap.session));
        }
        // Tabs a Nebo that stopped mid-run left open close in every browser
        // that connects. Their keys are never used again, so they need not
        // go ahead of this Nebo's own requests.
        let orphans = self.tabs.orphans().await;
        if !orphans.is_empty() {
            let tx = tx.clone();
            tokio::spawn(async move {
                for key in orphans {
                    if tx.send(session_command("close_session_tabs", &key)).await.is_err() {
                        break;
                    }
                }
            });
        }
        let mut conns = self.connections.lock().await;
        conns.insert(
            id,
            BrowserConnection {
                tx,
                browser: browser.clone(),
            },
        );
        info!(conn_id = id, browser = %browser, active = conns.len(), "extension connected");
        *self.last_connected.lock().await = Some(Instant::now());
        (id, rx)
    }

    /// Remove a browser connection.
    pub async fn disconnect(&self, conn_id: i64) {
        let mut conns = self.connections.lock().await;
        let browser = conns
            .remove(&conn_id)
            .map(|c| c.browser)
            .unwrap_or_default();
        let remaining = conns.len();
        drop(conns);
        self.opened_tab_ids.lock().await.clear();
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
    /// `session_id` scopes the tab group in the extension (each agent gets its own tabs).
    pub async fn execute(
        &self,
        tool: &str,
        args: &serde_json::Value,
        session_id: Option<&str>,
    ) -> Result<serde_json::Value, String> {
        let cache_key = session_id.unwrap_or("_default").to_string();

        // Check read_page cache (session-scoped, snapshot-then-release)
        if tool == "read_page" {
            let cached = {
                let guard = self.page_cache.lock().await;
                guard
                    .get(&cache_key)
                    .filter(|e| e.timestamp.elapsed() < Duration::from_millis(2500))
                    .map(|e| e.result.clone())
            };
            if let Some(result) = cached {
                debug!(session = %cache_key, "read_page cache hit");
                return Ok(result);
            }
        }

        if let Some(refusal) = self.close_refusal(tool, args, session_id).await {
            return Err(refusal);
        }

        // Invalidate this session's cache before mutation tools
        if MUTATION_TOOLS.contains(&tool) {
            self.page_cache.lock().await.remove(&cache_key);
        }

        let conns = self.connections.lock().await;
        if conns.is_empty() {
            return Err("Chrome extension not connected".to_string());
        }

        // Find the connection matching the default browser, or fall back to any.
        // Among matches, prefer the OLDEST connection (lowest id): HashMap order is
        // arbitrary, and a transient second relay from the same browser (e.g. the
        // options page's connection test) must never steal routing from the
        // long-lived one — requests sent to it are dropped and time out at 60s.
        let default = self
            .default_browser
            .lock()
            .await
            .clone()
            .unwrap_or_default();
        let target = conns
            .iter()
            .filter(|(_, c)| !default.is_empty() && c.browser.contains(&default))
            .min_by_key(|(id, _)| **id)
            .or_else(|| conns.iter().min_by_key(|(id, _)| **id))
            .map(|(id, c)| (*id, c.tx.clone()));

        let (target_conn, tx) = match target {
            Some(t) => t,
            None => return Err("No browser connection available".to_string()),
        };
        drop(conns);

        let tab_key = match session_id {
            Some(sid) => Some(self.tabs.key(sid).await),
            None => None,
        };
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (resp_tx, resp_rx) = oneshot::channel();
        self.pending.lock().await.insert(id, (target_conn, resp_tx));

        let request = ToolRequest {
            id,
            tool: tool.to_string(),
            args: args.clone(),
            is_batch: false,
            is_command: false,
            session_id: tab_key.clone(),
        };

        if tx.send(request).await.is_err() {
            self.pending.lock().await.remove(&id);
            return Err("Failed to send tool request to browser".to_string());
        }

        // Wait with timeout — 60s accommodates extension-side timeouts
        // (executeScript 45s + CDP sendCommand 30s + overhead).
        // The extension's own timeouts bound the call; a native-messaging
        // path would need no bridge timeout, so this one must be generous.
        let result = match tokio::time::timeout(std::time::Duration::from_secs(60), resp_rx).await {
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
                    "browser tool timed out after 60s"
                );
                Err(format!(
                    "Tool '{}' timed out after 60s (pending: {})",
                    tool, pending_count
                ))
            }
        };

        if let (true, Ok(value), Some(key)) = (tool == "new_tab", &result, &tab_key) {
            self.note_opened(key, value).await;
        }

        // Populate read_page cache on success (session-scoped)
        if tool == "read_page" {
            if let Ok(ref value) = result {
                self.page_cache.lock().await.insert(
                    cache_key,
                    PageCacheEntry {
                        result: value.clone(),
                        timestamp: Instant::now(),
                    },
                );
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
        session_id: Option<&str>,
    ) -> Result<Vec<Result<serde_json::Value, String>>, String> {
        if actions.is_empty() {
            return Ok(vec![]);
        }

        let cache_key = session_id.unwrap_or("_default").to_string();

        for action in &actions {
            if let Some(refusal) = self.close_refusal(&action.tool, &action.args, session_id).await {
                return Err(refusal);
            }
        }

        // Invalidate this session's page cache if any mutation tool is in the batch
        let has_mutation = actions
            .iter()
            .any(|a| MUTATION_TOOLS.contains(&a.tool.as_str()));
        if has_mutation {
            self.page_cache.lock().await.remove(&cache_key);
        }

        let conns = self.connections.lock().await;
        if conns.is_empty() {
            return Err("Chrome extension not connected".to_string());
        }

        let default = self
            .default_browser
            .lock()
            .await
            .clone()
            .unwrap_or_default();
        // Same oldest-matching-connection rule as execute() — never route to a
        // transient second relay (see comment there).
        let target = conns
            .iter()
            .filter(|(_, c)| !default.is_empty() && c.browser.contains(&default))
            .min_by_key(|(id, _)| **id)
            .or_else(|| conns.iter().min_by_key(|(id, _)| **id))
            .map(|(id, c)| (*id, c.tx.clone()));

        let (target_conn, tx) = match target {
            Some(t) => t,
            None => return Err("No browser connection available".to_string()),
        };
        drop(conns);

        let tab_key = match session_id {
            Some(sid) => Some(self.tabs.key(sid).await),
            None => None,
        };
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (resp_tx, resp_rx) = oneshot::channel();
        self.pending.lock().await.insert(id, (target_conn, resp_tx));

        let request = ToolRequest {
            id,
            tool: String::new(),
            args: serde_json::json!({
                "actions": actions,
                "stop_on_error": opts.stop_on_error,
            }),
            is_batch: true,
            is_command: false,
            session_id: tab_key.clone(),
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
                let results_arr = value
                    .as_array()
                    .ok_or_else(|| "Batch response is not an array".to_string())?;
                let results = results_arr
                    .iter()
                    .map(|item| {
                        if let Some(err) = item["error"].as_str() {
                            Err(err.to_string())
                        } else {
                            Ok(item["result"].clone())
                        }
                    })
                    .collect::<Vec<Result<serde_json::Value, String>>>();
                for (action, result) in actions.iter().zip(&results) {
                    if let (true, Ok(value), Some(key)) = (action.tool == "new_tab", result, &tab_key) {
                        self.note_opened(key, value).await;
                    }
                }

                // Populate read_page cache from last successful read_page in batch (session-scoped)
                if let Some(last_read) = actions.iter().rposition(|a| a.tool == "read_page") {
                    if let Some(Ok(ref value)) = results_arr.get(last_read).map(|item| {
                        if item["error"].is_null() {
                            Ok(item["result"].clone())
                        } else {
                            Err(())
                        }
                    }) {
                        self.page_cache.lock().await.insert(
                            cache_key,
                            PageCacheEntry {
                                result: value.clone(),
                                timestamp: Instant::now(),
                            },
                        );
                    }
                }

                Ok(results)
            }
            Err(e) => Err(e),
        }
    }

    /// Why a `close_tab` may not run: it names a tab this run didn't open
    /// (the owner's, another run's, or one handed to the owner). Every
    /// `tabId`/`tabIds` entry must be exactly the id of a tab the run opened
    /// under its current key; anything else (a float, a string, a tab of
    /// someone else's) is refused. A close naming no tab closes the run's own
    /// tab and always may.
    async fn close_refusal(&self, tool: &str, args: &serde_json::Value, session_id: Option<&str>) -> Option<String> {
        if tool != "close_tab" {
            return None;
        }
        let refusal = |tab: &serde_json::Value| {
            Some(format!(
                "Tab {tab} wasn't opened by this work, so it stays open: Nebo never closes the owner's tabs. \
                 Leave out tab_id to close this conversation's own tab."
            ))
        };
        let mut named: Vec<&serde_json::Value> = args.get("tabId").into_iter().collect();
        match args.get("tabIds") {
            None => {}
            Some(serde_json::Value::Array(ids)) => named.extend(ids),
            Some(other) => return refusal(other),
        }
        if named.is_empty() {
            return None;
        }
        let key = match session_id {
            Some(sid) => self.tabs.current(sid).await,
            None => None,
        };
        let opened = self.opened_tab_ids.lock().await;
        let allowed = key.as_ref().and_then(|k| opened.get(k));
        let refused = named
            .into_iter()
            .find(|v| !v.as_i64().filter(|_| v.is_i64() || v.is_u64()).is_some_and(|id| allowed.is_some_and(|ids| ids.contains(&id))))?;
        refusal(refused)
    }

    /// Keep the id of a tab `new_tab` opened, under the run's key.
    async fn note_opened(&self, key: &str, result: &serde_json::Value) {
        if let Some(id) = result.get("tabId").and_then(|v| v.as_i64()) {
            self.opened_tab_ids.lock().await.entry(key.to_string()).or_default().insert(id);
        }
    }

    /// Deliver a tool result from the extension (called by WS handler).
    /// `conn_id` is the connection the result arrived on: only the connection a
    /// request was routed to may resolve it, so one relay can never inject a
    /// forged result into a request sent to another. Exception: if the owning
    /// connection is gone (relay WS dropped mid-request and reconnected — the
    /// same grace period `disconnect` preserves pending for), the result is
    /// accepted from the surviving connection.
    pub async fn deliver_result(&self, conn_id: i64, id: i64, result: Result<serde_json::Value, String>) {
        let mut map = self.pending.lock().await;
        let Some((owner, _)) = map.get(&id) else {
            debug!(id, "no pending request for tool result");
            return;
        };
        let owner = *owner;
        if owner != conn_id && self.connections.lock().await.contains_key(&owner) {
            warn!(id, conn_id, owner, "ignoring tool result from a connection the request was not routed to");
            return;
        }
        if let Some((_, tx)) = map.remove(&id) {
            let _ = tx.send(result);
        }
    }

    /// Send a fire-and-forget command to the extension (e.g., show_indicators, hide_indicators).
    /// Broadcasts to all active connections. No response is expected. A
    /// session's command goes to its run's tab; a session with no tab open
    /// has nothing to send it to.
    pub async fn send_command(&self, command: &str, session_id: Option<&str>) {
        let request = match session_id {
            Some(sid) => match self.tabs.current(sid).await {
                Some(key) => session_command(command, &key),
                None => return,
            },
            None => ToolRequest {
                id: 0,
                tool: command.to_string(),
                args: serde_json::Value::Null,
                is_batch: false,
                is_command: true,
                session_id: None,
            },
        };
        self.broadcast(request).await;
    }

    /// The run on `session_id` is over: the tab it opened closes, unless
    /// `leave_open` (it holds something for the owner, such as a form
    /// waiting for his approval): then its indicators come off and the tab
    /// becomes the owner's (`leave_open` to the extension). Either way no run
    /// may name its tabs by id any more, and the session's next call opens a
    /// tab of its own. Returns whether the run had a tab open.
    pub async fn end_session(&self, session_id: &str, leave_open: bool) -> bool {
        let Some(key) = self.tabs.take(session_id).await else {
            return false;
        };
        self.opened_tab_ids.lock().await.remove(&key);
        self.broadcast(session_command("hide_indicators", &key)).await;
        let last = if leave_open { "leave_open" } else { "close_session_tabs" };
        self.broadcast(session_command(last, &key)).await;
        true
    }
}

impl ExtensionBridge {
    /// Begin element recording for teach session `session`, whose files go
    /// in `dir` and whose clock starts at `started_ms` (epoch ms). Every
    /// connected browser is told to record.
    pub async fn teach_start(&self, session: &str, dir: std::path::PathBuf, started_ms: u64) {
        *self.teach.lock().await = Some(crate::teach::TeachCapture::new(session.to_string(), dir, started_ms));
        self.broadcast(teach_command("teach_record_start", session)).await;
        info!(session, "teach: element recording on");
    }

    /// One event from a browser's recording. Events for any other session
    /// (a stale tab, a late message) are dropped.
    pub async fn teach_event(&self, session: &str, event: serde_json::Value, crop: Option<&str>) {
        if let Some(cap) = self.teach.lock().await.as_mut().filter(|c| c.session == session) {
            cap.add(event, crop);
        }
    }

    /// A browser says it has sent everything it held for `session`.
    pub fn teach_flushed(&self, session: &str) {
        debug!(session, "teach: extension flushed");
        self.teach_flushed.notify_waiters();
    }

    /// Stop element recording (`only`: just when it is that session) and
    /// write `elements.jsonl` + `actions.json` into the session dir. Waits
    /// briefly for the browsers to flush; with no browser connected there is
    /// nothing to wait for. Returns the number of actions written, or None
    /// when no such recording was on.
    pub async fn teach_stop(&self, only: Option<&str>) -> Option<Result<usize, String>> {
        let session = self.teach.lock().await.as_ref()?.session.clone();
        if only.is_some_and(|id| id != session) {
            return None;
        }
        if self.is_connected() {
            let flushed = self.teach_flushed.notified();
            self.broadcast(teach_command("teach_record_stop", &session)).await;
            if tokio::time::timeout(TEACH_FLUSH_WAIT, flushed).await.is_err() {
                warn!(session = %session, "teach: no flush from the extension; writing what arrived");
            }
        }
        let cap = self.teach.lock().await.take()?;
        let result = tokio::task::spawn_blocking(move || cap.finish())
            .await
            .unwrap_or_else(|e| Err(e.to_string()));
        info!(session = %session, result = ?result, "teach: element recording off");
        Some(result)
    }

    async fn broadcast(&self, request: ToolRequest) {
        for conn in self.connections.lock().await.values() {
            let _ = conn.tx.send(request.clone()).await;
        }
    }
}

/// A command for one run's tab: `{type, session_id}`.
fn session_command(command: &str, key: &str) -> ToolRequest {
    ToolRequest {
        id: 0,
        tool: command.to_string(),
        args: serde_json::Value::Null,
        is_batch: false,
        is_command: true,
        session_id: Some(key.to_string()),
    }
}

/// A teach-recording command for the extension: `{type, session}`.
fn teach_command(command: &str, session: &str) -> ToolRequest {
    ToolRequest {
        id: 0,
        tool: command.to_string(),
        args: serde_json::json!({ "session": session }),
        is_batch: false,
        is_command: true,
        session_id: None,
    }
}

/// The default browser's short name ("chrome", "brave", "firefox", "safari",
/// …), or "unknown" when the OS doesn't say (`crate::default_browser`).
async fn detect_default_browser() -> String {
    tokio::task::spawn_blocking(|| crate::default_browser::detect().map(|b| b.name.to_string()))
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| "unknown".to_string())
}

#[cfg(test)]
mod result_binding_tests {
    use super::*;

    #[tokio::test]
    async fn result_only_accepted_from_routed_connection() {
        let bridge = ExtensionBridge::new(None);
        *bridge.default_browser.lock().await = Some("chrome".to_string());
        let (real_conn, mut real_rx) = bridge.connect("chrome".to_string()).await;
        let (evil_conn, _evil_rx) = bridge.connect("chrome imposter".to_string()).await;

        let b = bridge.clone();
        let exec = tokio::spawn(async move { b.execute("read_page", &serde_json::json!({}), None).await });

        // The request routes to the oldest chrome match (real_conn).
        let req = real_rx.recv().await.expect("request routed to real connection");

        // A result from the other live connection must be ignored...
        bridge
            .deliver_result(evil_conn, req.id, Ok(serde_json::json!({"forged": true})))
            .await;
        assert!(!exec.is_finished());

        // ...while the routed connection resolves it.
        bridge
            .deliver_result(real_conn, req.id, Ok(serde_json::json!({"ok": true})))
            .await;
        let result = exec.await.unwrap().unwrap();
        assert_eq!(result["ok"], true);

        // If the owning connection is gone (relay reconnect grace), a surviving
        // connection may deliver the late result.
        let b = bridge.clone();
        let exec = tokio::spawn(async move { b.execute("navigate", &serde_json::json!({}), None).await });
        let req = real_rx.recv().await.expect("second request routed");
        bridge.disconnect(real_conn).await;
        bridge
            .deliver_result(evil_conn, req.id, Ok(serde_json::json!({"late": true})))
            .await;
        let result = exec.await.unwrap().unwrap();
        assert_eq!(result["late"], true);
    }
}

#[cfg(test)]
mod teach_tests {
    use super::*;

    #[tokio::test]
    async fn every_browser_records_and_stop_writes_what_they_sent() {
        let dir = std::env::temp_dir().join(format!("nebo-bridge-teach-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bridge = ExtensionBridge::new(None);
        let (_a, mut rx_a) = bridge.connect("chrome".into()).await;
        bridge.teach_start("t1", dir.clone(), 1_000).await;
        let start = rx_a.recv().await.unwrap();
        assert_eq!(start.tool, "teach_record_start");
        assert!(start.is_command);
        assert_eq!(start.args["session"], "t1");

        // A browser that connects mid-recording is told to record at once.
        let (_b, mut rx_b) = bridge.connect("chrome".into()).await;
        assert_eq!(rx_b.recv().await.unwrap().tool, "teach_record_start");

        bridge
            .teach_event("t1", serde_json::json!({"id": "c1", "kind": "click", "at_ms": 3_000, "tab": 1,
                "url": "https://a.test/", "element": {"role": "button", "name": "Go", "locators": ["role=button[name=\"Go\"]"]}}), None)
            .await;
        // Another session's late event is not this recording's.
        bridge.teach_event("old", serde_json::json!({"id": "x", "kind": "click", "at_ms": 3_500}), None).await;

        // The extension answers the stop with its flush.
        let b = bridge.clone();
        let flusher = tokio::spawn(async move {
            let stop = rx_a.recv().await.unwrap();
            assert_eq!(stop.tool, "teach_record_stop");
            b.teach_event("t1", serde_json::json!({"id": "a1", "kind": "after", "of": "c1", "at_ms": 4_000, "url": "https://a.test/done", "heading": "Done"}), None).await;
            b.teach_flushed("t1");
        });
        assert!(bridge.teach_stop(Some("another")).await.is_none(), "another session's stop leaves this one recording");
        let written = bridge.teach_stop(Some("t1")).await.unwrap().unwrap();
        flusher.await.unwrap();
        assert_eq!(written, 1);
        let line: serde_json::Value =
            serde_json::from_str(std::fs::read_to_string(dir.join("elements.jsonl")).unwrap().trim()).unwrap();
        assert_eq!(line["t_ms"], 2_000);
        assert_eq!(line["after"]["heading"], "Done", "what the flush carried is in the record");
        assert!(bridge.teach_stop(None).await.is_none(), "nothing is recording any more");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod tab_tests {
    use super::*;

    /// Answers every tool request it is sent and keeps the commands.
    fn fake_browser(bridge: &ExtensionBridge, conn: i64, mut rx: mpsc::Receiver<ToolRequest>) -> Arc<Mutex<Vec<ToolRequest>>> {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (b, log) = (bridge.clone(), seen.clone());
        tokio::spawn(async move {
            while let Some(req) = rx.recv().await {
                if !req.is_command {
                    b.deliver_result(conn, req.id, Ok(serde_json::json!({}))).await;
                }
                log.lock().await.push(req);
            }
        });
        seen
    }

    async fn settle() {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    fn commands(seen: &[ToolRequest]) -> Vec<(String, String)> {
        seen.iter()
            .filter(|r| r.is_command)
            .map(|r| (r.tool.clone(), r.session_id.clone().expect("every tab command names a run's tab")))
            .collect()
    }

    /// Each run works in a tab of its own and its end closes that tab; a
    /// tab left open for the owner stays open, and the run's next call
    /// opens another. The owner's own tabs are under no key the engine
    /// sends, so no close ever reaches them.
    #[tokio::test]
    async fn a_run_closes_the_tab_it_opened_and_keeps_the_one_it_left_for_the_owner() {
        let bridge = ExtensionBridge::new(None);
        let (conn, rx) = bridge.connect("chrome".into()).await;
        let seen = fake_browser(&bridge, conn, rx);

        bridge.execute("navigate", &serde_json::json!({"url": "https://a.test"}), Some("s1")).await.unwrap();
        let first = seen.lock().await[0].session_id.clone().unwrap();
        assert!(first.starts_with("s1#"), "{first}");
        assert!(bridge.end_session("s1", false).await);
        assert!(!bridge.end_session("s1", false).await, "nothing left to close");
        settle().await;
        assert_eq!(
            commands(&seen.lock().await),
            vec![("hide_indicators".into(), first.clone()), ("close_session_tabs".into(), first.clone())]
        );

        // The next run: a new tab, handed to the owner, then one more page.
        seen.lock().await.clear();
        bridge.execute("navigate", &serde_json::json!({"url": "https://form.test"}), Some("s1")).await.unwrap();
        let form = seen.lock().await[0].session_id.clone().unwrap();
        assert_ne!(form, first);
        assert!(bridge.end_session("s1", true).await);
        bridge.execute("navigate", &serde_json::json!({"url": "https://b.test"}), Some("s1")).await.unwrap();
        let after = seen.lock().await.iter().rev().find(|r| !r.is_command).unwrap().session_id.clone().unwrap();
        assert_ne!(after, form, "the run never works in the tab it left for the owner");
        bridge.end_session("s1", false).await;
        settle().await;
        let sent = commands(&seen.lock().await);
        assert!(!sent.contains(&("close_session_tabs".into(), form.clone())), "the owner's form stays open: {sent:?}");
        assert!(sent.contains(&("hide_indicators".into(), form.clone())));
        assert!(sent.contains(&("leave_open".into(), form)), "the extension hands the tab to the owner");
        assert!(sent.contains(&("close_session_tabs".into(), after)));
        // A session with no tab open sends nothing.
        bridge.send_command("show_indicators", Some("never-browsed")).await;
        assert!(!bridge.end_session("never-browsed", false).await);
    }

    /// A Nebo that stopped mid-run left its tabs open: the next Nebo closes
    /// them in every browser that connects to it.
    #[tokio::test]
    async fn tabs_left_by_a_stopped_nebo_close_when_the_browser_connects() {
        let file = std::env::temp_dir().join(format!("nebo-tabs-{}-{:x}.json", std::process::id(), rand::random::<u64>()));
        let before = ExtensionBridge::new(Some(file.clone()));
        let (conn, rx) = before.connect("chrome".into()).await;
        let seen = fake_browser(&before, conn, rx);
        before.execute("navigate", &serde_json::json!({"url": "https://a.test"}), Some("s1")).await.unwrap();
        before.execute("navigate", &serde_json::json!({"url": "https://b.test"}), Some("s2")).await.unwrap();
        before.end_session("s2", false).await;
        let open = seen.lock().await[0].session_id.clone().unwrap();

        let after = ExtensionBridge::new(Some(file.clone()));
        for browser in ["chrome", "chrome again"] {
            let (_conn, mut rx) = after.connect(browser.into()).await;
            let first = rx.recv().await.unwrap();
            assert_eq!((first.tool.as_str(), first.session_id.as_deref()), ("close_session_tabs", Some(open.as_str())), "{browser}");
            settle().await;
            assert!(rx.try_recv().is_err(), "only the tab that was still open");
        }
        let _ = std::fs::remove_file(&file);
    }
}

#[cfg(test)]
mod close_tests {
    use super::*;

    /// Runs `tool` on the bridge, answering it with `reply`; returns what
    /// the browser was sent.
    async fn run(
        bridge: &ExtensionBridge,
        conn: i64,
        rx: &mut mpsc::Receiver<ToolRequest>,
        tool: &'static str,
        args: serde_json::Value,
        session: &'static str,
        reply: serde_json::Value,
    ) -> ToolRequest {
        let b = bridge.clone();
        let call = tokio::spawn(async move { b.execute(tool, &args, Some(session)).await });
        let req = rx.recv().await.unwrap();
        bridge.deliver_result(conn, req.id, Ok(reply)).await;
        call.await.unwrap().unwrap();
        req
    }

    fn refused(result: Result<serde_json::Value, String>) -> bool {
        result.is_err_and(|e| e.contains("never closes the owner's tabs"))
    }

    /// A close by id may name only a tab this run opened. The owner's tab, a
    /// float or a string, another run's tab, and a tab handed to the owner
    /// are all refused before anything reaches the browser, alone or inside
    /// a batch. A tab the run opened closes by id; its own tab by no id.
    #[tokio::test]
    async fn a_close_by_id_reaches_only_tabs_this_run_opened() {
        let bridge = ExtensionBridge::new(None);
        let (conn, mut rx) = bridge.connect("chrome".into()).await;
        let close = |args: serde_json::Value, session: &'static str| {
            let b = bridge.clone();
            async move { b.execute("close_tab", &args, Some(session)).await }
        };

        // Nothing opened yet: the owner's tab 7 is refused, nothing sent.
        assert!(refused(close(serde_json::json!({"tabId": 7}), "s1").await));
        let batch = bridge
            .batch_execute(
                vec![BatchAction { tool: "close_tab".into(), args: serde_json::json!({"tabIds": [7]}) }],
                BatchOptions { stop_on_error: false },
                Some("s1"),
            )
            .await;
        assert!(batch.is_err_and(|e| e.contains("never closes the owner's tabs")));

        // s1 opens tab 42 and s2 opens tab 43.
        run(&bridge, conn, &mut rx, "new_tab", serde_json::json!({"url": "https://a.test"}), "s1", serde_json::json!({"tabId": 42})).await;
        run(&bridge, conn, &mut rx, "new_tab", serde_json::json!({"url": "https://b.test"}), "s2", serde_json::json!({"tabId": 43})).await;
        for args in [
            serde_json::json!({"tabId": 42.0}),
            serde_json::json!({"tabId": "42"}),
            serde_json::json!({"tabIds": [42, 7]}),
            serde_json::json!({"tabIds": 42}),
            serde_json::json!({"tabId": 43}),
        ] {
            assert!(refused(close(args.clone(), "s1").await), "{args}");
        }
        assert!(rx.try_recv().is_err(), "nothing refused reached the browser");

        // Its own tab 42 closes by id; its own tab by no id.
        let req = run(&bridge, conn, &mut rx, "close_tab", serde_json::json!({"tabId": 42}), "s1", serde_json::json!("Tab closed")).await;
        assert_eq!(req.args["tabId"], 42);
        run(&bridge, conn, &mut rx, "close_tab", serde_json::json!({}), "s1", serde_json::json!("Tab closed")).await;

        // s2 hands tab 43 to the owner: no later run of s2, nor any other
        // session, may close it by id.
        assert!(bridge.end_session("s2", true).await);
        while rx.try_recv().is_ok() {}
        assert!(refused(close(serde_json::json!({"tabId": 43}), "s2").await), "the next run of s2");
        assert!(refused(close(serde_json::json!({"tabId": 43}), "s1").await));
        assert!(rx.try_recv().is_err());

        // A run's ids go with its end, and every id with a closed connection
        // (a restarted Chrome numbers its tabs anew).
        run(&bridge, conn, &mut rx, "new_tab", serde_json::json!({"url": "https://c.test"}), "s3", serde_json::json!({"tabId": 44})).await;
        bridge.end_session("s3", false).await;
        while rx.try_recv().is_ok() {}
        assert!(refused(close(serde_json::json!({"tabId": 44}), "s3").await));
        run(&bridge, conn, &mut rx, "new_tab", serde_json::json!({"url": "https://d.test"}), "s4", serde_json::json!({"tabId": 45})).await;
        bridge.disconnect(conn).await;
        let (_conn, _rx) = bridge.connect("chrome".into()).await;
        assert!(refused(close(serde_json::json!({"tabId": 45}), "s4").await));
    }
}
