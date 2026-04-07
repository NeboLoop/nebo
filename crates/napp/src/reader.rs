//! Sealed .napp archive reader.
//!
//! Reads individual entries from a .napp (tar.gz) archive without extracting
//! the entire archive. Also provides extraction helpers for install-time
//! unpacking and a shared directory walker for loaders.

use std::io::Read;
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;
use tar::Archive;

use crate::NappError;

/// Read a single entry from a .napp (tar.gz) archive by name.
/// Returns the file contents as bytes.
///
/// Entry names are matched after stripping any leading `./` prefix.
/// Returns `NappError::NotFound` if the entry doesn't exist in the archive.
pub fn read_napp_entry(napp_path: &Path, entry_name: &str) -> Result<Vec<u8>, NappError> {
    let file = std::fs::File::open(napp_path)
        .map_err(|e| NappError::Io(e))?;
    let gz = GzDecoder::new(file);
    let mut archive = Archive::new(gz);

    let target = entry_name.trim_start_matches("./");

    for entry_result in archive.entries().map_err(|e| NappError::Extraction(e.to_string()))? {
        let mut entry = entry_result.map_err(|e| NappError::Extraction(e.to_string()))?;
        let path = entry
            .path()
            .map_err(|e| NappError::Extraction(e.to_string()))?;
        let name = path.to_string_lossy();
        let normalized = name.trim_start_matches("./");

        if normalized == target {
            let mut content = Vec::new();
            entry
                .read_to_end(&mut content)
                .map_err(|e| NappError::Extraction(e.to_string()))?;
            return Ok(content);
        }
    }

    Err(NappError::NotFound(format!(
        "{} not found in {}",
        entry_name,
        napp_path.display()
    )))
}

/// Read a single entry from a .napp archive and return it as a UTF-8 string.
pub fn read_napp_entry_string(napp_path: &Path, entry_name: &str) -> Result<String, NappError> {
    let bytes = read_napp_entry(napp_path, entry_name)?;
    String::from_utf8(bytes).map_err(|e| NappError::Extraction(format!("invalid UTF-8 in {}: {}", entry_name, e)))
}

/// List all entry names in a .napp (tar.gz) archive.
pub fn list_napp_entries(napp_path: &Path) -> Result<Vec<String>, NappError> {
    let file = std::fs::File::open(napp_path)
        .map_err(|e| NappError::Io(e))?;
    let gz = GzDecoder::new(file);
    let mut archive = Archive::new(gz);

    let mut entries = Vec::new();
    for entry_result in archive.entries().map_err(|e| NappError::Extraction(e.to_string()))? {
        let entry = entry_result.map_err(|e| NappError::Extraction(e.to_string()))?;
        let path = entry
            .path()
            .map_err(|e| NappError::Extraction(e.to_string()))?;
        let name = path.to_string_lossy();
        let normalized = name.trim_start_matches("./").to_string();
        if !normalized.is_empty() {
            entries.push(normalized);
        }
    }

    Ok(entries)
}

/// Extract a single entry from a .napp archive to a destination path.
///
/// Creates parent directories as needed. Used for extracting binaries and
/// ui/ assets from tool archives (the only files that need to be on disk).
pub fn extract_napp_entry(
    napp_path: &Path,
    entry_name: &str,
    dest: &Path,
) -> Result<(), NappError> {
    let content = read_napp_entry(napp_path, entry_name)?;

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }

    std::fs::write(dest, &content)?;

    // Set executable permissions for binaries, scripts, and bin/ entries
    #[cfg(unix)]
    if entry_name == "binary"
        || entry_name == "app"
        || entry_name.starts_with("bin/")
        || entry_name.starts_with("scripts/")
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dest, std::fs::Permissions::from_mode(0o755))?;
    }

    Ok(())
}

/// Extract all entries matching a prefix from a .napp archive to a destination directory.
///
/// Used for extracting `ui/` assets from tool archives.
pub fn extract_napp_prefix(
    napp_path: &Path,
    prefix: &str,
    dest_dir: &Path,
) -> Result<Vec<String>, NappError> {
    let file = std::fs::File::open(napp_path)
        .map_err(|e| NappError::Io(e))?;
    let gz = GzDecoder::new(file);
    let mut archive = Archive::new(gz);

    let target_prefix = prefix.trim_start_matches("./");
    let mut extracted = Vec::new();

    for entry_result in archive.entries().map_err(|e| NappError::Extraction(e.to_string()))? {
        let mut entry = entry_result.map_err(|e| NappError::Extraction(e.to_string()))?;
        let path = entry
            .path()
            .map_err(|e| NappError::Extraction(e.to_string()))?;
        let normalized = path.to_string_lossy().trim_start_matches("./").to_string();

        if normalized.starts_with(target_prefix) {
            let mut content = Vec::new();
            entry
                .read_to_end(&mut content)
                .map_err(|e| NappError::Extraction(e.to_string()))?;

            let dest_path = dest_dir.join(&normalized);
            if let Some(parent) = dest_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&dest_path, &content)?;
            extracted.push(normalized);
        }
    }

    Ok(extracted)
}

