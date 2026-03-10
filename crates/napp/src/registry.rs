use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use crate::manifest::Manifest;
use crate::runtime::{Process, Runtime};
use crate::signing::{RevocationChecker, SigningKeyProvider};
use crate::{NappError, InstallEvent, QuarantineEvent};

/// Where a tool was loaded from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolSource {
    /// Installed from NeboLoop marketplace (sealed .napp + extracted binary).
    Installed,
    /// User-created/sideloaded (loose files in user/tools/).
    User,
}

/// Registry configuration.
pub struct RegistryConfig {
    /// Marketplace tools — sealed .napp archives with extracted binaries.
    /// Directory: <data_dir>/nebo/tools/
    pub installed_tools_dir: PathBuf,
    /// User/sideloaded tools — loose files for development.
    /// Directory: <data_dir>/user/tools/
    pub user_tools_dir: PathBuf,
    pub neboloop_url: Option<String>,
}

/// Registered tool info (internal).
struct RegisteredTool {
    manifest: Manifest,
    process: Option<Process>,
    capabilities: Vec<String>,
    source: ToolSource,
    /// Path to the sealed .napp archive (for installed tools).
    napp_path: Option<PathBuf>,
    /// Directory where the binary lives (extracted dir or user dir).
    tool_dir: PathBuf,
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

        // Runtime uses user_tools_dir as its base (for backward compat with process launch)
        let runtime = Runtime::new(&config.user_tools_dir);

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

    /// Discover and launch all tools from both installed and user directories.
    pub async fn discover_and_launch(&self) -> Result<(), NappError> {
        // Discover installed tools from sealed .napp archives
        self.discover_installed_tools().await?;

        // Discover user/sideloaded tools (loose files)
        self.discover_user_tools().await?;

        let tools = self.tools.read().await;
        info!(count = tools.len(), "tool discovery complete");
        Ok(())
    }

    /// Discover tools in nebo/tools/ — sealed .napp archives with extracted binaries.
    async fn discover_installed_tools(&self) -> Result<(), NappError> {
        let dir = &self.config.installed_tools_dir;
        if !dir.exists() {
            std::fs::create_dir_all(dir)?;
            return Ok(());
        }

        // Walk the directory tree for .napp files
        let napp_files = find_napp_files(dir);

        for napp_path in napp_files {
            // The extracted binary dir is the same path without the .napp extension
            // e.g., @acme/tools/crm-lookup/1.2.0.napp → @acme/tools/crm-lookup/1.2.0/
            let version_dir = napp_path.with_extension("");

            // Skip quarantined tools
            if version_dir.join(".quarantined").exists() {
                warn!(dir = %version_dir.display(), "skipping quarantined installed tool");
                continue;
            }

            // Read manifest from sealed archive
            let manifest = match crate::reader::read_napp_entry(&napp_path, "manifest.json") {
                Ok(data) => {
                    match serde_json::from_slice::<Manifest>(&data) {
                        Ok(m) => m,
                        Err(e) => {
                            error!(path = %napp_path.display(), error = %e, "invalid manifest in .napp");
                            continue;
                        }
                    }
                }
                Err(e) => {
                    debug!(path = %napp_path.display(), error = %e, "no manifest in .napp (may not be a tool)");
                    continue;
                }
            };

            if let Err(e) = manifest.validate() {
                error!(path = %napp_path.display(), error = %e, "manifest validation failed");
                continue;
            }

            // Check that extracted binary exists
            let binary_path = version_dir.join("binary");
            let app_path = version_dir.join("app");
            if !binary_path.exists() && !app_path.exists() {
                warn!(
                    path = %napp_path.display(),
                    "installed tool missing extracted binary — run install again"
                );
                continue;
            }

            // Verify binary integrity against sealed archive
            if let Err(e) = self.verify_installed_binary(&napp_path, &version_dir).await {
                error!(path = %napp_path.display(), error = %e, "binary integrity check failed");
                self.quarantine(manifest.id(), &version_dir, &format!("integrity check failed: {}", e)).await;
                continue;
            }

            match self.launch_tool_from_dir(&version_dir, manifest, ToolSource::Installed, Some(napp_path)).await {
                Ok(()) => {}
                Err(e) => {
                    error!(dir = %version_dir.display(), error = %e, "failed to launch installed tool");
                }
            }
        }

        Ok(())
    }

