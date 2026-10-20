//! What changed, round by round: a shadow repository over the folder the
//! work happens in.
//!
//! The verify loop needs the whole change (round 1) and each round's delta
//! (the fixes a later round reviews), whatever the folder is: an isolated
//! git worktree, a scratch copy of a folder that is not a repository, an
//! app's page folder, or a linked coding agent's own project. One mechanism
//! serves all of them: a private bare repository under Nebo's data dir whose
//! work tree is that folder. A snapshot is `git add` + `git write-tree` into
//! the shadow's own index, so it never touches the folder's own `.git`, its
//! index, its branches or its stash (the house git rules hold by
//! construction), and it works where there is no repository at all.
//!
//! What a snapshot leaves out:
//! - In a repository (the folder is one, or sits inside one): exactly what
//!   that repository ignores, as the repository itself decides it (every
//!   `.gitignore` up to its root, its `info/exclude`, the global excludes),
//!   and never a file it tracks, even one an ignore pattern matches.
//! - In a folder that is no repository: its own `.gitignore` when it has
//!   one; otherwise the usual build output and installed packages
//!   ([`EXCLUDED`]). The same rule decides what a landing leaves out
//!   ([`crate::worktree::ls_folder`]): one definition of build output.
//! - Anywhere: an untracked file over [`BIG_FILE_BYTES`] (a video, a
//!   dataset, a disk image) is not hashed.
//!
//! A repository inside the folder (a nested clone or a submodule) is its
//! own: it is left out of the snapshot, and [`Shadow::nested_repositories`]
//! names it so the round says its changes were not reviewed.
//!
//! Inside a repository the shadow borrows that repository's objects
//! (`objects/info/alternates`), so a large project is not copied.

use std::path::{Path, PathBuf};

use crate::worktree::{EXCLUDED, git, git_bytes};

/// An untracked file larger than this is not hashed into a snapshot.
pub const BIG_FILE_BYTES: u64 = 50 * 1024 * 1024;

/// The most of a diff a change carries; past it the patch is cut and says so.
pub const MAX_PATCH_BYTES: usize = 4 * 1024 * 1024;

/// A tree snapshot of the folder (a git tree id in the shadow).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Snapshot(pub String);

/// The snapshot of an empty folder: the base of work that creates a folder
/// from nothing (a new app's pages).
pub const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// How one file changed between two snapshots.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FileChange {
    /// `added`, `modified`, `deleted` or `renamed`.
    pub status: String,
    /// The path relative to the folder, '/'-separated (a rename's new path).
    pub path: String,
    /// A rename's old path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    pub added_lines: u32,
    pub removed_lines: u32,
}

