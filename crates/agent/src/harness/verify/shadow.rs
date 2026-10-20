//! What changed, round by round: a shadow repository over the folder the
//! work happens in.
//!
//! The verify loop needs the whole change (round 1) and each round's delta
//! (the fixes a later round reviews), whatever the folder is: an isolated
//! git worktree, a scratch copy of a folder that is not a repository, an
//! app's page folder, or a linked coding agent's own project. One mechanism
//! serves all of them: a private bare repository under Nebo's data dir whose
//! work tree is that folder. A snapshot is `git add -A` + `git write-tree`
//! into the shadow's own index, so it never touches the folder's own `.git`,
//! its index, its branches or its stash (the house git rules hold by
//! construction), and it works where there is no repository at all.
//!
//! The folder's `.gitignore` files are honoured (build output and installed
//! packages stay out), and [`EXCLUDED`] covers a folder without one. When the
//! folder sits inside a repository, the shadow borrows that repository's
//! objects (`objects/info/alternates`), so a large project is not copied.

use std::path::{Path, PathBuf};

use crate::worktree::git;

/// Left out of every snapshot, on top of the folder's own `.gitignore`:
/// installed packages, build output and caches, rebuilt from their sources.
pub const EXCLUDED: &[&str] = &[
    "node_modules/",
    "target/",
    "dist/",
    "build/",
    ".svelte-kit/",
    ".next/",
    "__pycache__/",
    ".venv/",
    "venv/",
    ".dart_tool/",
    ".DS_Store",
];

/// A tree snapshot of the folder (a git tree id in the shadow).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Snapshot(pub String);

/// How one file changed between two snapshots.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FileChange {
    /// `added`, `modified`, `deleted` or `renamed`.
    pub status: String,
    /// The path relative to the folder, '/'-separated.
    pub path: String,
    pub added_lines: u32,
    pub removed_lines: u32,
}

/// Everything that changed between two snapshots.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChangeSet {
    pub files: Vec<FileChange>,
    /// The unified diff, as `git diff` prints it.
    pub patch: String,
}

impl ChangeSet {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Lines added plus lines removed.
    pub fn lines_changed(&self) -> u32 {
        self.files
            .iter()
            .map(|f| f.added_lines + f.removed_lines)
            .sum()
    }

