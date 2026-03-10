//! Data directory migration to sealed .napp layout.
//!
//! On first startup with the new directory structure:
//! - Moves `skills/*.yaml` and `skills/*/SKILL.md` → `user/skills/`
//! - Moves `tools/` contents → `user/tools/` (sideloaded tools)
//! - Marker file `.migrated-v2` prevents re-running.

use std::path::Path;
use tracing::{info, warn};

const MIGRATION_MARKER: &str = ".migrated-v2";

/// Run the data directory migration if it hasn't been run yet.
///
/// Call this during server startup after `ensure_artifact_dirs()`.
pub fn migrate_if_needed(data_dir: &Path) {
    let marker = data_dir.join(MIGRATION_MARKER);
    if marker.exists() {
        return;
    }

    info!("running one-time migration to sealed .napp directory layout");

    migrate_skills(data_dir);
    migrate_tools(data_dir);

    // Write migration marker
    if let Err(e) = std::fs::write(&marker, "migrated") {
        warn!(error = %e, "failed to write migration marker");
    }

    info!("migration complete");
}

/// Move skills from `<data_dir>/skills/` → `<data_dir>/user/skills/`
fn migrate_skills(data_dir: &Path) {
    let old_skills = data_dir.join("skills");
    let new_skills = data_dir.join("user").join("skills");

    if !old_skills.exists() {
        return;
    }

    let entries = match std::fs::read_dir(&old_skills) {
        Ok(e) => e,
        Err(e) => {
            warn!(error = %e, "failed to read old skills directory");
            return;
        }
    };

    let mut count = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();

        if path.is_dir() {
            // Directory-based skill (contains SKILL.md)
            let dest = new_skills.join(&name);
            if !dest.exists() {
                if let Err(e) = copy_dir_recursive(&path, &dest) {
                    warn!(src = %path.display(), error = %e, "failed to migrate skill directory");
                    continue;
                }
                count += 1;
            }
        } else {
            // Flat file (.yaml or .yaml.disabled)
            let name_str = name.to_string_lossy();
            if name_str.ends_with(".yaml") || name_str.ends_with(".yaml.disabled") {
                let dest = new_skills.join(&name);
                if !dest.exists() {
                    if let Err(e) = std::fs::copy(&path, &dest) {
                        warn!(src = %path.display(), error = %e, "failed to migrate skill file");
                        continue;
                    }
                    count += 1;
                }
            }
        }
    }

    if count > 0 {
        info!(count, "migrated skills to user/skills/");
    }
}

/// Move tools from `<data_dir>/tools/` → `<data_dir>/user/tools/`
fn migrate_tools(data_dir: &Path) {
    let old_tools = data_dir.join("tools");
    let new_tools = data_dir.join("user").join("tools");

    if !old_tools.exists() {
        return;
    }

    let entries = match std::fs::read_dir(&old_tools) {
        Ok(e) => e,
        Err(e) => {
            warn!(error = %e, "failed to read old tools directory");
            return;
        }
    };

    let mut count = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        // Skip hidden dirs (.tmp, etc.)
        if name_str.starts_with('.') {
            continue;
        }

        let dest = new_tools.join(&name);
        if !dest.exists() {
            if path.symlink_metadata().map(|m| m.file_type().is_symlink()).unwrap_or(false) {
                // Preserve symlinks (sideloaded tools)
                if let Ok(target) = std::fs::read_link(&path) {
                    #[cfg(unix)]
                    {
                        if let Err(e) = std::os::unix::fs::symlink(&target, &dest) {
                            warn!(src = %path.display(), error = %e, "failed to migrate tool symlink");
                            continue;
                        }
                    }
                    #[cfg(windows)]
                    {
                        if let Err(e) = std::os::windows::fs::symlink_dir(&target, &dest) {
                            warn!(src = %path.display(), error = %e, "failed to migrate tool symlink");
                            continue;
                        }
                    }
                }
            } else {
                if let Err(e) = copy_dir_recursive(&path, &dest) {
                    warn!(src = %path.display(), error = %e, "failed to migrate tool directory");
                    continue;
                }
            }
            count += 1;
        }
    }

    if count > 0 {
        info!(count, "migrated tools to user/tools/");
    }
}

