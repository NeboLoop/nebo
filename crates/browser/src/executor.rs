//! Action Executor — bridges agent tool calls to browser backends.
//!
//! Routes to the Chrome extension (via ExtensionBridge) when connected,
//! or falls back to headless agent-browser (via HeadlessBridge) when available.

use std::sync::Arc;
use std::time::Duration;

use tracing::info;

use crate::BrowserError;
use crate::extension_bridge::{BatchAction, BatchOptions, ExtensionBridge};
use crate::headless_bridge::HeadlessBridge;

/// Executes browser actions, routing to the best available backend.
pub struct ActionExecutor {
    bridge: Arc<ExtensionBridge>,
    headless: Option<Arc<HeadlessBridge>>,
}

impl ActionExecutor {
    pub fn new(bridge: Arc<ExtensionBridge>, headless: Option<Arc<HeadlessBridge>>) -> Self {
        Self { bridge, headless }
    }

    /// Check if any browser backend is available.
    pub fn is_connected(&self) -> bool {
        self.bridge.is_connected() || self.headless.is_some()
    }

    /// Check if the extension was recently connected (within the given duration).
    pub async fn was_recently_connected(&self, within: Duration) -> bool {
        self.bridge.was_recently_connected(within).await || self.headless.is_some()
    }

    /// Wait for the extension to reconnect within a timeout.
    /// Returns true immediately if headless is available.
    pub async fn wait_for_connection(&self, timeout: Duration) -> bool {
        if self.headless.is_some() {
            return true;
        }
        self.bridge.wait_for_connection(timeout).await
    }

    /// True if the Chrome extension is connected (not headless fallback).
    pub fn extension_connected(&self) -> bool {
        self.bridge.is_connected()
    }

    /// True if headless agent-browser is available.
    pub fn headless_available(&self) -> bool {
        self.headless.is_some()
    }

    /// Execute a named browser tool, routing to extension or headless.
    /// `session_id` scopes the tab group in the extension (each agent gets its own tabs).
    pub async fn execute(
        &self,
        tool: &str,
        args: &serde_json::Value,
        session_id: Option<&str>,
    ) -> Result<serde_json::Value, BrowserError> {
        if self.bridge.is_connected() {
            info!(
                tool = tool,
                backend = "extension",
                "executing browser action"
            );
            self.bridge
                .execute(tool, args, session_id)
                .await
                .map_err(BrowserError::Other)
        } else if let Some(ref headless) = self.headless {
            info!(
                tool = tool,
                backend = "headless",
                "executing browser action"
            );
            headless
                .execute(tool, args, session_id)
                .await
                .map_err(BrowserError::Other)
        } else {
            Err(BrowserError::ExtensionNotConnected)
        }
    }

    /// Execute multiple actions in a single round-trip (extension) or sequentially (headless).
    pub async fn batch_execute(
        &self,
        actions: Vec<BatchAction>,
        opts: BatchOptions,
        session_id: Option<&str>,
    ) -> Result<Vec<Result<serde_json::Value, String>>, BrowserError> {
        if self.bridge.is_connected() {
            self.bridge
                .batch_execute(actions, opts, session_id)
                .await
                .map_err(BrowserError::Other)
        } else if let Some(ref headless) = self.headless {
            headless
                .batch_execute(actions, opts, session_id)
                .await
                .map_err(BrowserError::Other)
        } else {
            Err(BrowserError::ExtensionNotConnected)
        }
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
        if let Some(ref headless) = self.headless {
            headless.close_session(Some(session_id)).await;
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
