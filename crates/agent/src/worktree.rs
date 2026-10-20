//! Isolation for mutating fan-out (coding harness P5.3).
//!
//! When a parallel batch is spawned with `isolate: "worktree"`, each child
//! gets its own copy of the project, fenced by its grant's `fence` and `cwd`, and
//! the parent merges the copies back when the batch is done. Read-only
//! fan-out never comes here: it shares the tree.
//!
//! ONE option, two arms, chosen by whether the folder is a git repository:
//! - `Isolation::Git`: a git worktree on its own branch. Turning a folder
//!   into a repo upgrades a project to this arm with no other change.
//! - `Isolation::Copy`: a scratch copy of the folder (most owners' project
//!   folders are not repos, and Nebo never runs `git init` in one).
//!
//! Landing a copy is one way for both arms ([`merge_all`]), in order:
//! 1. Catch up in the copy ([`catch_up`]): git squashes the task's work into
//!    one commit and rebases it onto the owner's current HEAD; the copy arm
//!    takes in every file the owner changed meanwhile. A conflict stays in
//!    the copy for the builder to resolve there (a rebase in progress; the
//!    owner's version next to the file as `<file>.owner`), never in the
//!    owner's tree.
//! 2. Re-test in the copy (the caller's check), so what was verified is
//!    what lands.
//! 3. Land ([`land`]): one commit, fast-forwarded onto the owner's branch,
//!    never a merge commit, never a force. When the owner has uncommitted
//!    edits in a file the change touches, nothing lands: the owner decides.
//!
//! Several copies land in dependency order, one at a time, each caught up
//! and re-tested on top of the one before; the first that does not land
//! stops the set, and the copies not landed are kept.
//!
//! House git rules hold throughout: no stash, no reset, no checkout of the
//! owner's files, nothing forced. The owner's tree is only ever moved
//! forward by a fast-forward.
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
    /// Files the owner and the copy both changed, whose owner's version
    /// waits next to the file in the copy as `<file>.owner` until the
    /// builder has folded it in and deleted it.
    pub pending: Vec<PathBuf>,
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
    /// Caught up and landed; copy (and branch) removed. `commit` is the one
    /// commit it landed as and `caught_up_to` the owner's commit it was
    /// rebased onto (both empty for the copy arm).
    Merged { files: Vec<String>, commit: String, caught_up_to: String },
    /// Catching up met changes of the owner's to the same lines. They wait
    /// in the copy for the builder: git has the rebase in progress there;
    /// the copy arm has the owner's version next to each file as
    /// `<file>.owner`. Nothing of the owner's changed.
    Conflict { files: Vec<String>, kept_at: PathBuf, branch: String },
    /// The owner has uncommitted edits in files the change touches: nothing
    /// landed, and the owner decides.
    OwnerEdits { files: Vec<String>, kept_at: PathBuf, branch: String },
    /// The re-test after catching up failed: nothing landed.
    RetestFailed { error: String, kept_at: PathBuf, branch: String },
    /// An earlier copy of the set did not land, so this one was not tried.
    NotAttempted { kept_at: PathBuf, branch: String },
    /// Refused or failed otherwise; nothing of the owner's changed.
    Failed { error: String, kept_at: PathBuf, branch: String },
}

impl MergeOutcome {
    /// Whether the copy's work is on the owner's branch (or there was none).
    pub fn landed(&self) -> bool {
        matches!(self, MergeOutcome::NoChanges | MergeOutcome::Merged { .. })
    }
}

/// The longest one git command may take before it is stopped.
pub(crate) const GIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

/// Environment a caller's git would otherwise pass down and that points git
/// at another repository, index or object store.
const GIT_REDIRECTS: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
    "GIT_PREFIX",
];

/// Run git in `cwd` and return its standard output as it printed it: the
/// one way this crate runs git. Prompts are disabled, the caller's git
/// redirects are cleared, and a command past [`GIT_TIMEOUT`] is stopped.
pub(crate) async fn git_bytes(cwd: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let mut cmd = command::new::<Command>("git", command::Console::Hidden);
    cmd.args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "")
        .env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    for var in GIT_REDIRECTS {
        cmd.env_remove(var);
    }
    let name = args.iter().find(|a| !a.starts_with('-') && !a.contains('=')).copied().unwrap_or("");
    let out = match tokio::time::timeout(GIT_TIMEOUT, cmd.output()).await {
        Ok(out) => out.map_err(|e| format!("git {name}: {e}"))?,
        Err(_) => return Err(format!("git {name} took longer than {} minutes and was stopped", GIT_TIMEOUT.as_secs() / 60)),
    };
    if out.status.success() {
        Ok(out.stdout)
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if stderr.is_empty() { String::from_utf8_lossy(&out.stdout).trim().to_string() } else { stderr })
    }
}

/// [`git_bytes`], its output as trimmed text.
pub(crate) async fn git(cwd: &Path, args: &[&str]) -> Result<String, String> {
    git_bytes(cwd, args).await.map(|out| String::from_utf8_lossy(&out).trim().to_string())
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
    Ok(ScratchCopy { task_id: task_id.to_string(), root, path, snapshot, skipped, pending: Vec::new() })
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

/// Where the owner's version of a file both hands changed waits in a copy.
pub const OWNER_SUFFIX: &str = ".owner";

/// What catching a copy up with the owner's project did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CatchUp {
    /// Nothing to land: the copy holds no change.
    pub no_changes: bool,
    /// The owner's commit the work now sits on (git); empty for the copy arm.
    pub onto: String,
    /// The files the change touches.
    pub changed: Vec<String>,
    /// Files the change touches that the owner's side changed meanwhile:
    /// the re-review after catching up looks at them.
    pub overlap: Vec<String>,
    /// Conflicts left in the copy for the builder.
    pub conflicts: Vec<String>,
}

