//! Which folder a code change belongs to: the project around a path.
//!
//! A write is code work when its path is in a code folder ([`code_folder`]):
//! - an app's own pages (the folders the caller passes in), wherever they
//!   live, Nebo's data folder included;
//! - one of Nebo's own working copies (`<copies>/worktrees/<project>/<id>`:
//!   a helper's, or the verify loop's), which is the project it copies;
//! - a project: the nearest folder at or above the path that is a git
//!   repository's root or holds a project manifest ([`MANIFESTS`]); a
//!   manifest inside a repository is one of its packages, and the
//!   repository is the project.
//!
//! Anything else is not code: the walk never climbs to the home folder or
//! the filesystem root, and never into Nebo's own data folder (outside its
//! working copies). A letter in the owner's files, an email draft or a
//! research note is never code work, and a folder of documents never becomes
//! a project because a parent holds a stray `package.json`.

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

/// What kind of code folder a path is in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Folder {
    /// The owner's project.
    Project(PathBuf),
    /// An app's own pages.
    AppPages(PathBuf),
    /// One of Nebo's working copies.
    Copy(PathBuf),
}

impl Folder {
    pub fn path(&self) -> &Path {
        match self {
            Folder::Project(p) | Folder::AppPages(p) | Folder::Copy(p) => p,
        }
    }
}

/// Where Nebo keeps its working copies of projects (`worktree::copies_root`).
pub fn copies_dir() -> Option<PathBuf> {
    crate::worktree::copies_root().ok()
}

/// The code folder `path` is in, if any (see the module docs). `app_pages`
/// are the page folders of the apps the caller may work on.
pub fn code_folder(path: &Path, app_pages: &[PathBuf]) -> Option<Folder> {
    let path = std::path::absolute(path).ok()?;
    if let Some(pages) = app_pages.iter().find(|p| path.starts_with(p)) {
        return Some(Folder::AppPages(pages.clone()));
    }
    if let Some(copies) = copies_dir()
        && let Ok(rest) = path.strip_prefix(&copies)
    {
        // `<project>/<id>/...`: the copy is the two segments below.
        let mut parts = rest.components();
        let (Some(project), Some(id)) = (parts.next(), parts.next()) else {
            return None;
        };
        return Some(Folder::Copy(copies.join(project).join(id)));
    }
    project_root(&path).map(Folder::Project)
}

/// The owner's project `path` belongs to, if any.
pub fn project_root(path: &Path) -> Option<PathBuf> {
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
            || data.as_deref().is_some_and(|d| dir.starts_with(d))
        {
            return None;
        }
        if dir.join(".git").exists() || MANIFESTS.iter().any(|m| dir.join(m).is_file()) {
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
        crate::test_home::with_home(|_| {
            let root = tempfile::tempdir().unwrap();
            let site = root.path().join("site");
            std::fs::create_dir_all(site.join("src")).unwrap();
            std::fs::write(site.join("package.json"), "{}").unwrap();
            assert_eq!(
                code_folder(&site.join("src/app.ts"), &[]),
                Some(Folder::Project(site.clone()))
            );

            let docs = root.path().join("Clients");
            std::fs::create_dir_all(&docs).unwrap();
            assert_eq!(
                code_folder(&docs.join("letter.docx"), &[]),
                None,
                "documents are not a project"
            );

            let repo = root.path().join("repo");
            std::fs::create_dir_all(repo.join(".git")).unwrap();
            std::fs::create_dir_all(repo.join("crates/a")).unwrap();
            std::fs::write(repo.join("crates/a/Cargo.toml"), "").unwrap();
            assert_eq!(
                code_folder(&repo.join("crates/a/lib.rs"), &[]),
                Some(Folder::Project(repo.clone())),
                "a package's project is its repository"
            );
        })
    }

    #[test]
    fn nebos_own_folder_is_never_a_project_except_its_working_copies_and_app_pages() {
        crate::test_home::with_home(|home| {
            // A stray manifest in Nebo's folder makes nothing a project.
            std::fs::write(home.join("package.json"), "{}").unwrap();
            std::fs::create_dir_all(home.join("files")).unwrap();
            assert_eq!(code_folder(&home.join("files/notes.md"), &[]), None);

            // A working copy is the project it copies.
            let copies = copies_dir().unwrap();
            let copy = copies.join("site/v-1");
            std::fs::create_dir_all(copy.join("src")).unwrap();
            assert_eq!(
                code_folder(&copy.join("src/app.ts"), &[]),
                Some(Folder::Copy(copy.clone()))
            );
            assert_eq!(
                code_folder(&copies.join("site"), &[]),
                None,
                "the copies' own folder is no copy"
            );
            // Nebo's own folder holds no copies.
            std::fs::create_dir_all(home.join("worktrees/site/v-1/src")).unwrap();
            assert_eq!(code_folder(&home.join("worktrees/site/v-1/src/app.ts"), &[]), None);

            // An app's pages, wherever they live.
            let pages = home.join("agents/flip/ui");
            std::fs::create_dir_all(&pages).unwrap();
            assert_eq!(
                code_folder(&pages.join("index.html"), std::slice::from_ref(&pages)),
                Some(Folder::AppPages(pages.clone()))
            );
            assert_eq!(
                code_folder(&pages.join("index.html"), &[]),
                None,
                "only the pages of an app the caller may work on"
            );
        })
    }
}
