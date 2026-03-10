//! Version resolution for .napp archives.
//!
//! Scans installed .napp files in a qualified name directory, parses versions
//! from filenames, and resolves semver ranges to the highest matching version.

use std::path::{Path, PathBuf};

use crate::NappError;

/// Resolve the best matching .napp file for a qualified name and semver range.
///
/// Scans the directory at `base_dir/qualified_name/` for `.napp` files,
/// parses versions from filenames (e.g., `1.2.0.napp`), and returns the path
/// to the highest version satisfying the given semver range.
///
/// If `range` is empty or "*", returns the highest available version.
///
/// # Example
/// ```ignore
/// let path = resolve_version(
///     &nebo_dir.join("skills"),
///     "@acme/skills/sales-qualification",
///     "^1.0.0",
/// )?;
/// ```
pub fn resolve_version(
    base_dir: &Path,
    qualified_name: &str,
    range: &str,
) -> Result<PathBuf, NappError> {
    let dir = base_dir.join(qualified_name);

    if !dir.exists() {
        return Err(NappError::NotFound(format!(
            "no versions found for {} (directory does not exist: {})",
            qualified_name,
            dir.display()
        )));
    }

    let req = if range.is_empty() || range == "*" {
        None
    } else {
        Some(
            semver::VersionReq::parse(range)
                .map_err(|e| NappError::Other(format!("invalid semver range '{}': {}", range, e)))?,
        )
    };

    let mut best: Option<(semver::Version, PathBuf)> = None;

    let entries = std::fs::read_dir(&dir)
        .map_err(|e| NappError::Io(e))?;

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let file_name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };

        // Must end with .napp
        let version_str = match file_name.strip_suffix(".napp") {
            Some(v) => v,
            None => continue,
        };

        let version = match semver::Version::parse(version_str) {
            Ok(v) => v,
            Err(_) => continue, // skip files that aren't valid semver
        };

        // Check against range if specified
        if let Some(ref req) = req {
            if !req.matches(&version) {
                continue;
            }
        }

        // Keep the highest matching version
        match &best {
            Some((current_best, _)) if &version <= current_best => {}
            _ => {
                best = Some((version, path));
            }
        }
    }

    match best {
        Some((_, path)) => Ok(path),
        None => Err(NappError::NotFound(format!(
            "no version of {} matches range '{}'",
            qualified_name, range
        ))),
    }
}

/// List all installed versions for a qualified name, sorted newest first.
pub fn list_versions(base_dir: &Path, qualified_name: &str) -> Result<Vec<(semver::Version, PathBuf)>, NappError> {
    let dir = base_dir.join(qualified_name);

    if !dir.exists() {
        return Ok(Vec::new());
    }

    let entries = std::fs::read_dir(&dir)
        .map_err(|e| NappError::Io(e))?;

    let mut versions: Vec<(semver::Version, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_file() {
                return None;
            }
            let name = path.file_name()?.to_str()?;
            let ver_str = name.strip_suffix(".napp")?;
            let version = semver::Version::parse(ver_str).ok()?;
            Some((version, path))
        })
        .collect();

    versions.sort_by(|a, b| b.0.cmp(&a.0)); // newest first
    Ok(versions)
}