// ── Phase 3: Extract sealed .napp archives alongside ────────────────

const EXTRACTION_MARKER: &str = ".migrated-v3";

/// Extract all .napp archives in nebo/ to sibling directories.
///
/// Idempotent: skips archives whose sibling directory already exists.
/// Marker `.migrated-v3` prevents re-running the walk.
pub fn migrate_napp_extraction(data_dir: &Path) {
    let marker = data_dir.join(EXTRACTION_MARKER);
    if marker.exists() {
        return;
    }

    let nebo_dir = data_dir.join("nebo");
    if !nebo_dir.exists() {
        // No sealed archives to extract
        if let Err(e) = std::fs::write(&marker, "migrated") {
            warn!(error = %e, "failed to write extraction migration marker");
        }
        return;
    }

    info!("extracting .napp archives to sibling directories");

    let mut count = 0usize;
    for subdir in &["skills", "workflows", "roles"] {
        let dir = nebo_dir.join(subdir);
        if dir.exists() {
            count += extract_napps_recursive(&dir);
        }
    }

    if count > 0 {
        info!(count, "extracted .napp archives");
    }

    if let Err(e) = std::fs::write(&marker, "migrated") {
        warn!(error = %e, "failed to write extraction migration marker");
    }
}

/// Walk a directory tree and extract every .napp file alongside itself.
fn extract_napps_recursive(dir: &Path) -> usize {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return 0,
    };

    let mut count = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            count += extract_napps_recursive(&path);
        } else if path.extension().is_some_and(|ext| ext == "napp") {
            match napp::reader::extract_napp_alongside(&path) {
                Ok(_) => count += 1,
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "failed to extract .napp archive");
                }
            }
        }
    }
    count
}

/// Recursively copy a directory.
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;

    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_migrate_skills() {
        let tmp = TempDir::new().unwrap();
        let data_dir = tmp.path();

        // Create old layout
        let old_skills = data_dir.join("skills");
        std::fs::create_dir_all(&old_skills).unwrap();
        std::fs::write(old_skills.join("legacy.yaml"), "content").unwrap();
        std::fs::write(old_skills.join("disabled.yaml.disabled"), "off").unwrap();
        let skill_dir = old_skills.join("my-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "---\nname: test\n---\nbody").unwrap();

        // Create new layout dirs
        let new_skills = data_dir.join("user").join("skills");
        std::fs::create_dir_all(&new_skills).unwrap();

        // Run migration
        migrate_skills(data_dir);

        // Verify
        assert!(new_skills.join("legacy.yaml").exists());
        assert!(new_skills.join("disabled.yaml.disabled").exists());
        assert!(new_skills.join("my-skill").join("SKILL.md").exists());
    }

    #[test]
    fn test_migrate_idempotent() {
        let tmp = TempDir::new().unwrap();
        let data_dir = tmp.path();

        // Create dirs
        std::fs::create_dir_all(data_dir.join("skills")).unwrap();
        std::fs::write(data_dir.join("skills").join("test.yaml"), "v1").unwrap();
        std::fs::create_dir_all(data_dir.join("user").join("skills")).unwrap();

        // Run migration twice
        migrate_skills(data_dir);
        // Modify original
        std::fs::write(data_dir.join("skills").join("test.yaml"), "v2").unwrap();
        migrate_skills(data_dir);

        // Should keep v1 (doesn't overwrite existing)
        let content = std::fs::read_to_string(data_dir.join("user").join("skills").join("test.yaml")).unwrap();
        assert_eq!(content, "v1");
    }

    #[test]
    fn test_marker_prevents_rerun() {
        let tmp = TempDir::new().unwrap();
        let data_dir = tmp.path();
        std::fs::create_dir_all(data_dir.join("user").join("skills")).unwrap();

        // First run
        migrate_if_needed(data_dir);
        assert!(data_dir.join(MIGRATION_MARKER).exists());

        // Create a skill that should NOT be migrated on second run
        std::fs::create_dir_all(data_dir.join("skills")).unwrap();
        std::fs::write(data_dir.join("skills").join("new.yaml"), "content").unwrap();

        // Second run — should be skipped
        migrate_if_needed(data_dir);
        assert!(!data_dir.join("user").join("skills").join("new.yaml").exists());
    }
}
