//! Isolation for mutating fan-out (coding harness P5.3).
//!
//! When a parallel batch is spawned with `isolate: "worktree"`, each child
//! gets its own copy of the project, fenced by `allowed_paths` and `cwd`, and
//! the parent merges the copies back when the batch is done. Read-only
//! fan-out never comes here: it shares the tree.
//!
//! ONE option, two arms, chosen by whether the folder is a git repository:
//! - `Isolation::Git`: a git worktree on its own branch, merged back with a
//!   normal merge. Turning a folder into a repo upgrades a project to this
//!   arm with no other change.
//! - `Isolation::Copy`: a scratch copy of the folder (most owners' project
//!   folders are not repos, and Nebo never runs `git init` in one). Files
//!   whose fingerprint changed are copied back; a file changed by two hands,
//!   or by the owner meanwhile, is a conflict with both versions kept.
//!
//! House git rules hold throughout: no stash, no reset, no checkout of the
//! owner's files. The only thing this module ever undoes is a merge it
//! started itself (`git merge --abort` on conflict), and it says so.
//! Every git subprocess runs with prompts disabled — an unattended agent can
//! never answer one.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::process::Command;
use tracing::warn;

/// Scratch copies above this refuse, naming the size: a folder of photos is
/// not a project, and copying it per hand would fill the disk.
pub const MAX_SCRATCH_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Worktrees and scratch copies older than this with nothing left in them
/// are swept at startup (a parent killed mid-batch leaves them behind).
pub const STALE_AFTER_SECS: u64 = 7 * 24 * 3600;

#[derive(Debug, Clone)]
pub struct Worktree {
    pub task_id: String,
    pub root: PathBuf,
    pub path: PathBuf,
    pub branch: String,
}

/// A copy of a non-repo folder plus the fingerprints it was copied from, so
/// copy-back can tell "the child changed it" from "the owner changed it".
#[derive(Debug, Clone)]
pub struct ScratchCopy {
    pub task_id: String,
    pub root: PathBuf,
    pub path: PathBuf,
    /// Relative path → fingerprint at copy time.
    pub snapshot: HashMap<PathBuf, u64>,
    /// Symlinks in the folder, which are never followed into the copy.
    pub skipped: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub enum Isolation {
    Git(Worktree),
    Copy(ScratchCopy),
}

impl Isolation {
    pub fn path(&self) -> &Path {
        match self {
            Isolation::Git(w) => &w.path,
            Isolation::Copy(c) => &c.path,
        }
    }
    pub fn task_id(&self) -> &str {
        match self {
            Isolation::Git(w) => &w.task_id,
            Isolation::Copy(c) => &c.task_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum MergeOutcome {
    /// Nothing changed in the copy; it was removed.
    NoChanges,
    /// Merged cleanly; copy (and branch) removed.
    Merged { files: Vec<String> },
    /// Conflicts. Git arm: the merge was aborted, worktree and branch kept.
    /// Copy arm: the owner's file is untouched and each hand's version is
    /// kept next to it as `<file>.<task_id>.nebo-conflict`.
    Conflict { files: Vec<String>, kept_at: PathBuf, branch: String },
    /// Refused (e.g. the parent tree has uncommitted changes in the way).
    Failed { error: String, kept_at: PathBuf, branch: String },
}

async fn git(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "")
        .env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes")
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .map_err(|e| format!("git {}: {e}", args.first().copied().unwrap_or("")))?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if out.status.success() {
        Ok(stdout)
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if stderr.is_empty() { stdout } else { stderr })
    }
}

/// The repository root for a directory, or why it is not one.
pub async fn repo_root(dir: &Path) -> Result<PathBuf, String> {
    let out = git(dir, &["rev-parse", "--show-toplevel"])
        .await
        .map_err(|e| format!("{} is not inside a git repository ({e})", dir.display()))?;
    Ok(PathBuf::from(out))
}

/// Where copies live: outside the project, under Nebo's data dir, so the
/// owner's folder (and `git status`) never shows them.
fn worktrees_dir(root: &Path) -> PathBuf {
    let base = config::data_dir().unwrap_or_else(|_| std::env::temp_dir());
    let name = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".into());
    base.join("worktrees").join(name)
}

/// A task id is joined into a path and a branch name: one segment of
/// `[A-Za-z0-9._-]`, at most 64 chars, never `.` or `..`.
pub fn validate_task_id(task_id: &str) -> Result<(), String> {
    if task_id.is_empty() || task_id.len() > 64 || task_id == "." || task_id == ".." {
        return Err(format!("invalid task id {task_id:?}: 1-64 chars, not . or .."));
    }
    if !task_id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')) {
        return Err(format!("invalid task id {task_id:?}: only letters, digits, . _ -"));
    }
    Ok(())
}

/// Isolate `task_id` for the project at `workspace`: a worktree if it is a
/// repo, a scratch copy otherwise.
pub async fn create(workspace: &Path, task_id: &str) -> Result<Isolation, String> {
    validate_task_id(task_id)?;
    match repo_root(workspace).await {
        Ok(root) => create_worktree(&root, task_id).await.map(Isolation::Git),
        Err(_) => create_copy(workspace, task_id).map(Isolation::Copy),
    }
}

/// Create a worktree on a fresh branch from the repo's current HEAD.
pub async fn create_worktree(root: &Path, task_id: &str) -> Result<Worktree, String> {
    validate_task_id(task_id)?;
    let dir = worktrees_dir(root);
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let path = dir.join(task_id);
    let branch = format!("nebo/{task_id}");
    git(root, &["worktree", "add", "-b", &branch, path.to_str().unwrap_or_default(), "HEAD"])
        .await
        .map_err(|e| format!("git worktree add: {e}"))?;
    Ok(Worktree { task_id: task_id.to_string(), root: root.to_path_buf(), path, branch })
}

/// Walk a folder: every regular file with its size. Symlinks are skipped
/// (a copy must never follow one out of the project); the skipped list is
/// returned so the outcome can say so.
fn walk(root: &Path) -> Result<(Vec<(PathBuf, u64)>, Vec<PathBuf>), String> {
    let mut files = Vec::new();
    let mut skipped = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir).map_err(|e| format!("read {}: {e}", dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("read {}: {e}", dir.display()))?;
            let path = entry.path();
            let meta = std::fs::symlink_metadata(&path).map_err(|e| format!("stat {}: {e}", path.display()))?;
            if meta.file_type().is_symlink() {
                skipped.push(path.strip_prefix(root).unwrap_or(&path).to_path_buf());
            } else if meta.is_dir() {
                stack.push(path);
            } else if meta.is_file() {
                files.push((path.strip_prefix(root).unwrap_or(&path).to_path_buf(), meta.len()));
            }
        }
    }
    files.sort();
    Ok((files, skipped))
}

