//! Agent filesystem loader.
//!
//! Loads agent definitions from:
//! - Embedded bundled agents (compiled into binary, lowest priority)
//! - `nebo/agents/` — sealed .napp archives (marketplace)
//! - `user/agents/` — loose files (user-created, highest priority)
//!
//! This is a content scanner only. The DB remains the single source of truth
//! for agent state (enabled, input_values, etc.). The loader reads filesystem
//! content so it can be synced INTO the DB at startup and on hot-reload.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::NappError;
use crate::agent::{AgentConfig, AgentDef, parse_agent, parse_agent_config};

/// Where an agent was loaded from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentSource {
    /// Installed from NeboLoop marketplace (sealed .napp archive).
    Installed,
    /// User-created (loose files in user/ directory).
    User,
}

/// An agent loaded from the filesystem.
#[derive(Debug, Clone)]
pub struct LoadedAgent {
    /// Agent persona (from AGENT.md).
    pub agent_def: AgentDef,
    /// Agent operational config (from agent.json).
    pub config: Option<AgentConfig>,
    /// Where this agent was loaded from.
    pub source: AgentSource,
    /// Path to .napp archive (for installed agents).
    pub napp_path: Option<PathBuf>,
    /// Path to the directory or file this was loaded from.
    pub source_path: PathBuf,
    /// Version from manifest.json (if present).
    pub version: Option<String>,
    /// Raw AGENT.md content (for DB sync).
    pub agent_md: String,
    /// Raw frontmatter JSON string from agent.json (for DB sync).
    pub frontmatter: String,
    /// Description from agent_def (for DB sync).
    pub description: String,
    /// NeboLoop artifact UUID from manifest.json (marketplace agents only).
    pub id: Option<String>,
    /// Deterministic views from views.json (rendered without LLM involvement).
    pub views: Option<serde_json::Value>,
    /// Theme CSS from theme.css (for A2UI workspace styling).
    pub theme_css: Option<String>,
    /// True if this agent is an app (artifact_type == "app").
    pub is_app: bool,
    /// Path to static UI directory (extracted from .napp "ui/" prefix).
    pub app_ui_path: Option<PathBuf>,
    /// Path to compiled sidecar binary.
    pub app_binary_path: Option<PathBuf>,
    /// Window config from manifest.json.
    pub app_window_config: Option<crate::manifest::AppWindowConfig>,
}

/// Events emitted by the filesystem watcher when agent content changes on disk.
#[derive(Debug, Clone)]
pub enum AgentFsEvent {
    /// A new agent appeared on the filesystem (not in previous scan).
    Added(LoadedAgent),
    /// An existing agent's content changed (agent_md or frontmatter differ).
    Changed(LoadedAgent),
    /// An agent was removed from the filesystem (was in previous scan, not in new).
    Removed {
        /// Lowercase name key from the cache HashMap.
        name_key: String,
        /// The agent that was removed (carries id/name for DB lookup).
        agent: LoadedAgent,
    },
}

/// Manages loading, caching, and hot-reloading of agents.
///
/// Content-only scanner — no CRUD, no state management. The DB owns all
/// mutable agent state (enabled, input_values, etc.).
///
/// Three-tier loading: embedded bundled (lowest) → installed (marketplace) → user (highest).
pub struct AgentLoader {
    /// Embedded bundled agents: `(name, AGENT.md, agent.json, manifest.json)`.
    /// Compiled into the binary — no filesystem directory needed.
    bundled: &'static [(&'static str, &'static str, &'static str, &'static str)],
    installed_dir: PathBuf,
    user_dir: PathBuf,
    agents: Arc<RwLock<HashMap<String, LoadedAgent>>>,
}

