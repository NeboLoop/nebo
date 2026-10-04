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
    read_entry_from_targz(napp_payload_targz(napp_path)?, entry_name, napp_path)
}

/// Read a single entry from a .napp archive and return it as a UTF-8 string.
pub fn read_napp_entry_string(napp_path: &Path, entry_name: &str) -> Result<String, NappError> {
    let bytes = read_napp_entry(napp_path, entry_name)?;
    String::from_utf8(bytes)
        .map_err(|e| NappError::Extraction(format!("invalid UTF-8 in {}: {}", entry_name, e)))
}

/// List all entry names in a .napp (tar.gz) archive.
pub fn list_napp_entries(napp_path: &Path) -> Result<Vec<String>, NappError> {
    list_entries_from_targz(napp_payload_targz(napp_path)?)
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
    let gz = GzDecoder::new(napp_payload_targz(napp_path)?);
    let mut archive = Archive::new(gz);

    let target_prefix = prefix.trim_start_matches("./");
    let mut extracted = Vec::new();

    for entry_result in archive
        .entries()
        .map_err(|e| NappError::Extraction(e.to_string()))?
    {
        let mut entry = entry_result.map_err(|e| NappError::Extraction(e.to_string()))?;
        let path = entry
            .path()
            .map_err(|e| NappError::Extraction(e.to_string()))?;
        let normalized = path.to_string_lossy().trim_start_matches("./").to_string();

        if normalized.starts_with(target_prefix) {
            let dest_path = dest_dir.join(&normalized);
            if let Some(parent) = dest_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            write_entry(&mut entry, &dest_path)?;
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
    let gz = GzDecoder::new(napp_payload_targz(napp_path)?);
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
        let normalized = path.to_string_lossy().trim_start_matches("./").to_string();

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

        // Capture the archived mode before the mutable read borrow — a binary
        // packaged as rwxr-xr-x must stay executable after extraction.
        #[cfg(unix)]
        let entry_mode = entry.header().mode().unwrap_or(0o644);

        write_entry(&mut entry, &dest_path)?;

        // Restore the executable bit when the archived entry was executable, or
        // for conventionally-named binary/script entries. `std::fs::write`
        // creates 0644, which would otherwise strip the +x off a packaged
        // binary named after its slug (e.g. "example-ops").
        #[cfg(unix)]
        if entry_mode & 0o111 != 0 || is_executable_entry(&normalized) {
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
///
/// Single-pass: one `read_dir()` per directory (checks for marker in the same
/// listing used to discover subdirectories).
pub fn walk_for_marker(dir: &Path, marker_file: &str, cb: &mut dyn FnMut(&Path)) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let entries: Vec<_> = entries.flatten().collect();

    // Check if marker exists in THIS directory's entries (single pass)
    let has_marker = entries
        .iter()
        .any(|e| e.file_name().eq_ignore_ascii_case(marker_file));

    if has_marker {
        cb(dir);
        return; // Don't recurse into children of a marker dir
    }

    // Recurse into subdirectories
    for entry in &entries {
        if entry.path().is_dir() {
            walk_for_marker(&entry.path(), marker_file, cb);
        }
    }
}

// ── Sealed .napp readers ─────────────────────────────────────────────────

/// Read a single entry from a sealed .napp file by name.
///
/// Decrypts the .napp envelope in memory, reads the entry from the inner
/// tar.gz, and returns the content. Plaintext never touches disk.
pub fn read_sealed_napp_entry(
    napp_path: &Path,
    entry_name: &str,
    license_key: &[u8; 32],
) -> Result<Vec<u8>, NappError> {
    let targz = unseal_napp_to_targz(napp_path, license_key)?;
    read_entry_from_targz(targz.as_slice(), entry_name, napp_path)
}

/// Read a single entry from a sealed .napp file as a UTF-8 string.
pub fn read_sealed_napp_entry_string(
    napp_path: &Path,
    entry_name: &str,
    license_key: &[u8; 32],
) -> Result<String, NappError> {
    let bytes = read_sealed_napp_entry(napp_path, entry_name, license_key)?;
    String::from_utf8(bytes)
        .map_err(|e| NappError::Extraction(format!("invalid UTF-8 in {}: {}", entry_name, e)))
}

/// List all entry names in a sealed .napp file.
pub fn list_sealed_napp_entries(
    napp_path: &Path,
    license_key: &[u8; 32],
) -> Result<Vec<String>, NappError> {
    let targz = unseal_napp_to_targz(napp_path, license_key)?;
    list_entries_from_targz(targz.as_slice())
}

/// Partially extract a sealed .napp — executables + metadata only, IP stays sealed.
///
/// Extracts `scripts/*`, `bin/*`, root `binary`, `manifest.json`, `plugin.json`,
/// and `signatures.json` to a sibling directory. SKILL.md, references/, assets/
/// stay inside the sealed .napp and are read in memory at runtime.
///
/// Returns the extraction directory path, or None if nothing was extracted.
pub fn partial_extract_sealed_napp(
    napp_path: &Path,
    license_key: &[u8; 32],
) -> Result<Option<PathBuf>, NappError> {
    let targz = unseal_napp_to_targz(napp_path, license_key)?;
    let dest_dir = napp_path.with_extension("");

    let gz = GzDecoder::new(targz.as_slice());
    let mut archive = Archive::new(gz);
    let mut extracted_any = false;

    for entry_result in archive
        .entries()
        .map_err(|e| NappError::Extraction(e.to_string()))?
    {
        let mut entry = entry_result.map_err(|e| NappError::Extraction(e.to_string()))?;
        let path = entry
            .path()
            .map_err(|e| NappError::Extraction(e.to_string()))?;
        let normalized = path.to_string_lossy().trim_start_matches("./").to_string();

        if normalized.is_empty() || entry.header().entry_type().is_dir() {
            continue;
        }

        // Only extract executables and metadata
        if !is_partial_extract_entry(&normalized) {
            continue;
        }

        let dest_path = dest_dir.join(&normalized);
        if let Some(parent) = dest_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut content = Vec::new();
        entry
            .read_to_end(&mut content)
            .map_err(|e| NappError::Extraction(e.to_string()))?;
        std::fs::write(&dest_path, &content)?;

        // Set +x only on actual executables, not metadata files
        #[cfg(unix)]
        if is_executable_entry(&normalized) {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dest_path, std::fs::Permissions::from_mode(0o755))?;
        }

        extracted_any = true;
    }

    if extracted_any {
        Ok(Some(dest_dir))
    } else {
        Ok(None)
    }
}

// ── Internal helpers ─────────────────────────────────────────────────────

/// The inner tar.gz of a `.napp` on disk, as a stream over the file.
///
/// Marketplace `.napp` files downloaded from NeboAI are wrapped in a signed
/// `NAPP` envelope (101-byte header: magic + version + ED25519 sig + SHA256);
/// bundled/legacy `.napp` files are raw tar.gz. This verifies the envelope
/// when present (reading the file a chunk at a time, never whole) and returns
/// the file positioned at the inner tar.gz either way.
///
/// Sealed (paid) payloads are encrypted and cannot be extracted to disk without
/// a license key — those are read in memory by the loaders via the sealed
/// readers below, so this returns an error rather than writing ciphertext.
fn napp_payload_targz(napp_path: &Path) -> Result<std::fs::File, NappError> {
    let mut file = std::fs::File::open(napp_path)?;
    // Raw tar.gz (no envelope) — bundled/legacy artifacts.
    if peek_head(&mut file)? == [0x1f, 0x8b] {
        return Ok(file);
    }
    // Signed NAPP envelope — verify signature + hash, then unwrap.
    let mut payload = crate::napp::open_napp_payload(napp_path)?;
    if crate::sealed::is_sealed(&peek_head(&mut payload)?) {
        return Err(NappError::Extraction(
            "sealed .napp requires a license key and cannot be extracted to disk".into(),
        ));
    }
    Ok(payload)
}

/// The first two bytes `file` reads from here on (fewer at its end); the
/// file is left where it was.
fn peek_head(file: &mut std::fs::File) -> Result<Vec<u8>, NappError> {
    use std::io::{Seek, SeekFrom};
    let at = file.stream_position()?;
    let mut head = Vec::with_capacity(2);
    Read::by_ref(file).take(2).read_to_end(&mut head)?;
    file.seek(SeekFrom::Start(at))?;
    Ok(head)
}

/// Copy one archive entry to `dest` as it decompresses: a 100 MB model is
/// never held whole.
fn write_entry(entry: &mut impl Read, dest: &Path) -> Result<(), NappError> {
    let mut out = std::fs::File::create(dest)?;
    std::io::copy(entry, &mut out).map_err(|e| NappError::Extraction(e.to_string()))?;
    Ok(())
}

/// Returns `true` if a `.napp` file wraps a sealed (encrypted) payload.
///
/// Verifies the `NAPP` envelope (streaming the file), then checks whether
/// the inner payload is encrypted (sealed/paid content) rather than a plain
/// tar.gz. Returns `false` for raw tar.gz files and for any file whose envelope
/// cannot be read or verified — callers treat those as plain and let extraction
/// surface the underlying error.
pub fn is_sealed_napp(napp_path: &Path) -> bool {
    let Ok(mut file) = std::fs::File::open(napp_path) else {
        return false;
    };
    if peek_head(&mut file).is_ok_and(|h| h == [0x1f, 0x8b]) {
        return false; // raw tar.gz — not sealed
    }
    match crate::napp::open_napp_payload(napp_path) {
        Ok(mut payload) => peek_head(&mut payload).is_ok_and(|h| crate::sealed::is_sealed(&h)),
        Err(_) => false,
    }
}

/// Unseal a .napp file: verify envelope → decrypt → return plain tar.gz bytes.
fn unseal_napp_to_targz(napp_path: &Path, license_key: &[u8; 32]) -> Result<Vec<u8>, NappError> {
    let mut sealed_payload = Vec::new();
    crate::napp::open_napp_payload(napp_path)?.read_to_end(&mut sealed_payload)?;
    crate::sealed::unseal_payload(&sealed_payload, license_key)
}

/// Read a single entry from a tar.gz stream.
fn read_entry_from_targz(
    targz: impl Read,
    entry_name: &str,
    source: &Path,
) -> Result<Vec<u8>, NappError> {
    let gz = GzDecoder::new(targz);
    let mut archive = Archive::new(gz);
    let target = entry_name.trim_start_matches("./");

    for entry_result in archive
        .entries()
        .map_err(|e| NappError::Extraction(e.to_string()))?
    {
        let mut entry = entry_result.map_err(|e| NappError::Extraction(e.to_string()))?;
        let path = entry
            .path()
            .map_err(|e| NappError::Extraction(e.to_string()))?;
        let normalized = path.to_string_lossy().trim_start_matches("./").to_string();

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
        source.display()
    )))
}