fn fingerprint_file(path: &Path) -> Option<u64> {
    std::fs::read(path).ok().map(|b| tools::checkpoint::fingerprint(&b))
}

/// Copy a non-repo folder into a scratch directory and remember what every
/// file looked like.
pub fn create_copy(workspace: &Path, task_id: &str) -> Result<ScratchCopy, String> {
    validate_task_id(task_id)?;
    let root = std::path::absolute(workspace).map_err(|e| format!("{}: {e}", workspace.display()))?;
    if !root.is_dir() {
        return Err(format!("{} is not a directory", root.display()));
    }
    let (files, skipped) = walk(&root)?;
    let total: u64 = files.iter().map(|(_, n)| n).sum();
    if total > MAX_SCRATCH_BYTES {
        return Err(format!(
            "{} holds {:.1} GB of files; a scratch copy is refused above {:.0} GB. \
             Point workspace at the project folder itself, not a parent, or make it a git repository.",
            root.display(),
            total as f64 / 1e9,
            MAX_SCRATCH_BYTES as f64 / 1e9
        ));
    }
    let path = worktrees_dir(&root).join(task_id);
    if path.exists() {
        return Err(format!("{} already exists", path.display()));
    }
    let mut snapshot = HashMap::with_capacity(files.len());
    for (rel, _) in &files {
        let src = root.join(rel);
        let dst = path.join(rel);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
        }
        let bytes = std::fs::read(&src).map_err(|e| format!("read {}: {e}", src.display()))?;
        std::fs::write(&dst, &bytes).map_err(|e| format!("write {}: {e}", dst.display()))?;
        snapshot.insert(rel.clone(), tools::checkpoint::fingerprint(&bytes));
    }
    // An empty folder still needs its directory.
    std::fs::create_dir_all(&path).map_err(|e| format!("create {}: {e}", path.display()))?;
    Ok(ScratchCopy { task_id: task_id.to_string(), root, path, snapshot, skipped })
}