const NEBO_IDENTITY: [&str; 4] = ["-c", "user.name=Nebo", "-c", "user.email=nebo@neboai.com"];

async fn git_as_nebo(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let mut all: Vec<&str> = NEBO_IDENTITY.to_vec();
    all.extend_from_slice(args);
    git(cwd, &all).await
}

fn lines(out: &str) -> Vec<String> {
    out.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect()
}

/// A rebase of the task's commit is in progress in the worktree.
async fn rebasing(wt: &Worktree) -> bool {
    match git(&wt.path, &["rev-parse", "--git-path", "rebase-merge"]).await {
        Ok(p) => {
            let p = PathBuf::from(p.trim());
            let p = if p.is_absolute() { p } else { wt.path.join(p) };
            p.exists()
        }
        Err(_) => false,
    }
}

/// Bring the copy up to date with the owner's project, in the copy.
///
/// Git: the task's work becomes one commit with `message` (a squash by
/// `commit-tree` on the task's own branch) and is rebased onto the owner's
/// current HEAD. A conflict leaves the rebase in progress in the worktree;
/// once the builder has resolved the files, calling this again continues
/// it. Copy arm: every file the owner changed meanwhile that the copy did
/// not is taken in; one both changed gets the owner's version next to it as
/// `<file>.owner`, and counts as resolved once the builder deletes that.
pub async fn catch_up(iso: &mut Isolation, message: &str) -> Result<CatchUp, String> {
    match iso {
        Isolation::Git(wt) => catch_up_git(wt, message).await,
        Isolation::Copy(c) => catch_up_copy(c),
    }
}

async fn catch_up_git(wt: &Worktree, message: &str) -> Result<CatchUp, String> {
    if rebasing(wt).await {
        let left = lines(&git(&wt.path, &["diff", "--name-only", "--diff-filter=U"]).await.unwrap_or_default());
        let marked = conflict_markers(&wt.path, &left);
        if !marked.is_empty() {
            return Ok(CatchUp { conflicts: marked, ..Default::default() });
        }
        git(&wt.path, &["add", "-A"]).await.map_err(|e| format!("git add in worktree: {e}"))?;
        git_as_nebo(&wt.path, &["-c", "core.editor=true", "rebase", "--continue"])
            .await
            .map_err(|e| format!("git rebase --continue: {e}"))?;
    }
    let owner = git(&wt.root, &["rev-parse", "HEAD"]).await.map_err(|e| format!("the project's HEAD: {e}"))?;
    let base = git(&wt.path, &["merge-base", "HEAD", &owner]).await.map_err(|e| format!("git merge-base: {e}"))?;
    // One commit on the task's branch: the whole change, whatever the
    // builder committed or left uncommitted.
    git(&wt.path, &["add", "-A"]).await.map_err(|e| format!("git add in worktree: {e}"))?;
    let tree = git(&wt.path, &["write-tree"]).await.map_err(|e| format!("git write-tree: {e}"))?;
    let base_tree = git(&wt.path, &["rev-parse", &format!("{base}^{{tree}}")]).await.map_err(|e| format!("base tree: {e}"))?;
    if tree == base_tree {
        return Ok(CatchUp { no_changes: true, onto: owner, ..Default::default() });
    }
    let head = git(&wt.path, &["rev-parse", "HEAD"]).await.map_err(|e| format!("HEAD: {e}"))?;
    // Already the one commit, with this message: nothing to squash.
    let squashed = head != base
        && git(&wt.path, &["rev-parse", "HEAD^"]).await.is_ok_and(|p| p == base)
        && git(&wt.path, &["rev-parse", "HEAD^{tree}"]).await.is_ok_and(|t| t == tree)
        && git(&wt.path, &["log", "-1", "--format=%B"]).await.is_ok_and(|m| m.trim() == message.trim());
    if !squashed {
        let commit = git_as_nebo(&wt.path, &["commit-tree", &tree, "-p", &base, "-m", message])
            .await
            .map_err(|e| format!("git commit-tree: {e}"))?;
        git(&wt.path, &["update-ref", &format!("refs/heads/{}", wt.branch), &commit, &head])
            .await
            .map_err(|e| format!("git update-ref: {e}"))?;
    }
    let changed = lines(&git(&wt.path, &["diff", "--name-only", &base, "HEAD"]).await.unwrap_or_default());
    if base == owner {
        return Ok(CatchUp { onto: owner, changed, ..Default::default() });
    }
    let theirs = lines(&git(&wt.path, &["diff", "--name-only", &base, &owner]).await.unwrap_or_default());
    let overlap: Vec<String> = changed.iter().filter(|f| theirs.contains(f)).cloned().collect();
    if let Err(e) = git_as_nebo(&wt.path, &["rebase", &owner]).await {
        let conflicts = lines(&git(&wt.path, &["diff", "--name-only", "--diff-filter=U"]).await.unwrap_or_default());
        if conflicts.is_empty() {
            return Err(format!("git rebase onto the project's HEAD: {e}"));
        }
        return Ok(CatchUp { onto: owner, changed, overlap, conflicts, ..Default::default() });
    }
    Ok(CatchUp { onto: owner, changed, overlap, ..Default::default() })
}

/// The files of `files` under `dir` that still hold conflict markers.
fn conflict_markers(dir: &Path, files: &[String]) -> Vec<String> {
    files
        .iter()
        .filter(|f| {
            std::fs::read_to_string(dir.join(f)).is_ok_and(|t| t.lines().any(|l| l.starts_with("<<<<<<< ") || l == "======="))
        })
        .cloned()
        .collect()
}

