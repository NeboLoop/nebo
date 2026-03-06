//! NappManagerImpl — bridges napp::Registry to tools::tools::NappManager trait.
//!
//! Wraps the napp registry and maintains a map of NappToolAdapter instances
//! for dispatching tool calls to running gRPC subprocesses.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::{info, warn};

use tools::tools::{NappManager, NappToolInfo};
use tools::registry::{DynTool, ToolResult};
use tools::origin::ToolContext;

use crate::napp_adapter::NappToolAdapter;

/// Concrete implementation of NappManager.
///
/// Holds the napp registry for lifecycle operations and a map of adapters
/// for dispatching tool calls via gRPC.
pub struct NappManagerImpl {
    registry: Arc<napp::Registry>,
    store: Arc<db::Store>,
    config: config::Config,
    /// tool_id → adapter. Snapshot-then-release for dispatch (CODE_AUDITOR Rule 9.3).
    adapters: RwLock<HashMap<String, Arc<NappToolAdapter>>>,
}

impl NappManagerImpl {
    pub fn new(
        registry: Arc<napp::Registry>,
        store: Arc<db::Store>,
        config: config::Config,
    ) -> Self {
        Self {
            registry,
            store,
            config,
            adapters: RwLock::new(HashMap::new()),
        }
    }

    /// Sync adapters for all currently running tools.
    /// Called after discover_and_launch() to create gRPC clients.
    pub async fn sync_adapters(&self) {
        let tools = self.registry.list_tools().await;
        for tool in &tools {
            if !tool.running {
                continue;
            }
            if let Err(e) = self.create_adapter_inner(&tool.id).await {
                warn!(tool_id = %tool.id, error = %e, "failed to create adapter during sync");
            }
        }
        let adapters = self.adapters.read().await;
        info!(count = adapters.len(), "napp adapters synced");
    }

    /// Create an adapter for a single tool (after install/sideload).
    pub async fn create_adapter(&self, tool_id: &str) -> Result<(), String> {
        self.create_adapter_inner(tool_id).await
    }

    /// Remove an adapter (after uninstall/unsideload).
    pub async fn remove_adapter(&self, tool_id: &str) {
        let mut adapters = self.adapters.write().await;
        if adapters.remove(tool_id).is_some() {
            info!(tool_id = %tool_id, "removed napp adapter");
        }
    }

    async fn create_adapter_inner(&self, tool_id: &str) -> Result<(), String> {
        let endpoint = self.registry.get_endpoint(tool_id).await
            .ok_or_else(|| format!("no endpoint for tool {}", tool_id))?;

        let adapter = NappToolAdapter::new(&endpoint, tool_id.to_string()).await?;
        let mut adapters = self.adapters.write().await;
        adapters.insert(tool_id.to_string(), Arc::new(adapter));
        info!(tool_id = %tool_id, "created napp adapter");
        Ok(())
    }

    fn build_api_client(&self) -> Result<comm::api::NeboLoopApi, String> {
        let bot_id = config::read_bot_id()
            .ok_or_else(|| "no bot_id configured".to_string())?;
        let profiles = self.store
            .list_active_auth_profiles_by_provider("neboloop")
            .unwrap_or_default();
        let profile = profiles
            .first()
            .ok_or_else(|| "not connected to NeboLoop".to_string())?;
        let api_server = self.config.neboloop.api_url.clone();
        Ok(comm::api::NeboLoopApi::new(api_server, bot_id, profile.api_key.clone()))
    }

    fn tool_info_from_napp(info: &napp::registry::ToolInfo) -> NappToolInfo {
        NappToolInfo {
            id: info.id.clone(),
            name: info.name.clone(),
            version: info.version.clone(),
            description: info.description.clone(),
            provides: info.provides.clone(),
            running: info.running,
            sideloaded: info.sideloaded,
        }
    }
}

impl NappManager for NappManagerImpl {
    fn list(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<NappToolInfo>> + Send + '_>> {
        Box::pin(async move {
            let tools = self.registry.list_tools().await;
            tools.iter().map(Self::tool_info_from_napp).collect()
        })
    }

    fn install<'a>(&'a self, code: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<NappToolInfo, String>> + Send + 'a>> {
        Box::pin(async move {
            let api = self.build_api_client()?;
            let resp = api
                .install_app(code)
                .await
                .map_err(|e| format!("install_app: {}", e))?;

            let download_url = resp.download_url(
                api.api_server(),
                &format!("/api/v1/apps/{}/download", code),
            );

            let tool_id = self.registry
                .install_from_url(&download_url)
                .await
                .map_err(|e| format!("install_from_url: {}", e))?;

            // Create adapter for the newly installed tool
            if let Err(e) = self.create_adapter_inner(&tool_id).await {
                warn!(tool_id = %tool_id, error = %e, "adapter creation failed after install");
            }

            // Return info about the installed tool
            let tools = self.registry.list_tools().await;
            tools.iter()
                .find(|t| t.id == tool_id)
                .map(Self::tool_info_from_napp)
                .ok_or_else(|| "tool installed but not found in registry".to_string())
        })
    }

    fn uninstall<'a>(&'a self, tool_id: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            self.remove_adapter(tool_id).await;
            self.registry.uninstall(tool_id).await
                .map_err(|e| format!("uninstall: {}", e))
        })
    }

    fn sideload<'a>(&'a self, path: &'a str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<NappToolInfo, String>> + Send + 'a>> {
        Box::pin(async move {
            let project_dir = std::path::Path::new(path);
            let tool_id = self.registry.sideload(project_dir).await
                .map_err(|e| format!("sideload: {}", e))?;

            // Create adapter for the sideloaded tool
            if let Err(e) = self.create_adapter_inner(&tool_id).await {
                warn!(tool_id = %tool_id, error = %e, "adapter creation failed after sideload");
            }

            let tools = self.registry.list_tools().await;
            tools.iter()
                .find(|t| t.id == tool_id)
                .map(Self::tool_info_from_napp)
                .ok_or_else(|| "tool sideloaded but not found in registry".to_string())
        })
    }

    fn dispatch<'a>(
        &'a self,
        tool_name: &'a str,
        input: serde_json::Value,
        ctx: &'a ToolContext,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            // Snapshot adapter map, release lock, then call (Rule 9.3)
            let adapter = {
                let adapters = self.adapters.read().await;
                // Try exact ID match first
                if let Some(a) = adapters.get(tool_name) {
                    Some(a.clone())
                } else {
                    // Fall back to matching by adapter name
                    adapters.values().find(|a| a.name() == tool_name).cloned()
                }
            };

            match adapter {
                Some(a) => a.execute_dyn(ctx, input).await,
                None => {
                    let available = self.tool_names().await;
                    ToolResult::error(format!(
                        "no installed tool named {:?}. Available: {}",
                        tool_name,
                        if available.is_empty() { "none".to_string() } else { available.join(", ") }
                    ))
                }
            }
        })
    }

    fn tool_names(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<String>> + Send + '_>> {
        Box::pin(async move {
            let adapters = self.adapters.read().await;
            adapters.values().map(|a| a.name().to_string()).collect()
        })
    }
}