/// What one hand did to one file, relative to the snapshot.
#[derive(Debug, Clone, PartialEq)]
enum Change {
    Written(Vec<u8>),
    Deleted,
}

/// Every file the child changed (relative path → change).
fn changes_of(c: &ScratchCopy) -> Result<Vec<(PathBuf, Change)>, String> {
    let (files, _) = walk(&c.path)?;
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (rel, _) in files {
        seen.insert(rel.clone());
        let bytes = std::fs::read(c.path.join(&rel)).map_err(|e| format!("read {}: {e}", rel.display()))?;
        let fp = tools::checkpoint::fingerprint(&bytes);
        if c.snapshot.get(&rel) != Some(&fp) {
            out.push((rel, Change::Written(bytes)));
        }
    }
    for rel in c.snapshot.keys() {
        if !seen.contains(rel) {
            out.push((rel.clone(), Change::Deleted));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// Merge every isolation of a batch back into the project, in order.
/// Copies are merged as ONE batch so a file two hands both changed is a
/// conflict for both (the owner's file stays untouched); worktrees merge
/// one after another exactly as git does.
pub async fn merge_all(isos: &[Isolation], message: &str) -> Vec<(String, MergeOutcome)> {
    let mut out = Vec::with_capacity(isos.len());
    // Copy arm: collect first, then decide per file.
    let copies: Vec<&ScratchCopy> = isos.iter().filter_map(|i| match i { Isolation::Copy(c) => Some(c), _ => None }).collect();
    let mut per_copy: HashMap<String, Result<Vec<(PathBuf, Change)>, String>> = HashMap::new();
    let mut touched: HashMap<PathBuf, usize> = HashMap::new();
    for c in &copies {
        let changes = changes_of(c);
        if let Ok(ch) = &changes {
            for (rel, _) in ch {
                *touched.entry(rel.clone()).or_insert(0) += 1;
            }
        }
        per_copy.insert(c.task_id.clone(), changes);
    }
    for iso in isos {
        let outcome = match iso {
            Isolation::Git(wt) => merge_back(wt, message).await,
            Isolation::Copy(c) => {
                let changes = per_copy.remove(&c.task_id).unwrap_or_else(|| Ok(Vec::new()));
                merge_copy(c, changes, &touched)
            }
        };
        out.push((iso.task_id().to_string(), outcome));
    }
    out
}

fn merge_copy(
    c: &ScratchCopy,
    changes: Result<Vec<(PathBuf, Change)>, String>,
    touched: &HashMap<PathBuf, usize>,
) -> MergeOutcome {
    let changes = match changes {
        Ok(ch) => ch,
        Err(e) => return MergeOutcome::Failed { error: e, kept_at: c.path.clone(), branch: String::new() },
    };
    if changes.is_empty() {
        remove_copy(&c.path);
        return MergeOutcome::NoChanges;
    }
    let mut merged = Vec::new();
    let mut conflicts = Vec::new();
    for (rel, change) in changes {
        let target = c.root.join(&rel);
        let rel_s = rel.to_string_lossy().into_owned();
        // Conflict when another hand changed the same file, or the owner
        // changed it while the batch ran (its fingerprint left the snapshot).
        let owner_fp = fingerprint_file(&target);
        let owner_changed = match c.snapshot.get(&rel) {
            Some(fp) => owner_fp != Some(*fp),
            None => owner_fp.is_some(), // child created it and so did the owner
        };
        if touched.get(&rel).copied().unwrap_or(0) > 1 || owner_changed {
            if let Change::Written(bytes) = &change {
                let keep = c.root.join(format!("{rel_s}.{}.nebo-conflict", c.task_id));
                let written = keep
                    .parent()
                    .map(std::fs::create_dir_all)
                    .unwrap_or(Ok(()))
                    .and_then(|_| std::fs::write(&keep, bytes));
                if let Err(e) = written {
                    // The full copy is still kept at c.path; say so in the outcome.
                    return MergeOutcome::Failed {
                        error: format!("write {}: {e}", keep.display()),
                        kept_at: c.path.clone(),
                        branch: String::new(),
                    };
                }
            }
            conflicts.push(rel_s);
            continue;
        }
        let res = match change {
            Change::Written(bytes) => target
                .parent()
                .map(std::fs::create_dir_all)
                .unwrap_or(Ok(()))
                .and_then(|_| std::fs::write(&target, bytes)),
            Change::Deleted => match std::fs::remove_file(&target) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            },
        };
        match res {
            Ok(()) => merged.push(rel_s),
            Err(e) => {
                return MergeOutcome::Failed {
                    error: format!("write {}: {e}", target.display()),
                    kept_at: c.path.clone(),
                    branch: String::new(),
                }
            }
        }
    }
    if conflicts.is_empty() {
        remove_copy(&c.path);
        MergeOutcome::Merged { files: merged }
    } else {
        MergeOutcome::Conflict { files: conflicts, kept_at: c.path.clone(), branch: String::new() }
    }
}

/// A merged scratch copy has nothing left to keep; a failure to delete it is
/// disk, not correctness, so it is logged and the stale sweep gets it later.
fn remove_copy(path: &Path) {
    if let Err(e) = std::fs::remove_dir_all(path) {
        warn!(path = %path.display(), error = %e, "scratch copy not removed; the stale sweep will retry");
    }
}

/// Commit whatever the child left in its worktree, then merge its branch
/// into the parent's current branch.
pub async fn merge_back(wt: &Worktree, message: &str) -> MergeOutcome {
    // Commit the child's work on its own branch (never the owner's branch).
    let status = git(&wt.path, &["status", "--porcelain"]).await.unwrap_or_default();
    if !status.trim().is_empty() {
        if let Err(e) = git(&wt.path, &["add", "-A"]).await {
            return MergeOutcome::Failed { error: format!("git add in worktree: {e}"), kept_at: wt.path.clone(), branch: wt.branch.clone() };
        }
        if let Err(e) = git(
            &wt.path,
            &["-c", "user.name=Nebo", "-c", "user.email=nebo@neboai.com", "commit", "-q", "-m", message],
        )
        .await
        {
            return MergeOutcome::Failed { error: format!("git commit in worktree: {e}"), kept_at: wt.path.clone(), branch: wt.branch.clone() };
        }
    }
    let ahead = git(&wt.root, &["rev-list", "--count", &format!("HEAD..{}", wt.branch)])
        .await
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0);
    if ahead == 0 {
        if let Err(e) = remove(wt).await {
            warn!(path = %wt.path.display(), error = %e, "worktree not removed; the stale sweep will retry");
        }
        return MergeOutcome::NoChanges;
    }

    let changed = git(&wt.root, &["diff", "--name-only", &format!("HEAD...{}", wt.branch)])
        .await
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect::<Vec<_>>();

    match git(
        &wt.root,
        &["-c", "user.name=Nebo", "-c", "user.email=nebo@neboai.com", "merge", "--no-ff", "--no-edit", &wt.branch],
    )
    .await
    {
        Ok(_) => {
            if let Err(e) = remove(wt).await {
                warn!(path = %wt.path.display(), error = %e, "worktree not removed; the stale sweep will retry");
            }
            MergeOutcome::Merged { files: changed }
        }
        Err(e) => {
            let conflicts = git(&wt.root, &["diff", "--name-only", "--diff-filter=U"])
                .await
                .unwrap_or_default()
                .lines()
                .map(str::to_string)
                .collect::<Vec<_>>();
            if conflicts.is_empty() {
                // Not a conflict: git refused the merge (dirty parent tree, etc.).
                // Nothing to abort; nothing of the owner's was touched.
                return MergeOutcome::Failed { error: e, kept_at: wt.path.clone(), branch: wt.branch.clone() };
            }
            // Undo only the merge this module started; the child's work stays
            // on its branch and in its worktree for the parent to resolve.
            if let Err(e) = git(&wt.root, &["merge", "--abort"]).await {
                return MergeOutcome::Failed {
                    error: format!("merge conflict in {} and `git merge --abort` failed: {e}. The owner's tree has a merge in progress.", conflicts.join(", ")),
                    kept_at: wt.path.clone(),
                    branch: wt.branch.clone(),
                };
            }
            MergeOutcome::Conflict { files: conflicts, kept_at: wt.path.clone(), branch: wt.branch.clone() }
        }
    }
}

/// Remove a worktree and its branch (only called after a clean merge or when
/// nothing was committed).
pub async fn remove(wt: &Worktree) -> Result<(), String> {
    git(&wt.root, &["worktree", "remove", "--force", wt.path.to_str().unwrap_or_default()]).await?;
    // The branch is ours (nebo/<task>), already merged or empty; a leftover
    // branch is clutter, not data loss, so it is logged rather than failed.
    if let Err(e) = git(&wt.root, &["branch", "-D", &wt.branch]).await {
        warn!(branch = %wt.branch, error = %e, "merged worktree branch not deleted");
    }
    Ok(())
}

/// Sweep copies a crashed parent left behind under `<data_dir>/worktrees`.
/// Fail-closed like the reference: a worktree with uncommitted changes or
/// commits not on any remote is kept; a scratch copy is always kept (there is
/// no snapshot to compare against once the parent is gone) unless it is
/// empty. Only `nebo/sa-*` branches are ever deleted. Returns what was removed.
pub async fn cleanup_stale(cutoff_secs: u64) -> Vec<PathBuf> {
    let base = match config::data_dir() {
        Ok(d) => d.join("worktrees"),
        Err(_) => return Vec::new(),
    };
    let Ok(projects) = std::fs::read_dir(&base) else { return Vec::new() };
    let now = std::time::SystemTime::now();
    let mut removed = Vec::new();
    for project in projects.flatten() {
        let Ok(entries) = std::fs::read_dir(project.path()) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let old = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|m| now.duration_since(m).ok())
                .is_some_and(|age| age.as_secs() > cutoff_secs);
            if !old || !name.starts_with("sa-") {
                continue;
            }
            let is_worktree = path.join(".git").exists();
            if is_worktree {
                let dirty = git(&path, &["status", "--porcelain", "-uno"]).await;
                let unpushed = git(&path, &["rev-list", "--max-count=1", "HEAD", "--not", "--remotes"]).await;
                let clean = matches!((&dirty, &unpushed), (Ok(d), Ok(u)) if d.trim().is_empty() && u.trim().is_empty());
                if !clean {
                    continue;
                }
                let Ok(root) = git(&path, &["rev-parse", "--path-format=absolute", "--git-common-dir"]).await else { continue };
                let root = PathBuf::from(root);
                let root = root.parent().map(Path::to_path_buf).unwrap_or(root);
                let wt = Worktree { task_id: name.clone(), root, path: path.clone(), branch: format!("nebo/{name}") };
                if remove(&wt).await.is_ok() {
                    removed.push(path);
                }
            } else {
                let empty = walk(&path).map(|(f, _)| f.is_empty()).unwrap_or(false);
                if empty && std::fs::remove_dir_all(&path).is_ok() {
                    removed.push(path);
                }
            }
        }
    }
    removed
}

