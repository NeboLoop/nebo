//! Plugin primitive — managed binaries downloaded once, shared across skills.
//!
//! Skills declare plugin dependencies in SKILL.md frontmatter:
//! ```yaml
//! plugins:
//!   - name: gws
//!     version: ">=1.2.0"
//! ```
//!
//! Plugins are downloaded from NeboLoop, verified (SHA256 + ED25519), and stored at
//! `<data_dir>/nebo/plugins/<slug>/<version>/`. User-provided plugins live at
//! `<data_dir>/user/plugins/<slug>/<version>/` and override installed versions.
//! Multiple skills can share the same plugin binary. Scripts access the binary via
//! `{SLUG}_BIN` environment variable.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{debug, info, warn};

use crate::signing::SigningKeyProvider;
use crate::NappError;

// ── Types ───────────────────────────────────────────────────────────

/// Plugin manifest stored locally at `<data_dir>/nebo/plugins/<slug>/<version>/plugin.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginManifest {
    /// NeboLoop artifact ID.
    pub id: String,
    /// URL-safe slug — matches skill's `plugins[].name`.
    pub slug: String,
    /// Human-readable display name.
    pub name: String,
    /// Semver version string.
    pub version: String,
    /// Brief description.
    #[serde(default)]
    pub description: String,
    /// Publisher name.
    #[serde(default)]
    pub author: String,
    /// Platform-specific binary entries keyed by platform key (e.g., "macos-arm64").
    pub platforms: HashMap<String, PlatformBinary>,
    /// ED25519 signing key ID used to sign binaries.
    #[serde(default)]
    pub signing_key_id: String,
    /// Custom env var name override. Defaults to `{SLUG}_BIN`.
    #[serde(default)]
    pub env_var: String,
    /// Optional authentication configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<PluginAuth>,
}

/// Authentication configuration for a plugin binary.
///
/// Plugins that require credentials (e.g., OAuth for Google Workspace) declare
/// their auth requirements here. Nebo runs the plugin's auth commands and injects
/// the publisher-provided env vars (client_id, client_secret, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginAuth {
    /// Auth type identifier (e.g., "oauth_cli").
    #[serde(rename = "type")]
    pub auth_type: String,
    /// Environment variables to inject before running auth commands.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// CLI subcommands (appended to plugin binary path).
    pub commands: PluginAuthCommands,
    /// Human-readable label for the auth button (e.g., "Google Account").
    #[serde(default)]
    pub label: String,
    /// Description shown to user during auth step.
    #[serde(default)]
    pub description: String,
}

/// CLI commands for plugin authentication lifecycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginAuthCommands {
    /// Subcommand to trigger authentication (e.g., "auth login").
    pub login: String,
    /// Subcommand to check auth status, must return JSON (e.g., "auth status").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Subcommand to clear credentials (e.g., "auth logout").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logout: Option<String>,
}

/// Binary artifact for a specific platform.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformBinary {
    /// Binary filename (e.g., "gws" or "gws.exe").
    pub binary_name: String,
    /// SHA256 hex hash of the binary.
    pub sha256: String,
    /// ED25519 signature (base64).
    pub signature: String,
    /// File size in bytes.
    pub size: u64,
    /// Download URL for the binary.
    pub download_url: String,
}

// ── PluginStore ─────────────────────────────────────────────────────

/// Manages downloaded plugin binaries.
///
/// Lives in the napp crate alongside Registry — shares `SigningKeyProvider` and
/// version resolution infrastructure. Scans two directories:
/// - `installed_dir` (`<data_dir>/nebo/plugins/`) — marketplace downloads
/// - `user_dir` (`<data_dir>/user/plugins/`) — user-provided, overrides installed
pub struct PluginStore {
    /// Marketplace plugin storage: `<data_dir>/nebo/plugins/`.
    installed_dir: PathBuf,
    /// User plugin storage: `<data_dir>/user/plugins/`.
    user_dir: PathBuf,
    /// ED25519 signing key provider for signature verification.
    signing_key: Option<Arc<SigningKeyProvider>>,
    /// Cached manifests keyed by `slug:version`.
    manifests: Arc<tokio::sync::RwLock<HashMap<String, PluginManifest>>>,
    /// Prevents concurrent downloads of the same plugin slug.
    downloading: Arc<tokio::sync::Mutex<HashSet<String>>>,
}

impl PluginStore {
    pub fn new(installed_dir: PathBuf, user_dir: PathBuf, signing_key: Option<Arc<SigningKeyProvider>>) -> Self {
        Self {
            installed_dir,
            user_dir,
            signing_key,
            manifests: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            downloading: Arc::new(tokio::sync::Mutex::new(HashSet::new())),
        }
    }

    /// Root directory for installed (marketplace) plugin storage.
    pub fn plugins_dir(&self) -> &Path {
        &self.installed_dir
    }