/// List all entry names from a tar.gz stream.
fn list_entries_from_targz(targz: impl Read) -> Result<Vec<String>, NappError> {
    let gz = GzDecoder::new(targz);
    let mut archive = Archive::new(gz);
    let mut entries = Vec::new();

    for entry_result in archive
        .entries()
        .map_err(|e| NappError::Extraction(e.to_string()))?
    {
        let entry = entry_result.map_err(|e| NappError::Extraction(e.to_string()))?;
        let path = entry
            .path()
            .map_err(|e| NappError::Extraction(e.to_string()))?;
        let normalized = path.to_string_lossy().trim_start_matches("./").to_string();
        if !normalized.is_empty() {
            entries.push(normalized);
        }
    }

    Ok(entries)
}

/// Check if a tar entry name is an executable (needs +x permission).
fn is_executable_entry(name: &str) -> bool {
    name == "binary" || name == "app" || name.starts_with("scripts/") || name.starts_with("bin/")
}

/// Check if a tar entry should be extracted during partial extraction.
/// Includes executables (not readable IP) and metadata (needed for discovery).
fn is_partial_extract_entry(name: &str) -> bool {
    is_executable_entry(name)
        // Metadata — needed for discovery and artifact_id lookup
        || name == "manifest.json"
        || name == "plugin.json"
        || name == "signatures.json"
}

