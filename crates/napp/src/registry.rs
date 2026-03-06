use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tokio::sync::RwLock;
use tracing::{error, info, warn};

use crate::manifest::Manifest;
use crate::runtime::{Process, Runtime};
use crate::signing::{RevocationChecker, SigningKeyProvider};
use crate::{NappError, InstallEvent, QuarantineEvent};

/// Registry configuration.
pub struct RegistryConfig {
    pub tools_dir: PathBuf,
    pub neboloop_url: Option<String>,
}

/// Registered tool info (internal).
struct RegisteredTool {
    manifest: Manifest,
    process: Option<Process>,
    capabilities: Vec<String>,
    is_sideloaded: bool,
}

/// Tool registry manages discovery, launching, and capability registration.
pub struct Registry {
    config: RegistryConfig,
    runtime: Runtime,
    signing: Option<SigningKeyProvider>,
    revocation: Option<RevocationChecker>,
    tools: RwLock<HashMap<String, RegisteredTool>>,
    on_quarantine: RwLock<Option<Box<dyn Fn(QuarantineEvent) + Send + Sync>>>,
}

impl Registry {
    pub fn new(config: RegistryConfig) -> Self {
        let signing = config
            .neboloop_url
            .as_ref()
            .map(|url| SigningKeyProvider::new(url));
        let revocation = config
            .neboloop_url
            .as_ref()
            .map(|url| RevocationChecker::new(url));

        let runtime = Runtime::new(&config.tools_dir);

        Self {
            config,
            runtime,
            signing,
            revocation,
            tools: RwLock::new(HashMap::new()),
            on_quarantine: RwLock::new(None),
        }
    }

    /// Set callback for quarantine events.
    pub async fn set_quarantine_handler(
        &self,
        handler: impl Fn(QuarantineEvent) + Send + Sync + 'static,
    ) {
        let mut h = self.on_quarantine.write().await;
        *h = Some(Box::new(handler));
    }

    /// Discover and launch all tools in the tools directory.
    pub async fn discover_and_launch(&self) -> Result<(), NappError> {
        let tools_dir = &self.config.tools_dir;
        if !tools_dir.exists() {
            std::fs::create_dir_all(tools_dir)?;
            return Ok(());
        }

        let entries: Vec<_> = std::fs::read_dir(tools_dir)?
            .flatten()
            .filter(|e| e.path().is_dir())
            .collect();

        for entry in entries {
            let tool_dir = entry.path();

            // Skip quarantined tools
            if tool_dir.join(".quarantined").exists() {
                warn!(dir = %tool_dir.display(), "skipping quarantined tool");
                continue;
            }

            // Skip if no manifest
            if !tool_dir.join("manifest.json").exists() {
                continue;
            }

            match self.launch_tool(&tool_dir).await {
                Ok(()) => {}
                Err(e) => {
                    error!(dir = %tool_dir.display(), error = %e, "failed to launch tool");
                }
            }
        }

        let tools = self.tools.read().await;
        info!(count = tools.len(), "tool discovery complete");
        Ok(())
    }

    /// Launch a single tool.
    async fn launch_tool(&self, tool_dir: &Path) -> Result<(), NappError> {
        let manifest = Manifest::load(&tool_dir.join("manifest.json"))?;
        manifest.validate()?;

        // Revocation check
        if let Some(ref checker) = self.revocation {
            if checker.is_revoked(&manifest.id).await? {
                self.quarantine(&manifest.id, tool_dir, "tool revoked by NeboLoop")
                    .await;
                return Err(NappError::Revoked(manifest.id.clone()));
            }
        }

        // Signature verification (skip for sideloaded)
        let is_sideloaded = tool_dir
            .symlink_metadata()
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);

        if !is_sideloaded {
            if let Some(ref signing) = self.signing {
                if tool_dir.join("signatures.json").exists() {
                    let key = signing.get_key().await?;
                    crate::signing::verify_signatures(&key, tool_dir)?;
                }
            }
        }