    /// Discover tools in user/tools/ — loose files (sideloaded).
    async fn discover_user_tools(&self) -> Result<(), NappError> {
        let dir = &self.config.user_tools_dir;
        if !dir.exists() {
            std::fs::create_dir_all(dir)?;
            return Ok(());
        }

        let entries: Vec<_> = std::fs::read_dir(dir)?
            .flatten()
            .filter(|e| e.path().is_dir())
            .collect();

        for entry in entries {
            let tool_dir = entry.path();

            // Skip quarantined tools
            if tool_dir.join(".quarantined").exists() {
                warn!(dir = %tool_dir.display(), "skipping quarantined user tool");
                continue;
            }

            // Skip if no manifest
            if !tool_dir.join("manifest.json").exists() {
                continue;
            }

            let manifest = match Manifest::load(&tool_dir.join("manifest.json")) {
                Ok(m) => m,
                Err(e) => {
                    error!(dir = %tool_dir.display(), error = %e, "failed to load manifest");
                    continue;
                }
            };

            if let Err(e) = manifest.validate() {
                error!(dir = %tool_dir.display(), error = %e, "manifest validation failed");
                continue;
            }

            // Detect symlinks (sideloaded via symlink)
            let is_sideloaded = tool_dir
                .symlink_metadata()
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false);

            let source = if is_sideloaded {
                ToolSource::User
            } else {
                ToolSource::User
            };

            match self.launch_tool_from_dir(&tool_dir, manifest, source, None).await {
                Ok(()) => {}
                Err(e) => {
                    error!(dir = %tool_dir.display(), error = %e, "failed to launch user tool");
                }
            }
        }