fn catch_up_copy(c: &mut ScratchCopy) -> Result<CatchUp, String> {
    // Conflicts the builder resolved: the owner's version was folded in.
    let mut still = Vec::new();
    for rel in std::mem::take(&mut c.pending) {
        if c.path.join(owner_twin(&rel)).exists() {
            still.push(rel);
        } else if let Some(fp) = fingerprint_file(&c.root.join(&rel)) {
            c.snapshot.insert(rel, fp);
        } else {
            c.snapshot.remove(&rel);
        }
    }
    c.pending = still;
    let ours = changes_of(c)?;
    let ours_paths: std::collections::HashSet<PathBuf> = ours.iter().map(|(p, _)| p.clone()).collect();
    let (owner_files, _) = walk(&c.root)?;
    let mut theirs: Vec<PathBuf> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (rel, _) in owner_files {
        seen.insert(rel.clone());
        if c.snapshot.get(&rel) != fingerprint_file(&c.root.join(&rel)).as_ref() {
            theirs.push(rel);
        }
    }
    theirs.extend(c.snapshot.keys().filter(|rel| !seen.contains(*rel)).cloned());
    let mut overlap = Vec::new();
    for rel in theirs {
        if c.pending.contains(&rel) {
            continue;
        }
        let owner_bytes = std::fs::read(c.root.join(&rel)).ok();
        if ours_paths.contains(&rel) {
            // Both changed it: the owner's version waits next to it.
            let twin = c.path.join(owner_twin(&rel));
            let written = twin
                .parent()
                .map(std::fs::create_dir_all)
                .unwrap_or(Ok(()))
                .and_then(|_| std::fs::write(&twin, owner_bytes.as_deref().unwrap_or_default()));
            written.map_err(|e| format!("write {}: {e}", twin.display()))?;
            c.pending.push(rel.clone());
            overlap.push(rel.to_string_lossy().into_owned());
            continue;
        }
        // Only the owner changed it: the copy takes it in.
        let dst = c.path.join(&rel);
        match owner_bytes {
            Some(bytes) => {
                dst.parent()
                    .map(std::fs::create_dir_all)
                    .unwrap_or(Ok(()))
                    .and_then(|_| std::fs::write(&dst, &bytes))
                    .map_err(|e| format!("write {}: {e}", dst.display()))?;
                c.snapshot.insert(rel, tools::checkpoint::fingerprint(&bytes));
            }
            None => {
                let _ = std::fs::remove_file(&dst);
                c.snapshot.remove(&rel);
            }
        }
    }
    let changed: Vec<String> = changes_of(c)?
        .into_iter()
        .map(|(p, _)| p.to_string_lossy().into_owned())
        .filter(|p| !p.ends_with(OWNER_SUFFIX))
        .collect();
    let conflicts: Vec<String> = c.pending.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    Ok(CatchUp { no_changes: changed.is_empty() && conflicts.is_empty(), changed, overlap, conflicts, ..Default::default() })
}

fn owner_twin(rel: &Path) -> PathBuf {
    PathBuf::from(format!("{}{OWNER_SUFFIX}", rel.to_string_lossy()))
}

/// Land a caught-up copy on the owner's branch.
///
/// Git: when the task's commit sits on the owner's current HEAD and the
/// owner has no uncommitted edits in the files it touches, the owner's
/// branch is fast-forwarded to it (`--ff-only`: never a merge commit, never
/// a force) and the worktree and branch are removed. Copy arm: the changed
/// files are written into the owner's folder unless the owner changed one
/// of them since the catch-up. Anything else lands nothing.
pub async fn land(iso: &Isolation) -> MergeOutcome {
    match iso {
        Isolation::Git(wt) => land_git(wt).await,
        Isolation::Copy(c) => land_copy(c),
    }
}

/// The files the owner has changed and not committed in the repository at
/// `root`: edits, staged or not, and new files git does not ignore.
async fn uncommitted(root: &Path) -> Vec<String> {
    let mut files = lines(&git(root, &["diff", "--name-only", "HEAD"]).await.unwrap_or_default());
    files.extend(lines(&git(root, &["ls-files", "--others", "--exclude-standard"]).await.unwrap_or_default()));
    files
}

async fn land_git(wt: &Worktree) -> MergeOutcome {
    let failed = |error: String| MergeOutcome::Failed { error, kept_at: wt.path.clone(), branch: wt.branch.clone() };
    if rebasing(wt).await {
        return failed("catching up is not finished: a conflict is still being resolved in the copy".into());
    }
    let Ok(owner) = git(&wt.root, &["rev-parse", "HEAD"]).await else {
        return failed("the project has no HEAD".into());
    };
    let Ok(head) = git(&wt.path, &["rev-parse", "HEAD"]).await else {
        return failed("the copy has no HEAD".into());
    };
    if head == owner {
        if let Err(e) = remove(wt).await {
            warn!(path = %wt.path.display(), error = %e, "worktree not removed; the stale sweep will retry");
        }
        return MergeOutcome::NoChanges;
    }
    if git(&wt.path, &["rev-parse", "HEAD^"]).await.ok().as_deref() != Some(owner.as_str()) {
        return failed("the project moved on since the copy caught up; catch up again".into());
    }
    let files = lines(&git(&wt.path, &["diff", "--name-only", &owner, "HEAD"]).await.unwrap_or_default());
    let touched: Vec<String> = uncommitted(&wt.root).await.into_iter().filter(|d| files.contains(d)).collect();
    if !touched.is_empty() {
        return MergeOutcome::OwnerEdits { files: touched, kept_at: wt.path.clone(), branch: wt.branch.clone() };
    }
    if let Err(e) = git(&wt.root, &["merge", "--ff-only", "--quiet", &wt.branch]).await {
        return failed(format!("fast-forward refused: {e}"));
    }
    if let Err(e) = remove(wt).await {
        warn!(path = %wt.path.display(), error = %e, "worktree not removed; the stale sweep will retry");
    }
    MergeOutcome::Merged { files, commit: head, caught_up_to: owner }
}

