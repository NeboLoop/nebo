//! Which folder a code change belongs to: the project around a path.
//!
//! A project is the nearest folder, at or above the path, that is a git
//! repository's root or holds a project manifest ([`MANIFESTS`]). The walk
//! never climbs to the home folder or the filesystem root, and never into
//! Nebo's own data folder: a document in the owner's files is not code, and
//! a folder of documents never becomes a project because a parent holds a
//! stray `package.json`.

use std::path::{Path, PathBuf};

/// The files that make a folder a code project.
pub const MANIFESTS: &[&str] = &[
    "Cargo.toml",
    "package.json",
    "go.mod",
    "pyproject.toml",
    "setup.py",
    "requirements.txt",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "Gemfile",
    "composer.json",
    "deno.json",
    "pubspec.yaml",
    "Package.swift",
    "Makefile",
    "CMakeLists.txt",
];

/// The project `path` belongs to, if any. `stops` are folders the walk never
/// reaches (the home folder; Nebo's data folder is always one).
pub fn project_root(path: &Path, stops: &[PathBuf]) -> Option<PathBuf> {
    let path = std::path::absolute(path).ok()?;
    let data = config::data_dir().ok();
    let home = dirs::home_dir();
    let mut dir = if path.is_dir() {
        path.clone()
    } else {
        path.parent()?.to_path_buf()
    };
    loop {
        if dir.parent().is_none()
            || home.as_deref() == Some(dir.as_path())
            || stops.iter().any(|s| s == &dir)
            || data.as_deref().is_some_and(|d| dir.starts_with(d))
        {
            return None;
        }
        if dir.join(".git").exists() || MANIFESTS.iter().any(|m| dir.join(m).is_file()) {
            // A manifest inside a repository is one of its packages: the
            // repository is the project.
            return Some(repository_above(&dir, home.as_deref()).unwrap_or(dir));
        }
        dir = dir.parent()?.to_path_buf();
    }
}

/// The repository root at or above `dir`, below `home`.
fn repository_above(dir: &Path, home: Option<&Path>) -> Option<PathBuf> {
    let mut at = Some(dir);
    while let Some(d) = at {
        if home == Some(d) {
            return None;
        }
        if d.join(".git").exists() {
            return Some(d.to_path_buf());
        }
        at = d.parent();
    }
    None
}

/// Whether `path` is inside `folder`.
pub fn inside(path: &Path, folder: &Path) -> bool {
    std::path::absolute(path).is_ok_and(|p| p.starts_with(folder))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_manifest_or_a_repository_makes_a_project_and_a_document_folder_does_not() {
        let root = tempfile::tempdir().unwrap();
        let stops = vec![root.path().to_path_buf()];
        let site = root.path().join("site");
        std::fs::create_dir_all(site.join("src")).unwrap();
        std::fs::write(site.join("package.json"), "{}").unwrap();
        assert_eq!(
            project_root(&site.join("src/app.ts"), &stops),
            Some(site.clone())
        );

        let docs = root.path().join("Clients");
        std::fs::create_dir_all(&docs).unwrap();
        assert_eq!(
            project_root(&docs.join("letter.docx"), &stops),
            None,
            "documents are not a project"
        );

        let repo = root.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(repo.join("crates/a")).unwrap();
        std::fs::write(repo.join("crates/a/Cargo.toml"), "").unwrap();
        assert_eq!(
            project_root(&repo.join("crates/a/lib.rs"), &stops),
            Some(repo.clone()),
            "a package's project is its repository"
        );
    }
}
