//! Role filesystem loader.
//!
//! Loads role definitions from:
//! - `nebo/roles/` — sealed .napp archives (marketplace)
//! - `user/roles/` — loose files (user-created)

use std::path::{Path, PathBuf};

use tracing::{debug, warn};

use crate::NappError;
use crate::role::{RoleConfig, RoleDef, parse_role, parse_role_config};

/// Where a role was loaded from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoleSource {
    /// Installed from NeboLoop marketplace (sealed .napp archive).
    Installed,
    /// User-created (loose files in user/ directory).
    User,
}

/// A role loaded from the filesystem.
#[derive(Debug, Clone)]
pub struct LoadedRole {
    /// Role persona (from ROLE.md).
    pub role_def: RoleDef,
    /// Role operational config (from role.json).
    pub config: Option<RoleConfig>,
    /// Where this role was loaded from.
    pub source: RoleSource,
    /// Path to .napp archive (for installed roles).
    pub napp_path: Option<PathBuf>,
    /// Path to the directory or file this was loaded from.
    pub source_path: PathBuf,
    /// Version from manifest.json (if present).
    pub version: Option<String>,
}

/// Load a role from a directory (loose files or extracted .napp).
pub fn load_from_dir(dir: &Path, source: RoleSource) -> Result<LoadedRole, NappError> {
    let role_md_path = dir.join("ROLE.md");
    if !role_md_path.exists() {
        return Err(NappError::NotFound(format!(
            "ROLE.md not found in {}",
            dir.display()
        )));
    }

    let role_md = std::fs::read_to_string(&role_md_path)
        .map_err(NappError::Io)?;
    let mut role_def = parse_role(&role_md)?;

    // Fall back to directory name when ROLE.md has no frontmatter name
    if role_def.name.is_empty() {
        if let Some(name) = dir.file_name().and_then(|n| n.to_str()) {
            role_def.name = name.to_string();
        }
    }

    let config = {
        let config_path = dir.join("role.json");
        if config_path.exists() {
            let json = std::fs::read_to_string(&config_path)
                .map_err(NappError::Io)?;
            Some(parse_role_config(&json)?)
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

    Ok(LoadedRole {
        role_def,
        config,
        source,
        napp_path: None,
        source_path: dir.to_path_buf(),
        version,
    })
}

/// Scan installed (nebo/) roles directory for extracted role directories.
pub fn scan_installed_roles(dir: &Path) -> Vec<LoadedRole> {
    let mut roles = Vec::new();
    if !dir.exists() {
        return roles;
    }
    crate::reader::walk_for_marker(dir, "ROLE.md", &mut |role_dir| {
        match load_from_dir(role_dir, RoleSource::Installed) {
            Ok(role) => roles.push(role),
            Err(e) => {
                debug!(path = %role_dir.display(), error = %e, "skipping directory (not a role)");
            }
        }
    });
    roles
}

/// Scan user roles directory for loose role directories.
pub fn scan_user_roles(dir: &Path) -> Vec<LoadedRole> {
    let mut roles = Vec::new();
    if !dir.exists() {
        return roles;
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            warn!(error = %e, dir = %dir.display(), "failed to read user roles directory");
            return roles;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && path.join("ROLE.md").exists() {
            match load_from_dir(&path, RoleSource::User) {
                Ok(role) => roles.push(role),
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "failed to load user role");
                }
            }
        }
    }

    roles
}