/// Everything that changed between two snapshots.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChangeSet {
    pub files: Vec<FileChange>,
    /// The unified diff, as `git diff` prints it, cut at [`MAX_PATCH_BYTES`].
    pub patch: String,
    /// The patch was cut.
    #[serde(default)]
    pub patch_cut: bool,
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

    /// Every path the change touches, a rename's old path included.
    pub fn paths(&self) -> Vec<String> {
        let mut out = Vec::new();
        for f in &self.files {
            out.push(f.path.clone());
            if let Some(from) = &f.from {
                out.push(from.clone());
            }
        }
        out
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
                let path = match &f.from {
                    Some(from) => format!("{from} -> {}", f.path),
                    None => f.path.clone(),
                };
                format!("{mark} {path} (+{} -{})", f.added_lines, f.removed_lines)
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
        // Inside a repository: borrow its objects instead of copying them.
        if let Some(objects) = shadow
            .repository()
            .await
            .map(|common| common.join("objects"))
            .filter(|o| o.is_dir())
        {
            let alternates = git_dir.join("objects").join("info");
            std::fs::create_dir_all(&alternates)
                .map_err(|e| format!("create {}: {e}", alternates.display()))?;
            std::fs::write(
                alternates.join("alternates"),
                format!("{}\n", objects.display()),
            )
            .map_err(|e| format!("write alternates: {e}"))?;
        }
        Ok(shadow)
    }

    /// The git directory of the repository the folder sits in, if any.
    async fn repository(&self) -> Option<PathBuf> {
        git(
            &self.work_tree,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )
        .await
        .ok()
        .map(|c| PathBuf::from(c.trim()))
    }

    fn with_dirs(&self, args: &[&str]) -> Vec<String> {
        let mut all = vec![
            format!("--git-dir={}", self.git_dir.display()),
            format!("--work-tree={}", self.work_tree.display()),
            "-c".into(),
            "core.quotepath=off".into(),
            "-c".into(),
            "core.autocrlf=false".into(),
        ];
        all.extend(args.iter().map(|a| a.to_string()));
        all
    }

    async fn run(&self, args: &[&str]) -> Result<String, String> {
        let all = self.with_dirs(args);
        let all: Vec<&str> = all.iter().map(String::as_str).collect();
        git(&self.work_tree, &all).await
    }

    async fn run_bytes(&self, args: &[&str]) -> Result<Vec<u8>, String> {
        let all = self.with_dirs(args);
        let all: Vec<&str> = all.iter().map(String::as_str).collect();
        git_bytes(&self.work_tree, &all).await
    }

    /// The folder as it is now.
    pub async fn snapshot(&self) -> Result<Snapshot, String> {
        let left_out = self.left_out().await?;
        let pathspec = self.git_dir.join("snapshot.pathspec");
        let mut spec: Vec<u8> = b".\0".to_vec();
        for p in &left_out {
            spec.extend_from_slice(p.as_bytes());
            spec.push(0);
        }
        std::fs::write(&pathspec, spec)
            .map_err(|e| format!("write {}: {e}", pathspec.display()))?;
        let from_file = format!("--pathspec-from-file={}", pathspec.display());
        // `-f`: what is left out is exactly `left_out`. The folder's own
        // ignore files are read by its repository, not by the shadow, so a
        // tracked file an ignore pattern matches is never lost.
        self.run(&["add", "-A", "-f", "--pathspec-file-nul", &from_file])
            .await
            .map_err(|e| format!("snapshot (add): {e}"))?;
        let tree = self
            .run(&["write-tree"])
            .await
            .map_err(|e| format!("snapshot (write-tree): {e}"))?;
        Ok(Snapshot(tree.trim().to_string()))
    }

    /// `git ls-files` over the folder ([`crate::worktree::ls_folder`]):
    /// read by its repository when it is in one, else through the shadow.
    async fn ls(&self, in_repo: bool, args: &[&str]) -> Result<Vec<u8>, String> {
        crate::worktree::ls_folder(&self.work_tree, (!in_repo).then_some(self.git_dir.as_path()), args).await
    }

    /// What a snapshot leaves out, as exclude pathspecs.
    async fn left_out(&self) -> Result<Vec<String>, String> {
        let literal = |p: &str| format!(":(exclude,literal){}", p.trim_end_matches('/'));
        let mut out: Vec<String> = Vec::new();
        let in_repo = self.repository().await.is_some();
        if !in_repo && !self.work_tree.join(".gitignore").is_file() {
            for p in EXCLUDED {
                match p.strip_suffix('/') {
                    Some(dir) => out.push(format!(":(exclude,glob)**/{dir}/**")),
                    None => out.push(format!(":(exclude,glob)**/{p}")),
                }
            }
        }
        // What the repository (or the folder's own .gitignore) ignores, as
        // git decides it, and the new files a snapshot would hash.
        let ignored = self
            .ls(
                in_repo,
                &[
                    "ls-files",
                    "-z",
                    "--others",
                    "--ignored",
                    "--exclude-standard",
                    "--directory",
                ],
            )
            .await
            .map_err(|e| format!("snapshot (ignored files): {e}"))?;
        out.extend(nul_split(&ignored).map(literal));
        let others = self
            .ls(
                in_repo,
                &["ls-files", "-z", "--others", "--exclude-standard"],
            )
            .await
            .map_err(|e| format!("snapshot (new files): {e}"))?;
        for rel in nul_split(&others) {
            let big = std::fs::symlink_metadata(self.work_tree.join(rel))
                .is_ok_and(|m| m.is_file() && m.len() > BIG_FILE_BYTES);
            if big {
                out.push(literal(rel));
            }
        }
        // A repository inside the folder is its own: named, never hashed.
        out.extend(self.nested_repositories().await.iter().map(|d| literal(d)));
        Ok(out)
    }

    /// Repositories inside the folder (nested clones and submodules): a
    /// snapshot leaves them out, so changes inside one are not in the
    /// change and a round says so.
    pub async fn nested_repositories(&self) -> Vec<String> {
        let mut out = Vec::new();
        let in_repo = self.repository().await.is_some();
        if in_repo
            && let Ok(staged) = git_bytes(&self.work_tree, &["ls-files", "-z", "--stage"]).await
        {
            for entry in nul_split(&staged) {
                // `<mode> <object> <stage>\t<path>`: a gitlink is mode 160000.
                if let Some((meta, path)) = entry.split_once('\t')
                    && meta.starts_with("160000 ")
                {
                    out.push(path.to_string());
                }
            }
        }
        // git lists an embedded repository as one entry, `dir/`.
        let others = self
            .ls(
                in_repo,
                &["ls-files", "-z", "--others", "--exclude-standard"],
            )
            .await;
        for entry in others.as_deref().map(nul_split).into_iter().flatten() {
            let Some(dir) = entry.strip_suffix('/') else {
                continue;
            };
            if self.work_tree.join(dir).join(".git").exists() && !out.iter().any(|o| o == dir) {
                out.push(dir.to_string());
            }
        }
        out
    }

    /// What changed from `from` to `to`.
    pub async fn diff(&self, from: &Snapshot, to: &Snapshot) -> Result<ChangeSet, String> {
        if from == to {
            return Ok(ChangeSet::default());
        }
        let numstat = self
            .run_bytes(&[
                "diff",
                "-z",
                "--numstat",
                "-M",
                "--no-color",
                &from.0,
                &to.0,
            ])
            .await
            .map_err(|e| format!("diff --numstat: {e}"))?;
        let status = self
            .run_bytes(&[
                "diff",
                "-z",
                "--name-status",
                "-M",
                "--no-color",
                &from.0,
                &to.0,
            ])
            .await
            .map_err(|e| format!("diff --name-status: {e}"))?;
        let patch = self
            .run_bytes(&["diff", "-M", "--no-color", "--no-ext-diff", &from.0, &to.0])
            .await
            .map_err(|e| format!("diff: {e}"))?;
        let patch_cut = patch.len() > MAX_PATCH_BYTES;
        let mut patch =
            String::from_utf8_lossy(&patch[..patch.len().min(MAX_PATCH_BYTES)]).into_owned();
        if patch_cut {
            patch.push_str("\n[… the diff is longer than this and was cut here]\n");
        }
        Ok(ChangeSet {
            files: parse_changes(&status, &numstat),
            patch,
            patch_cut,
        })
    }

    /// Write `snapshot` out as a folder of its own at `dest` (made new): a
    /// copy of exactly what was snapshotted, for reading and building, that
    /// shares nothing with the folder it was taken from.
    pub async fn export(&self, snapshot: &Snapshot, dest: &Path) -> Result<(), String> {
        std::fs::create_dir_all(dest).map_err(|e| format!("create {}: {e}", dest.display()))?;
        let tar = dest.with_extension("tar");
        let out = format!("--output={}", tar.display());
        self.run(&["archive", "--format=tar", &out, &snapshot.0])
            .await
            .map_err(|e| format!("export (archive): {e}"))?;
        let mut cmd = command::new::<tokio::process::Command>("tar", command::Console::Hidden);
        cmd.arg("-xf").arg(&tar).arg("-C").arg(dest).stdin(std::process::Stdio::null()).kill_on_drop(true);
        let done = cmd.output().await.map_err(|e| format!("export (tar): {e}"));
        let _ = std::fs::remove_file(&tar);
        let done = done?;
        if !done.status.success() {
            return Err(format!("export (tar): {}", String::from_utf8_lossy(&done.stderr).trim()));
        }
        Ok(())
    }

    /// What `snapshot` holds at each of `paths`, as a landing compares it
    /// ([`crate::worktree::content_id`]; `None`: nothing there).
    pub async fn contents(&self, snapshot: &Snapshot, paths: &[String]) -> Result<crate::worktree::Reviewed, String> {
        let mut out: crate::worktree::Reviewed = paths.iter().map(|p| (p.clone(), None)).collect();
        if paths.is_empty() {
            return Ok(out);
        }
        let mut args = vec!["ls-tree", "-r", "-z", "--full-tree", snapshot.0.as_str(), "--"];
        args.extend(paths.iter().map(String::as_str));
        let listed = self.run_bytes(&args).await.map_err(|e| format!("ls-tree: {e}"))?;
        for entry in nul_split(&listed) {
            // `<mode> <type> <object>\t<path>`
            if let Some((meta, path)) = entry.split_once('\t')
                && let Some(slot) = out.get_mut(path)
            {
                *slot = meta.split(' ').nth(2).map(str::to_string);
            }
        }
        Ok(out)
    }

    /// The text of the regular file `rel` as `snapshot` holds it; `None`
    /// for a link, a folder or nothing there.
    pub async fn read(&self, snapshot: &Snapshot, rel: &str) -> Option<String> {
        let entry = self.run(&["ls-tree", &snapshot.0, "--", rel]).await.ok()?;
        if !entry.starts_with("100") {
            return None;
        }
        self.run(&["cat-file", "-p", &format!("{}:{rel}", snapshot.0)]).await.ok()
    }

    /// Remove the shadow repository (the folder itself is untouched).
    pub fn remove(&self) {
        if let Err(e) = std::fs::remove_dir_all(&self.git_dir) {
            tracing::warn!(path = %self.git_dir.display(), error = %e, "shadow repository not removed");
        }
    }
}

