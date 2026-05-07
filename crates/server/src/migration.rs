//! Data directory migration to sealed .napp layout.
//!
//! On first startup with the new directory structure:
//! - Moves `skills/*.yaml` and `skills/*/SKILL.md` → `user/skills/`
//! - Moves `tools/` contents → `user/tools/` (sideloaded tools)
//! - Marker file `.migrated-v2` prevents re-running.

use std::path::Path;
use tracing::{debug, info, warn};

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
    for subdir in &["skills", "workflows", "agents"] {
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
///
/// Skips sealed .napp files (paid content) — those are read in memory at runtime
/// and never fully extracted to disk. Detection: after unwrapping the envelope,
/// if the payload does NOT start with gzip magic bytes, it's sealed.
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
            // Check if this .napp is sealed (encrypted) — skip if so
            if is_sealed_napp(&path) {
                debug!(path = %path.display(), "skipping sealed .napp (paid content)");
                continue;
            }
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

/// Check if a .napp file contains a sealed (encrypted) payload.
///
/// Reads the file, unwraps the envelope (verifies signature), then checks
/// if the inner payload starts with gzip magic bytes. If not, it's sealed.
fn is_sealed_napp(path: &Path) -> bool {
    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(_) => return false,
    };
    match napp::napp::unwrap_napp_builtin(&data) {
        Ok(payload) => napp::sealed::is_sealed(&payload),
        Err(_) => false, // Can't unwrap — let extract_napp_alongside handle the error
    }
}

// ── Phase 4: Rename roles/ → agents/ and ROLE.md → AGENT.md ─────────

#[allow(dead_code)] // One-time migration, kept for users upgrading from older versions
const ROLES_TO_AGENTS_MARKER: &str = ".migrated-v4";

/// Rename `roles/` directories to `agents/` and ROLE.md/role.json → AGENT.md/agent.json.
///
/// Must run BEFORE `ensure_artifact_dirs()` so the renamed directories are in place
/// before the directory structure is validated.
#[allow(dead_code)]
pub fn migrate_roles_to_agents(data_dir: &Path) {
    let marker = data_dir.join(ROLES_TO_AGENTS_MARKER);
    if marker.exists() {
        return;
    }

    info!("migrating roles/ → agents/ directory layout");

    let mut moved = 0usize;
    let mut renamed = 0usize;

    for namespace in &["nebo", "user"] {
        let roles_dir = data_dir.join(namespace).join("roles");
        let agents_dir = data_dir.join(namespace).join("agents");

        if roles_dir.exists() {
            // Move contents from roles/ into agents/
            if let Err(e) = std::fs::create_dir_all(&agents_dir) {
                warn!(error = %e, dir = %agents_dir.display(), "failed to create agents directory");
                continue;
            }

            let entries = match std::fs::read_dir(&roles_dir) {
                Ok(e) => e,
                Err(e) => {
                    warn!(error = %e, dir = %roles_dir.display(), "failed to read roles directory");
                    continue;
                }
            };

            for entry in entries.flatten() {
                let src = entry.path();
                let dest = agents_dir.join(entry.file_name());
                if !dest.exists() {
                    if let Err(e) = std::fs::rename(&src, &dest) {
                        // Cross-device? Fall back to copy + delete.
                        if let Err(e2) = copy_dir_recursive(&src, &dest) {
                            warn!(src = %src.display(), error = %e, copy_error = %e2, "failed to move agent");
                            continue;
                        }
                        let _ = std::fs::remove_dir_all(&src);
                    }
                    moved += 1;
                }
            }

            // Remove now-empty roles/ directory
            let _ = std::fs::remove_dir(&roles_dir);
        }

        // Rename ROLE.md → AGENT.md and role.json → agent.json inside agents/
        if agents_dir.exists() {
            renamed += rename_role_files_recursive(&agents_dir);
        }
    }

    if moved > 0 || renamed > 0 {
        info!(moved, renamed, "roles → agents migration complete");
    }

    if let Err(e) = std::fs::write(&marker, "migrated") {
        warn!(error = %e, "failed to write roles-to-agents migration marker");
    }
}