impl AgentLoader {
    pub fn new(installed_dir: PathBuf, user_dir: PathBuf) -> Self {
        Self {
            bundled: &[],
            installed_dir,
            user_dir,
            agents: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Set the embedded bundled agents (compiled into the binary, lowest priority).
    pub fn with_bundled(mut self, bundled: &'static [(&'static str, &'static str, &'static str, &'static str)]) -> Self {
        self.bundled = bundled;
        self
    }

    /// Load all agents from embedded bundled, installed, and user directories.
    /// Loading order: embedded → installed → user (user overrides by name).
    pub async fn load_all(&self) -> usize {
        let mut loaded = HashMap::new();

        // 0. Load embedded bundled agents (lowest priority — compiled into binary)
        for (name, agent_md, agent_json, manifest_json) in self.bundled {
            match load_from_embedded(name, agent_md, agent_json, manifest_json) {
                Ok(agent) => {
                    loaded.insert(agent.agent_def.name.to_lowercase(), agent);
                }
                Err(e) => {
                    warn!(agent = name, error = %e, "failed to load bundled agent");
                }
            }
        }

        // 1. Load installed agents from extracted .napp directories
        for agent in scan_installed_agents(&self.installed_dir) {
            loaded.insert(agent.agent_def.name.to_lowercase(), agent);
        }

        // 2. Load user agents (override installed by name)
        for agent in scan_user_agents(&self.user_dir) {
            loaded.insert(agent.agent_def.name.to_lowercase(), agent);
        }

        let count = loaded.len();
        *self.agents.write().await = loaded;
        info!(count, installed_dir = %self.installed_dir.display(), user_dir = %self.user_dir.display(), "loaded agents from filesystem");
        count
    }

    /// Get an agent by name (case-insensitive).
    pub async fn get_by_name(&self, name: &str) -> Option<LoadedAgent> {
        self.agents.read().await.get(&name.to_lowercase()).cloned()
    }

    /// List all loaded agents.
    pub async fn list(&self) -> Vec<LoadedAgent> {
        self.agents.read().await.values().cloned().collect()
    }

    /// Get the user agents directory path.
    pub fn user_dir(&self) -> &Path {
        &self.user_dir
    }

    /// Get the installed agents directory path.
    pub fn installed_dir(&self) -> &Path {
        &self.installed_dir
    }

    /// Start watching for filesystem changes, reload on modification, and emit
    /// diff events through the returned channel for DB/registry/WS sync.
    pub fn watch(&self) -> (tokio::task::JoinHandle<()>, tokio::sync::mpsc::Receiver<AgentFsEvent>) {
        let installed_dir = self.installed_dir.clone();
        let user_dir = self.user_dir.clone();
        let agents = self.agents.clone();
        let (event_tx, event_rx) = tokio::sync::mpsc::channel::<AgentFsEvent>(32);

        let handle = tokio::spawn(async move {
            use notify::{Event, EventKind, RecursiveMode, Watcher};
            use tokio::sync::mpsc;

            let (tx, mut rx) = mpsc::channel::<notify::Result<Event>>(32);

            let mut watcher = match notify::RecommendedWatcher::new(
                move |res| {
                    let _ = tx.blocking_send(res);
                },
                notify::Config::default()
                    .with_poll_interval(std::time::Duration::from_secs(2)),
            ) {
                Ok(w) => w,
                Err(e) => {
                    warn!(error = %e, "failed to create filesystem watcher for agents");
                    return;
                }
            };

            if user_dir.exists() {
                if let Err(e) = watcher.watch(&user_dir, RecursiveMode::Recursive) {
                    warn!(error = %e, dir = %user_dir.display(), "failed to watch user agents dir");
                }
            }

            if installed_dir.exists() {
                if let Err(e) = watcher.watch(&installed_dir, RecursiveMode::Recursive) {
                    warn!(error = %e, dir = %installed_dir.display(), "failed to watch installed agents dir");
                }
            }

            let mut last_reload = std::time::Instant::now();
            let debounce = std::time::Duration::from_secs(1);

            while let Some(result) = rx.recv().await {
                match result {
                    Ok(event) => {
                        let dominated = matches!(
                            event.kind,
                            EventKind::Create(_)
                                | EventKind::Modify(_)
                                | EventKind::Remove(_)
                        );
                        if !dominated {
                            continue;
                        }

                        let relevant = event.paths.iter().any(|p| {
                            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                            name.eq_ignore_ascii_case("agent.md")
                                || name == "agent.json"
                                || name == "manifest.json"
                                || name == "views.json"
                                || name == "theme.css"
                                || name.ends_with(".napp")
                        });
                        if !relevant {
                            continue;
                        }

                        if last_reload.elapsed() < debounce {
                            continue;
                        }
                        last_reload = std::time::Instant::now();

                        debug!("agents directory changed, reloading");
                        let mut loaded = HashMap::new();

                        for agent in scan_installed_agents(&installed_dir) {
                            loaded.insert(agent.agent_def.name.to_lowercase(), agent);
                        }
                        for agent in scan_user_agents(&user_dir) {
                            loaded.insert(agent.agent_def.name.to_lowercase(), agent);
                        }

                        // Diff old cache vs new scan and emit events
                        {
                            let old = agents.read().await;
                            for (key, new_agent) in &loaded {
                                match old.get(key) {
                                    None => {
                                        let _ = event_tx.send(AgentFsEvent::Added(new_agent.clone())).await;
                                    }
                                    Some(old_agent) => {
                                        if old_agent.agent_md != new_agent.agent_md
                                            || old_agent.frontmatter != new_agent.frontmatter
                                            || old_agent.theme_css != new_agent.theme_css
                                        {
                                            let _ = event_tx.send(AgentFsEvent::Changed(new_agent.clone())).await;
                                        }
                                    }
                                }
                            }
                            for (key, old_agent) in old.iter() {
                                if !loaded.contains_key(key) {
                                    let _ = event_tx.send(AgentFsEvent::Removed {
                                        name_key: key.clone(),
                                        agent: old_agent.clone(),
                                    }).await;
                                }
                            }
                        }

                        let count = loaded.len();
                        *agents.write().await = loaded;
                        info!(count, "reloaded agents after filesystem change");
                    }
                    Err(e) => {
                        warn!(error = %e, "filesystem watch error (agents)");
                    }
                }
            }
        });

        (handle, event_rx)
    }
}

/// Load an agent from embedded (compiled-in) content strings.
fn load_from_embedded(
    name: &str,
    agent_md_raw: &str,
    agent_json_raw: &str,
    manifest_json_raw: &str,
) -> Result<LoadedAgent, NappError> {
    let mut agent_def = parse_agent(agent_md_raw)?;
    if agent_def.name.is_empty() {
        agent_def.name = name.to_string();
    }

    let (config, frontmatter) = if agent_json_raw.is_empty() {
        (None, String::new())
    } else {
        let cfg = parse_agent_config(agent_json_raw)?;
        (Some(cfg), agent_json_raw.to_string())
    };

    let (version, id, manifest_name) = if manifest_json_raw.is_empty() {
        (None, None, None)
    } else {
        match serde_json::from_str::<serde_json::Value>(manifest_json_raw) {
            Ok(v) => (
                v["version"].as_str().map(String::from),
                v["id"].as_str().map(String::from),
                v["name"].as_str().map(String::from),
            ),
            Err(_) => (None, None, None),
        }
    };

    // Prefer manifest.json display name, but skip package-style identifiers
    if let Some(ref mname) = manifest_name {
        if !mname.is_empty() && !mname.contains('@') && !mname.contains('/') {
            agent_def.name = mname.clone();
        }
    }

    let description = agent_def.description.clone();

    Ok(LoadedAgent {
        agent_def,
        config,
        source: AgentSource::Installed,
        napp_path: None,
        source_path: PathBuf::new(),
        version,
        agent_md: agent_md_raw.to_string(),
        frontmatter,
        description,
        id,
        views: None,
        theme_css: None,
        is_app: false,
        app_ui_path: None,
        app_binary_path: None,
        app_window_config: None,
    })
}

/// Load an agent from a directory (loose files or extracted .napp).
pub fn load_from_dir(dir: &Path, source: AgentSource) -> Result<LoadedAgent, NappError> {
    let agent_md_path = dir.join("AGENT.md");
    if !agent_md_path.exists() {
        return Err(NappError::NotFound(format!(
            "AGENT.md not found in {}",
            dir.display()
        )));
    }

    let agent_md_raw = std::fs::read_to_string(&agent_md_path)
        .map_err(NappError::Io)?;
    let mut agent_def = parse_agent(&agent_md_raw)?;

    // Fall back to directory name when AGENT.md has no frontmatter name
    if agent_def.name.is_empty() {
        if let Some(name) = dir.file_name().and_then(|n| n.to_str()) {
            agent_def.name = name.to_string();
        }
    }

    let (config, frontmatter) = {
        let config_path = dir.join("agent.json");
        if config_path.exists() {
            let json = std::fs::read_to_string(&config_path)
                .map_err(NappError::Io)?;
            let cfg = parse_agent_config(&json)?;
            (Some(cfg), json)
        } else {
            (None, String::new())
        }
    };

    // Read version, id, display name, and app config from manifest.json if available
    let (version, id, manifest_name, artifact_type, window_config) = {
        let manifest_path = dir.join("manifest.json");
        if manifest_path.exists() {
            let raw = std::fs::read_to_string(&manifest_path).ok();
            let parsed = raw.as_ref()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok());
            let manifest_full = raw.as_ref()
                .and_then(|s| serde_json::from_str::<crate::manifest::Manifest>(s).ok());
            match parsed {
                Some(v) => (
                    v["version"].as_str().map(String::from),
                    v["id"].as_str().map(String::from),
                    v["name"].as_str().map(String::from),
                    v["type"].as_str().map(String::from),
                    manifest_full.and_then(|m| m.window),
                ),
                None => (None, None, None, None, None),
            }
        } else {
            (None, None, None, None, None)
        }
    };

    // Prefer manifest.json display name over AGENT.md frontmatter slug,
    // but skip package-style identifiers (contain @ or /)
    if let Some(ref name) = manifest_name {
        if !name.is_empty() && !name.contains('@') && !name.contains('/') {
            agent_def.name = name.clone();
        }
    }

    let description = agent_def.description.clone();

    // Read views.json if present (deterministic UI declarations)
    let views = {
        let views_path = dir.join("views.json");
        if views_path.exists() {
            std::fs::read_to_string(&views_path)
                .ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        } else {
            None
        }
    };

    // Read theme.css if present (A2UI workspace styling)
    let theme_css = {
        let theme_path = dir.join("theme.css");
        if theme_path.exists() {
            std::fs::read_to_string(&theme_path).ok()
        } else {
            None
        }
    };

    // Detect app-type agents
    let is_app = artifact_type.as_deref() == Some("app");
    let app_ui_path = if is_app {
        let ui_dir = dir.join("ui");
        if ui_dir.is_dir() { Some(ui_dir) } else { None }
    } else {
        None
    };
    let app_binary_path = if is_app {
        // Look for sidecar binary in bin/ directory
        let bin_dir = dir.join("bin");
        if bin_dir.is_dir() {
            std::fs::read_dir(&bin_dir).ok()
                .and_then(|mut entries| entries.find_map(|e| {
                    let p = e.ok()?.path();
                    if p.is_file() { Some(p) } else { None }
                }))
        } else {
            // Also check for "binary" or "app" directly
            let binary = dir.join("binary");
            if binary.exists() {
                Some(binary)
            } else {
                let app_bin = dir.join("app");
                if app_bin.exists() { Some(app_bin) } else { None }
            }
        }
    } else {
        None
    };

    Ok(LoadedAgent {
        agent_def,
        config,
        source,
        napp_path: None,
        source_path: dir.to_path_buf(),
        version,
        agent_md: agent_md_raw,
        frontmatter,
        description,
        id,
        views,
        theme_css,
        is_app,
        app_ui_path,
        app_binary_path,
        app_window_config: window_config,
    })
}

/// Scan installed (nebo/) agents directory for extracted agent directories
/// and sealed .napp archives.
pub fn scan_installed_agents(dir: &Path) -> Vec<LoadedAgent> {
    let mut agents = Vec::new();
    if !dir.exists() {
        return agents;
    }
    // Load from extracted directories (free content)
    crate::reader::walk_for_marker(dir, "AGENT.md", &mut |agent_dir| {
        match load_from_dir(agent_dir, AgentSource::Installed) {
            Ok(agent) => agents.push(agent),
            Err(e) => {
                debug!(path = %agent_dir.display(), error = %e, "skipping directory (not an agent)");
            }
        }
    });
    agents
}

/// Scan for sealed .napp agent archives and load their content in memory.
///
/// Finds .napp files without a sibling AGENT.md directory (sealed content).
/// Reads AGENT.md, agent.json, manifest.json, views.json, and theme.css from
/// the encrypted archive using the provided license key. Plaintext never touches disk.
pub fn scan_sealed_agents(
    dir: &Path,
    license_keys: &std::collections::HashMap<String, [u8; 32]>,
) -> Vec<LoadedAgent> {
    let mut agents = Vec::new();
    if !dir.exists() {
        return agents;
    }
    scan_sealed_agents_recursive(dir, license_keys, &mut agents);
    agents
}

fn scan_sealed_agents_recursive(
    dir: &Path,
    license_keys: &std::collections::HashMap<String, [u8; 32]>,
    out: &mut Vec<LoadedAgent>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_sealed_agents_recursive(&path, license_keys, out);
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("napp") {
            continue;
        }

        // Skip if sibling extracted directory has AGENT.md (free content)
        let sibling = path.with_extension("");
        if sibling.is_dir() && sibling.join("AGENT.md").exists() {
            continue;
        }

        // Read artifact_id from partially-extracted manifest.json
        let artifact_id = if sibling.is_dir() {
            let manifest = sibling.join("manifest.json");
            if manifest.exists() {
                std::fs::read_to_string(&manifest)
                    .ok()
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                    .and_then(|v| v["id"].as_str().map(String::from))
            } else {
                None
            }
        } else {
            None
        };

        let artifact_id = match artifact_id {
            Some(id) => id,
            None => continue,
        };

        let license_key = match license_keys.get(&artifact_id) {
            Some(k) => k,
            None => {
                debug!(path = %path.display(), artifact_id, "sealed agent: no license key, skipping");
                continue;
            }
        };

        match load_from_sealed_napp(&path, license_key) {
            Ok(agent) => out.push(agent),
            Err(e) => {
                warn!(path = %path.display(), error = %e, "failed to load sealed agent");
            }
        }
    }
}

