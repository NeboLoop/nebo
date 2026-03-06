use std::io::Read;
use std::path::Path;

use flate2::read::GzDecoder;
use tar::Archive;
use tracing::info;

use crate::manifest::Manifest;
use crate::NappError;

const MAX_BINARY_SIZE: u64 = 500 * 1024 * 1024; // 500MB
const MAX_UI_FILE_SIZE: u64 = 5 * 1024 * 1024;  // 5MB
const MAX_METADATA_SIZE: u64 = 1024 * 1024;      // 1MB

/// Allowed file names in a .napp archive.
const ALLOWED_FILES: &[&str] = &[
    "manifest.json",
    "binary",
    "app",
    "signatures.json",
    "SKILL.md",
    "skill.md",
];

/// Extract a .napp (tar.gz) archive securely.
pub fn extract_napp(napp_path: &Path, dest_dir: &Path) -> Result<Manifest, NappError> {
    std::fs::create_dir_all(dest_dir)?;

    let file = std::fs::File::open(napp_path)?;
    let gz = GzDecoder::new(file);
    let mut archive = Archive::new(gz);

    let mut found_manifest = false;

    for entry_result in archive.entries().map_err(|e| NappError::Extraction(e.to_string()))? {
        let mut entry = entry_result.map_err(|e| NappError::Extraction(e.to_string()))?;
        let path = entry
            .path()
            .map_err(|e| NappError::Extraction(e.to_string()))?
            .to_path_buf();

        let name = path
            .to_str()
            .ok_or_else(|| NappError::Extraction("invalid path encoding".into()))?;

        // Security: reject path traversal
        if name.contains("..") || name.starts_with('/') {
            return Err(NappError::Extraction(format!(
                "path traversal detected: {}",
                name
            )));
        }

        // Security: reject symlinks and hardlinks
        let entry_type = entry.header().entry_type();
        if entry_type == tar::EntryType::Symlink || entry_type == tar::EntryType::Link {
            return Err(NappError::Extraction(format!(
                "symlinks not allowed in .napp: {}",
                name
            )));
        }

        // Check if file is allowed
        let is_ui = name.starts_with("ui/");
        let base_name = name.trim_start_matches("./");

        if !is_ui && !ALLOWED_FILES.contains(&base_name) {
            return Err(NappError::Extraction(format!(
                "unexpected file in .napp: {}",
                name
            )));
        }

        // Enforce size limits
        let size = entry.size();
        let max_size = match base_name {
            "binary" | "app" => MAX_BINARY_SIZE,
            _ if is_ui => MAX_UI_FILE_SIZE,
            _ => MAX_METADATA_SIZE,
        };
        if size > max_size {
            return Err(NappError::Extraction(format!(
                "file {} exceeds size limit ({} > {})",
                name, size, max_size
            )));
        }

        // Build target path and verify it's within dest_dir
        let target = dest_dir.join(base_name);
        let canonical_dest = dest_dir
            .canonicalize()
            .unwrap_or_else(|_| dest_dir.to_path_buf());
        if let Some(canonical_target) = target.parent().and_then(|p| p.canonicalize().ok().or(Some(p.to_path_buf()))) {
            if !canonical_target.starts_with(&canonical_dest) {
                return Err(NappError::Extraction(format!(
                    "path escape detected: {}",
                    name
                )));
            }
        }

        // Create parent dirs for ui/ files
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Extract file
        let mut content = Vec::new();
        entry
            .read_to_end(&mut content)
            .map_err(|e| NappError::Extraction(e.to_string()))?;

        // Defense in depth: verify actual size
        if content.len() as u64 > max_size {
            return Err(NappError::Extraction(format!(
                "file {} actual size exceeds limit",
                name
            )));
        }

        std::fs::write(&target, &content)?;

        // Validate binary format
        if base_name == "binary" || base_name == "app" {
            validate_binary_format(&content)?;

            // Make executable on Unix
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755))?;
            }
        }

        if base_name == "manifest.json" {
            found_manifest = true;
        }
    }

    if !found_manifest {
        return Err(NappError::Extraction("manifest.json not found in .napp".into()));
    }

    // Load and validate manifest
    let manifest = Manifest::load(&dest_dir.join("manifest.json"))?;
    manifest.validate()?;

    info!(app = manifest.id.as_str(), version = manifest.version.as_str(), "extracted .napp");
    Ok(manifest)
}

/// Validate that a binary is a native executable (not a script).
fn validate_binary_format(content: &[u8]) -> Result<(), NappError> {
    if content.len() < 4 {
        return Err(NappError::Extraction("binary too small".into()));
    }

    // Check magic bytes
    let is_elf = content.starts_with(&[0x7f, 0x45, 0x4c, 0x46]); // ELF
    let is_macho = content.starts_with(&[0xfe, 0xed, 0xfa, 0xce]) // Mach-O 32
        || content.starts_with(&[0xfe, 0xed, 0xfa, 0xcf])         // Mach-O 64
        || content.starts_with(&[0xce, 0xfa, 0xed, 0xfe])         // Mach-O 32 (swapped)
        || content.starts_with(&[0xcf, 0xfa, 0xed, 0xfe])         // Mach-O 64 (swapped)
        || content.starts_with(&[0xca, 0xfe, 0xba, 0xbe]);        // Universal
    let is_pe = content.starts_with(&[0x4d, 0x5a]);               // PE/COFF

    if !is_elf && !is_macho && !is_pe {
        // Reject scripts
        if content.starts_with(b"#!") {
            return Err(NappError::Extraction(
                "shebang scripts not allowed — compiled binaries only".into(),
            ));
        }
        return Err(NappError::Extraction(
            "unrecognized binary format — only native executables allowed".into(),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_binary_elf() {
        let elf = [0x7f, 0x45, 0x4c, 0x46, 0x02, 0x01];
        assert!(validate_binary_format(&elf).is_ok());
    }

    #[test]
    fn test_reject_shebang() {
        let script = b"#!/bin/bash\necho hello";
        assert!(validate_binary_format(script).is_err());
    }

    #[test]
    fn test_reject_too_small() {
        assert!(validate_binary_format(&[0, 1]).is_err());
    }
}
