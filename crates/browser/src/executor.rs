//! Action Executor — bridges agent tool calls to browser backends.
//!
//! ONE entry point, an ordered fallback inside it:
//!   tier 1 — the user's Chrome extension (authenticated, human session),
//!   tier 2 — the built-in Rust Chrome driven over CDP (via `CdpBridge`).
//! Tier 3 (direct HTTP) lives in the caller (`web_tool`). Failover is per-call: if the extension
//! errors or times out (e.g. the native-host relay dropped), the same action is retried on CDP.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tracing::{info, warn};

use crate::BrowserError;
use crate::cdp_bridge::CdpBridge;
use crate::extension_bridge::{BatchAction, BatchOptions, ExtensionBridge};

/// Executes browser actions, routing to the best available backend.
#[derive(Clone)]
pub struct ActionExecutor {
    bridge: Arc<ExtensionBridge>,
    cdp: Option<Arc<CdpBridge>>,
    /// The built-in browser that opens this computer's own pages
    /// ([`ActionExecutor::execute_headless`]).
    own_pages: Option<Arc<CdpBridge>>,
    /// The sessions whose tab is one of this computer's own pages, open in
    /// [`Self::own_pages`] ([`ActionExecutor::open_own_page`]): every step
    /// of theirs runs there until they leave it.
    own_page_tabs: OwnPageTabs,
}

/// What closes a session's own page ([`ActionExecutor::open_own_page`]),
/// the way its opener closes it: its tab and what it was opened with.
pub type OnClose = Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send>;

/// A session's tab on one of this computer's own pages.
pub struct OwnPage {
    /// The credential in the page's address (`/k/<pass>/`): never in what
    /// a step on the page answers.
    pass: String,
    close: OnClose,
}

/// The sessions on one of this computer's own pages, shared by every
/// executor of one [`crate::Manager`].
pub type OwnPageTabs = Arc<Mutex<HashMap<String, OwnPage>>>;

/// `text` without the own page's credential (`/k/<pass>`): what a step on
/// the page answers names the page by its address alone.
fn strip_pass(text: &str, pass: &str) -> String {
    text.replace(&format!("/k/{pass}"), "")
}

/// [`strip_pass`] over every string in an answer.
fn without_pass(value: serde_json::Value, pass: &str) -> serde_json::Value {
    match value {
        serde_json::Value::String(s) => serde_json::Value::String(strip_pass(&s, pass)),
        serde_json::Value::Array(items) => items.into_iter().map(|v| without_pass(v, pass)).collect(),
        serde_json::Value::Object(map) => map.into_iter().map(|(k, v)| (k, without_pass(v, pass))).collect(),
        other => other,
    }
}

/// True when an extension error is a TRANSPORT failure (disconnect / timeout / not connected)
/// — the only case where falling back to the built-in CDP browser helps. Tool-level errors
/// (page too big, frame error, element not found, unsupported tool) mean the extension is
/// working fine and CDP can't do better, so we surface them instead of spawning a blank CDP page.
fn is_transport_failure(err: &str) -> bool {
    let e = err.to_ascii_lowercase();
    e.contains("disconnected")
        || e.contains("timed out")
        || e.contains("not connected")
        || e.contains("native host")
        || e.contains("no browser")
}

impl ActionExecutor {
    pub fn new(
        bridge: Arc<ExtensionBridge>,
        cdp: Option<Arc<CdpBridge>>,
        own_pages: Option<Arc<CdpBridge>>,
        own_page_tabs: OwnPageTabs,
    ) -> Self {
        Self { bridge, cdp, own_pages, own_page_tabs }
    }