    /// Resolve a plugin binary path from local storage only. Non-async.
    ///
    /// Checks user directory first (override), then installed directory.
    /// Scans `<dir>/<slug>/` for version directories, matches the
    /// semver range, and returns the binary path if found.
    pub fn resolve(&self, slug: &str, version_range: &str) -> Option<PathBuf> {
        // User dir takes priority
        if let Some(path) = self.resolve_in_dir(&self.user_dir, slug, version_range) {
            return Some(path);
        }
        self.resolve_in_dir(&self.installed_dir, slug, version_range)
    }

    /// Resolve a plugin binary within a single root directory.
    fn resolve_in_dir(&self, root: &Path, slug: &str, version_range: &str) -> Option<PathBuf> {
        let slug_dir = root.join(slug);
        if !slug_dir.exists() {
            return None;
        }

        let req = if version_range.is_empty() || version_range == "*" {
            None
        } else {
            match semver::VersionReq::parse(version_range) {
                Ok(r) => Some(r),
                Err(_) => return None,
            }
        };

        let mut best: Option<(semver::Version, PathBuf)> = None;

        let entries = match std::fs::read_dir(&slug_dir) {
            Ok(e) => e,
            Err(_) => return None,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let dir_name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n,
                None => continue,
            };

            let version = match semver::Version::parse(dir_name) {
                Ok(v) => v,
                Err(_) => continue,
            };

            // Check version range
            if let Some(ref req) = req {
                if !req.matches(&version) {
                    continue;
                }
            }

            // Check for quarantine marker
            if path.join(".quarantined").exists() {
                continue;
            }

            // Check for binary — read manifest to find binary_name, or scan for executable
            let binary_path = self.find_binary_in_version_dir(&path);
            if binary_path.is_none() {
                continue;
            }

            match &best {
                Some((current_best, _)) if &version <= current_best => {}
                _ => {
                    best = Some((version, binary_path.unwrap()));
                }
            }
        }