/// Extract all entries from a .napp (tar.gz) to a destination directory.
///
/// Preserves internal structure, creates parent dirs, sets +x on binary/app.
/// Returns a list of extracted entry names.
pub fn extract_all(napp_path: &Path, dest_dir: &Path) -> Result<Vec<String>, NappError> {
    let file = std::fs::File::open(napp_path).map_err(NappError::Io)?;
    let gz = GzDecoder::new(file);
    let mut archive = Archive::new(gz);

    let mut extracted = Vec::new();

    for entry_result in archive
        .entries()
        .map_err(|e| NappError::Extraction(e.to_string()))?
    {
        let mut entry = entry_result.map_err(|e| NappError::Extraction(e.to_string()))?;
        let path = entry
            .path()
            .map_err(|e| NappError::Extraction(e.to_string()))?;
        let normalized = path
            .to_string_lossy()
            .trim_start_matches("./")
            .to_string();

        if normalized.is_empty() {
            continue;
        }

        let dest_path = dest_dir.join(&normalized);

        // Directory entry — just create it
        if entry.header().entry_type().is_dir() {
            std::fs::create_dir_all(&dest_path)?;
            extracted.push(normalized);
            continue;
        }

        // File entry — read content and write
        if let Some(parent) = dest_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut content = Vec::new();
        entry
            .read_to_end(&mut content)
            .map_err(|e| NappError::Extraction(e.to_string()))?;

        std::fs::write(&dest_path, &content)?;

        // Set executable permissions for binaries, scripts, and bin/ entries
        #[cfg(unix)]
        if normalized == "binary"
            || normalized == "app"
            || normalized.starts_with("bin/")
            || normalized.starts_with("scripts/")
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dest_path, std::fs::Permissions::from_mode(0o755))?;
        }

        extracted.push(normalized);
    }

    Ok(extracted)
}

/// Extract a .napp to its sibling directory (strip .napp extension).
///
/// Always re-extracts: removes the existing directory first so updates
/// replace stale files.
/// Returns the destination directory path.
pub fn extract_napp_alongside(napp_path: &Path) -> Result<PathBuf, NappError> {
    let dest_dir = napp_path.with_extension("");
    if dest_dir.is_dir() {
        std::fs::remove_dir_all(&dest_dir)?;
    }

    extract_all(napp_path, &dest_dir)?;
    Ok(dest_dir)
}

/// Walk a directory tree. When a dir contains `marker_file`, call `cb(dir_path)`
/// and stop recursing into that dir (prevents finding nested markers).
pub fn walk_for_marker(dir: &Path, marker_file: &str, cb: &mut dyn FnMut(&Path)) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        // Check for marker file (case-insensitive)
        if has_marker(&path, marker_file) {
            cb(&path);
        } else {
            walk_for_marker(&path, marker_file, cb);
        }
    }
}