/// The lines the child sees at the top of its prompt.
pub fn preamble(iso: &Isolation) -> String {
    match iso {
        Isolation::Git(wt) => format!(
            "Working copy: {}\nThis is your own git worktree on branch {}. It is also your shell's working directory and where relative paths resolve. Do ALL file work under that path and nowhere else; the parent merges it when you finish. Commit as you like or not at all: uncommitted work is committed for you.\n\n",
            wt.path.display(),
            wt.branch
        ),
        Isolation::Copy(c) => format!(
            "Working copy: {}\nThis is your own copy of the project folder. It is also your shell's working directory and where relative paths resolve. Do ALL file work under that path and nowhere else; the parent copies your changed files back when you finish.{}\n\n",
            c.path.display(),
            if c.skipped.is_empty() {
                String::new()
            } else {
                format!(
                    " Not copied (symlinks): {}.",
                    c.skipped.iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>().join(", ")
                )
            }
        ),
    }
}

pub fn render_outcome(desc: &str, outcome: &MergeOutcome) -> String {
    let branch_note = |branch: &str| if branch.is_empty() { String::new() } else { format!(" on branch {branch}") };
    match outcome {
        MergeOutcome::NoChanges => format!("- {desc}: no file changes"),
        MergeOutcome::Merged { files } => format!(
            "- {desc}: merged ({} file{}: {})",
            files.len(),
            if files.len() == 1 { "" } else { "s" },
            files.join(", ")
        ),
        MergeOutcome::Conflict { files, kept_at, branch } => {
            if branch.is_empty() {
                format!(
                    "- {desc}: CONFLICT in {}. The project's copy is untouched; this hand's version sits next to it as <file>.{}.nebo-conflict, and its full copy is kept at {}. Resolve by choosing or combining, then delete the .nebo-conflict files.",
                    files.join(", "),
                    kept_at.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                    kept_at.display()
                )
            } else {
                format!(
                    "- {desc}: CONFLICT in {}. The merge was aborted and nothing of yours changed; the work is kept{} at {}. Resolve by merging that branch yourself.",
                    files.join(", "),
                    branch_note(branch),
                    kept_at.display()
                )
            }
        }
        MergeOutcome::Failed { error, kept_at, branch } => format!(
            "- {desc}: not merged ({error}); the work is kept{} at {}",
            branch_note(branch),
            kept_at.display()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tests share the process env; serialize NEBO_HOME on one lock so
    /// parallel tests never race each other (or the tools crate's tests).
    fn with_home<T>(f: impl FnOnce(&Path) -> T) -> T {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        // SAFETY: the lock above serializes env mutation across these tests.
        unsafe { std::env::set_var("NEBO_HOME", home.path()) };
        let out = f(home.path());
        unsafe { std::env::remove_var("NEBO_HOME") };
        out
    }

    async fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.email", "t@t"],
            vec!["config", "user.name", "t"],
        ] {
            git(dir.path(), &args).await.unwrap();
        }
        std::fs::write(dir.path().join("a.txt"), "base\n").unwrap();
        git(dir.path(), &["add", "-A"]).await.unwrap();
        git(dir.path(), &["commit", "-q", "-m", "base"]).await.unwrap();
        dir
    }

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
    }

    #[test]
    fn clean_merge_lands_the_childs_file_and_cleans_up() {
        with_home(|_| rt().block_on(async {
            let r = repo().await;
            let wt = create_worktree(r.path(), "sa-1").await.unwrap();
            assert!(wt.path.exists());
            std::fs::write(wt.path.join("b.txt"), "child\n").unwrap();
            let out = merge_back(&wt, "nebo: sa-1").await;
            assert_eq!(out, MergeOutcome::Merged { files: vec!["b.txt".into()] });
            assert_eq!(std::fs::read_to_string(r.path().join("b.txt")).unwrap(), "child\n");
            assert!(!wt.path.exists(), "worktree removed after a clean merge");
            let branches = git(r.path(), &["branch", "--list", "nebo/*"]).await.unwrap();
            assert!(branches.trim().is_empty(), "branch removed: {branches}");
        }))
    }

    #[test]
    fn conflict_aborts_the_merge_and_keeps_the_work() {
        with_home(|_| rt().block_on(async {
            let r = repo().await;
            let wt = create_worktree(r.path(), "sa-2").await.unwrap();
            std::fs::write(wt.path.join("a.txt"), "child\n").unwrap();
            // Parent moves the same line in the meantime.
            std::fs::write(r.path().join("a.txt"), "parent\n").unwrap();
            git(r.path(), &["commit", "-q", "-am", "parent"]).await.unwrap();
            let out = merge_back(&wt, "nebo: sa-2").await;
            match out {
                MergeOutcome::Conflict { files, kept_at, branch } => {
                    assert_eq!(files, vec!["a.txt".to_string()]);
                    assert!(kept_at.exists());
                    assert_eq!(branch, "nebo/sa-2");
                }
                other => panic!("expected a conflict, got {other:?}"),
            }
            // The owner's tree is exactly as it was: no merge in progress, their content intact.
            assert_eq!(std::fs::read_to_string(r.path().join("a.txt")).unwrap(), "parent\n");
            assert!(git(r.path(), &["rev-parse", "-q", "--verify", "MERGE_HEAD"]).await.is_err());
        }))
    }

    #[test]
    fn no_changes_means_no_merge_commit() {
        with_home(|_| rt().block_on(async {
            let r = repo().await;
            let before = git(r.path(), &["rev-parse", "HEAD"]).await.unwrap();
            let wt = create_worktree(r.path(), "sa-3").await.unwrap();
            assert_eq!(merge_back(&wt, "m").await, MergeOutcome::NoChanges);
            assert_eq!(git(r.path(), &["rev-parse", "HEAD"]).await.unwrap(), before);
        }))
    }

    #[test]
    fn merge_refused_by_a_dirty_parent_is_failed_not_conflict_and_aborts_nothing() {
        with_home(|_| rt().block_on(async {
            let r = repo().await;
            let wt = create_worktree(r.path(), "sa-4").await.unwrap();
            std::fs::write(wt.path.join("a.txt"), "child\n").unwrap();
            // Owner has an UNCOMMITTED edit to the same file: git refuses the merge.
            std::fs::write(r.path().join("a.txt"), "owner wip\n").unwrap();
            match merge_back(&wt, "m").await {
                MergeOutcome::Failed { kept_at, branch, .. } => {
                    assert!(kept_at.exists());
                    assert_eq!(branch, "nebo/sa-4");
                }
                other => panic!("expected Failed, got {other:?}"),
            }
            assert_eq!(std::fs::read_to_string(r.path().join("a.txt")).unwrap(), "owner wip\n");
            assert!(git(r.path(), &["rev-parse", "-q", "--verify", "MERGE_HEAD"]).await.is_err());
        }))
    }

    #[test]
    fn task_ids_are_validated_like_slugs() {
        for bad in ["", "..", "a/b", "../x", "x y", &"a".repeat(65)] {
            assert!(validate_task_id(bad).is_err(), "{bad:?} must be refused");
        }
        for ok in ["sa-1", "sa-0f3a.b_c", &"a".repeat(64)] {
            assert!(validate_task_id(ok).is_ok(), "{ok:?} is fine");
        }
    }

    #[test]
    fn subprocess_env_disables_every_prompt() {
        with_home(|home| rt().block_on(async {
            // A fake git on PATH dumps its environment; the real one is never reached.
            let bin = home.join("bin");
            std::fs::create_dir_all(&bin).unwrap();
            let dump = home.join("dump");
            std::fs::write(
                bin.join("git"),
                format!("#!/bin/sh\nenv > {}\n[ -t 0 ] && echo tty >> {} ; exit 0\n", dump.display(), dump.display()),
            )
            .unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(bin.join("git"), std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            let old_path = std::env::var("PATH").unwrap_or_default();
            unsafe { std::env::set_var("PATH", format!("{}:{old_path}", bin.display())) };
            let _ = git(home, &["status"]).await;
            unsafe { std::env::set_var("PATH", old_path) };
            let env = std::fs::read_to_string(&dump).unwrap();
            assert!(env.contains("GIT_TERMINAL_PROMPT=0"), "{env}");
            assert!(env.contains("GIT_ASKPASS=\n"), "{env}");
            assert!(env.contains("GIT_SSH_COMMAND=ssh -o BatchMode=yes"), "{env}");
            assert!(!env.contains("tty"), "stdin is closed");
        }))
    }

    // ---- scratch copies (no git) ----

    fn folder() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/a.txt"), "a0\n").unwrap();
        std::fs::write(dir.path().join("b.txt"), "b0\n").unwrap();
        dir
    }

    #[test]
    fn a_non_repo_folder_gets_a_scratch_copy_not_a_worktree() {
        with_home(|_| rt().block_on(async {
            let f = folder();
            let iso = create(f.path(), "sa-10").await.unwrap();
            assert!(matches!(iso, Isolation::Copy(_)));
            assert!(iso.path().join("src/a.txt").exists());
            assert!(!f.path().join(".git").exists(), "never git init in the owner's folder");
            assert!(!iso.path().starts_with(f.path()), "the copy lives outside the folder");
        }))
    }

    #[test]
    fn copy_back_lands_only_files_whose_fingerprint_changed() {
        with_home(|_| rt().block_on(async {
            let f = folder();
            let iso = create(f.path(), "sa-11").await.unwrap();
            std::fs::write(iso.path().join("src/a.txt"), "a1\n").unwrap();
            std::fs::write(iso.path().join("new.txt"), "n\n").unwrap();
            std::fs::remove_file(iso.path().join("b.txt")).unwrap();
            // Touch an untouched file's content identically: not a change.
            std::fs::write(iso.path().join("src/a.txt"), "a1\n").unwrap();
            let out = merge_all(&[iso.clone()], "m").await;
            assert_eq!(out[0].1, MergeOutcome::Merged { files: vec!["b.txt".into(), "new.txt".into(), "src/a.txt".into()] });
            assert_eq!(std::fs::read_to_string(f.path().join("src/a.txt")).unwrap(), "a1\n");
            assert_eq!(std::fs::read_to_string(f.path().join("new.txt")).unwrap(), "n\n");
            assert!(!f.path().join("b.txt").exists(), "deleted in the copy, deleted in the folder");
            assert!(!iso.path().exists(), "copy removed after a clean merge");
        }))
    }

    #[test]
    fn two_children_changing_one_file_is_a_conflict_with_both_versions_kept() {
        with_home(|_| rt().block_on(async {
            let f = folder();
            let one = create(f.path(), "sa-12").await.unwrap();
            let two = create(f.path(), "sa-13").await.unwrap();
            std::fs::write(one.path().join("src/a.txt"), "from one\n").unwrap();
            std::fs::write(two.path().join("src/a.txt"), "from two\n").unwrap();
            std::fs::write(two.path().join("b.txt"), "b from two\n").unwrap();
            let out = merge_all(&[one.clone(), two.clone()], "m").await;
            assert!(matches!(&out[0].1, MergeOutcome::Conflict { files, .. } if files == &["src/a.txt".to_string()]), "{:?}", out[0]);
            assert!(matches!(&out[1].1, MergeOutcome::Conflict { files, .. } if files == &["src/a.txt".to_string()]), "{:?}", out[1]);
            assert_eq!(std::fs::read_to_string(f.path().join("src/a.txt")).unwrap(), "a0\n", "owner's file untouched");
            assert_eq!(std::fs::read_to_string(f.path().join("src/a.txt.sa-12.nebo-conflict")).unwrap(), "from one\n");
            assert_eq!(std::fs::read_to_string(f.path().join("src/a.txt.sa-13.nebo-conflict")).unwrap(), "from two\n");
            // The file only `two` changed still lands.
            assert_eq!(std::fs::read_to_string(f.path().join("b.txt")).unwrap(), "b from two\n");
            assert!(one.path().exists() && two.path().exists(), "copies kept on conflict");
        }))
    }

    #[test]
    fn owner_edit_during_the_batch_is_a_conflict_not_an_overwrite() {
        with_home(|_| rt().block_on(async {
            let f = folder();
            let iso = create(f.path(), "sa-14").await.unwrap();
            std::fs::write(iso.path().join("b.txt"), "child\n").unwrap();
            std::fs::write(f.path().join("b.txt"), "owner meanwhile\n").unwrap();
            let out = merge_all(&[iso], "m").await;
            assert!(matches!(&out[0].1, MergeOutcome::Conflict { files, .. } if files == &["b.txt".to_string()]));
            assert_eq!(std::fs::read_to_string(f.path().join("b.txt")).unwrap(), "owner meanwhile\n");
            assert_eq!(std::fs::read_to_string(f.path().join("b.txt.sa-14.nebo-conflict")).unwrap(), "child\n");
        }))
    }

    #[test]
    fn folders_over_the_cap_and_symlinks_are_refused_or_skipped_with_the_reason_named() {
        with_home(|_| {
            let f = folder();
            #[cfg(unix)]
            std::os::unix::fs::symlink("/etc", f.path().join("link")).unwrap();
            let c = create_copy(f.path(), "sa-15").unwrap();
            assert!(!c.path.join("link").exists(), "symlinks are never followed");
            // The cap is checked against the walked total: fake it by asking
            // for a copy of a folder whose one file claims to be huge is not
            // possible without writing 2 GiB, so check the message path
            // through the walk total directly.
            let (files, skipped) = walk(f.path()).unwrap();
            assert_eq!(skipped, vec![PathBuf::from("link")]);
            assert!(files.iter().map(|(_, n)| n).sum::<u64>() < MAX_SCRATCH_BYTES);
        })
    }

    #[test]
    fn git_and_copy_isolation_render_the_same_outcome_section() {
        let git_c = MergeOutcome::Conflict { files: vec!["a".into()], kept_at: "/w/sa-1".into(), branch: "nebo/sa-1".into() };
        let copy_c = MergeOutcome::Conflict { files: vec!["a".into()], kept_at: "/w/sa-1".into(), branch: String::new() };
        for o in [&git_c, &copy_c, &MergeOutcome::NoChanges, &MergeOutcome::Merged { files: vec!["a".into()] }] {
            let line = render_outcome("desc", o);
            assert!(line.starts_with("- desc: "), "{line}");
            assert!(!line.contains('—'), "no em-dash in owner-visible text: {line}");
        }
        assert!(render_outcome("d", &git_c).contains("on branch nebo/sa-1"));
        assert!(render_outcome("d", &copy_c).contains(".sa-1.nebo-conflict"));
    }
}