        Ok(())
    }

    /// Launch a tool from a directory containing its binary.
    async fn launch_tool_from_dir(
        &self,
        tool_dir: &Path,
        manifest: Manifest,
        source: ToolSource,
        napp_path: Option<PathBuf>,
    ) -> Result<(), NappError> {
        // Revocation check
        if let Some(ref checker) = self.revocation {
            if checker.is_revoked(manifest.id()).await? {
                self.quarantine(manifest.id(), tool_dir, "tool revoked by NeboLoop")
                    .await;
                return Err(NappError::Revoked(manifest.id().to_string()));
            }
        }

        // Signature verification for installed tools (not sideloaded)
        if source == ToolSource::Installed {
            if let Some(ref napp) = napp_path {
                if let Some(ref signing) = self.signing {
                    let key = signing.get_key().await?;
                    if let Err(e) = crate::signing::verify_signatures(&key, tool_dir) {
                        self.quarantine(manifest.id(), tool_dir, &format!("signature verification failed: {}", e)).await;
                        return Err(NappError::Signing(format!("signature verification failed: {}", e)));
                    }
                    let _ = napp; // napp_path used only to gate this branch
                }
            }
        }

        // Launch process
        let process = self.runtime.launch(tool_dir).await?;
        let capabilities = manifest.provides.clone();

        let tool_id = manifest.id().to_string();
        let mut tools = self.tools.write().await;
        tools.insert(
            tool_id,
            RegisteredTool {
                manifest,
                process: Some(process),
                capabilities,
                source,
                napp_path,
                tool_dir: tool_dir.to_path_buf(),
            },
        );

        Ok(())
    }

    /// Verify that the extracted binary matches the hash in the sealed archive.
    async fn verify_installed_binary(&self, napp_path: &Path, version_dir: &Path) -> Result<(), NappError> {
        // Read manifest from sealed archive to get expected binary hash
        let manifest_data = crate::reader::read_napp_entry(napp_path, "manifest.json")?;
        let manifest: Manifest = serde_json::from_slice(&manifest_data)
            .map_err(|e| NappError::Signing(format!("invalid manifest: {}", e)))?;

        // If manifest has a signature with binary_hash, verify it
        if let Some(ref sig) = manifest.signature {
            if !sig.binary_hash.is_empty() {
                let binary_path = if version_dir.join("binary").exists() {
                    version_dir.join("binary")
                } else if version_dir.join("app").exists() {
                    version_dir.join("app")
                } else {
                    return Err(NappError::Signing("no binary found for verification".into()));
                };

                let binary_data = std::fs::read(&binary_path)?;
                use sha2::Digest;
                let hash = hex::encode(sha2::Sha256::digest(&binary_data));

                if hash != sig.binary_hash {
                    return Err(NappError::Signing(format!(
                        "binary hash mismatch: expected {}, got {}",
                        sig.binary_hash, hash
                    )));
                }
            }
        }

        Ok(())
    }

    /// Stop and unregister a tool.
    pub async fn uninstall(&self, tool_id: &str) -> Result<(), NappError> {
        let tool_dir = {
            let mut tools = self.tools.write().await;
            if let Some(mut tool) = tools.remove(tool_id) {
                if let Some(ref mut process) = tool.process {
                    process.stop().await;
                }
                Some((tool.tool_dir.clone(), tool.napp_path.clone()))
            } else {
                None
            }
        };

        if let Some((dir, napp_path)) = tool_dir {
            if dir.exists() {
                std::fs::remove_dir_all(&dir)?;
            }
            // Also remove the .napp archive if it exists
            if let Some(napp) = napp_path {
                let _ = std::fs::remove_file(&napp);
            }
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

        // Create symlink in user tools dir
        let link_path = self.config.user_tools_dir.join(manifest.id());
        if link_path.exists() {
            std::fs::remove_file(&link_path).or_else(|_| std::fs::remove_dir_all(&link_path))?;
        }

        #[cfg(unix)]
        std::os::unix::fs::symlink(project_dir, &link_path)
            .map_err(|e| NappError::Other(format!("create symlink: {}", e)))?;

        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(project_dir, &link_path)
            .map_err(|e| NappError::Other(format!("create symlink: {}", e)))?;

        let tool_id = manifest.id().to_string();
        self.launch_tool_from_dir(&link_path, manifest, ToolSource::User, None).await?;
        info!(tool = tool_id.as_str(), "tool sideloaded");
        Ok(tool_id)
    }

    /// Remove a sideloaded tool.
    pub async fn unsideload(&self, tool_id: &str) -> Result<(), NappError> {
        let link_path = self.config.user_tools_dir.join(tool_id);

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

    /// List registered processes.
    pub async fn list_processes(&self) -> Vec<ProcessInfo> {
        let tools = self.tools.read().await;
        tools.values()
            .map(|t| ProcessInfo {
                id: t.manifest.id().to_string(),
                name: t.manifest.name.clone(),
                version: t.manifest.version.clone(),
                description: t.manifest.description.clone(),
                provides: t.capabilities.clone(),
                running: t.process.as_ref().map(|p| p.is_alive()).unwrap_or(false),
                sideloaded: t.source == ToolSource::User,
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
    ///
    /// For marketplace tools: stores sealed .napp in nebo/tools/ and extracts
    /// only the binary (and ui/) to a version directory alongside it.
    pub async fn install_from_url(&self, url: &str) -> Result<String, NappError> {
        let tmp_dir = self.config.installed_tools_dir.join(".tmp");
        std::fs::create_dir_all(&tmp_dir)?;

        let tmp_file = tmp_dir.join(format!("{}.napp", uuid::Uuid::new_v4()));

        // Download
        let resp = reqwest::get(url).await?;
        if !resp.status().is_success() {
            return Err(NappError::Http(resp.error_for_status().unwrap_err()));
        }
        let bytes = resp.bytes().await?;

        // 500MB limit
        if bytes.len() > 500 * 1024 * 1024 {
            return Err(NappError::Extraction("download exceeds 500MB limit".into()));
        }

        std::fs::write(&tmp_file, &bytes)?;

        // Read manifest from the archive (don't fully extract)
        let manifest_data = crate::reader::read_napp_entry(&tmp_file, "manifest.json")?;
        let manifest: Manifest = serde_json::from_slice(&manifest_data)
            .map_err(|e| NappError::Manifest(format!("invalid manifest: {}", e)))?;
        manifest.validate()?;

        // Build qualified path: nebo/tools/<tool_id>/<version>.napp
        let tool_base = self.config.installed_tools_dir.join(manifest.id());
        std::fs::create_dir_all(&tool_base)?;

        let napp_dest = tool_base.join(format!("{}.napp", manifest.version));
        let version_dir = tool_base.join(&manifest.version);

        // Move .napp to permanent location
        if napp_dest.exists() {
            let _ = std::fs::remove_file(&napp_dest);
        }
        std::fs::rename(&tmp_file, &napp_dest)?;

        // Extract only binary and ui/ to the version directory
        if version_dir.exists() {
            let _ = std::fs::remove_dir_all(&version_dir);
        }
        std::fs::create_dir_all(&version_dir)?;

        // Extract binary
        for binary_name in &["binary", "app"] {
            let dest = version_dir.join(binary_name);
            match crate::reader::extract_napp_entry(&napp_dest, binary_name, &dest) {
                Ok(()) => {
                    info!(binary = binary_name, "extracted binary from .napp");
                    break;
                }
                Err(NappError::NotFound(_)) => continue,
                Err(e) => return Err(e),
            }
        }

        // Extract ui/ assets if present
        let _ = crate::reader::extract_napp_prefix(&napp_dest, "ui/", &version_dir);

        // Also write manifest.json to the version dir so Runtime can find it
        std::fs::write(version_dir.join("manifest.json"), &manifest_data)?;

        let tool_id = manifest.id().to_string();
        self.launch_tool_from_dir(&version_dir, manifest, ToolSource::Installed, Some(napp_dest)).await?;

        info!(tool = tool_id.as_str(), "tool installed from URL (sealed .napp)");
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
                // Find the tool's directory
                let tool_dir = {
                    let tools = self.tools.read().await;
                    tools.get(&event.tool_id).map(|t| t.tool_dir.clone())
                };
                if let Some(dir) = tool_dir {
                    self.quarantine(&event.tool_id, &dir, "revoked by NeboLoop").await;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

/// Recursively find all .napp files in a directory tree.
fn find_napp_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    fn walk(dir: &Path, files: &mut Vec<PathBuf>) {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                // Skip .tmp and hidden directories
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.starts_with('.') {
                        continue;
                    }
                }
                walk(&path, files);
            } else if path.extension().is_some_and(|ext| ext == "napp") {
                files.push(path);
            }
        }
    }
    walk(dir, &mut files);
    files
}

/// Public process info.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub provides: Vec<String>,
    pub running: bool,
    pub sideloaded: bool,
}