        best.map(|(_, path)| path)
    }

    /// Ensure a plugin is installed, downloading from NeboLoop if missing.
    ///
    /// Deduplicates concurrent downloads via the `downloading` mutex.
    /// The `download_fn` callback queries NeboLoop for the manifest and binary bytes.
    pub async fn ensure<F, Fut>(
        &self,
        slug: &str,
        version_range: &str,
        download_fn: F,
    ) -> Result<PathBuf, NappError>
    where
        F: FnOnce(String, String) -> Fut,
        Fut: std::future::Future<Output = Result<(PluginManifest, Vec<u8>), NappError>>,
    {
        // Fast path: already installed locally
        if let Some(path) = self.resolve(slug, version_range) {
            return Ok(path);
        }

        // Dedup concurrent downloads for the same slug
        {
            let mut downloading = self.downloading.lock().await;
            if downloading.contains(slug) {
                // Another task is downloading this plugin — wait and retry
                drop(downloading);
                // Simple retry loop: wait briefly, then check local storage
                for _ in 0..30 {
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    if let Some(path) = self.resolve(slug, version_range) {
                        return Ok(path);
                    }
                }
                return Err(NappError::PluginDownloadFailed(format!(
                    "timed out waiting for concurrent download of plugin '{}'",
                    slug
                )));
            }
            downloading.insert(slug.to_string());
        }

        // Download and install
        let result = self
            .download_and_install(slug, version_range, download_fn)
            .await;

        // Release download lock
        {
            let mut downloading = self.downloading.lock().await;
            downloading.remove(slug);
        }

        result
    }

    /// Download, verify, and install a plugin binary.
    async fn download_and_install<F, Fut>(
        &self,
        slug: &str,
        _version_range: &str,
        download_fn: F,
    ) -> Result<PathBuf, NappError>
    where
        F: FnOnce(String, String) -> Fut,
        Fut: std::future::Future<Output = Result<(PluginManifest, Vec<u8>), NappError>>,
    {
        let platform = current_platform_key();

        let (manifest, binary_data) =
            download_fn(slug.to_string(), platform.clone()).await?;

        // Find the platform binary entry
        let platform_binary = manifest
            .platforms
            .get(&platform)
            .ok_or_else(|| NappError::PluginPlatformUnavailable {
                plugin: slug.to_string(),
                platform: platform.clone(),
            })?;

        // Verify SHA256 hash
        let mut hasher = Sha256::new();
        hasher.update(&binary_data);
        let actual_hash = hex::encode(hasher.finalize());
        if actual_hash != platform_binary.sha256 {
            return Err(NappError::PluginDownloadFailed(format!(
                "SHA256 mismatch for plugin '{}': expected {}, got {}",
                slug, platform_binary.sha256, actual_hash
            )));
        }

        // Verify ED25519 signature if signing key is available
        if let Some(ref signing_key) = self.signing_key {
            match signing_key.get_key().await {
                Ok(verifying_key) => {
                    use base64::Engine;
                    use ed25519_dalek::{Signature, Verifier};

                    let sig_bytes = base64::engine::general_purpose::STANDARD
                        .decode(&platform_binary.signature)
                        .map_err(|e| {
                            NappError::Signing(format!("decode plugin signature: {}", e))
                        })?;
                    let signature = Signature::from_slice(&sig_bytes).map_err(|e| {
                        NappError::Signing(format!("invalid plugin signature: {}", e))
                    })?;
                    verifying_key
                        .verify(&binary_data, &signature)
                        .map_err(|_| {
                            NappError::Signing(format!(
                                "plugin '{}' signature verification failed",
                                slug
                            ))
                        })?;
                    debug!(plugin = slug, "ED25519 signature verified");
                }
                Err(e) => {
                    warn!(plugin = slug, error = %e, "could not fetch signing key, skipping signature verification");
                }
            }
        }

        // Store binary on disk (always in installed_dir — marketplace downloads)
        let version_dir = self
            .installed_dir
            .join(slug)
            .join(&manifest.version);
        std::fs::create_dir_all(&version_dir)?;

        let binary_path = version_dir.join(&platform_binary.binary_name);
        std::fs::write(&binary_path, &binary_data)?;

        // Set executable permission on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&binary_path, std::fs::Permissions::from_mode(0o755))?;
        }

        // Write manifest for future reference
        let manifest_path = version_dir.join("plugin.json");
        let manifest_json = serde_json::to_string_pretty(&manifest)?;
        std::fs::write(&manifest_path, manifest_json)?;

        // Cache manifest in memory
        {
            let cache_key = format!("{}:{}", slug, manifest.version);
            let mut manifests = self.manifests.write().await;
            manifests.insert(cache_key, manifest.clone());
        }

        info!(
            plugin = slug,
            version = %manifest.version,
            platform = %platform,
            path = %binary_path.display(),
            size = binary_data.len(),
            "installed plugin binary"
        );

        Ok(binary_path)
    }

    /// Install a plugin from a .napp archive containing binary + plugin.json + PLUGIN.md + skills/.
    ///
    /// Stores the .napp archive at `<installed_dir>/<slug>/<version>.napp`, then
    /// extracts alongside (into `<version>/`) — same pattern as agent install.
    /// Reads plugin.json for metadata, verifies binary integrity (SHA256 + ED25519).
    pub async fn install_from_napp(&self, slug: &str, version: &str, napp_data: &[u8]) -> Result<PathBuf, NappError> {
        // Dedup concurrent installs
        {
            let mut downloading = self.downloading.lock().await;
            if downloading.contains(slug) {
                drop(downloading);
                for _ in 0..30 {
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    if let Some(path) = self.resolve(slug, "*") {
                        return Ok(path);
                    }
                }
                return Err(NappError::PluginDownloadFailed(format!(
                    "timed out waiting for concurrent install of plugin '{}'",
                    slug
                )));
            }
            downloading.insert(slug.to_string());
        }

        let result = self.install_from_napp_inner(slug, version, napp_data).await;

        // Release install lock
        {
            let mut downloading = self.downloading.lock().await;
            downloading.remove(slug);
        }

        result
    }

    /// Inner implementation of .napp-based plugin install.
    async fn install_from_napp_inner(&self, slug: &str, version: &str, napp_data: &[u8]) -> Result<PathBuf, NappError> {
        // Store .napp archive alongside version dir (same pattern as agent install)
        let plugin_dir = self.installed_dir.join(slug);
        std::fs::create_dir_all(&plugin_dir)?;
        let napp_path = plugin_dir.join(format!("{version}.napp"));
        std::fs::write(&napp_path, napp_data)?;
        info!(plugin = slug, path = %napp_path.display(), size = napp_data.len(), "stored sealed .napp");

        // Remove existing version dir so extract_napp_alongside re-extracts
        let version_dir = plugin_dir.join(version);
        if version_dir.exists() {
            std::fs::remove_dir_all(&version_dir)?;
        }

        // Extract alongside: <slug>/<version>.napp → <slug>/<version>/
        let extract_dir = crate::reader::extract_napp_alongside(&napp_path)?;
        info!(plugin = slug, dir = %extract_dir.display(), "extracted .napp");

        // Read plugin.json from extracted dir (plugin-specific metadata)
        let plugin_json_path = extract_dir.join("plugin.json");
        let plugin_manifest: Option<PluginManifest> = if plugin_json_path.exists() {
            let data = std::fs::read_to_string(&plugin_json_path)?;
            match serde_json::from_str(&data) {
                Ok(m) => Some(m),
                Err(e) => {
                    warn!(plugin = slug, error = %e, "failed to parse plugin.json");
                    None
                }
            }
        } else {
            None
        };

        // Find binary and make executable
        let binary_path = self
            .find_binary_in_version_dir(&extract_dir)
            .ok_or_else(|| NappError::Extraction("no binary found in extracted .napp".into()))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&binary_path, std::fs::Permissions::from_mode(0o755))?;
        }

        // Verify SHA256 + ED25519 if plugin.json exists with platform entry
        if let Some(ref pm) = plugin_manifest {
            let platform = current_platform_key();
            if let Some(pb) = pm.platforms.get(&platform) {
                let binary_data = std::fs::read(&binary_path)?;

                // SHA256
                let mut hasher = Sha256::new();
                hasher.update(&binary_data);
                let actual_hash = hex::encode(hasher.finalize());
                if actual_hash != pb.sha256 {
                    let _ = std::fs::remove_dir_all(&extract_dir);
                    let _ = std::fs::remove_file(&napp_path);
                    return Err(NappError::PluginDownloadFailed(format!(
                        "SHA256 mismatch for plugin '{}': expected {}, got {}",
                        slug, pb.sha256, actual_hash
                    )));
                }

                // ED25519
                if let Some(ref signing_key) = self.signing_key {
                    match signing_key.get_key().await {
                        Ok(verifying_key) => {
                            use base64::Engine;
                            use ed25519_dalek::{Signature, Verifier};

                            let sig_bytes = base64::engine::general_purpose::STANDARD
                                .decode(&pb.signature)
                                .map_err(|e| {
                                    NappError::Signing(format!("decode plugin signature: {}", e))
                                })?;
                            let signature = Signature::from_slice(&sig_bytes).map_err(|e| {
                                NappError::Signing(format!("invalid plugin signature: {}", e))
                            })?;
                            verifying_key.verify(&binary_data, &signature).map_err(|_| {
                                NappError::Signing(format!(
                                    "plugin '{}' signature verification failed",
                                    slug
                                ))
                            })?;
                            debug!(plugin = slug, "ED25519 signature verified (.napp)");
                        }
                        Err(e) => {
                            warn!(plugin = slug, error = %e, "could not fetch signing key, skipping signature verification");
                        }
                    }
                }
            }

            // Cache manifest in memory
            let cache_key = format!("{}:{}", slug, version);
            let mut manifests = self.manifests.write().await;
            manifests.insert(cache_key, pm.clone());
        }

        info!(
            plugin = slug,
            version = %version,
            path = %binary_path.display(),
            "installed plugin from .napp"
        );

        Ok(binary_path)
    }

    /// Verify binary integrity against cached manifest.
    pub fn verify_integrity(&self, slug: &str, version: &str) -> Result<(), NappError> {
        // Check user dir first, then installed dir
        let user_dir = self.user_dir.join(slug).join(version);
        let version_dir = if user_dir.exists() {
            user_dir
        } else {
            self.installed_dir.join(slug).join(version)
        };
        let manifest_path = version_dir.join("plugin.json");

        let manifest_data = std::fs::read_to_string(&manifest_path).map_err(|e| {
            NappError::PluginNotFound(format!("manifest for {}@{}: {}", slug, version, e))
        })?;
        let manifest: PluginManifest = serde_json::from_str(&manifest_data)?;

        let platform = current_platform_key();
        let platform_binary = manifest.platforms.get(&platform).ok_or_else(|| {
            NappError::PluginPlatformUnavailable {
                plugin: slug.to_string(),
                platform,
            }
        })?;

        let binary_path = version_dir.join(&platform_binary.binary_name);
        let binary_data = std::fs::read(&binary_path).map_err(|e| {
            NappError::PluginNotFound(format!("binary for {}@{}: {}", slug, version, e))
        })?;

        let mut hasher = Sha256::new();
        hasher.update(&binary_data);
        let actual_hash = hex::encode(hasher.finalize());

        if actual_hash != platform_binary.sha256 {
            return Err(NappError::Signing(format!(
                "integrity check failed for {}@{}: expected {}, got {}",
                slug, version, platform_binary.sha256, actual_hash
            )));
        }

        Ok(())
    }

    /// List all installed plugins as `(slug, version, binary_path, source)`.
    ///
    /// Scans both user and installed directories. User plugins override
    /// installed plugins when the same slug+version exists in both.
    pub fn list_installed(&self) -> Vec<(String, semver::Version, PathBuf, &'static str)> {
        let mut results = Vec::new();
        let mut seen = HashSet::new();

        // User dir first — takes priority
        self.collect_from_dir(&self.user_dir, "user", &mut results, &mut seen);
        // Installed dir — skips slug+version already seen in user
        self.collect_from_dir(&self.installed_dir, "installed", &mut results, &mut seen);

        results.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.cmp(&a.1)));
        results
    }

    /// Scan a single root directory for plugins.
    fn collect_from_dir(
        &self,
        root: &Path,
        source: &'static str,
        results: &mut Vec<(String, semver::Version, PathBuf, &'static str)>,
        seen: &mut HashSet<(String, String)>,
    ) {
        let entries = match std::fs::read_dir(root) {
            Ok(e) => e,
            Err(_) => return,
        };

        for entry in entries.flatten() {
            let slug_path = entry.path();
            if !slug_path.is_dir() {
                continue;
            }
            let slug = match slug_path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };

            let version_entries = match std::fs::read_dir(&slug_path) {
                Ok(e) => e,
                Err(_) => continue,
            };

            for ver_entry in version_entries.flatten() {
                let ver_path = ver_entry.path();
                if !ver_path.is_dir() {
                    continue;
                }
                if ver_path.join(".quarantined").exists() {
                    continue;
                }

                let ver_name = match ver_path.file_name().and_then(|n| n.to_str()) {
                    Some(n) => n,
                    None => continue,
                };
                let version = match semver::Version::parse(ver_name) {
                    Ok(v) => v,
                    Err(_) => continue,
                };

                let key = (slug.clone(), ver_name.to_string());
                if !seen.insert(key) {
                    continue; // Already seen from higher-priority dir
                }

                if let Some(binary_path) = self.find_binary_in_version_dir(&ver_path) {
                    results.push((slug.clone(), version, binary_path, source));
                }
            }
        }
    }

    /// Build env var pairs for all installed (non-quarantined) plugins.
    ///
    /// Returns `Vec<(env_name, binary_path)>` — e.g., `("GWS_BIN", "/path/to/gws")`.
    /// For plugins with multiple versions, picks the highest semver
    /// (`list_installed` sorts by slug asc, version desc — first per slug wins).
    pub fn build_env_map(&self) -> Vec<(String, String)> {
        let installed = self.list_installed();
        let mut seen = std::collections::HashSet::new();
        let mut result = Vec::new();
        for (slug, _version, binary_path, _source) in installed {
            if seen.insert(slug.clone()) {
                result.push((
                    plugin_env_var(&slug),
                    binary_path.to_string_lossy().into_owned(),
                ));
            }
        }
        result
    }

    /// Build a PATH string that prepends all installed plugin directories
    /// to the system PATH. This ensures plugin binaries can find themselves
    /// and sibling binaries when spawned as subprocesses.
    pub fn path_with_plugins(&self) -> String {
        let installed = self.list_installed();
        let mut dirs = std::collections::HashSet::new();
        let mut prefix_parts = Vec::new();
        for (_slug, _version, binary_path, _source) in &installed {
            if let Some(dir) = binary_path.parent() {
                if dirs.insert(dir.to_path_buf()) {
                    prefix_parts.push(dir.to_string_lossy().into_owned());
                }
            }
        }
        let system_path = std::env::var("PATH").unwrap_or_default();
        if prefix_parts.is_empty() {
            system_path
        } else {
            let sep = if cfg!(windows) { ";" } else { ":" };
            prefix_parts.push(system_path);
            prefix_parts.join(sep)
        }
    }

    /// Remove plugin versions not referenced by any of the given slugs.
    ///
    /// Takes a snapshot of referenced slugs to avoid lock coupling with skill loader.
    pub fn garbage_collect(&self, referenced_slugs: &HashSet<String>) -> Vec<String> {
        let mut removed = Vec::new();

        // GC both directories
        for root in [&self.installed_dir, &self.user_dir] {
            let entries = match std::fs::read_dir(root) {
                Ok(e) => e,
                Err(_) => continue,
            };

            for entry in entries.flatten() {
                let slug_path = entry.path();
                if !slug_path.is_dir() {
                    continue;
                }
                let slug = match slug_path.file_name().and_then(|n| n.to_str()) {
                    Some(n) => n.to_string(),
                    None => continue,
                };

                if !referenced_slugs.contains(&slug) {
                    if let Err(e) = std::fs::remove_dir_all(&slug_path) {
                        warn!(slug = %slug, error = %e, "failed to garbage collect plugin");
                    } else {
                        info!(slug = %slug, "garbage collected unreferenced plugin");
                        removed.push(slug);
                    }
                }
            }
        }

        removed
    }

    /// Remove a plugin entirely (all versions). Checks both user and installed dirs.
    pub fn remove(&self, slug: &str) -> Result<(), NappError> {
        let mut found = false;
        // Remove from user dir if present
        let user_slug_dir = self.user_dir.join(slug);
        if user_slug_dir.exists() {
            std::fs::remove_dir_all(&user_slug_dir)?;
            found = true;
        }
        // Remove from installed dir if present
        let installed_slug_dir = self.installed_dir.join(slug);
        if installed_slug_dir.exists() {
            std::fs::remove_dir_all(&installed_slug_dir)?;
            found = true;
        }
        if !found {
            return Err(NappError::PluginNotFound(slug.to_string()));
        }
        info!(slug = slug, "removed plugin");
        Ok(())
    }

    /// Quarantine a plugin version (delete binary, write `.quarantined` marker).
    pub fn quarantine(&self, slug: &str, version: &str, reason: &str) {
        // Find which dir contains this version
        let user_ver = self.user_dir.join(slug).join(version);
        let version_dir = if user_ver.exists() {
            user_ver
        } else {
            self.installed_dir.join(slug).join(version)
        };
        if !version_dir.exists() {
            return;
        }

        // Write quarantine marker
        let marker = version_dir.join(".quarantined");
        let _ = std::fs::write(&marker, reason);

        // Remove the binary (preserve manifest for investigation)
        if let Some(binary_path) = self.find_binary_in_version_dir(&version_dir) {
            let _ = std::fs::remove_file(&binary_path);
        }

        warn!(
            plugin = slug,
            version = version,
            reason = reason,
            "quarantined plugin"
        );
    }

    /// Read the manifest for a plugin's latest installed version.
    /// Checks user dir first (override), then installed dir.
    pub fn get_manifest(&self, slug: &str) -> Option<PluginManifest> {
        // User dir takes priority
        if let Some(m) = self.get_manifest_from_dir(&self.user_dir, slug) {
            return Some(m);
        }
        self.get_manifest_from_dir(&self.installed_dir, slug)
    }

    /// Read manifest from a single root directory.
    fn get_manifest_from_dir(&self, root: &Path, slug: &str) -> Option<PluginManifest> {
        let slug_dir = root.join(slug);
        if !slug_dir.exists() {
            return None;
        }

        // Find the latest version directory
        let mut best: Option<(semver::Version, PathBuf)> = None;
        let entries = std::fs::read_dir(&slug_dir).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() || path.join(".quarantined").exists() {
                continue;
            }
            let ver_name = path.file_name()?.to_str()?;
            let version = semver::Version::parse(ver_name).ok()?;
            match &best {
                Some((current, _)) if &version <= current => {}
                _ => best = Some((version, path)),
            }
        }

        let (_, version_dir) = best?;
        let manifest_path = version_dir.join("plugin.json");
        let data = std::fs::read_to_string(&manifest_path).ok()?;
        serde_json::from_str(&data).ok()
    }

    /// Get binary path and auth config for a plugin, if auth is declared.
    pub fn get_auth_info(&self, slug: &str) -> Option<(PathBuf, PluginAuth)> {
        let binary_path = self.resolve(slug, "*")?;
        let manifest = self.get_manifest(slug)?;
        let auth = manifest.auth?;
        Some((binary_path, auth))
    }

    /// Find a binary in a version directory by reading plugin.json or scanning for executables.
    fn find_binary_in_version_dir(&self, version_dir: &Path) -> Option<PathBuf> {
        // Try plugin.json first
        let manifest_path = version_dir.join("plugin.json");
        if manifest_path.exists() {
            if let Ok(data) = std::fs::read_to_string(&manifest_path) {
                if let Ok(manifest) = serde_json::from_str::<PluginManifest>(&data) {
                    let platform = current_platform_key();
                    if let Some(pb) = manifest.platforms.get(&platform) {
                        let binary_path = version_dir.join(&pb.binary_name);
                        if binary_path.is_file() {
                            return Some(binary_path);
                        }
                    }
                }
            }
        }

        // Fallback: find first executable file
        let entries = std::fs::read_dir(version_dir).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            // Skip metadata files
            let name = path.file_name()?.to_str()?;
            if name == "plugin.json" || name.starts_with('.') {
                continue;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = path.metadata() {
                    if meta.permissions().mode() & 0o111 != 0 {
                        return Some(path);
                    }
                }
            }
            #[cfg(not(unix))]
            {
                // On Windows, check for common executable extensions
                if name.ends_with(".exe") || name.ends_with(".bat") || name.ends_with(".cmd") {
                    return Some(path);
                }
            }
        }

        None
    }
}