/// Read plugin slug + version from a `plugin.json` entry inside a tar.gz payload.
///
/// Used during bundled plugin seeding to identify the plugin before extraction.
pub fn read_plugin_identity_from_tar_gz(payload: &[u8]) -> Result<(String, String), NappError> {
    let gz = GzDecoder::new(payload);
    let mut archive = Archive::new(gz);

    for entry_result in archive
        .entries()
        .map_err(|e| NappError::Extraction(e.to_string()))?
    {
        let mut entry = entry_result.map_err(|e| NappError::Extraction(e.to_string()))?;
        let path = entry
            .path()
            .map_err(|e| NappError::Extraction(e.to_string()))?
            .to_path_buf();
        let name = path.to_string_lossy();
        let normalized = name.trim_start_matches("./");

        if normalized == "plugin.json" {
            let mut content = String::new();
            entry
                .read_to_string(&mut content)
                .map_err(|e| NappError::Extraction(e.to_string()))?;

            #[derive(serde::Deserialize)]
            struct PluginIdentity {
                slug: String,
                version: String,
            }

            let id: PluginIdentity = serde_json::from_str(&content)
                .map_err(|e| NappError::Extraction(format!("invalid plugin.json: {}", e)))?;
            return Ok((id.slug, id.version));
        }
    }

    Err(NappError::NotFound(
        "plugin.json not found in .napp archive".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;

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
            builder.append_data(&mut header, name, &data[..]).unwrap();
        }

        builder.finish().unwrap();
        napp_path
    }

    #[test]
    fn test_read_napp_entry() {
        let tmp = tempfile::TempDir::new().unwrap();
        let manifest = br#"{"id":"test","name":"Test","version":"1.0.0","artifact_type":"skill"}"#;
        let skill_md = b"---\nname: test\ndescription: A test\n---\nBody content";

        let napp = create_test_napp(
            tmp.path(),
            &[("manifest.json", manifest), ("SKILL.md", skill_md)],
        );

        let result = read_napp_entry(&napp, "manifest.json").unwrap();
        assert_eq!(result, manifest);

        let result = read_napp_entry(&napp, "SKILL.md").unwrap();
        assert_eq!(result, skill_md);
    }

    #[test]
    fn test_read_napp_entry_not_found() {
        let tmp = tempfile::TempDir::new().unwrap();
        let napp = create_test_napp(tmp.path(), &[("manifest.json", b"{}")]);

        let result = read_napp_entry(&napp, "nonexistent.txt");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), NappError::NotFound(_)));
    }

    #[test]
    fn test_read_napp_entry_string() {
        let tmp = tempfile::TempDir::new().unwrap();
        let content = "Hello, world!";
        let napp = create_test_napp(tmp.path(), &[("SKILL.md", content.as_bytes())]);

        let result = read_napp_entry_string(&napp, "SKILL.md").unwrap();
        assert_eq!(result, content);
    }

    #[test]
    fn test_list_napp_entries() {
        let tmp = tempfile::TempDir::new().unwrap();
        let napp = create_test_napp(
            tmp.path(),
            &[
                ("manifest.json", b"{}"),
                ("SKILL.md", b"content"),
                ("signatures.json", b"{}"),
            ],
        );

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
        let napp = create_test_napp(tmp.path(), &[("binary", data)]);

        let dest = tmp.path().join("extracted").join("binary");
        extract_napp_entry(&napp, "binary", &dest).unwrap();

        assert_eq!(std::fs::read(&dest).unwrap(), data);
    }

    #[test]
    fn test_extract_napp_prefix() {
        let tmp = tempfile::TempDir::new().unwrap();
        let napp = create_test_napp(
            tmp.path(),
            &[
                ("manifest.json", b"{}"),
                ("ui/index.html", b"<html></html>"),
                ("ui/style.css", b"body {}"),
            ],
        );

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
        let napp = create_test_napp(
            tmp.path(),
            &[
                ("manifest.json", b"{}"),
                ("SKILL.md", b"---\nname: test\n---\nbody"),
                ("scripts/run.py", b"print('hello')"),
            ],
        );

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
        builder
            .append_data(&mut header, "manifest.json", &data[..])
            .unwrap();
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

#[cfg(test)]
mod signed_napp_tests {
    use super::*;
    use crate::test_signing::{sealed_napp, signed_napp};

    /// Bytes that do not compress away, `len` long.
    fn noise(len: usize) -> Vec<u8> {
        let mut x: u32 = 0x9e37_79b9;
        (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x as u8
            })
            .collect()
    }

    // A signed app package many verify chunks long is checked and extracted
    // from the file on disk, every byte where it belongs.
    #[test]
    fn a_large_signed_package_extracts_from_the_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let model = noise(3 * 1024 * 1024 + 17);
        let manifest = br#"{"id":"game","name":"Game","version":"1.0.0"}"#;
        let napp_path = tmp.path().join("1.0.0.napp");
        std::fs::write(
            &napp_path,
            signed_napp(&[("manifest.json", manifest), ("ui/index.html", b"<html></html>"), ("ui/models/world.glb", &model)]),
        )
        .unwrap();

        assert!(!is_sealed_napp(&napp_path));
        let dest = extract_napp_alongside(&napp_path).unwrap();
        assert_eq!(std::fs::read(dest.join("ui/models/world.glb")).unwrap(), model);
        assert_eq!(std::fs::read(dest.join("manifest.json")).unwrap(), manifest);
        assert_eq!(read_napp_entry(&napp_path, "ui/index.html").unwrap(), b"<html></html>");
        assert_eq!(list_napp_entries(&napp_path).unwrap().len(), 3);
    }

    // One flipped byte anywhere in the payload, or a signature from another
    // key, and nothing is extracted.
    #[test]
    fn a_tampered_or_foreign_package_extracts_nothing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let good = signed_napp(&[("manifest.json", b"{}"), ("ui/a.bin", &noise(600 * 1024))]);

        let mut flipped = good.clone();
        let at = flipped.len() - 1000;
        flipped[at] ^= 0x01;
        let napp_path = tmp.path().join("flipped.napp");
        std::fs::write(&napp_path, &flipped).unwrap();
        let err = extract_all(&napp_path, &tmp.path().join("out1")).unwrap_err();
        assert!(err.to_string().contains("hash mismatch"), "{err}");
        assert!(!tmp.path().join("out1").exists());

        // Re-hashed but not re-signed: the hash holds, the signature does not.
        use sha2::Digest;
        let mut forged = flipped.clone();
        let hash = sha2::Sha256::digest(&forged[crate::napp::NAPP_HEADER_SIZE..]);
        forged[69..101].copy_from_slice(&hash);
        let napp_path = tmp.path().join("forged.napp");
        std::fs::write(&napp_path, &forged).unwrap();
        let err = extract_all(&napp_path, &tmp.path().join("out2")).unwrap_err();
        assert!(err.to_string().contains("signature verification failed"), "{err}");

        let napp_path = tmp.path().join("short.napp");
        std::fs::write(&napp_path, &good[..50]).unwrap();
        let err = extract_all(&napp_path, &tmp.path().join("out3")).unwrap_err();
        assert!(err.to_string().contains("too small"), "{err}");

        // The in-memory unwrap agrees with the file reader.
        assert!(crate::napp::unwrap_napp_builtin(&good).is_ok());
        assert!(crate::napp::unwrap_napp_builtin(&flipped).is_err());
        assert!(crate::napp::unwrap_napp_builtin(&forged).is_err());
    }

    // A sealed package is still told apart and still refused extraction.
    #[test]
    fn a_sealed_package_stays_sealed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let key = [3u8; 32];
        let napp_path = tmp.path().join("paid.napp");
        std::fs::write(&napp_path, sealed_napp(&[("manifest.json", b"{}"), ("SKILL.md", b"secret")], &key)).unwrap();

        assert!(is_sealed_napp(&napp_path));
        let err = extract_all(&napp_path, &tmp.path().join("out")).unwrap_err();
        assert!(err.to_string().contains("sealed .napp requires a license key"), "{err}");
        assert_eq!(read_sealed_napp_entry(&napp_path, "SKILL.md", &key).unwrap(), b"secret");
    }
}