    /// Keep `session_id`'s tab on the own page its opener just opened in
    /// [`Self::execute_headless`]'s browser (an app as the bot serves it on
    /// 127.0.0.1, which an employee works and tests): from now on every step
    /// of the session runs there, never in the owner's Chrome or the
    /// browsing browser, until the session leaves it ([`Self::leave_own_page`],
    /// [`Self::close_session`]) or the tab is gone. `pass` is the credential
    /// in the page's address: no step's answer carries it. `close` runs when
    /// the session leaves, or when another own page takes its place.
    pub async fn open_own_page(&self, session_id: &str, pass: String, close: OnClose) -> Result<(), BrowserError> {
        if session_id.is_empty() {
            close().await;
            return Err(BrowserError::Other("an own page needs a session of its own".into()));
        }
        let replaced = self
            .own_page_tabs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(session_id.to_string(), OwnPage { pass, close });
        if let Some(old) = replaced {
            (old.close)().await;
        }
        Ok(())
    }

    /// Whether `session_id`'s tab is on one of this computer's own pages. A
    /// tab that is gone (its browser shut down idle, or closed) takes the
    /// session off it, so the session's next step runs where browsing runs.
    pub async fn on_own_page(&self, session_id: &str) -> bool {
        self.own_page_pass(session_id).await.is_some()
    }

    /// The pass of `session_id`'s own page while its tab is open.
    async fn own_page_pass(&self, session_id: &str) -> Option<String> {
        let pass = self
            .own_page_tabs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(session_id)
            .map(|p| p.pass.clone())?;
        let open = match self.own_pages {
            Some(ref own) => own.has_session(session_id).await,
            None => false,
        };
        if !open {
            self.leave_own_page(session_id).await;
            return None;
        }
        Some(pass)
    }

    /// Take `session_id` off its own page: the page closes the way its
    /// opener closes it ([`Self::open_own_page`]'s `close`), and the
    /// session's next step runs where browsing runs. False when it was on
    /// none.
    pub async fn leave_own_page(&self, session_id: &str) -> bool {
        let page = self
            .own_page_tabs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(session_id);
        let Some(page) = page else { return false };
        (page.close)().await;
        true
    }

    /// Close `session_id`'s tab in [`Self::execute_headless`]'s browser.
    pub async fn close_headless(&self, session_id: &str) {
        if let Some(ref own) = self.own_pages {
            own.close_session(session_id).await;
        }
    }

