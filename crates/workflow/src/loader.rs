//! Workflow filesystem loader.
//!
//! Loads workflow definitions from:
//! - `nebo/workflows/` — sealed .napp archives (marketplace)
//! - `user/workflows/` — loose files (user-created)

use std::path::{Path, PathBuf};

use tracing::{debug, warn};

use crate::parser::{WorkflowDef, parse_workflow};

/// Where a workflow was loaded from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowSource {
    /// Installed from NeboLoop marketplace (sealed .napp archive).
    Installed,
    /// User-created (loose files in user/ directory).
    User,
}

/// A workflow loaded from the filesystem.
#[derive(Debug, Clone)]
pub struct LoadedWorkflow {
    pub def: WorkflowDef,
    pub source: WorkflowSource,
    /// Path to .napp archive (for installed workflows).
    pub napp_path: Option<PathBuf>,
    /// Path to the directory or file this was loaded from.
    pub source_path: PathBuf,
    /// Optional skill markdown content.
    pub skill_md: Option<String>,
}

/// Load a workflow definition from a directory (loose files or extracted .napp).
pub fn load_from_dir(dir: &Path, source: WorkflowSource) -> Result<LoadedWorkflow, String> {
    let json_path = dir.join("workflow.json");
    if !json_path.exists() {
        return Err(format!("workflow.json not found in {}", dir.display()));
    }

    let json_data = std::fs::read_to_string(&json_path)
        .map_err(|e| format!("failed to read {}: {}", json_path.display(), e))?;

    let def = parse_workflow(&json_data)
        .map_err(|e| format!("failed to parse {}: {}", json_path.display(), e))?;

    let md_path = dir.join("WORKFLOW.md");
    let skill_md = if md_path.exists() {
        std::fs::read_to_string(&md_path).ok()
    } else {
        None
    };

    Ok(LoadedWorkflow {
        def,
        source,
        napp_path: None,
        source_path: dir.to_path_buf(),
        skill_md,
    })
}

/// Scan installed (nebo/) workflows directory for extracted workflow directories.
pub fn scan_installed_workflows(dir: &Path) -> Vec<LoadedWorkflow> {
    let mut workflows = Vec::new();
    if !dir.exists() {
        return workflows;
    }
    napp::reader::walk_for_marker(dir, "workflow.json", &mut |wf_dir| {
        match load_from_dir(wf_dir, WorkflowSource::Installed) {
            Ok(wf) => workflows.push(wf),
            Err(e) => {
                debug!(path = %wf_dir.display(), error = %e, "skipping directory (not a workflow)");
            }
        }
    });
    workflows
}

/// Scan user workflows directory for loose workflow directories.
pub fn scan_user_workflows(dir: &Path) -> Vec<LoadedWorkflow> {
    let mut workflows = Vec::new();
    if !dir.exists() {
        return workflows;
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            warn!(error = %e, dir = %dir.display(), "failed to read user workflows directory");
            return workflows;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && path.join("workflow.json").exists() {
            match load_from_dir(&path, WorkflowSource::User) {
                Ok(wf) => workflows.push(wf),
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "failed to load user workflow");
                }
            }
        }
    }

    workflows
}