fn nul_split(bytes: &[u8]) -> impl Iterator<Item = &str> {
    bytes
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .filter_map(|s| std::str::from_utf8(s).ok())
}

/// `diff -z --name-status -M` and `diff -z --numstat -M` output, joined per
/// path. A rename is `R<score>\0old\0new\0` in the first and
/// `added\tremoved\t\0old\0new\0` in the second.
fn parse_changes(name_status: &[u8], numstat: &[u8]) -> Vec<FileChange> {
    let mut counts: std::collections::HashMap<String, (u32, u32)> =
        std::collections::HashMap::new();
    let mut fields = numstat
        .split(|b| *b == 0)
        .map(|f| String::from_utf8_lossy(f).into_owned());
    while let Some(head) = fields.next() {
        if head.is_empty() {
            continue;
        }
        let mut parts = head.splitn(3, '\t');
        let added = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0);
        let removed = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0);
        let path = match parts.next() {
            Some(p) if !p.is_empty() => p.to_string(),
            // A rename: the old path, then the new one.
            _ => {
                let _old = fields.next();
                fields.next().unwrap_or_default()
            }
        };
        counts.insert(path, (added, removed));
    }
    let mut out = Vec::new();
    let mut fields = name_status
        .split(|b| *b == 0)
        .map(|f| String::from_utf8_lossy(f).into_owned());
    while let Some(code) = fields.next() {
        let Some(first) = code.chars().next() else {
            continue;
        };
        let (status, from, path) = match first {
            'R' | 'C' => {
                let old = fields.next().unwrap_or_default();
                let new = fields.next().unwrap_or_default();
                (
                    if first == 'R' { "renamed" } else { "added" },
                    (first == 'R').then_some(old),
                    new,
                )
            }
            'A' => ("added", None, fields.next().unwrap_or_default()),
            'D' => ("deleted", None, fields.next().unwrap_or_default()),
            _ => ("modified", None, fields.next().unwrap_or_default()),
        };
        let (added_lines, removed_lines) = counts.get(&path).copied().unwrap_or((0, 0));
        out.push(FileChange {
            status: status.to_string(),
            path,
            from,
            added_lines,
            removed_lines,
        });
    }
    out
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

    async fn repo(dir: &Path) {
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.email", "t@t"],
            vec!["config", "user.name", "t"],
        ] {
            git(dir, &args).await.unwrap();
        }
    }

    async fn commit_all(dir: &Path) {
        git(dir, &["add", "-A"]).await.unwrap();
        git(dir, &["commit", "-q", "-m", "c"]).await.unwrap();
    }

    #[test]
    fn a_folder_that_is_no_repository_has_its_rounds_diffed() {
        crate::test_home::with_home(|_| {
            rt().block_on(async {
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
            })
        })
    }

    #[test]
    fn a_folder_with_its_own_ignore_file_is_read_by_it_not_by_the_defaults() {
        crate::test_home::with_home(|_| {
            rt().block_on(async {
                let folder = tempfile::tempdir().unwrap();
                let shadows = tempfile::tempdir().unwrap();
                std::fs::write(folder.path().join(".gitignore"), "secret.env\n").unwrap();
                std::fs::create_dir_all(folder.path().join("build")).unwrap();
                let shadow = Shadow::open(&shadows.path().join("s.git"), folder.path())
                    .await
                    .unwrap();
                let base = shadow.snapshot().await.unwrap();
                std::fs::write(folder.path().join("build/gen.sh"), "echo\n").unwrap();
                std::fs::write(folder.path().join("secret.env"), "KEY=1\n").unwrap();
                let change = shadow
                    .diff(&base, &shadow.snapshot().await.unwrap())
                    .await
                    .unwrap();
                assert_eq!(
                    change.stat(),
                    "A build/gen.sh (+1 -0)",
                    "its own ignore file decides: build/ is in"
                );
            })
        })
    }

    #[test]
    fn a_repository_keeps_its_own_index_and_never_loses_a_tracked_file_an_ignore_matches() {
        crate::test_home::with_home(|_| {
            rt().block_on(async {
                let r = tempfile::tempdir().unwrap();
                let shadows = tempfile::tempdir().unwrap();
                repo(r.path()).await;
                std::fs::write(r.path().join(".gitignore"), "build/\nout/\n").unwrap();
                std::fs::write(r.path().join("lib.rs"), "fn a() {}\n").unwrap();
                std::fs::create_dir_all(r.path().join("tools/build")).unwrap();
                std::fs::write(r.path().join("tools/build/gen.sh"), "echo 1\n").unwrap();
                git(r.path(), &["add", "-A", "-f"]).await.unwrap();
                git(r.path(), &["commit", "-q", "-m", "base"])
                    .await
                    .unwrap();

                let shadow = Shadow::open(&shadows.path().join("s.git"), r.path())
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
                std::fs::write(r.path().join("lib.rs"), "fn a() { b() }\n").unwrap();
                std::fs::write(r.path().join("tools/build/gen.sh"), "echo 2\n").unwrap();
                std::fs::create_dir_all(r.path().join("out")).unwrap();
                std::fs::write(r.path().join("out/x"), "built").unwrap();
                let change = shadow
                    .diff(&base, &shadow.snapshot().await.unwrap())
                    .await
                    .unwrap();
                assert_eq!(
                    change.stat(),
                    "M lib.rs (+1 -1)\nM tools/build/gen.sh (+1 -1)",
                    "the tracked file under an ignored name is seen"
                );

                let status = git(r.path(), &["status", "--porcelain"]).await.unwrap();
                assert_eq!(
                    status, "M lib.rs\n M tools/build/gen.sh",
                    "the repository's own index is untouched: {status}"
                );
                shadow.remove();
                assert!(!shadows.path().join("s.git").exists());
            })
        })
    }

    #[test]
    fn a_sub_folder_of_a_repository_honours_the_repositorys_ignores() {
        crate::test_home::with_home(|_| {
            rt().block_on(async {
                let r = tempfile::tempdir().unwrap();
                let shadows = tempfile::tempdir().unwrap();
                repo(r.path()).await;
                std::fs::write(r.path().join(".gitignore"), "*.log\n").unwrap();
                std::fs::create_dir_all(r.path().join("app/ui")).unwrap();
                std::fs::write(r.path().join("app/ui/index.html"), "<p>1</p>\n").unwrap();
                commit_all(r.path()).await;
                let shadow = Shadow::open(&shadows.path().join("s.git"), &r.path().join("app/ui"))
                    .await
                    .unwrap();
                let base = shadow.snapshot().await.unwrap();
                std::fs::write(r.path().join("app/ui/index.html"), "<p>2</p>\n").unwrap();
                std::fs::write(r.path().join("app/ui/debug.log"), "noise\n").unwrap();
                let change = shadow
                    .diff(&base, &shadow.snapshot().await.unwrap())
                    .await
                    .unwrap();
                assert_eq!(change.stat(), "M index.html (+1 -1)");
            })
        })
    }

    #[test]
    fn a_nested_repository_is_named_and_renames_and_odd_names_are_read_whole() {
        crate::test_home::with_home(|_| {
            rt().block_on(async {
                let r = tempfile::tempdir().unwrap();
                let shadows = tempfile::tempdir().unwrap();
                repo(r.path()).await;
                std::fs::write(
                    r.path().join("old name.rs"),
                    "fn a() {}\nfn b() {}\nfn c() {}\n",
                )
                .unwrap();
                commit_all(r.path()).await;
                std::fs::create_dir_all(r.path().join("vendor/lib")).unwrap();
                repo(&r.path().join("vendor/lib")).await;
                let shadow = Shadow::open(&shadows.path().join("s.git"), r.path())
                    .await
                    .unwrap();
                let base = shadow.snapshot().await.unwrap();
                git(r.path(), &["mv", "old name.rs", "naïve name.rs"])
                    .await
                    .unwrap();
                let change = shadow
                    .diff(&base, &shadow.snapshot().await.unwrap())
                    .await
                    .unwrap();
                assert_eq!(change.files.len(), 1, "{:?}", change.files);
                assert_eq!(change.files[0].status, "renamed");
                assert_eq!(change.files[0].path, "naïve name.rs");
                assert_eq!(change.files[0].from.as_deref(), Some("old name.rs"));
                assert_eq!(
                    change.paths(),
                    vec!["naïve name.rs".to_string(), "old name.rs".to_string()]
                );
                assert_eq!(
                    shadow.nested_repositories().await,
                    vec!["vendor/lib".to_string()]
                );
            })
        })
    }

    #[test]
    fn a_huge_new_file_is_not_hashed() {
        crate::test_home::with_home(|_| {
            rt().block_on(async {
                let folder = tempfile::tempdir().unwrap();
                let shadows = tempfile::tempdir().unwrap();
                let shadow = Shadow::open(&shadows.path().join("s.git"), folder.path())
                    .await
                    .unwrap();
                let base = shadow.snapshot().await.unwrap();
                let f = std::fs::File::create(folder.path().join("video.mov")).unwrap();
                f.set_len(BIG_FILE_BYTES + 1).unwrap();
                std::fs::write(folder.path().join("a.txt"), "a\n").unwrap();
                let change = shadow
                    .diff(&base, &shadow.snapshot().await.unwrap())
                    .await
                    .unwrap();
                assert_eq!(change.stat(), "A a.txt (+1 -0)");
            })
        })
    }

    #[test]
    fn the_empty_tree_is_the_base_of_work_from_nothing() {
        crate::test_home::with_home(|_| {
            rt().block_on(async {
                let folder = tempfile::tempdir().unwrap();
                let shadows = tempfile::tempdir().unwrap();
                let shadow = Shadow::open(&shadows.path().join("s.git"), folder.path())
                    .await
                    .unwrap();
                assert_eq!(
                    shadow.snapshot().await.unwrap(),
                    Snapshot(EMPTY_TREE.into())
                );
                std::fs::write(folder.path().join("index.html"), "<p>\n").unwrap();
                let change = shadow
                    .diff(
                        &Snapshot(EMPTY_TREE.into()),
                        &shadow.snapshot().await.unwrap(),
                    )
                    .await
                    .unwrap();
                assert_eq!(change.stat(), "A index.html (+1 -0)");
            })
        })
    }

    /// Review round 4: what a review saw is compared by content when the
    /// change lands, so a snapshot's record of each path is exactly what a
    /// landing reads off the disk ([`crate::worktree::content_id`]), in a
    /// repository and in a folder that is none, line endings and links
    /// included; a path with nothing there is `None`.
    #[test]
    fn a_snapshot_records_each_path_as_a_landing_reads_it() {
        crate::test_home::with_home(|_| {
            rt().block_on(async {
                for in_repo in [true, false] {
                    let folder = tempfile::tempdir().unwrap();
                    let shadows = tempfile::tempdir().unwrap();
                    if in_repo {
                        repo(folder.path()).await;
                        std::fs::write(folder.path().join("keep.txt"), "k\n").unwrap();
                        commit_all(folder.path()).await;
                    }
                    std::fs::create_dir_all(folder.path().join("scripts/build")).unwrap();
                    std::fs::write(folder.path().join("scripts/build/run.sh"), "echo hi\r\n").unwrap();
                    std::fs::write(folder.path().join("a b.txt"), "spaced\n").unwrap();
                    #[cfg(unix)]
                    std::os::unix::fs::symlink("a b.txt", folder.path().join("link")).unwrap();
                    let shadow = Shadow::open(&shadows.path().join("s.git"), folder.path()).await.unwrap();
                    let snap = shadow.snapshot().await.unwrap();
                    let mut paths = vec!["a b.txt".to_string(), "gone.txt".to_string()];
                    if in_repo {
                        // A folder that is no repository leaves build folders out.
                        paths.push("scripts/build/run.sh".into());
                    }
                    #[cfg(unix)]
                    paths.push("link".into());
                    let seen = shadow.contents(&snap, &paths).await.unwrap();
                    for p in &paths {
                        let on_disk = crate::worktree::content_id(folder.path(), p).await.unwrap();
                        assert_eq!(seen.get(p), Some(&on_disk), "{p} (in a repository: {in_repo})");
                    }
                    assert_eq!(seen["gone.txt"], None);
                }
            })
        })
    }

}