/// Check if a directory contains a file matching `marker_file` (case-insensitive).
fn has_marker(dir: &Path, marker_file: &str) -> bool {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return false,
    };
    for entry in entries.flatten() {
        if let Some(name) = entry.file_name().to_str() {
            if name.eq_ignore_ascii_case(marker_file) {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::GzEncoder;
    use flate2::Compression;

    /// Create a synthetic .napp archive with the given entries.
    fn create_test_napp(dir: &Path, entries: &[(&str, &[u8])]) -> std::path::PathBuf {
        let napp_path = dir.join("test.napp");
        let file = std::fs::File::create(&napp_path).unwrap();
        let gz = GzEncoder::new(file, Compression::default());
        let mut builder = tar::Builder::new(gz);

        for (name, data) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, name, &data[..])
                .unwrap();
        }

        builder.finish().unwrap();
        napp_path
    }

    #[test]
    fn test_read_napp_entry() {
        let tmp = tempfile::TempDir::new().unwrap();
        let manifest = br#"{"id":"test","name":"Test","version":"1.0.0","artifact_type":"skill"}"#;
        let skill_md = b"---\nname: test\ndescription: A test\n---\nBody content";

        let napp = create_test_napp(tmp.path(), &[
            ("manifest.json", manifest),
            ("SKILL.md", skill_md),
        ]);

        let result = read_napp_entry(&napp, "manifest.json").unwrap();
        assert_eq!(result, manifest);

        let result = read_napp_entry(&napp, "SKILL.md").unwrap();
        assert_eq!(result, skill_md);
    }

    #[test]
    fn test_read_napp_entry_not_found() {
        let tmp = tempfile::TempDir::new().unwrap();
        let napp = create_test_napp(tmp.path(), &[
            ("manifest.json", b"{}"),
        ]);

        let result = read_napp_entry(&napp, "nonexistent.txt");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), NappError::NotFound(_)));
    }

    #[test]
    fn test_read_napp_entry_string() {
        let tmp = tempfile::TempDir::new().unwrap();
        let content = "Hello, world!";
        let napp = create_test_napp(tmp.path(), &[
            ("SKILL.md", content.as_bytes()),
        ]);

        let result = read_napp_entry_string(&napp, "SKILL.md").unwrap();
        assert_eq!(result, content);
    }

    #[test]
    fn test_list_napp_entries() {
        let tmp = tempfile::TempDir::new().unwrap();
        let napp = create_test_napp(tmp.path(), &[
            ("manifest.json", b"{}"),
            ("SKILL.md", b"content"),
            ("signatures.json", b"{}"),
        ]);

        let entries = list_napp_entries(&napp).unwrap();
        assert_eq!(entries.len(), 3);
        assert!(entries.contains(&"manifest.json".to_string()));
        assert!(entries.contains(&"SKILL.md".to_string()));
        assert!(entries.contains(&"signatures.json".to_string()));
    }

    #[test]
    fn test_extract_napp_entry() {
        let tmp = tempfile::TempDir::new().unwrap();
        let data = b"binary content here";
        let napp = create_test_napp(tmp.path(), &[
            ("binary", data),
        ]);

        let dest = tmp.path().join("extracted").join("binary");
        extract_napp_entry(&napp, "binary", &dest).unwrap();

        assert_eq!(std::fs::read(&dest).unwrap(), data);
    }

    #[test]
    fn test_extract_napp_prefix() {
        let tmp = tempfile::TempDir::new().unwrap();
        let napp = create_test_napp(tmp.path(), &[
            ("manifest.json", b"{}"),
            ("ui/index.html", b"<html></html>"),
            ("ui/style.css", b"body {}"),
        ]);

        let dest_dir = tmp.path().join("extracted");
        let extracted = extract_napp_prefix(&napp, "ui/", &dest_dir).unwrap();

        assert_eq!(extracted.len(), 2);
        assert!(dest_dir.join("ui/index.html").exists());
        assert!(dest_dir.join("ui/style.css").exists());
    }

    #[test]
    fn test_read_with_dot_slash_prefix() {
        let tmp = tempfile::TempDir::new().unwrap();
        // Create archive where entries have ./ prefix (common with some tar tools)
        let napp = create_test_napp(tmp.path(), &[("./SKILL.md", b"test content")]);

        // Should find it even when querying without ./
        let result = read_napp_entry(&napp, "SKILL.md").unwrap();
        assert_eq!(result, b"test content");
    }

    #[test]
    fn test_extract_all() {
        let tmp = tempfile::TempDir::new().unwrap();
        let napp = create_test_napp(tmp.path(), &[
            ("manifest.json", b"{}"),
            ("SKILL.md", b"---\nname: test\n---\nbody"),
            ("scripts/run.py", b"print('hello')"),
        ]);

        let dest = tmp.path().join("extracted");
        let entries = extract_all(&napp, &dest).unwrap();

        assert_eq!(entries.len(), 3);
        assert!(dest.join("manifest.json").exists());
        assert!(dest.join("SKILL.md").exists());
        assert!(dest.join("scripts/run.py").exists());
        assert_eq!(
            std::fs::read_to_string(dest.join("scripts/run.py")).unwrap(),
            "print('hello')"
        );
    }

    #[test]
    fn test_extract_napp_alongside() {
        let tmp = tempfile::TempDir::new().unwrap();
        let skill_dir = tmp.path().join("@acme").join("skills").join("web");
        std::fs::create_dir_all(&skill_dir).unwrap();

        let napp_path = skill_dir.join("1.0.0.napp");
        // Create archive at specific path using the test helper
        let file = std::fs::File::create(&napp_path).unwrap();
        let gz = GzEncoder::new(file, Compression::default());
        let mut builder = tar::Builder::new(gz);
        let data = b"{}";
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, "manifest.json", &data[..]).unwrap();
        // Properly finalize: into_inner flushes the tar, then finish() the gz
        builder.into_inner().unwrap().finish().unwrap();

        let dest = extract_napp_alongside(&napp_path).unwrap();
        assert_eq!(dest, skill_dir.join("1.0.0"));
        assert!(dest.join("manifest.json").exists());

        // Idempotent — second call returns same path without error
        let dest2 = extract_napp_alongside(&napp_path).unwrap();
        assert_eq!(dest, dest2);
    }

    #[test]
    fn test_walk_for_marker() {
        let tmp = tempfile::TempDir::new().unwrap();
        let base = tmp.path();

        // Create directory tree with SKILL.md markers
        let skill_a = base.join("@acme").join("skills").join("web").join("1.0.0");
        std::fs::create_dir_all(&skill_a).unwrap();
        std::fs::write(skill_a.join("SKILL.md"), "a").unwrap();

        let skill_b = base.join("@acme").join("skills").join("data").join("2.0.0");
        std::fs::create_dir_all(&skill_b).unwrap();
        std::fs::write(skill_b.join("SKILL.md"), "b").unwrap();

        // Non-skill directory (no marker)
        let other = base.join("@acme").join("tools").join("crm");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("manifest.json"), "{}").unwrap();

        let mut found = Vec::new();
        walk_for_marker(base, "SKILL.md", &mut |path| {
            found.push(path.to_path_buf());
        });

        assert_eq!(found.len(), 2);
        assert!(found.contains(&skill_a));
        assert!(found.contains(&skill_b));
    }
}
