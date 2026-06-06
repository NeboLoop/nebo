//! Action Executor — bridges agent tool calls to browser backends.
//!
//! ONE entry point, an ordered fallback inside it:
//!   tier 1 — the user's Chrome extension (authenticated, human session),
//!   tier 2 — the built-in Rust Chrome driven over CDP (via `CdpBridge`).
//! Tier 3 (direct HTTP) lives in the caller (`web_tool`). Failover is per-call: if the extension
//! errors or times out (e.g. the native-host relay dropped), the same action is retried on CDP.

use std::sync::Arc;
use std::time::Duration;

use tracing::{info, warn};

use crate::BrowserError;
use crate::cdp_bridge::CdpBridge;
use crate::extension_bridge::{BatchAction, BatchOptions, ExtensionBridge};

/// Executes browser actions, routing to the best available backend.
pub struct ActionExecutor {
    bridge: Arc<ExtensionBridge>,
    cdp: Option<Arc<CdpBridge>>,
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
    pub fn new(bridge: Arc<ExtensionBridge>, cdp: Option<Arc<CdpBridge>>) -> Self {
        Self { bridge, cdp }
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

    /// Execute multiple actions: extension batch (one round-trip) → CDP (sequential), failing
    /// over on error.
    pub async fn batch_execute(
        &self,
        actions: Vec<BatchAction>,
        opts: BatchOptions,
        session_id: Option<&str>,
    ) -> Result<Vec<Result<serde_json::Value, String>>, BrowserError> {
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
        self.bridge
            .send_command("hide_indicators", Some(session_id))
            .await;
        self.bridge
            .send_command("close_session_tabs", Some(session_id))
            .await;
        if let Some(ref cdp) = self.cdp {
            cdp.close_session(session_id).await;
        }
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