// ── Helpers ─────────────────────────────────────────────────────────

/// Detect the current platform key matching NeboLoop conventions.
///
/// Returns e.g., "darwin-arm64", "linux-amd64", "windows-amd64".
pub fn current_platform_key() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "amd64",
        other => other,
    };
    format!("{}-{}", os, arch)
}

/// Derive the environment variable name for a plugin binary path.
///
/// `gws` → `GWS_BIN`, `my-tool` → `MY_TOOL_BIN`.
pub fn plugin_env_var(slug: &str) -> String {
    format!("{}_BIN", slug.to_uppercase().replace('-', "_"))
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_current_platform_key() {
        let key = current_platform_key();
        // Should be non-empty and contain a dash
        assert!(key.contains('-'), "platform key should be os-arch: {}", key);
    }

    #[test]
    fn test_plugin_env_var() {
        assert_eq!(plugin_env_var("gws"), "GWS_BIN");
        assert_eq!(plugin_env_var("my-tool"), "MY_TOOL_BIN");
        assert_eq!(plugin_env_var("ffmpeg"), "FFMPEG_BIN");
    }

    #[test]
    fn test_resolve_empty_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        let user_dir = tmp.path().join("user");
        std::fs::create_dir_all(&user_dir).unwrap();
        let store = PluginStore::new(tmp.path().to_path_buf(), user_dir, None);
        assert!(store.resolve("nonexistent", "*").is_none());
    }

    #[test]
    fn test_resolve_with_installed_plugin() {
        let tmp = tempfile::TempDir::new().unwrap();
        let plugins_dir = tmp.path();

        // Create a plugin version directory with a binary
        let version_dir = plugins_dir.join("gws").join("1.2.0");
        std::fs::create_dir_all(&version_dir).unwrap();
        let binary_path = version_dir.join("gws");
        std::fs::write(&binary_path, b"fake binary").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&binary_path, std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }

        let user_plugins_dir = tmp.path().join("user_plugins");
        std::fs::create_dir_all(&user_plugins_dir).unwrap();
        let store = PluginStore::new(plugins_dir.to_path_buf(), user_plugins_dir, None);
        let result = store.resolve("gws", "*");
        assert!(result.is_some());
        assert!(result.unwrap().ends_with("gws"));
    }

    #[test]
    fn test_resolve_semver_range() {
        let tmp = tempfile::TempDir::new().unwrap();
        let plugins_dir = tmp.path();

        // Create multiple versions
        for version in &["1.0.0", "1.2.0", "2.0.0"] {
            let version_dir = plugins_dir.join("gws").join(version);
            std::fs::create_dir_all(&version_dir).unwrap();
            let binary_path = version_dir.join("gws");
            std::fs::write(&binary_path, b"fake binary").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&binary_path, std::fs::Permissions::from_mode(0o755))
                    .unwrap();
            }
        }

        let user_plugins_dir = tmp.path().join("user_plugins");
        std::fs::create_dir_all(&user_plugins_dir).unwrap();
        let store = PluginStore::new(plugins_dir.to_path_buf(), user_plugins_dir, None);

        // ^1.0.0 should resolve to 1.2.0 (not 2.0.0)
        let result = store.resolve("gws", "^1.0.0");
        assert!(result.is_some());
        let path = result.unwrap();
        assert!(
            path.to_string_lossy().contains("1.2.0"),
            "expected 1.2.0 but got {}",
            path.display()
        );

        // >=2.0.0 should resolve to 2.0.0
        let result = store.resolve("gws", ">=2.0.0");
        assert!(result.is_some());
        assert!(result.unwrap().to_string_lossy().contains("2.0.0"));
    }

    #[test]
    fn test_resolve_skips_quarantined() {
        let tmp = tempfile::TempDir::new().unwrap();
        let plugins_dir = tmp.path();

        let version_dir = plugins_dir.join("gws").join("1.0.0");
        std::fs::create_dir_all(&version_dir).unwrap();
        let binary_path = version_dir.join("gws");
        std::fs::write(&binary_path, b"fake binary").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&binary_path, std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }

        // Quarantine it
        std::fs::write(version_dir.join(".quarantined"), "test reason").unwrap();

        let user_plugins_dir = tmp.path().join("user_plugins");
        std::fs::create_dir_all(&user_plugins_dir).unwrap();
        let store = PluginStore::new(plugins_dir.to_path_buf(), user_plugins_dir, None);
        assert!(store.resolve("gws", "*").is_none());
    }

    #[test]
    fn test_list_installed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let plugins_dir = tmp.path();

        for (slug, version) in &[("gws", "1.0.0"), ("gws", "1.2.0"), ("ffmpeg", "5.0.0")] {
            let version_dir = plugins_dir.join(slug).join(version);
            std::fs::create_dir_all(&version_dir).unwrap();
            let binary_path = version_dir.join(slug);
            std::fs::write(&binary_path, b"fake binary").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&binary_path, std::fs::Permissions::from_mode(0o755))
                    .unwrap();
            }
        }

        let user_plugins_dir = tmp.path().join("user_plugins");
        std::fs::create_dir_all(&user_plugins_dir).unwrap();
        let store = PluginStore::new(plugins_dir.to_path_buf(), user_plugins_dir, None);
        let installed = store.list_installed();
        assert_eq!(installed.len(), 3);
    }

    #[test]
    fn test_garbage_collect() {
        let tmp = tempfile::TempDir::new().unwrap();
        let plugins_dir = tmp.path();

        for slug in &["gws", "ffmpeg", "orphan"] {
            let version_dir = plugins_dir.join(slug).join("1.0.0");
            std::fs::create_dir_all(&version_dir).unwrap();
            std::fs::write(version_dir.join(slug), b"fake").unwrap();
        }

        // Use a separate temp dir for user_plugins to avoid GC seeing it as a slug
        let user_tmp = tempfile::TempDir::new().unwrap();
        let user_plugins_dir = user_tmp.path().to_path_buf();
        let store = PluginStore::new(plugins_dir.to_path_buf(), user_plugins_dir, None);
        let referenced: HashSet<String> = ["gws", "ffmpeg"].iter().map(|s| s.to_string()).collect();
        let removed = store.garbage_collect(&referenced);
        assert_eq!(removed, vec!["orphan"]);
        assert!(!plugins_dir.join("orphan").exists());
        assert!(plugins_dir.join("gws").exists());
    }

    #[test]
    fn test_quarantine() {
        let tmp = tempfile::TempDir::new().unwrap();
        let plugins_dir = tmp.path();

        let version_dir = plugins_dir.join("bad-plugin").join("1.0.0");
        std::fs::create_dir_all(&version_dir).unwrap();
        let binary_path = version_dir.join("bad-plugin");
        std::fs::write(&binary_path, b"malicious").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&binary_path, std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }

        let user_plugins_dir = tmp.path().join("user_plugins");
        std::fs::create_dir_all(&user_plugins_dir).unwrap();
        let store = PluginStore::new(plugins_dir.to_path_buf(), user_plugins_dir, None);
        store.quarantine("bad-plugin", "1.0.0", "revoked by NeboLoop");

        // Binary should be removed, marker should exist
        assert!(!binary_path.exists());
        assert!(version_dir.join(".quarantined").exists());

        // resolve should skip quarantined
        assert!(store.resolve("bad-plugin", "*").is_none());
    }

    #[test]
    fn test_verify_integrity_missing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let user_dir = tmp.path().join("user");
        std::fs::create_dir_all(&user_dir).unwrap();
        let store = PluginStore::new(tmp.path().to_path_buf(), user_dir, None);
        assert!(store.verify_integrity("nonexistent", "1.0.0").is_err());
    }

    #[test]
    fn test_manifest_serde() {
        let manifest = PluginManifest {
            id: "uuid-1234".into(),
            slug: "gws".into(),
            name: "Google Workspace CLI".into(),
            version: "1.2.0".into(),
            description: "CLI for Google Workspace".into(),
            author: "neboloop".into(),
            platforms: {
                let mut m = HashMap::new();
                m.insert(
                    "darwin-arm64".into(),
                    PlatformBinary {
                        binary_name: "gws".into(),
                        sha256: "abc123".into(),
                        signature: "sig==".into(),
                        size: 1024,
                        download_url: "https://cdn.neboloop.com/plugins/gws/1.2.0/darwin-arm64/gws".into(),
                    },
                );
                m
            },
            signing_key_id: "key-1".into(),
            env_var: String::new(),
            auth: None,
        };

        let json = serde_json::to_string(&manifest).unwrap();
        let parsed: PluginManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.slug, "gws");
        assert_eq!(parsed.version, "1.2.0");
        assert!(parsed.platforms.contains_key("darwin-arm64"));
        assert!(parsed.auth.is_none());
    }

    #[test]
    fn test_manifest_serde_with_auth() {
        let json = r#"{
            "id": "uuid-1234",
            "slug": "gws",
            "name": "gws",
            "version": "0.22.3",
            "platforms": {},
            "auth": {
                "type": "oauth_cli",
                "env": {
                    "GOOGLE_WORKSPACE_CLI_CLIENT_ID": "123.apps.googleusercontent.com",
                    "GOOGLE_WORKSPACE_CLI_CLIENT_SECRET": "GOCSPX-test"
                },
                "commands": {
                    "login": "auth login",
                    "status": "auth status",
                    "logout": "auth logout"
                },
                "label": "Google Account",
                "description": "Sign in to access Gmail, Drive, and more."
            }
        }"#;

        let parsed: PluginManifest = serde_json::from_str(json).unwrap();
        let auth = parsed.auth.unwrap();
        assert_eq!(auth.auth_type, "oauth_cli");
        assert_eq!(auth.commands.login, "auth login");
        assert_eq!(auth.commands.status.as_deref(), Some("auth status"));
        assert_eq!(auth.env.len(), 2);
        assert_eq!(auth.label, "Google Account");
    }

    #[test]
    fn test_manifest_without_auth_field() {
        // Existing manifests without auth field should deserialize fine
        let json = r#"{
            "id": "uuid-1234",
            "slug": "ffmpeg",
            "name": "ffmpeg",
            "version": "5.0.0",
            "platforms": {}
        }"#;

        let parsed: PluginManifest = serde_json::from_str(json).unwrap();
        assert!(parsed.auth.is_none());
    }
}
