//! Action Executor — bridges agent tool calls to the Chrome extension
//! via the ExtensionBridge (WebSocket relay to native messaging bridge process).
//!
//! The executor sends tool requests through the bridge queue. The server's
//! WS handler relays them to the bridge process, which forwards to the
//! extension via stdout (native messaging). Results flow back the same path.

use std::sync::Arc;
use std::time::Duration;

use tracing::info;

use crate::extension_bridge::{BatchAction, BatchOptions, ExtensionBridge};
use crate::BrowserError;

/// Executes browser actions by dispatching them to the Chrome extension
/// via the extension bridge.
pub struct ActionExecutor {
    bridge: Arc<ExtensionBridge>,
}

impl ActionExecutor {
    pub fn new(bridge: Arc<ExtensionBridge>) -> Self {
        Self { bridge }
    }

    /// Check if the extension is connected.
    pub fn is_connected(&self) -> bool {
        self.bridge.is_connected()
    }

    /// Check if the extension was recently connected (within the given duration).
    pub async fn was_recently_connected(&self, within: Duration) -> bool {
        self.bridge.was_recently_connected(within).await
    }

    /// Wait for the extension to reconnect within a timeout.
    pub async fn wait_for_connection(&self, timeout: Duration) -> bool {
        self.bridge.wait_for_connection(timeout).await
    }

    /// Execute a named browser tool.
    ///
    /// Tool names: read_page, navigate, click, fill, type, select, screenshot,
    ///             scroll, press, go_back, go_forward, wait, evaluate,
    ///             new_tab, close_tab, list_tabs
    pub async fn execute(
        &self,
        tool: &str,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, BrowserError> {
        info!(tool = tool, "executing browser action via extension");
        self.bridge
            .execute(tool, args)
            .await
            .map_err(|e| BrowserError::Other(e))
    }

    /// Execute multiple actions in a single WS round-trip.
    pub async fn batch_execute(
        &self,
        actions: Vec<BatchAction>,
        opts: BatchOptions,
    ) -> Result<Vec<Result<serde_json::Value, String>>, BrowserError> {
        self.bridge
            .batch_execute(actions, opts)
            .await
            .map_err(|e| BrowserError::Other(e))
    }

    /// Click an element then immediately read the page — one round-trip.
    pub async fn click_and_read(
        &self,
        click_args: serde_json::Value,
    ) -> Result<Vec<Result<serde_json::Value, String>>, BrowserError> {
        self.batch_execute(
            vec![
                BatchAction { tool: "click".to_string(), args: click_args },
                BatchAction { tool: "read_page".to_string(), args: serde_json::json!({}) },
            ],
            BatchOptions { stop_on_error: false },
        ).await
    }

    /// Fill a form field then read the page — one round-trip.
    pub async fn fill_and_read(
        &self,
        fill_args: serde_json::Value,
    ) -> Result<Vec<Result<serde_json::Value, String>>, BrowserError> {
        self.batch_execute(
            vec![
                BatchAction { tool: "form_input".to_string(), args: fill_args },
                BatchAction { tool: "read_page".to_string(), args: serde_json::json!({}) },
            ],
            BatchOptions { stop_on_error: false },
        ).await
    }

    /// Navigate to a URL then read the page — stops on nav error.
    pub async fn navigate_and_read(
        &self,
        nav_args: serde_json::Value,
    ) -> Result<Vec<Result<serde_json::Value, String>>, BrowserError> {
        self.batch_execute(
            vec![
                BatchAction { tool: "navigate".to_string(), args: nav_args },
                BatchAction { tool: "read_page".to_string(), args: serde_json::json!({}) },
            ],
            BatchOptions { stop_on_error: true },
        ).await
    }
}
