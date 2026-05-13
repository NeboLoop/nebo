//! Data binding manager for A2UI surfaces.
//!
//! Polls MCP tools on a configurable interval and injects results into the
//! surface data model. No LLM involvement — purely deterministic data refresh.
//!
//! Lifecycle:
//! - `start_bindings()` — called when a surface is created with data_bindings
//! - `stop_bindings()` — called when a surface is deleted
//!
//! Each binding spawns a tokio task with `tokio::time::interval()`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::a2ui::A2UIManager;

/// A single data binding definition from views.json.
#[derive(Debug, Clone, Deserialize)]
pub struct DataBinding {
    /// JSON Pointer path where the result is injected (e.g. "/metrics")
    pub path: String,
    /// MCP tool source
    pub source: DataBindingSource,
    /// Optional parameters to pass to the tool
    #[serde(default)]
    pub params: serde_json::Value,
    /// Poll interval in seconds (default: 30)
    #[serde(default = "default_interval")]
    pub interval_secs: u64,
}

fn default_interval() -> u64 {
    30
}

/// MCP tool source for a data binding.
#[derive(Debug, Clone, Deserialize)]
pub struct DataBindingSource {
    /// MCP server/integration slug
    pub server: String,
    /// Tool name on the MCP server
    pub tool: String,
}

/// Manages active data binding poll tasks for A2UI surfaces.
pub struct DataBindingManager {
    a2ui: Arc<A2UIManager>,
    bridge: Arc<mcp::Bridge>,
    /// Active poll tasks per surface_id.
    tasks: RwLock<HashMap<String, Vec<JoinHandle<()>>>>,
}

impl DataBindingManager {
    pub fn new(a2ui: Arc<A2UIManager>, bridge: Arc<mcp::Bridge>) -> Self {
        Self {
            a2ui,
            bridge,
            tasks: RwLock::new(HashMap::new()),
        }
    }

    /// Start polling for all data bindings associated with a surface.
    pub async fn start_bindings(&self, surface_id: &str, bindings: Vec<DataBinding>) {
        let mut handles = Vec::new();

        for binding in bindings {
            let surface_id = surface_id.to_string();
            let a2ui = self.a2ui.clone();
            let bridge = self.bridge.clone();

            let handle = tokio::spawn(async move {
                let interval = Duration::from_secs(binding.interval_secs.max(1));
                let mut ticker = tokio::time::interval(interval);
                let mut consecutive_errors: u32 = 0;
                let max_backoff = Duration::from_secs(60);

                // Do initial fetch immediately
                ticker.tick().await;

                loop {
                    // Call MCP tool
                    match bridge
                        .call_tool(
                            &binding.source.server,
                            &binding.source.tool,
                            binding.params.clone(),
                        )
                        .await
                    {
                        Ok(result) => {
                            consecutive_errors = 0;
                            let value =
                                serde_json::to_value(&result.content).unwrap_or(json!(null));
                            if let Err(e) = a2ui
                                .update_data_model(&surface_id, Some(&binding.path), value)
                                .await
                            {
                                warn!(
                                    error = %e,
                                    surface_id = %surface_id,
                                    path = %binding.path,
                                    "data binding: failed to update data model"
                                );
                            }
                        }
                        Err(e) => {
                            consecutive_errors = consecutive_errors.saturating_add(1);
                            warn!(
                                error = %e,
                                server = %binding.source.server,
                                tool = %binding.source.tool,
                                consecutive_errors,
                                "data binding: MCP tool call failed"
                            );

                            // Exponential backoff on repeated failures
                            if consecutive_errors > 1 {
                                let backoff = Duration::from_secs(
                                    (2u64.saturating_pow(consecutive_errors.min(6)))
                                        .min(max_backoff.as_secs()),
                                );
                                tokio::time::sleep(backoff).await;
                            }
                        }
                    }

                    // Wait for next tick
                    ticker.tick().await;
                }
            });

            handles.push(handle);
        }

        if !handles.is_empty() {
            info!(
                surface_id = %surface_id,
                count = handles.len(),
                "started data binding poll tasks"
            );
            self.tasks
                .write()
                .await
                .insert(surface_id.to_string(), handles);
        }
    }

    /// Stop all poll tasks for a surface.
    pub async fn stop_bindings(&self, surface_id: &str) {
        if let Some(handles) = self.tasks.write().await.remove(surface_id) {
            for handle in handles {
                handle.abort();
            }
            debug!(surface_id = %surface_id, "stopped data binding poll tasks");
        }
    }

    /// Stop all active bindings (called on shutdown).
    pub async fn stop_all(&self) {
        let mut tasks = self.tasks.write().await;
        for (sid, handles) in tasks.drain() {
            for handle in handles {
                handle.abort();
            }
            debug!(surface_id = %sid, "stopped data binding poll tasks (shutdown)");
        }
    }
}

/// Parse data bindings from a views.json view definition.
pub fn parse_bindings(view: &serde_json::Value) -> Vec<DataBinding> {
    view.get("data_bindings")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|b| serde_json::from_value::<DataBinding>(b.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}