    /// The browser a session on its own page runs in, and its page's pass.
    async fn own_page_browser<'a>(&'a self, session_id: Option<&'a str>) -> Option<(&'a Arc<CdpBridge>, &'a str, String)> {
        let sid = session_id?;
        let own = self.own_pages.as_ref()?;
        let pass = self.own_page_pass(sid).await?;
        Some((own, sid, pass))
    }

    /// Check if any browser backend is available.
    pub fn is_connected(&self) -> bool {
        self.bridge.is_connected() || self.cdp.is_some()
    }

    /// Check if the extension was recently connected (within the given duration).
    pub async fn was_recently_connected(&self, within: Duration) -> bool {
        self.bridge.was_recently_connected(within).await || self.cdp.is_some()
    }

    /// Wait for the extension to reconnect within a timeout.
    /// Returns true immediately if the built-in CDP browser is available.
    pub async fn wait_for_connection(&self, timeout: Duration) -> bool {
        if self.cdp.is_some() {
            return true;
        }
        self.bridge.wait_for_connection(timeout).await
    }

    /// True if the Chrome extension is connected (tier 1, not the CDP fallback).
    pub fn extension_connected(&self) -> bool {
        self.bridge.is_connected()
    }

    /// True if the built-in Rust Chrome (CDP tier-2) backend is available.
    pub fn cdp_available(&self) -> bool {
        self.cdp.is_some()
    }

    /// Execute a named browser tool: tier 1 (extension) → tier 2 (CDP), failing over on
    /// error/timeout. `session_id` scopes the tab (each sub-agent gets its own).
    pub async fn execute(
        &self,
        tool: &str,
        args: &serde_json::Value,
        session_id: Option<&str>,
    ) -> Result<serde_json::Value, BrowserError> {
        if let Some((own, sid, pass)) = self.own_page_browser(session_id).await {
            info!(tool = tool, backend = "cdp", "executing browser action on this computer's own page");
            return own
                .execute(tool, args, sid)
                .await
                .map(|v| without_pass(v, &pass))
                .map_err(|e| BrowserError::Other(strip_pass(&e.to_string(), &pass)));
        }
        if self.bridge.is_connected() {
            info!(tool = tool, backend = "extension", "executing browser action");
            match self.bridge.execute(tool, args, session_id).await {
                Ok(v) => return Ok(v),
                Err(e) => {
                    // Only fail over on a TRANSPORT failure (disconnect/timeout). Tool-level
                    // errors (page too big, frame error, element not found) mean the extension
                    // is working fine — surface them so the agent adapts; don't spin up CDP.
                    if self.cdp.is_none() || !is_transport_failure(&e) {
                        return Err(BrowserError::Other(e));
                    }
                    warn!(tool, error = %e, "extension transport failed — falling back to built-in Chrome (CDP)");
                }
            }
        }
        if let Some(ref cdp) = self.cdp {
            info!(tool = tool, backend = "cdp", "executing browser action");
            return cdp.execute(tool, args, session_id.unwrap_or("_default")).await;
        }
        Err(BrowserError::ExtensionNotConnected)
    }

    /// Execute a browser tool in the built-in headless browser that opens
    /// this computer's own pages and draws them, never the owner's Chrome
    /// window: work the owner should not see open in his own browser (an app
    /// as the bot serves it on 127.0.0.1, for its screenshots). The browsing
    /// browser, Obscura, refuses those addresses and has no paint engine
    /// (2026-10-02: every app screenshot failed, first with "Access to
    /// private/internal IP address 127.0.0.1 is not allowed"). Errors when
    /// this machine has no Chrome or Chromium.
    pub async fn execute_headless(
        &self,
        tool: &str,
        args: &serde_json::Value,
        session_id: &str,
    ) -> Result<serde_json::Value, BrowserError> {
        let cdp = self
            .own_pages
            .as_ref()
            .ok_or_else(|| {
                BrowserError::Other(
                    "this computer has no Chrome or Chromium, which drawing a page takes. Install Google Chrome \
                     and try again"
                        .into(),
                )
            })?;
        info!(tool = tool, backend = "cdp", "executing headless browser action on this computer's own pages");
        cdp.execute(tool, args, session_id).await
    }

    /// Execute multiple actions: extension batch (one round-trip) → CDP (sequential), failing
    /// over on error.
    pub async fn batch_execute(
        &self,
        actions: Vec<BatchAction>,
        opts: BatchOptions,
        session_id: Option<&str>,
    ) -> Result<Vec<Result<serde_json::Value, String>>, BrowserError> {
        if let Some((own, _, pass)) = self.own_page_browser(session_id).await {
            return Self::run_batch(own, actions, opts, session_id).await.map(|steps| {
                steps
                    .into_iter()
                    .map(|r| r.map(|v| without_pass(v, &pass)).map_err(|e| strip_pass(&e, &pass)))
                    .collect()
            });
        }
        if self.bridge.is_connected() {
            match self
                .bridge
                .batch_execute(actions.clone(), opts.clone(), session_id)
                .await
            {
                Ok(v) => return Ok(v),
                Err(e) => {
                    if self.cdp.is_none() || !is_transport_failure(&e) {
                        return Err(BrowserError::Other(e));
                    }
                    warn!(error = %e, "extension transport failed — falling back to built-in Chrome (CDP)");
                }
            }
        }
        if self.cdp.is_some() {
            return self.cdp_batch(actions, opts, session_id).await;
        }
        Err(BrowserError::ExtensionNotConnected)
    }

    /// Run a batch sequentially on the CDP backend (no native batching over CDP).
    async fn cdp_batch(
        &self,
        actions: Vec<BatchAction>,
        opts: BatchOptions,
        session_id: Option<&str>,
    ) -> Result<Vec<Result<serde_json::Value, String>>, BrowserError> {
        let cdp = self.cdp.as_ref().ok_or(BrowserError::ExtensionNotConnected)?;
        Self::run_batch(cdp, actions, opts, session_id).await
    }

    /// Run a batch step by step in one built-in browser.
    async fn run_batch(
        cdp: &CdpBridge,
        actions: Vec<BatchAction>,
        opts: BatchOptions,
        session_id: Option<&str>,
    ) -> Result<Vec<Result<serde_json::Value, String>>, BrowserError> {
        let sid = session_id.unwrap_or("_default");
        let mut out = Vec::with_capacity(actions.len());
        for a in &actions {
            match cdp.execute(&a.tool, &a.args, sid).await {
                Ok(v) => out.push(Ok(v)),
                Err(e) => {
                    out.push(Err(e.to_string()));
                    if opts.stop_on_error {
                        break;
                    }
                }
            }
        }
        Ok(out)
    }

    /// Click an element then immediately read the page — one round-trip.
    pub async fn click_and_read(
        &self,
        click_args: serde_json::Value,
        session_id: Option<&str>,
    ) -> Result<Vec<Result<serde_json::Value, String>>, BrowserError> {
        self.batch_execute(
            vec![
                BatchAction {
                    tool: "click".to_string(),
                    args: click_args,
                },
                BatchAction {
                    tool: "read_page".to_string(),
                    args: serde_json::json!({}),
                },
            ],
            BatchOptions {
                stop_on_error: false,
            },
            session_id,
        )
        .await
    }

    /// Fill a form field then read the page — one round-trip.
    pub async fn fill_and_read(
        &self,
        fill_args: serde_json::Value,
        session_id: Option<&str>,
    ) -> Result<Vec<Result<serde_json::Value, String>>, BrowserError> {
        self.batch_execute(
            vec![
                BatchAction {
                    tool: "form_input".to_string(),
                    args: fill_args,
                },
                BatchAction {
                    tool: "read_page".to_string(),
                    args: serde_json::json!({}),
                },
            ],
            BatchOptions {
                stop_on_error: false,
            },
            session_id,
        )
        .await
    }

    /// Send a fire-and-forget command to the extension (e.g., show_indicators, hide_indicators).
    pub async fn send_command(&self, command: &str, session_id: Option<&str>) {
        self.bridge.send_command(command, session_id).await;
    }

    /// Close the browser tab/page a session opened — the canonical cleanup for a
    /// finished sub-agent. Routes to BOTH backends so it works regardless of which
    /// one served the calls; each is a no-op if it never opened anything for the
    /// session, so this is safe to call unconditionally.
    pub async fn close_session(&self, session_id: &str) {
        self.leave_own_page(session_id).await;
        self.bridge.end_session(session_id, false).await;
        if let Some(ref cdp) = self.cdp {
            cdp.close_session(session_id).await;
        }
        // The same browser on a machine with a stock Chromium: closing a
        // tab already closed does nothing.
        if let Some(ref own) = self.own_pages {
            own.close_session(session_id).await;
        }
    }

    /// Leave the session's tab in the owner's Chrome open for him when the
    /// work ends (a form waiting for his approval): it is no longer the
    /// run's, so the run's end never closes it and its next browser call
    /// opens a tab of its own. False when the session has no tab open in
    /// his Chrome (the built-in browser's pages are never his to take over).
    pub async fn leave_open_for_owner(&self, session_id: &str) -> bool {
        self.bridge.end_session(session_id, true).await
    }

    /// Navigate to a URL then read the page — stops on nav error.
    pub async fn navigate_and_read(
        &self,
        nav_args: serde_json::Value,
        session_id: Option<&str>,
    ) -> Result<Vec<Result<serde_json::Value, String>>, BrowserError> {
        self.batch_execute(
            vec![
                BatchAction {
                    tool: "navigate".to_string(),
                    args: nav_args,
                },
                BatchAction {
                    tool: "read_page".to_string(),
                    args: serde_json::json!({}),
                },
            ],
            BatchOptions {
                stop_on_error: true,
            },
            session_id,
        )
        .await
    }
}