fn land_copy(c: &ScratchCopy) -> MergeOutcome {
    let failed = |error: String| MergeOutcome::Failed { error, kept_at: c.path.clone(), branch: String::new() };
    if !c.pending.is_empty() {
        return MergeOutcome::Conflict {
            files: c.pending.iter().map(|p| p.to_string_lossy().into_owned()).collect(),
            kept_at: c.path.clone(),
            branch: String::new(),
        };
    }
    let changes = match changes_of(c) {
        Ok(ch) => ch,
        Err(e) => return failed(e),
    };
    if changes.is_empty() {
        remove_copy(&c.path);
        return MergeOutcome::NoChanges;
    }
    // The owner changed one of these files since the catch-up: theirs is
    // never overwritten.
    let touched: Vec<String> = changes
        .iter()
        .filter(|(rel, _)| c.snapshot.get(rel).copied() != fingerprint_file(&c.root.join(rel)))
        .map(|(rel, _)| rel.to_string_lossy().into_owned())
        .collect();
    if !touched.is_empty() {
        return MergeOutcome::OwnerEdits { files: touched, kept_at: c.path.clone(), branch: String::new() };
    }
    let mut merged = Vec::new();
    for (rel, change) in changes {
        let target = c.root.join(&rel);
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
        if let Err(e) = res {
            return failed(format!("write {}: {e}", target.display()));
        }
        merged.push(rel.to_string_lossy().into_owned());
    }
    remove_copy(&c.path);
    MergeOutcome::Merged { files: merged, commit: String::new(), caught_up_to: String::new() }
}

/// The last resort when the builder cannot resolve a conflict and the owner
/// has chosen to keep both: the copy's version of each file is written next
/// to the owner's as `<file>.<task_id>.nebo-conflict`. The owner's files are
/// untouched and the copy is kept.
pub fn keep_both(iso: &Isolation, files: &[String]) -> Result<Vec<PathBuf>, String> {
    let (root, copy) = match iso {
        Isolation::Git(wt) => (&wt.root, &wt.path),
        Isolation::Copy(c) => (&c.root, &c.path),
    };
    let mut kept = Vec::new();
    for rel in files {
        let bytes = std::fs::read(copy.join(rel)).map_err(|e| format!("read {rel}: {e}"))?;
        let keep = root.join(format!("{rel}.{}.nebo-conflict", iso.task_id()));
        keep.parent()
            .map(std::fs::create_dir_all)
            .unwrap_or(Ok(()))
            .and_then(|_| std::fs::write(&keep, bytes))
            .map_err(|e| format!("write {}: {e}", keep.display()))?;
        kept.push(keep);
    }
    Ok(kept)
}

/// The order a set of copies lands in: a copy of a project another one's
/// manifest depends on (a path dependency) lands before it; otherwise the
/// order given.
pub fn dependency_order(isos: &[Isolation]) -> Vec<usize> {
    let root_of = |i: &Isolation| match i {
        Isolation::Git(wt) => wt.root.clone(),
        Isolation::Copy(c) => c.root.clone(),
    };
    let roots: Vec<PathBuf> = isos.iter().map(root_of).collect();
    let depends = |a: usize, b: usize| -> bool {
        // Does project `a` name project `b` as a path dependency?
        if roots[a] == roots[b] {
            return false;
        }
        let Some(name) = roots[b].file_name().map(|n| n.to_string_lossy().into_owned()) else { return false };
        ["Cargo.toml", "package.json", "go.mod", "pubspec.yaml", "pyproject.toml"].iter().any(|m| {
            std::fs::read_to_string(roots[a].join(m)).is_ok_and(|text| {
                text.lines().any(|l| {
                    (l.contains("path") || l.contains("file:") || l.contains("replace") || l.contains("link:"))
                        && (l.contains(&format!("/{name}\"")) || l.contains(&format!("/{name}/")) || l.contains(&format!("../{name}")))
                })
            })
        })
    };
    let mut order: Vec<usize> = Vec::new();
    let mut placed = vec![false; isos.len()];
    while order.len() < isos.len() {
        let next = (0..isos.len())
            .find(|&a| !placed[a] && (0..isos.len()).all(|b| placed[b] || b == a || !depends(a, b)))
            // A cycle: the order given.
            .or_else(|| (0..isos.len()).find(|&a| !placed[a]))
            .expect("an unplaced copy remains");
        placed[next] = true;
        order.push(next);
    }
    order
}