/// Load an agent from a sealed .napp archive (decrypted in memory only).
fn load_from_sealed_napp(
    napp_path: &Path,
    license_key: &[u8; 32],
) -> Result<LoadedAgent, NappError> {
    let agent_md_raw = crate::reader::read_sealed_napp_entry_string(napp_path, "AGENT.md", license_key)?;
    let mut agent_def = parse_agent(&agent_md_raw)?;

    if agent_def.name.is_empty() {
        if let Some(stem) = napp_path.file_stem().and_then(|n| n.to_str()) {
            agent_def.name = stem.to_string();
        }
    }

    let (config, frontmatter) = match crate::reader::read_sealed_napp_entry_string(napp_path, "agent.json", license_key) {
        Ok(json) => {
            let cfg = parse_agent_config(&json)?;
            (Some(cfg), json)
        }
        Err(NappError::NotFound(_)) => (None, String::new()),
        Err(e) => return Err(e),
    };

    let (version, id, manifest_name) = match crate::reader::read_sealed_napp_entry_string(napp_path, "manifest.json", license_key) {
        Ok(s) => match serde_json::from_str::<serde_json::Value>(&s) {
            Ok(v) => (
                v["version"].as_str().map(String::from),
                v["id"].as_str().map(String::from),
                v["name"].as_str().map(String::from),
            ),
            Err(_) => (None, None, None),
        },
        // manifest.json may have been partially extracted — try sibling dir
        Err(NappError::NotFound(_)) => {
            let sibling = napp_path.with_extension("");
            if sibling.is_dir() {
                let manifest = sibling.join("manifest.json");
                if manifest.exists() {
                    std::fs::read_to_string(&manifest)
                        .ok()
                        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                        .map(|v| (
                            v["version"].as_str().map(String::from),
                            v["id"].as_str().map(String::from),
                            v["name"].as_str().map(String::from),
                        ))
                        .unwrap_or((None, None, None))
                } else {
                    (None, None, None)
                }
            } else {
                (None, None, None)
            }
        }
        Err(_) => (None, None, None),
    };

    if let Some(ref mname) = manifest_name {
        if !mname.is_empty() && !mname.contains('@') && !mname.contains('/') {
            agent_def.name = mname.clone();
        }
    }

    let description = agent_def.description.clone();

    // Read optional views.json and theme.css from sealed archive
    let views = crate::reader::read_sealed_napp_entry_string(napp_path, "views.json", license_key)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());

    let theme_css = crate::reader::read_sealed_napp_entry_string(napp_path, "theme.css", license_key).ok();

    Ok(LoadedAgent {
        agent_def,
        config,
        source: AgentSource::Installed,
        napp_path: Some(napp_path.to_path_buf()),
        source_path: napp_path.to_path_buf(),
        version,
        agent_md: agent_md_raw,
        frontmatter,
        description,
        id,
        views,
        theme_css,
        is_app: false,
        app_ui_path: None,
        app_binary_path: None,
        app_window_config: None,
    })
}

/// Scan user agents directory for loose agent directories.
pub fn scan_user_agents(dir: &Path) -> Vec<LoadedAgent> {
    let mut agents = Vec::new();
    if !dir.exists() {
        return agents;
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            warn!(error = %e, dir = %dir.display(), "failed to read user agents directory");
            return agents;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && path.join("AGENT.md").exists() {
            match load_from_dir(&path, AgentSource::User) {
                Ok(agent) => agents.push(agent),
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "failed to load user agent");
                }
            }
        }
    }

    agents
}