    /// One line per file: `M src/lib.rs (+4 -1)`.
    pub fn stat(&self) -> String {
        self.files
            .iter()
            .map(|f| {
                let mark = match f.status.as_str() {
                    "added" => "A",
                    "deleted" => "D",
                    "renamed" => "R",
                    _ => "M",
                };
                format!(
                    "{mark} {} (+{} -{})",
                    f.path, f.added_lines, f.removed_lines
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// A shadow repository over one folder.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Shadow {
    /// The shadow's own bare repository.
    pub git_dir: PathBuf,
    /// The folder it snapshots.
    pub work_tree: PathBuf,
}

impl Shadow {
    /// Open (creating when new) the shadow at `git_dir` over `work_tree`.
    pub async fn open(git_dir: &Path, work_tree: &Path) -> Result<Self, String> {
        let shadow = Self {
            git_dir: git_dir.to_path_buf(),
            work_tree: work_tree.to_path_buf(),
        };
        if git_dir.join("HEAD").exists() {
            return Ok(shadow);
        }
        std::fs::create_dir_all(git_dir)
            .map_err(|e| format!("create {}: {e}", git_dir.display()))?;
        let dir = git_dir.to_string_lossy().into_owned();
        git(git_dir, &["init", "-q", "--bare", &dir])
            .await
            .map_err(|e| format!("git init: {e}"))?;
        let info = git_dir.join("info");
        std::fs::create_dir_all(&info).map_err(|e| format!("create {}: {e}", info.display()))?;
        std::fs::write(info.join("exclude"), EXCLUDED.join("\n") + "\n")
            .map_err(|e| format!("write {}: {e}", info.join("exclude").display()))?;
        // Inside a repository: borrow its objects instead of copying them.
        if let Ok(common) = git(
            work_tree,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )
        .await
        {
            let objects = PathBuf::from(common.trim()).join("objects");
            if objects.is_dir() {
                let alternates = git_dir.join("objects").join("info");
                std::fs::create_dir_all(&alternates)
                    .map_err(|e| format!("create {}: {e}", alternates.display()))?;
                std::fs::write(
                    alternates.join("alternates"),
                    format!("{}\n", objects.display()),
                )
                .map_err(|e| format!("write alternates: {e}"))?;
            }
        }
        Ok(shadow)
    }

    async fn run(&self, args: &[&str]) -> Result<String, String> {
        let git_dir = format!("--git-dir={}", self.git_dir.display());
        let work_tree = format!("--work-tree={}", self.work_tree.display());
        let mut all: Vec<&str> = vec![
            &git_dir,
            &work_tree,
            "-c",
            "core.quotepath=off",
            "-c",
            "core.autocrlf=false",
        ];
        all.extend_from_slice(args);
        git(&self.work_tree, &all).await
    }

    /// The folder as it is now.
    pub async fn snapshot(&self) -> Result<Snapshot, String> {
        self.run(&["add", "-A", "--", "."])
            .await
            .map_err(|e| format!("snapshot (add): {e}"))?;
        let tree = self
            .run(&["write-tree"])
            .await
            .map_err(|e| format!("snapshot (write-tree): {e}"))?;
        Ok(Snapshot(tree.trim().to_string()))
    }

    /// What changed from `from` to `to`.
    pub async fn diff(&self, from: &Snapshot, to: &Snapshot) -> Result<ChangeSet, String> {
        if from == to {
            return Ok(ChangeSet::default());
        }
        let numstat = self
            .run(&["diff", "--numstat", "-M", "--no-color", &from.0, &to.0])
            .await
            .map_err(|e| format!("diff --numstat: {e}"))?;
        let status = self
            .run(&["diff", "--name-status", "-M", "--no-color", &from.0, &to.0])
            .await
            .map_err(|e| format!("diff --name-status: {e}"))?;
        let patch = self
            .run(&["diff", "-M", "--no-color", "--no-ext-diff", &from.0, &to.0])
            .await
            .map_err(|e| format!("diff: {e}"))?;
        Ok(ChangeSet {
            files: parse_changes(&status, &numstat),
            patch,
        })
    }

    /// Remove the shadow repository (the folder itself is untouched).
    pub fn remove(&self) {
        if let Err(e) = std::fs::remove_dir_all(&self.git_dir) {
            tracing::warn!(path = %self.git_dir.display(), error = %e, "shadow repository not removed");
        }
    }
}

/// `--name-status` and `--numstat` output, joined per path. A rename's path
/// is its new one.
fn parse_changes(name_status: &str, numstat: &str) -> Vec<FileChange> {
    let counts: std::collections::HashMap<String, (u32, u32)> = numstat
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let added = parts.next()?.parse().unwrap_or(0);
            let removed = parts.next()?.parse().unwrap_or(0);
            let path = parts.next_back()?.to_string();
            // A rename prints `old => new` (or `dir/{a => b}`) in numstat.
            let path = renamed_to(&path);
            Some((path, (added, removed)))
        })
        .collect();
    name_status
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let code = parts.next()?;
            let path = parts.next_back()?.to_string();
            let status = match code.chars().next()? {
                'A' => "added",
                'D' => "deleted",
                'R' => "renamed",
                _ => "modified",
            };
            let (added_lines, removed_lines) = counts.get(&path).copied().unwrap_or((0, 0));
            Some(FileChange {
                status: status.to_string(),
                path,
                added_lines,
                removed_lines,
            })
        })
        .collect()
}