/// Get the latest installed version for a qualified name.
pub fn latest_version(base_dir: &Path, qualified_name: &str) -> Result<PathBuf, NappError> {
    resolve_version(base_dir, qualified_name, "*")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_napp_file(dir: &Path, version: &str) {
        let path = dir.join(format!("{}.napp", version));
        std::fs::write(&path, "fake napp content").unwrap();
    }

    #[test]
    fn test_resolve_latest() {
        let tmp = tempfile::TempDir::new().unwrap();
        let skill_dir = tmp.path().join("@acme/skills/test");
        std::fs::create_dir_all(&skill_dir).unwrap();

        create_napp_file(&skill_dir, "1.0.0");
        create_napp_file(&skill_dir, "1.1.0");
        create_napp_file(&skill_dir, "2.0.0");

        let result = resolve_version(
            tmp.path(),
            "@acme/skills/test",
            "*",
        ).unwrap();

        assert!(result.ends_with("2.0.0.napp"));
    }

    #[test]
    fn test_resolve_caret_range() {
        let tmp = tempfile::TempDir::new().unwrap();
        let skill_dir = tmp.path().join("@acme/skills/test");
        std::fs::create_dir_all(&skill_dir).unwrap();

        create_napp_file(&skill_dir, "1.0.0");
        create_napp_file(&skill_dir, "1.2.0");
        create_napp_file(&skill_dir, "1.5.0");
        create_napp_file(&skill_dir, "2.0.0");

        let result = resolve_version(
            tmp.path(),
            "@acme/skills/test",
            "^1.0.0",
        ).unwrap();

        // ^1.0.0 matches >=1.0.0, <2.0.0
        assert!(result.ends_with("1.5.0.napp"));
    }

    #[test]
    fn test_resolve_tilde_range() {
        let tmp = tempfile::TempDir::new().unwrap();
        let skill_dir = tmp.path().join("@acme/skills/test");
        std::fs::create_dir_all(&skill_dir).unwrap();

        create_napp_file(&skill_dir, "1.0.0");
        create_napp_file(&skill_dir, "1.0.5");
        create_napp_file(&skill_dir, "1.1.0");

        let result = resolve_version(
            tmp.path(),
            "@acme/skills/test",
            "~1.0.0",
        ).unwrap();

        // ~1.0.0 matches >=1.0.0, <1.1.0
        assert!(result.ends_with("1.0.5.napp"));
    }

    #[test]
    fn test_resolve_exact_version() {
        let tmp = tempfile::TempDir::new().unwrap();
        let skill_dir = tmp.path().join("@acme/skills/test");
        std::fs::create_dir_all(&skill_dir).unwrap();

        create_napp_file(&skill_dir, "1.0.0");
        create_napp_file(&skill_dir, "1.1.0");

        let result = resolve_version(
            tmp.path(),
            "@acme/skills/test",
            "=1.0.0",
        ).unwrap();

        assert!(result.ends_with("1.0.0.napp"));
    }

    #[test]
    fn test_resolve_no_match() {
        let tmp = tempfile::TempDir::new().unwrap();
        let skill_dir = tmp.path().join("@acme/skills/test");
        std::fs::create_dir_all(&skill_dir).unwrap();

        create_napp_file(&skill_dir, "1.0.0");

        let result = resolve_version(
            tmp.path(),
            "@acme/skills/test",
            "^2.0.0",
        );

        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_missing_directory() {
        let tmp = tempfile::TempDir::new().unwrap();

        let result = resolve_version(
            tmp.path(),
            "@acme/skills/nonexistent",
            "*",
        );

        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_empty_range() {
        let tmp = tempfile::TempDir::new().unwrap();
        let skill_dir = tmp.path().join("@acme/skills/test");
        std::fs::create_dir_all(&skill_dir).unwrap();

        create_napp_file(&skill_dir, "1.0.0");
        create_napp_file(&skill_dir, "2.0.0");

        // Empty range should return latest
        let result = resolve_version(
            tmp.path(),
            "@acme/skills/test",
            "",
        ).unwrap();

        assert!(result.ends_with("2.0.0.napp"));
    }

    #[test]
    fn test_list_versions() {
        let tmp = tempfile::TempDir::new().unwrap();
        let skill_dir = tmp.path().join("@acme/skills/test");
        std::fs::create_dir_all(&skill_dir).unwrap();

        create_napp_file(&skill_dir, "1.0.0");
        create_napp_file(&skill_dir, "2.0.0");
        create_napp_file(&skill_dir, "1.5.0");

        let versions = list_versions(tmp.path(), "@acme/skills/test").unwrap();
        assert_eq!(versions.len(), 3);
        // Should be sorted newest first
        assert_eq!(versions[0].0, semver::Version::new(2, 0, 0));
        assert_eq!(versions[1].0, semver::Version::new(1, 5, 0));
        assert_eq!(versions[2].0, semver::Version::new(1, 0, 0));
    }

    #[test]
    fn test_list_versions_empty() {
        let tmp = tempfile::TempDir::new().unwrap();
        let versions = list_versions(tmp.path(), "@acme/skills/nonexistent").unwrap();
        assert!(versions.is_empty());
    }

    #[test]
    fn test_ignores_non_napp_files() {
        let tmp = tempfile::TempDir::new().unwrap();
        let skill_dir = tmp.path().join("@acme/skills/test");
        std::fs::create_dir_all(&skill_dir).unwrap();

        create_napp_file(&skill_dir, "1.0.0");
        std::fs::write(skill_dir.join("README.md"), "readme").unwrap();
        std::fs::write(skill_dir.join("not-semver.napp"), "bad").unwrap();

        let versions = list_versions(tmp.path(), "@acme/skills/test").unwrap();
        assert_eq!(versions.len(), 1);
    }
}