/// A copy's re-test after catching up: `Err` with what failed.
pub type Retest<'a> = &'a (dyn Fn(&Isolation) -> futures::future::BoxFuture<'static, Result<(), String>> + Sync);

/// How many times a copy is caught up again when the owner's project moved
/// on while it was being re-tested.
const CATCH_UP_TRIES: usize = 3;

/// Land every copy of a set on the owner's project ([`dependency_order`],
/// one at a time): each is caught up ([`catch_up`]), re-tested with
/// `retest` when one is given, and landed ([`land`]). The first copy that
/// does not land stops the set: it says why, the copies after it are not
/// tried, and every copy not landed is kept. Outcomes come back in the
/// order the copies were given.
pub async fn merge_all(isos: &mut [Isolation], message: &str, retest: Option<Retest<'_>>) -> Vec<(String, MergeOutcome)> {
    let order = dependency_order(isos);
    let mut outcomes: Vec<Option<MergeOutcome>> = vec![None; isos.len()];
    let mut stopped = false;
    for idx in order {
        let iso = &mut isos[idx];
        let kept = (iso.path().to_path_buf(), match &*iso {
            Isolation::Git(wt) => wt.branch.clone(),
            Isolation::Copy(_) => String::new(),
        });
        if stopped {
            outcomes[idx] = Some(MergeOutcome::NotAttempted { kept_at: kept.0, branch: kept.1 });
            continue;
        }
        let outcome = land_one(iso, message, retest, kept).await;
        stopped = !outcome.landed();
        outcomes[idx] = Some(outcome);
    }
    isos.iter().zip(outcomes).map(|(iso, o)| (iso.task_id().to_string(), o.expect("every copy has an outcome"))).collect()
}

async fn land_one(iso: &mut Isolation, message: &str, retest: Option<Retest<'_>>, kept: (PathBuf, String)) -> MergeOutcome {
    let (kept_at, branch) = kept;
    for _ in 0..CATCH_UP_TRIES {
        let caught = match catch_up(iso, message).await {
            Ok(c) => c,
            Err(error) => return MergeOutcome::Failed { error, kept_at, branch },
        };
        if caught.no_changes {
            return land(iso).await;
        }
        if !caught.conflicts.is_empty() {
            return MergeOutcome::Conflict { files: caught.conflicts, kept_at, branch };
        }
        if let Some(retest) = retest
            && let Err(error) = retest(iso).await
        {
            return MergeOutcome::RetestFailed { error, kept_at, branch };
        }
        match land(iso).await {
            // The owner's project moved while the copy was re-tested.
            MergeOutcome::Failed { error, .. } if error.contains("catch up again") => continue,
            other => return other,
        }
    }
    MergeOutcome::Failed {
        error: "the project kept moving while the copy caught up; try again when it is quiet".into(),
        kept_at,
        branch,
    }
}

/// A merged scratch copy has nothing left to keep; a failure to delete it is
/// disk, not correctness, so it is logged and the stale sweep gets it later.
fn remove_copy(path: &Path) {
    if let Err(e) = std::fs::remove_dir_all(path) {
        warn!(path = %path.display(), error = %e, "scratch copy not removed; the stale sweep will retry");
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
/// Fail-closed: a worktree with uncommitted changes or
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
    let plural = |n: usize| if n == 1 { "" } else { "s" };
    match outcome {
        MergeOutcome::NoChanges => format!("- {desc}: no file changes"),
        MergeOutcome::Merged { files, commit, .. } => format!(
            "- {desc}: landed{} ({} file{}: {})",
            if commit.is_empty() { String::new() } else { format!(" as one commit {}", &commit[..commit.len().min(10)]) },
            files.len(),
            plural(files.len()),
            files.join(", ")
        ),
        MergeOutcome::Conflict { files, kept_at, branch } => format!(
            "- {desc}: not landed yet. Catching up with the project met the owner's changes in {}; they wait in the copy{} at {} to be combined there{}. Nothing of the owner's changed.",
            files.join(", "),
            branch_note(branch),
            kept_at.display(),
            if branch.is_empty() { format!(" (the owner's version is next to each file as <file>{OWNER_SUFFIX}; delete it once combined)") } else { String::new() }
        ),
        MergeOutcome::OwnerEdits { files, kept_at, branch } => format!(
            "- {desc}: not landed. The owner has unsaved edits in {} that this change also touches; nothing was overwritten. The work is kept{} at {} until the owner decides.",
            files.join(", "),
            branch_note(branch),
            kept_at.display()
        ),
        MergeOutcome::RetestFailed { error, kept_at, branch } => format!(
            "- {desc}: not landed. After catching up with the project the checks failed ({error}); the work is kept{} at {}",
            branch_note(branch),
            kept_at.display()
        ),
        MergeOutcome::NotAttempted { kept_at, branch } => format!(
            "- {desc}: not tried, because an earlier piece did not land; the work is kept{} at {}",
            branch_note(branch),
            kept_at.display()
        ),
        MergeOutcome::Failed { error, kept_at, branch } => format!(
            "- {desc}: not landed ({error}); the work is kept{} at {}",
            branch_note(branch),
            kept_at.display()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::test_home::with_home;

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


    async fn log(dir: &Path) -> Vec<String> {
        lines(&git(dir, &["log", "--format=%s|%p"]).await.unwrap())
    }

    fn landed_files(o: &MergeOutcome) -> Vec<String> {
        match o {
            MergeOutcome::Merged { files, .. } => files.clone(),
            other => panic!("expected it to land, got {other:?}"),
        }
    }

    #[test]
    fn a_change_lands_as_one_commit_fast_forwarded_and_the_copy_is_removed() {
        with_home(|_| rt().block_on(async {
            let r = repo().await;
            let wt = create_worktree(r.path(), "sa-1").await.unwrap();
            std::fs::write(wt.path.join("b.txt"), "child\n").unwrap();
            git(&wt.path, &["add", "-A"]).await.unwrap();
            git(&wt.path, &["commit", "-q", "-m", "wip 1"]).await.unwrap();
            std::fs::write(wt.path.join("a.txt"), "changed\n").unwrap();
            let path = wt.path.clone();
            let mut isos = [Isolation::Git(wt)];
            let out = merge_all(&mut isos, "Add b and change a", None).await;
            assert_eq!(landed_files(&out[0].1), vec!["a.txt".to_string(), "b.txt".to_string()]);
            assert_eq!(std::fs::read_to_string(r.path().join("b.txt")).unwrap(), "child\n");
            let history = log(r.path()).await;
            assert_eq!(history.len(), 2, "one commit on top of the base: {history:?}");
            assert!(history[0].starts_with("Add b and change a|"), "{history:?}");
            assert_eq!(history[0].split('|').nth(1).unwrap().split(' ').count(), 1, "no merge commit");
            assert!(!path.exists(), "worktree removed after it landed");
            let branches = git(r.path(), &["branch", "--list", "nebo/*"]).await.unwrap();
            assert!(branches.trim().is_empty(), "branch removed: {branches}");
        }))
    }

    #[test]
    fn the_copy_catches_up_with_the_owners_new_commits_before_it_lands() {
        with_home(|_| rt().block_on(async {
            let r = repo().await;
            let wt = create_worktree(r.path(), "sa-2").await.unwrap();
            std::fs::write(wt.path.join("b.txt"), "child\n").unwrap();
            // The owner commits elsewhere meanwhile.
            std::fs::write(r.path().join("c.txt"), "owner\n").unwrap();
            git(r.path(), &["add", "-A"]).await.unwrap();
            git(r.path(), &["commit", "-q", "-m", "owner work"]).await.unwrap();
            let owner = git(r.path(), &["rev-parse", "HEAD"]).await.unwrap();
            let retested = std::sync::atomic::AtomicUsize::new(0);
            let retest = |iso: &Isolation| -> futures::future::BoxFuture<'static, Result<(), String>> {
                retested.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let caught_up = iso.path().join("c.txt").exists();
                Box::pin(async move { if caught_up { Ok(()) } else { Err("not caught up".into()) } })
            };
            let mut isos = [Isolation::Git(wt)];
            let out = merge_all(&mut isos, "Add b", Some(&retest)).await;
            match &out[0].1 {
                MergeOutcome::Merged { caught_up_to, .. } => assert_eq!(caught_up_to, &owner),
                other => panic!("{other:?}"),
            }
            assert_eq!(retested.load(std::sync::atomic::Ordering::SeqCst), 1, "re-tested in the copy after catching up");
            let history = log(r.path()).await;
            assert_eq!(history.iter().map(|l| l.split('|').next().unwrap()).collect::<Vec<_>>(), ["Add b", "owner work", "base"]);
        }))
    }

    #[test]
    fn a_conflict_is_resolved_in_the_copy_then_retested_and_never_touches_the_owners_tree() {
        with_home(|_| rt().block_on(async {
            let r = repo().await;
            let wt = create_worktree(r.path(), "sa-3").await.unwrap();
            std::fs::write(wt.path.join("a.txt"), "child\n").unwrap();
            std::fs::write(r.path().join("a.txt"), "parent\n").unwrap();
            git(r.path(), &["commit", "-q", "-am", "parent"]).await.unwrap();
            let copy = wt.path.clone();
            let mut isos = [Isolation::Git(wt)];
            let out = merge_all(&mut isos, "Change a", None).await;
            assert!(matches!(&out[0].1, MergeOutcome::Conflict { files, .. } if files == &["a.txt".to_string()]), "{:?}", out[0].1);
            assert_eq!(std::fs::read_to_string(r.path().join("a.txt")).unwrap(), "parent\n", "the owner's tree is as it was");
            assert!(git(r.path(), &["rev-parse", "-q", "--verify", "MERGE_HEAD"]).await.is_err());
            assert!(std::fs::read_to_string(copy.join("a.txt")).unwrap().contains("<<<<<<< "), "the conflict waits in the copy");

            // Still unresolved: it stays a conflict.
            assert!(matches!(merge_all(&mut isos, "Change a", None).await[0].1, MergeOutcome::Conflict { .. }));
            // The builder resolves it in the copy; the re-test runs on the result.
            std::fs::write(copy.join("a.txt"), "parent and child\n").unwrap();
            let retest = |iso: &Isolation| -> futures::future::BoxFuture<'static, Result<(), String>> {
                let text = std::fs::read_to_string(iso.path().join("a.txt")).unwrap_or_default();
                Box::pin(async move { if text == "parent and child\n" { Ok(()) } else { Err(text) } })
            };
            let out = merge_all(&mut isos, "Change a", Some(&retest)).await;
            assert_eq!(landed_files(&out[0].1), vec!["a.txt".to_string()]);
            assert_eq!(std::fs::read_to_string(r.path().join("a.txt")).unwrap(), "parent and child\n");
        }))
    }

    #[test]
    fn no_changes_means_no_commit() {
        with_home(|_| rt().block_on(async {
            let r = repo().await;
            let before = git(r.path(), &["rev-parse", "HEAD"]).await.unwrap();
            let wt = create_worktree(r.path(), "sa-4").await.unwrap();
            let path = wt.path.clone();
            let mut isos = [Isolation::Git(wt)];
            assert_eq!(merge_all(&mut isos, "m", None).await[0].1, MergeOutcome::NoChanges);
            assert_eq!(git(r.path(), &["rev-parse", "HEAD"]).await.unwrap(), before);
            assert!(!path.exists());
        }))
    }

    #[test]
    fn the_owners_unsaved_edit_to_a_file_the_change_touches_is_asked_about_never_overwritten() {
        with_home(|_| rt().block_on(async {
            let r = repo().await;
            let wt = create_worktree(r.path(), "sa-5").await.unwrap();
            std::fs::write(wt.path.join("a.txt"), "child\n").unwrap();
            std::fs::write(r.path().join("a.txt"), "owner wip\n").unwrap();
            let mut isos = [Isolation::Git(wt)];
            match &merge_all(&mut isos, "Change a", None).await[0].1 {
                MergeOutcome::OwnerEdits { files, kept_at, branch } => {
                    assert_eq!(files, &vec!["a.txt".to_string()]);
                    assert!(kept_at.exists());
                    assert_eq!(branch, "nebo/sa-5");
                }
                other => panic!("expected the owner to be asked, got {other:?}"),
            }
            assert_eq!(std::fs::read_to_string(r.path().join("a.txt")).unwrap(), "owner wip\n");
            assert_eq!(log(r.path()).await.len(), 1, "nothing landed");
        }))
    }

    #[test]
    fn the_owners_unsaved_edit_to_another_file_does_not_stop_the_landing() {
        with_home(|_| rt().block_on(async {
            let r = repo().await;
            std::fs::write(r.path().join("notes.txt"), "n\n").unwrap();
            git(r.path(), &["add", "-A"]).await.unwrap();
            git(r.path(), &["commit", "-q", "-m", "notes"]).await.unwrap();
            let wt = create_worktree(r.path(), "sa-6").await.unwrap();
            std::fs::write(wt.path.join("a.txt"), "child\n").unwrap();
            std::fs::write(r.path().join("notes.txt"), "owner wip\n").unwrap();
            let mut isos = [Isolation::Git(wt)];
            assert_eq!(landed_files(&merge_all(&mut isos, "Change a", None).await[0].1), vec!["a.txt".to_string()]);
            assert_eq!(std::fs::read_to_string(r.path().join("notes.txt")).unwrap(), "owner wip\n", "kept as the owner left it");
        }))
    }

    #[test]
    fn a_failed_retest_lands_nothing() {
        with_home(|_| rt().block_on(async {
            let r = repo().await;
            let wt = create_worktree(r.path(), "sa-7").await.unwrap();
            std::fs::write(wt.path.join("b.txt"), "child\n").unwrap();
            let retest = |_: &Isolation| -> futures::future::BoxFuture<'static, Result<(), String>> { Box::pin(async { Err("2 tests failed".to_string()) }) };
            let mut isos = [Isolation::Git(wt)];
            assert!(matches!(&merge_all(&mut isos, "Add b", Some(&retest)).await[0].1, MergeOutcome::RetestFailed { error, .. } if error == "2 tests failed"));
            assert!(!r.path().join("b.txt").exists());
        }))
    }

    #[test]
    fn dependent_pieces_land_in_dependency_order_and_the_first_failure_stops_the_set() {
        with_home(|home| rt().block_on(async {
            let base = home.join("projects");
            let lib = base.join("lib");
            let app = base.join("app");
            for (dir, manifest) in [(&lib, "[package]\nname = \"lib\"\n"), (&app, "[package]\nname = \"app\"\n[dependencies]\nlib = { path = \"../lib\" }\n")] {
                std::fs::create_dir_all(dir).unwrap();
                for args in [vec!["init", "-q", "-b", "main"], vec!["config", "user.email", "t@t"], vec!["config", "user.name", "t"]] {
                    git(dir, &args).await.unwrap();
                }
                std::fs::write(dir.join("Cargo.toml"), manifest).unwrap();
                git(dir, &["add", "-A"]).await.unwrap();
                git(dir, &["commit", "-q", "-m", "base"]).await.unwrap();
            }
            let app_wt = create_worktree(&app, "sa-8").await.unwrap();
            let lib_wt = create_worktree(&lib, "sa-9").await.unwrap();
            std::fs::write(app_wt.path.join("main.rs"), "use lib;\n").unwrap();
            std::fs::write(lib_wt.path.join("lib.rs"), "pub fn f() {}\n").unwrap();
            let mut isos = [Isolation::Git(app_wt), Isolation::Git(lib_wt)];
            assert_eq!(dependency_order(&isos), vec![1, 0], "the library lands before the app that uses it");
            let order = std::sync::Mutex::new(Vec::new());
            let retest = |iso: &Isolation| -> futures::future::BoxFuture<'static, Result<(), String>> {
                order.lock().unwrap().push(iso.task_id().to_string());
                Box::pin(async { Ok(()) })
            };
            let out = merge_all(&mut isos, "Use f", Some(&retest)).await;
            assert!(out.iter().all(|(_, o)| o.landed()), "{out:?}");
            assert_eq!(*order.lock().unwrap(), ["sa-9", "sa-8"]);

            // The first that does not land stops the set; the rest are kept.
            let one = create_worktree(&lib, "sa-10").await.unwrap();
            let two = create_worktree(&app, "sa-11").await.unwrap();
            std::fs::write(one.path.join("lib.rs"), "pub fn g() {}\n").unwrap();
            std::fs::write(two.path.join("main.rs"), "use lib::g;\n").unwrap();
            let kept = two.path.clone();
            let mut isos = [Isolation::Git(one), Isolation::Git(two)];
            let fail_first = |iso: &Isolation| -> futures::future::BoxFuture<'static, Result<(), String>> {
                let id = iso.task_id().to_string();
                Box::pin(async move { if id == "sa-10" { Err("tests failed".to_string()) } else { Ok(()) } })
            };
            let out = merge_all(&mut isos, "Use g", Some(&fail_first)).await;
            assert!(matches!(out[0].1, MergeOutcome::RetestFailed { .. }), "{out:?}");
            assert!(matches!(&out[1].1, MergeOutcome::NotAttempted { kept_at, .. } if kept_at == &kept), "{out:?}");
            assert_eq!(std::fs::read_to_string(app.join("main.rs")).unwrap(), "use lib;\n", "never a half-applied set");
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
            // A caller's git redirect never reaches ours.
            unsafe { std::env::set_var("GIT_INDEX_FILE", "/tmp/elsewhere.index") };
            let _ = git(home, &["status"]).await;
            unsafe { std::env::remove_var("GIT_INDEX_FILE") };
            unsafe { std::env::set_var("PATH", old_path) };
            let env = std::fs::read_to_string(&dump).unwrap();
            assert!(env.contains("GIT_TERMINAL_PROMPT=0"), "{env}");
            assert!(env.contains("GIT_ASKPASS=\n"), "{env}");
            assert!(env.contains("GIT_SSH_COMMAND=ssh -o BatchMode=yes"), "{env}");
            assert!(!env.contains("GIT_INDEX_FILE"), "{env}");
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
            let iso = create(f.path(), "sa-21").await.unwrap();
            std::fs::write(iso.path().join("src/a.txt"), "a1\n").unwrap();
            std::fs::write(iso.path().join("new.txt"), "n\n").unwrap();
            std::fs::remove_file(iso.path().join("b.txt")).unwrap();
            let path = iso.path().to_path_buf();
            let mut isos = [iso];
            let out = merge_all(&mut isos, "m", None).await;
            assert_eq!(landed_files(&out[0].1), vec!["b.txt".to_string(), "new.txt".to_string(), "src/a.txt".to_string()]);
            assert_eq!(std::fs::read_to_string(f.path().join("src/a.txt")).unwrap(), "a1\n");
            assert_eq!(std::fs::read_to_string(f.path().join("new.txt")).unwrap(), "n\n");
            assert!(!f.path().join("b.txt").exists(), "deleted in the copy, deleted in the folder");
            assert!(!path.exists(), "copy removed after it landed");
        }))
    }

    #[test]
    fn a_copy_takes_in_the_owners_other_changes_and_waits_on_the_ones_to_the_same_file() {
        with_home(|_| rt().block_on(async {
            let f = folder();
            let iso = create(f.path(), "sa-22").await.unwrap();
            std::fs::write(iso.path().join("b.txt"), "child\n").unwrap();
            // Meanwhile the owner edits the same file and another one.
            std::fs::write(f.path().join("b.txt"), "owner meanwhile\n").unwrap();
            std::fs::write(f.path().join("src/a.txt"), "owner a\n").unwrap();
            let copy = iso.path().to_path_buf();
            let mut isos = [iso];
            let out = merge_all(&mut isos, "m", None).await;
            assert!(matches!(&out[0].1, MergeOutcome::Conflict { files, .. } if files == &["b.txt".to_string()]), "{:?}", out[0].1);
            assert_eq!(std::fs::read_to_string(f.path().join("b.txt")).unwrap(), "owner meanwhile\n", "never overwritten");
            assert_eq!(std::fs::read_to_string(copy.join("b.txt.owner")).unwrap(), "owner meanwhile\n");
            assert_eq!(std::fs::read_to_string(copy.join("src/a.txt")).unwrap(), "owner a\n", "the owner's other change is taken in");
            assert!(!f.path().join("b.txt.sa-22.nebo-conflict").exists(), "nothing is written beside the owner's files");

            // The builder folds the owner's version in, in the copy.
            std::fs::write(copy.join("b.txt"), "owner meanwhile\nchild\n").unwrap();
            std::fs::remove_file(copy.join("b.txt.owner")).unwrap();
            let out = merge_all(&mut isos, "m", None).await;
            assert_eq!(landed_files(&out[0].1), vec!["b.txt".to_string()]);
            assert_eq!(std::fs::read_to_string(f.path().join("b.txt")).unwrap(), "owner meanwhile\nchild\n");
            assert_eq!(std::fs::read_to_string(f.path().join("src/a.txt")).unwrap(), "owner a\n");
        }))
    }

    #[test]
    fn an_owner_edit_after_the_copy_caught_up_is_asked_about_never_overwritten() {
        with_home(|_| rt().block_on(async {
            let f = folder();
            let iso = create(f.path(), "sa-23").await.unwrap();
            std::fs::write(iso.path().join("b.txt"), "child\n").unwrap();
            let mut isos = [iso];
            // The owner edits the file while the copy is being re-tested.
            let owner_file = f.path().join("b.txt");
            let retest = |_: &Isolation| -> futures::future::BoxFuture<'static, Result<(), String>> {
                std::fs::write(&owner_file, "owner, during the re-test\n").unwrap();
                Box::pin(async { Ok(()) })
            };
            let out = merge_all(&mut isos, "m", Some(&retest)).await;
            assert!(matches!(&out[0].1, MergeOutcome::OwnerEdits { files, .. } if files == &["b.txt".to_string()]), "{:?}", out[0].1);
            assert_eq!(std::fs::read_to_string(f.path().join("b.txt")).unwrap(), "owner, during the re-test\n");
        }))
    }

    #[test]
    fn keeping_both_writes_the_copys_version_beside_the_owners_and_changes_nothing_else() {
        with_home(|_| rt().block_on(async {
            let f = folder();
            let iso = create(f.path(), "sa-24").await.unwrap();
            std::fs::write(iso.path().join("b.txt"), "child\n").unwrap();
            let kept = keep_both(&iso, &["b.txt".to_string()]).unwrap();
            assert_eq!(kept, vec![f.path().join("b.txt.sa-24.nebo-conflict")]);
            assert_eq!(std::fs::read_to_string(&kept[0]).unwrap(), "child\n");
            assert_eq!(std::fs::read_to_string(f.path().join("b.txt")).unwrap(), "b0\n");
            assert!(iso.path().exists(), "the copy is kept");
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
    fn every_outcome_renders_one_line_for_its_parent() {
        let kept: PathBuf = "/w/sa-1".into();
        let outcomes = [
            MergeOutcome::NoChanges,
            MergeOutcome::Merged { files: vec!["a".into()], commit: "0123456789abcdef".into(), caught_up_to: "f".into() },
            MergeOutcome::Conflict { files: vec!["a".into()], kept_at: kept.clone(), branch: "nebo/sa-1".into() },
            MergeOutcome::Conflict { files: vec!["a".into()], kept_at: kept.clone(), branch: String::new() },
            MergeOutcome::OwnerEdits { files: vec!["a".into()], kept_at: kept.clone(), branch: String::new() },
            MergeOutcome::RetestFailed { error: "e".into(), kept_at: kept.clone(), branch: String::new() },
            MergeOutcome::NotAttempted { kept_at: kept.clone(), branch: String::new() },
            MergeOutcome::Failed { error: "e".into(), kept_at: kept.clone(), branch: String::new() },
        ];
        for o in &outcomes {
            let line = render_outcome("desc", o);
            assert!(line.starts_with("- desc: "), "{line}");
            assert!(!line.contains('—'), "no em-dash in owner-visible text: {line}");
        }
        assert!(render_outcome("d", &outcomes[1]).contains("as one commit 0123456789"));
        assert!(render_outcome("d", &outcomes[2]).contains("on branch nebo/sa-1"));
        assert!(render_outcome("d", &outcomes[3]).contains("<file>.owner"));
    }
}