/// The new path of a numstat rename entry; the path itself otherwise.
fn renamed_to(path: &str) -> String {
    if let (Some(open), Some(close)) = (path.find('{'), path.find('}'))
        && let Some(arrow) = path[open..close].find(" => ")
    {
        let new = &path[open + arrow + 4..close];
        return format!("{}{}{}", &path[..open], new, &path[close + 1..]).replace("//", "/");
    }
    match path.split_once(" => ") {
        Some((_, new)) => new.to_string(),
        None => path.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Under the one test-environment lock: another test swaps `git` on PATH.
    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    #[test]
    fn a_folder_that_is_no_repository_has_its_rounds_diffed() {
        crate::test_home::with_home(|_| rt().block_on(async {
            let folder = tempfile::tempdir().unwrap();
            let shadows = tempfile::tempdir().unwrap();
            std::fs::write(folder.path().join("a.py"), "x = 1\n").unwrap();
            std::fs::create_dir_all(folder.path().join("node_modules/pkg")).unwrap();
            std::fs::write(folder.path().join("node_modules/pkg/i.js"), "big").unwrap();
            let shadow = Shadow::open(&shadows.path().join("s.git"), folder.path())
                .await
                .unwrap();
            let base = shadow.snapshot().await.unwrap();

            std::fs::write(folder.path().join("a.py"), "x = 2\n").unwrap();
            std::fs::write(folder.path().join("b.py"), "y = 1\n").unwrap();
            let round1 = shadow.snapshot().await.unwrap();
            let whole = shadow.diff(&base, &round1).await.unwrap();
            assert_eq!(whole.stat(), "M a.py (+1 -1)\nA b.py (+1 -0)");
            assert!(
                whole.patch.contains("-x = 1") && whole.patch.contains("+x = 2"),
                "{}",
                whole.patch
            );
            assert!(
                !whole.patch.contains("node_modules"),
                "installed packages stay out"
            );

            std::fs::remove_file(folder.path().join("b.py")).unwrap();
            let round2 = shadow.snapshot().await.unwrap();
            let delta = shadow.diff(&round1, &round2).await.unwrap();
            assert_eq!(
                delta.stat(),
                "D b.py (+0 -1)",
                "a later round's delta is only what it changed"
            );
            assert!(shadow.diff(&round2, &round2).await.unwrap().is_empty());
            assert!(
                !folder.path().join(".git").exists(),
                "the folder never becomes a repository"
            );
        }))
    }

    #[test]
    fn a_repository_keeps_its_own_index_and_honours_its_ignores() {
        crate::test_home::with_home(|_| rt().block_on(async {
            let repo = tempfile::tempdir().unwrap();
            let shadows = tempfile::tempdir().unwrap();
            for args in [
                vec!["init", "-q", "-b", "main"],
                vec!["config", "user.email", "t@t"],
                vec!["config", "user.name", "t"],
            ] {
                git(repo.path(), &args).await.unwrap();
            }
            std::fs::write(repo.path().join(".gitignore"), "out/\n").unwrap();
            std::fs::write(repo.path().join("lib.rs"), "fn a() {}\n").unwrap();
            git(repo.path(), &["add", "-A"]).await.unwrap();
            git(repo.path(), &["commit", "-q", "-m", "base"])
                .await
                .unwrap();

            let shadow = Shadow::open(&shadows.path().join("s.git"), repo.path())
                .await
                .unwrap();
            assert!(
                shadows
                    .path()
                    .join("s.git/objects/info/alternates")
                    .exists(),
                "borrows the repository's objects"
            );
            let base = shadow.snapshot().await.unwrap();
            std::fs::write(repo.path().join("lib.rs"), "fn a() { b() }\n").unwrap();
            std::fs::create_dir_all(repo.path().join("out")).unwrap();
            std::fs::write(repo.path().join("out/x"), "built").unwrap();
            let after = shadow.snapshot().await.unwrap();
            let change = shadow.diff(&base, &after).await.unwrap();
            assert_eq!(change.stat(), "M lib.rs (+1 -1)");

            let status = git(repo.path(), &["status", "--porcelain"]).await.unwrap();
            assert_eq!(
                status, "M lib.rs",
                "the repository's own index is untouched: {status}"
            );
            shadow.remove();
            assert!(!shadows.path().join("s.git").exists());
        }))
    }

    #[test]
    fn numstat_renames_name_the_new_path() {
        assert_eq!(renamed_to("src/{a.rs => b.rs}"), "src/b.rs");
        assert_eq!(renamed_to("old.rs => new.rs"), "new.rs");
        assert_eq!(renamed_to("plain.rs"), "plain.rs");
    }
}