/// Recursively rename ROLE.md → AGENT.md, role.json → agent.json,
/// and fix manifest.json `"type": "role"` → `"type": "agent"`.
#[allow(dead_code)]
fn rename_role_files_recursive(dir: &Path) -> usize {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return 0,
    };

    let mut count = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            count += rename_role_files_recursive(&path);
        } else {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();

            // Rename role files
            let new_name = match name_str.as_ref() {
                "ROLE.md" => Some("AGENT.md"),
                "role.md" => Some("agent.md"),
                "role.json" => Some("agent.json"),
                _ => None,
            };
            if let Some(new) = new_name {
                let new_path = dir.join(new);
                if !new_path.exists() {
                    if let Err(e) = std::fs::rename(&path, &new_path) {
                        warn!(src = %path.display(), error = %e, "failed to rename role file");
                    } else {
                        count += 1;
                    }
                }
            }

            // Fix manifest.json: "type": "role" → "type": "agent", and "role" → "agent" in descriptions
            if name_str == "manifest.json" {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    if let Ok(mut json) = serde_json::from_str::<serde_json::Value>(&content) {
                        let mut changed = false;
                        if json.get("type").and_then(|v| v.as_str()) == Some("role") {
                            json["type"] = serde_json::Value::String("agent".into());
                            changed = true;
                        }
                        if let Some(desc) = json.get("description").and_then(|v| v.as_str()) {
                            let new_desc = desc.replace(" role ", " agent ").replace(" Role ", " Agent ");
                            if new_desc != desc {
                                json["description"] = serde_json::Value::String(new_desc);
                                changed = true;
                            }
                        }
                        if changed {
                            if let Ok(pretty) = serde_json::to_string_pretty(&json) {
                                if let Err(e) = std::fs::write(&path, pretty) {
                                    warn!(path = %path.display(), error = %e, "failed to update manifest.json");
                                } else {
                                    count += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    count
}

// ── Phase 5: Migrate data directory to ~/.nebo/ ──────────────────────

const DATA_DIR_MARKER: &str = ".migrated-v5";

/// Migrate data from the old platform-specific directory to `~/.nebo/`.
///
/// - macOS:   ~/Library/Application Support/Nebo/ → ~/.nebo/
/// - Windows: %AppData%\Nebo\ → ~/.nebo/
/// - Linux:   ~/.config/nebo/ → ~/.nebo/
///
/// Must run BEFORE `ensure_data_dir()`. Idempotent via marker file.
pub fn migrate_data_dir() {
    let new_dir = match config::data_dir() {
        Ok(d) => d,
        Err(_) => return,
    };

    // If new dir already has the marker, migration already ran
    if new_dir.join(DATA_DIR_MARKER).exists() {
        return;
    }

    let old_dir = match config::legacy_data_dir() {
        Some(d) => d,
        None => {
            // No legacy path known — fresh install, just write marker
            let _ = std::fs::create_dir_all(&new_dir);
            let _ = std::fs::write(new_dir.join(DATA_DIR_MARKER), "fresh");
            return;
        }
    };

    // Same path (shouldn't happen but guard against it)
    if old_dir == new_dir {
        let _ = std::fs::write(new_dir.join(DATA_DIR_MARKER), "same");
        return;
    }

    // Old dir doesn't exist — fresh install
    if !old_dir.exists() {
        let _ = std::fs::create_dir_all(&new_dir);
        let _ = std::fs::write(new_dir.join(DATA_DIR_MARKER), "fresh");
        return;
    }

    // Both exist — don't interfere, user may have set up manually
    if new_dir.exists() && std::fs::read_dir(&new_dir).map(|mut d| d.next().is_some()).unwrap_or(false) {
        info!("both old and new data dirs exist, skipping migration");
        let _ = std::fs::write(new_dir.join(DATA_DIR_MARKER), "skipped");
        return;
    }

    info!(
        old = %old_dir.display(),
        new = %new_dir.display(),
        "migrating data directory to ~/.nebo/"
    );

    // Move (rename) the old directory to the new location
    if let Err(_rename_err) = std::fs::rename(&old_dir, &new_dir) {
        // Cross-device or permission issue — fall back to recursive copy
        if let Err(e) = copy_dir_recursive(&old_dir, &new_dir) {
            warn!(error = %e, "failed to copy data directory during migration");
            return;
        }
        // Don't delete old dir — leave it as a backup
        info!("data directory copied (old directory preserved as backup)");
    } else {
        info!("data directory moved successfully");
    }

    if let Err(e) = std::fs::write(new_dir.join(DATA_DIR_MARKER), "migrated") {
        warn!(error = %e, "failed to write data dir migration marker");
    }
}

// ── Phase 6: Seed bundled .napp files from app resources ──────────

/// Seed `.napp` files from app bundle resources into the data directory.
///
/// On desktop installs, the app bundle ships with pre-signed `.napp` files in
/// `bundled-napps/{skills,agents,plugins}/`. This function copies them into the
/// writable data directory so Phase 3 extraction (and plugin install) can process
/// them.
///
/// Marker: `.bundled-<app_version>` — re-runs on app upgrade so new bundled
/// content is seeded.
pub fn seed_bundled_napps(data_dir: &Path) {
    let app_version = env!("CARGO_PKG_VERSION");
    let marker = data_dir.join(format!(".bundled-{}", app_version));
    if marker.exists() {
        return;
    }

    let resources_dir = match config::bundled_napps_dir() {
        Some(d) => d,
        None => {
            // No bundled resources (dev/CLI mode) — write marker and return
            let _ = std::fs::write(&marker, "no-resources");
            return;
        }
    };

    info!(dir = %resources_dir.display(), "seeding bundled .napp files");

    let nebo_dir = data_dir.join("nebo");
    let mut count = 0usize;

    // Skills and agents: copy .napp files → nebo/{skills,agents}/
    // Phase 3 (migrate_napp_extraction) handles the actual extraction.
    for artifact_type in &["skills", "agents"] {
        let src_dir = resources_dir.join(artifact_type);
        if !src_dir.is_dir() {
            continue;
        }
        let dest_dir = nebo_dir.join(artifact_type);
        let _ = std::fs::create_dir_all(&dest_dir);
        count += seed_napp_files(&src_dir, &dest_dir);
    }

    // Plugins: verify envelope + extract directly to nebo/plugins/<slug>/<version>/
    let plugins_src = resources_dir.join("plugins");
    if plugins_src.is_dir() {
        count += seed_plugin_napps(&plugins_src, &nebo_dir.join("plugins"));
    }

    if count > 0 {
        info!(count, "seeded bundled .napp files");

        // Remove the Phase 3 extraction marker so newly seeded .napp files
        // get extracted on this startup.
        let extraction_marker = data_dir.join(EXTRACTION_MARKER);
        if extraction_marker.exists() {
            let _ = std::fs::remove_file(&extraction_marker);
            info!("cleared extraction marker — Phase 3 will re-run for new .napp files");
        }
    }

    if let Err(e) = std::fs::write(&marker, "seeded") {
        warn!(error = %e, "failed to write bundled seed marker");
    }
}

/// Copy .napp files from `src_dir` to `dest_dir`, preserving subdirectory structure.
/// Skips files that already exist at the destination.
fn seed_napp_files(src_dir: &Path, dest_dir: &Path) -> usize {
    seed_napp_files_recursive(src_dir, dest_dir, src_dir)
}

fn seed_napp_files_recursive(dir: &Path, dest_base: &Path, src_base: &Path) -> usize {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return 0,
    };

    let mut count = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            count += seed_napp_files_recursive(&path, dest_base, src_base);
        } else if path.extension().is_some_and(|ext| ext == "napp") {
            // Preserve relative path structure
            let rel = match path.strip_prefix(src_base) {
                Ok(r) => r,
                Err(_) => continue,
            };
            let dest = dest_base.join(rel);
            if dest.exists() {
                continue;
            }
            if let Some(parent) = dest.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match std::fs::copy(&path, &dest) {
                Ok(_) => {
                    info!(src = %path.display(), dest = %dest.display(), "seeded .napp");
                    count += 1;
                }
                Err(e) => {
                    warn!(src = %path.display(), error = %e, "failed to seed .napp file");
                }
            }
        }
    }
    count
}

/// Seed plugin .napp files: verify envelope, extract to nebo/plugins/<slug>/<version>/.
///
/// Plugins use a different directory layout than skills/agents, so we can't
/// just copy the .napp and let Phase 3 handle it. Instead we:
/// 1. Verify the .napp envelope with the embedded NeboLoop public key
/// 2. Read plugin.json from the tar.gz to get slug + version
/// 3. Skip if nebo/plugins/<slug>/<version>/ already exists
/// 4. Store the .napp and extract alongside
fn seed_plugin_napps(src_dir: &Path, plugins_dest: &Path) -> usize {
    let entries = match std::fs::read_dir(src_dir) {
        Ok(e) => e,
        Err(_) => return 0,
    };

    let mut count = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.extension().is_some_and(|ext| ext == "napp") {
            continue;
        }

        match seed_single_plugin(&path, plugins_dest) {
            Ok(true) => count += 1,
            Ok(false) => {} // skipped (already exists)
            Err(e) => {
                warn!(path = %path.display(), error = %e, "failed to seed bundled plugin");
            }
        }
    }
    count
}

/// Seed a single plugin .napp. Returns Ok(true) if installed, Ok(false) if skipped.
fn seed_single_plugin(
    napp_path: &Path,
    plugins_dest: &Path,
) -> Result<bool, Box<dyn std::error::Error>> {
    let data = std::fs::read(napp_path)?;

    // Verify envelope (magic + SHA256 + ED25519 with embedded key)
    let payload = napp::napp::unwrap_napp_builtin(&data)?;

    // Read plugin.json from tar.gz to get slug + version
    let (slug, version) = napp::reader::read_plugin_identity_from_tar_gz(&payload)?;

    // Skip if already installed
    let version_dir = plugins_dest.join(&slug).join(&version);
    if version_dir.exists() {
        return Ok(false);
    }

    // Store .napp and extract alongside
    let plugin_dir = plugins_dest.join(&slug);
    std::fs::create_dir_all(&plugin_dir)?;

    let dest_napp = plugin_dir.join(format!("{}.napp", version));
    std::fs::write(&dest_napp, &data)?;

    // extract_napp_alongside: <slug>/<version>.napp → <slug>/<version>/
    napp::reader::extract_napp_alongside(&dest_napp)?;

    // Set +x on any binary in the extracted dir
    set_executable_in_dir(&version_dir);

    info!(
        plugin = slug,
        version = version,
        path = %version_dir.display(),
        "seeded bundled plugin"
    );

    Ok(true)
}

/// Set +x on executables in a directory (plugin binaries).
#[cfg(unix)]
fn set_executable_in_dir(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        // Skip metadata files
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.ends_with(".json") || name_str.ends_with(".md") {
            continue;
        }
        // Check if it's a native binary (ELF/Mach-O/PE)
        if let Ok(data) = std::fs::read(&path) {
            if data.len() >= 4 {
                let is_native = data.starts_with(&[0x7f, 0x45, 0x4c, 0x46]) // ELF
                    || data.starts_with(&[0xfe, 0xed, 0xfa, 0xce])          // Mach-O 32
                    || data.starts_with(&[0xfe, 0xed, 0xfa, 0xcf])          // Mach-O 64
                    || data.starts_with(&[0xce, 0xfa, 0xed, 0xfe])          // Mach-O 32 (swapped)
                    || data.starts_with(&[0xcf, 0xfa, 0xed, 0xfe])          // Mach-O 64 (swapped)
                    || data.starts_with(&[0xca, 0xfe, 0xba, 0xbe])          // Universal
                    || data.starts_with(&[0x4d, 0x5a]);                     // PE
                if is_native {
                    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
                }
            }
        }
    }
}