        // Launch process
        let process = self.runtime.launch(tool_dir).await?;
        let capabilities = manifest.provides.clone();

        let mut tools = self.tools.write().await;
        tools.insert(
            manifest.id.clone(),
            RegisteredTool {
                manifest,
                process: Some(process),
                capabilities,
                is_sideloaded,
            },
        );

        Ok(())
    }

    /// Stop and unregister a tool.
    pub async fn uninstall(&self, tool_id: &str) -> Result<(), NappError> {
        let mut tools = self.tools.write().await;
        if let Some(mut tool) = tools.remove(tool_id) {
            if let Some(ref mut process) = tool.process {
                process.stop().await;
            }
        }

        // Remove directory
        let tool_dir = self.config.tools_dir.join(tool_id);
        if tool_dir.exists() {
            std::fs::remove_dir_all(&tool_dir)?;
        }

        info!(tool = tool_id, "tool uninstalled");
        Ok(())
    }

    /// Quarantine a tool (preserve data, remove binary).
    async fn quarantine(&self, tool_id: &str, tool_dir: &Path, reason: &str) {
        // Stop process
        {
            let mut tools = self.tools.write().await;
            if let Some(mut tool) = tools.remove(tool_id) {
                if let Some(ref mut process) = tool.process {
                    process.stop().await;
                }
            }
        }

        // Remove binary but preserve data/ and logs/
        for name in &["binary", "app"] {
            let _ = std::fs::remove_file(tool_dir.join(name));
        }

        // Create quarantine marker
        let _ = std::fs::write(tool_dir.join(".quarantined"), reason);

        // Emit event
        let handler = self.on_quarantine.read().await;
        if let Some(ref cb) = *handler {
            cb(QuarantineEvent {
                tool_id: tool_id.to_string(),
                reason: reason.to_string(),
            });
        }

        warn!(tool = tool_id, reason, "tool quarantined");
    }

    /// Sideload a tool from a developer directory.
    pub async fn sideload(&self, project_dir: &Path) -> Result<String, NappError> {
        let manifest = Manifest::load(&project_dir.join("manifest.json"))?;
        manifest.validate()?;

        // Create symlink in tools dir
        let link_path = self.config.tools_dir.join(&manifest.id);
        if link_path.exists() {
            std::fs::remove_file(&link_path).or_else(|_| std::fs::remove_dir_all(&link_path))?;
        }

        #[cfg(unix)]
        std::os::unix::fs::symlink(project_dir, &link_path)
            .map_err(|e| NappError::Other(format!("create symlink: {}", e)))?;

        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(project_dir, &link_path)
            .map_err(|e| NappError::Other(format!("create symlink: {}", e)))?;

        let tool_id = manifest.id.clone();
        self.launch_tool(&link_path).await?;
        info!(tool = tool_id.as_str(), "tool sideloaded");
        Ok(tool_id)
    }

    /// Remove a sideloaded tool.
    pub async fn unsideload(&self, tool_id: &str) -> Result<(), NappError> {
        let link_path = self.config.tools_dir.join(tool_id);

        // Safety: verify it's a symlink
        let meta = std::fs::symlink_metadata(&link_path)
            .map_err(|e| NappError::NotFound(format!("tool {}: {}", tool_id, e)))?;
        if !meta.file_type().is_symlink() {
            return Err(NappError::Other(format!(
                "{} is not sideloaded (not a symlink)",
                tool_id
            )));
        }

        // Stop process
        let mut tools = self.tools.write().await;
        if let Some(mut tool) = tools.remove(tool_id) {
            if let Some(ref mut process) = tool.process {
                process.stop().await;
            }
        }

        // Remove symlink
        std::fs::remove_file(&link_path)?;
        info!(tool = tool_id, "tool unsideloaded");
        Ok(())
    }

    /// List registered tools.
    pub async fn list_tools(&self) -> Vec<ToolInfo> {
        let tools = self.tools.read().await;
        tools.values()
            .map(|t| ToolInfo {
                id: t.manifest.id.clone(),
                name: t.manifest.name.clone(),
                version: t.manifest.version.clone(),
                description: t.manifest.description.clone(),
                provides: t.capabilities.clone(),
                running: t.process.as_ref().map(|p| p.is_alive()).unwrap_or(false),
                sideloaded: t.is_sideloaded,
            })
            .collect()
    }

    /// Get the gRPC endpoint for a running tool.
    pub async fn get_endpoint(&self, tool_id: &str) -> Option<String> {
        let tools = self.tools.read().await;
        tools.get(tool_id).and_then(|t| {
            t.process.as_ref().filter(|p| p.is_alive()).map(|p| p.grpc_endpoint())
        })
    }

    /// Get a tool's manifest.
    pub async fn get_manifest(&self, tool_id: &str) -> Option<Manifest> {
        let tools = self.tools.read().await;
        tools.get(tool_id).map(|t| t.manifest.clone())
    }

    /// Shutdown all tools.
    pub async fn shutdown(&self) {
        let mut tools = self.tools.write().await;
        for (id, tool) in tools.iter_mut() {
            if let Some(ref mut process) = tool.process {
                process.stop().await;
            }
            info!(tool = id.as_str(), "tool shutdown");
        }
        tools.clear();
    }

    /// Download and install a .napp from a URL.
    pub async fn install_from_url(&self, url: &str) -> Result<String, NappError> {
        let tmp_dir = self.config.tools_dir.join(".tmp");
        std::fs::create_dir_all(&tmp_dir)?;

        let tmp_file = tmp_dir.join(format!("{}.napp", uuid::Uuid::new_v4()));

        // Download
        let resp = reqwest::get(url).await?;
        if !resp.status().is_success() {
            return Err(NappError::Http(resp.error_for_status().unwrap_err()));
        }
        let bytes = resp.bytes().await?;

        // 600MB limit
        if bytes.len() > 600 * 1024 * 1024 {
            return Err(NappError::Extraction("download exceeds 600MB limit".into()));
        }

        std::fs::write(&tmp_file, &bytes)?;

        // Extract to temp dir
        let extract_dir = tmp_dir.join(format!("extract-{}", uuid::Uuid::new_v4()));
        let manifest = crate::napp::extract_napp(&tmp_file, &extract_dir)?;

        // Move to permanent location
        let tool_dir = self.config.tools_dir.join(&manifest.id);
        if tool_dir.exists() {
            // Already installed — replace
            let _ = std::fs::remove_dir_all(&tool_dir);
        }
        std::fs::rename(&extract_dir, &tool_dir)?;

        // Clean up temp file
        let _ = std::fs::remove_file(&tmp_file);

        let tool_id = manifest.id.clone();
        self.launch_tool(&tool_dir).await?;

        info!(tool = tool_id.as_str(), "tool installed from URL");
        Ok(tool_id)
    }

    /// Handle an install event from NeboLoop.
    pub async fn handle_install_event(&self, event: InstallEvent) -> Result<(), NappError> {
        match event.event_type.as_str() {
            "tool_installed" => {
                let url = event.payload["download_url"].as_str()
                    .ok_or(NappError::Other("missing download_url".into()))?;
                self.install_from_url(url).await?;
            }
            "tool_uninstalled" => {
                self.uninstall(&event.tool_id).await?;
            }
            "tool_revoked" => {
                let dir = self.config.tools_dir.join(&event.tool_id);
                self.quarantine(&event.tool_id, &dir, "revoked by NeboLoop").await;
            }
            _ => {}
        }
        Ok(())
    }
}

/// Public tool info.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub provides: Vec<String>,
    pub running: bool,
    pub sideloaded: bool,
}
