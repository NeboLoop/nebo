//! Agent filesystem loader.
//!
//! Loads agent definitions from:
//! - `nebo/agents/` — sealed .napp archives (marketplace)
//! - `user/agents/` — loose files (user-created)

use std::path::{Path, PathBuf};

use tracing::{debug, warn};

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

    let agent_md = std::fs::read_to_string(&agent_md_path)
        .map_err(NappError::Io)?;
    let mut agent_def = parse_agent(&agent_md)?;

    // Fall back to directory name when AGENT.md has no frontmatter name
    if agent_def.name.is_empty() {
        if let Some(name) = dir.file_name().and_then(|n| n.to_str()) {
            agent_def.name = name.to_string();
        }
    }

    let config = {
        let config_path = dir.join("agent.json");
        if config_path.exists() {
            let json = std::fs::read_to_string(&config_path)
                .map_err(NappError::Io)?;
            Some(parse_agent_config(&json)?)
        } else {
            None
        }
    };

    // Read version from manifest.json if available
    let version = {
        let manifest_path = dir.join("manifest.json");
        if manifest_path.exists() {
            std::fs::read_to_string(&manifest_path)
                .ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .and_then(|v| v["version"].as_str().map(String::from))
        } else {
            None
        }
    };

    Ok(LoadedAgent {
        agent_def,
        config,
        source,
        napp_path: None,
        source_path: dir.to_path_buf(),
        version,
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