#[cfg(not(unix))]
fn set_executable_in_dir(_dir: &Path) {
    // No-op on Windows — executables don't need +x
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

    #[test]
    fn test_seed_napp_files_copies_with_structure() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dest = tmp.path().join("dest");
        std::fs::create_dir_all(src.join("@acme")).unwrap();
        std::fs::create_dir_all(&dest).unwrap();

        // Create test .napp files
        std::fs::write(src.join("skill-a.napp"), b"fake-napp-a").unwrap();
        std::fs::write(src.join("@acme").join("skill-b.napp"), b"fake-napp-b").unwrap();

        let count = seed_napp_files(&src, &dest);
        assert_eq!(count, 2);
        assert!(dest.join("skill-a.napp").exists());
        assert!(dest.join("@acme").join("skill-b.napp").exists());
    }

    #[test]
    fn test_seed_napp_files_skips_existing() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dest = tmp.path().join("dest");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dest).unwrap();

        // Pre-existing file at destination
        std::fs::write(dest.join("existing.napp"), b"original").unwrap();
        std::fs::write(src.join("existing.napp"), b"newer").unwrap();
        std::fs::write(src.join("new.napp"), b"new-content").unwrap();

        let count = seed_napp_files(&src, &dest);
        assert_eq!(count, 1); // Only new.napp

        // existing.napp should NOT be overwritten
        let content = std::fs::read_to_string(dest.join("existing.napp")).unwrap();
        assert_eq!(content, "original");
    }

    #[test]
    fn test_seed_bundled_marker_per_version() {
        let tmp = TempDir::new().unwrap();
        let data_dir = tmp.path();
        std::fs::create_dir_all(data_dir.join("nebo").join("skills")).unwrap();

        // seed_bundled_napps writes marker based on CARGO_PKG_VERSION
        seed_bundled_napps(data_dir);

        let marker = data_dir.join(format!(".bundled-{}", env!("CARGO_PKG_VERSION")));
        assert!(marker.exists());

        // Second call is a no-op (marker exists)
        seed_bundled_napps(data_dir);
    }
}
