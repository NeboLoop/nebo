//! Agent filesystem loader.
//!
//! Loads agent definitions from:
//! - `nebo/agents/` — sealed .napp archives (marketplace)
//! - `user/agents/` — loose files (user-created)
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
}

/// Manages loading, caching, and hot-reloading of agents from the filesystem.
///
/// Content-only scanner — no CRUD, no state management. The DB owns all
/// mutable agent state (enabled, input_values, etc.).
pub struct AgentLoader {
    installed_dir: PathBuf,
    user_dir: PathBuf,
    agents: Arc<RwLock<HashMap<String, LoadedAgent>>>,
}

impl AgentLoader {
    pub fn new(installed_dir: PathBuf, user_dir: PathBuf) -> Self {
        Self {
            installed_dir,
            user_dir,
            agents: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Load all agents from installed and user directories.
    /// Loading order: installed → user (user overrides by name).
    pub async fn load_all(&self) -> usize {
        let mut loaded = HashMap::new();

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

    /// Start watching for filesystem changes and reload on modification.
    pub fn watch(&self) -> tokio::task::JoinHandle<()> {
        let installed_dir = self.installed_dir.clone();
        let user_dir = self.user_dir.clone();
        let agents = self.agents.clone();

        tokio::spawn(async move {
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

                        let count = loaded.len();
                        *agents.write().await = loaded;
                        info!(count, "reloaded agents after filesystem change");
                    }
                    Err(e) => {
                        warn!(error = %e, "filesystem watch error (agents)");
                    }
                }
            }
        })
    }
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

    // Read version, id, and display name from manifest.json if available
    let (version, id, manifest_name) = {
        let manifest_path = dir.join("manifest.json");
        if manifest_path.exists() {
            let parsed = std::fs::read_to_string(&manifest_path)
                .ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
            match parsed {
                Some(v) => (
                    v["version"].as_str().map(String::from),
                    v["id"].as_str().map(String::from),
                    v["name"].as_str().map(String::from),
                ),
                None => (None, None, None),
            }
        } else {
            (None, None, None)
        }
    };

    // Prefer manifest.json display name over AGENT.md frontmatter slug
    if let Some(ref name) = manifest_name {
        if !name.is_empty() {
            agent_def.name = name.clone();
        }
    }

    let description = agent_def.description.clone();

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
    })
}

/// Scan installed (nebo/) agents directory for extracted agent directories.
pub fn scan_installed_agents(dir: &Path) -> Vec<LoadedAgent> {
    let mut agents = Vec::new();
    if !dir.exists() {
        return agents;
    }
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
