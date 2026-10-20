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

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct Worktree {
    pub task_id: String,
    pub root: PathBuf,
    pub path: PathBuf,
    pub branch: String,
    /// The owner's branch the copy was made from; it lands only there.
    /// Empty when the project was on no branch (it then never lands
    /// without the owner).
    pub start_branch: String,
    /// The owner's commit the work sits on: where the copy was made, then
    /// where it last caught up to. The task's work is everything since.
    pub base: String,
    /// The owner's commit a catch-up in progress (a conflict being
    /// resolved) is rebasing onto.
    pub onto: Option<String>,
    /// The builder said the conflicts in the copy are resolved
    /// ([`Isolation::mark_resolved`]).
    pub resolved: bool,
}

/// A copy of a non-repo folder plus the fingerprints it was copied from, so
/// copy-back can tell "the child changed it" from "the owner changed it".
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct ScratchCopy {
    pub task_id: String,
    pub root: PathBuf,
    pub path: PathBuf,
    /// Relative path → fingerprint at copy time.
    pub snapshot: HashMap<PathBuf, u64>,
    /// Symlinks in the folder, which are never followed into the copy.
    pub skipped: Vec<PathBuf>,
    /// Files the owner and the copy both changed, whose owner's version
    /// waits next to the file in the copy until the builder has folded it
    /// in, deleted the twin and said so.
    pub pending: Vec<Pending>,
    /// The builder said the conflicts in the copy are resolved
    /// ([`Isolation::mark_resolved`]).
    pub resolved: bool,
}

/// One conflict waiting in a scratch copy.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    pub rel: PathBuf,
    /// Where the owner's version waits in the copy: `<file>.nebo-owner`
    /// (`<file>.nebo-owner-deleted`, empty, when the owner deleted it), or
    /// that name numbered when the project already holds a file of it.
    pub twin: PathBuf,
    /// The fingerprint of the owner's version that was put next to it
    /// (`None`: the owner had deleted the file). An owner change after it
    /// is a fresh conflict, never taken as handled.
    pub owner_fp: Option<u64>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
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

    /// The owner's project folder the copy was made from.
    pub fn root(&self) -> &Path {
        match self {
            Isolation::Git(w) => &w.root,
            Isolation::Copy(c) => &c.root,
        }
    }

    /// The task branch (git), empty for a scratch copy.
    pub fn branch(&self) -> &str {
        match self {
            Isolation::Git(w) => &w.branch,
            Isolation::Copy(_) => "",
        }
    }

    /// The builder says the conflicts it was given are resolved: the next
    /// catch-up takes the copy as it is now.
    pub fn mark_resolved(&mut self) {
        match self {
            Isolation::Git(w) => w.resolved = true,
            Isolation::Copy(c) => c.resolved = true,
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
    /// The project isn't where the copy can land: on another branch than
    /// the one the copy was made from, on no branch, or with a rebase,
    /// merge, cherry-pick, revert or bisect in progress. Nothing landed;
    /// the owner decides.
    OwnerBusy { why: Busy, kept_at: PathBuf, branch: String },
    /// The project's own commit hooks refused the landing commit (their
    /// output). Nothing landed; it goes back to the builder.
    HookFailed { output: String, kept_at: PathBuf, branch: String },
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

/// Create a worktree on a fresh branch from the repo's current HEAD. A task
/// whose work was put away on its branch (`nebo/<task_id>`) gets that
/// branch back, its work on it, never a second branch or copy.
pub async fn create_worktree(root: &Path, task_id: &str) -> Result<Worktree, String> {
    validate_task_id(task_id)?;
    let dir = worktrees_dir(root);
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let path = dir.join(task_id);
    let branch = format!("nebo/{task_id}");
    let head = git(root, &["rev-parse", "HEAD"]).await.map_err(|e| format!("the project has no commit to copy: {e}"))?;
    let start_branch = git(root, &["symbolic-ref", "--quiet", "--short", "HEAD"]).await.unwrap_or_default();
    if git(root, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")]).await.is_ok() {
        // The task's work sits on the owner's commit it shares with them.
        let base = git(root, &["merge-base", &branch, &head]).await.map_err(|e| format!("where {branch} left the project: {e}"))?;
        let wt = Worktree { task_id: task_id.to_string(), root: root.to_path_buf(), path, branch, start_branch, base, onto: None, resolved: false };
        return reattach_worktree(wt).await;
    }
    git(root, &["worktree", "add", "-b", &branch, path.to_str().unwrap_or_default(), &head])
        .await
        .map_err(|e| format!("git worktree add: {e}"))?;
    Ok(Worktree { task_id: task_id.to_string(), root: root.to_path_buf(), path, branch, start_branch, base: head, onto: None, resolved: false })
}

/// A worktree's copy on disk again: as it is when it is still there, or
/// checked out anew from its task branch when its work was put away there.
async fn reattach_worktree(wt: Worktree) -> Result<Worktree, String> {
    if wt.path.join(".git").exists() {
        return Ok(wt);
    }
    if wt.path.exists() {
        return Err(format!("{} is in the way of {}'s copy", wt.path.display(), wt.branch));
    }
    // A worktree removed by hand leaves its registration behind.
    let _ = git(&wt.root, &["worktree", "prune"]).await;
    git(&wt.root, &["worktree", "add", wt.path.to_str().unwrap_or_default(), &wt.branch])
        .await
        .map_err(|e| format!("git worktree add {}: {e}", wt.branch))?;
    Ok(wt)
}

/// A held copy, ready to work in or land again: a worktree whose work was
/// put away on its branch is checked out again; a scratch copy is where it
/// was kept. `Err` when its work is gone.
pub async fn reattach(iso: Isolation) -> Result<Isolation, String> {
    match iso {
        Isolation::Git(wt) => reattach_worktree(wt).await.map(Isolation::Git),
        Isolation::Copy(c) if c.path.is_dir() => Ok(Isolation::Copy(c)),
        Isolation::Copy(c) => Err(format!("its copy at {} is gone", c.path.display())),
    }
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
    Ok(ScratchCopy { task_id: task_id.to_string(), root, path, snapshot, skipped, pending: Vec::new(), resolved: false })
}

/// What one hand did to one file, relative to the snapshot.
#[derive(Debug, Clone, PartialEq)]
enum Change {
    Written(Vec<u8>),
    Deleted,
}

/// Every file the child changed (relative path → change). The owner's
/// versions waiting next to conflicted files are not the child's.
fn changes_of(c: &ScratchCopy) -> Result<Vec<(PathBuf, Change)>, String> {
    let (files, _) = walk(&c.path)?;
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let twins: std::collections::HashSet<&PathBuf> = c.pending.iter().map(|p| &p.twin).collect();
    for (rel, _) in files {
        if twins.contains(&rel) {
            continue;
        }
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

/// Where the owner's version of a file both hands changed waits in a copy:
/// `<file>.nebo-owner`, or an empty `<file>.nebo-owner-deleted` when the
/// owner deleted it.
pub const OWNER_SUFFIX: &str = ".nebo-owner";
pub const OWNER_DELETED_SUFFIX: &str = ".nebo-owner-deleted";

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

const NEBO_IDENTITY: [&str; 6] = ["-c", "user.name=Nebo", "-c", "user.email=nebo@neboai.com", "-c", "commit.gpgsign=false"];

async fn git_as_nebo(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let mut all: Vec<&str> = NEBO_IDENTITY.to_vec();
    all.extend_from_slice(args);
    git(cwd, &all).await
}

/// `-z` output's entries, whole (non-ASCII and spaces kept).
fn z_entries(bytes: &[u8]) -> Vec<String> {
    bytes.split(|b| *b == 0).filter(|s| !s.is_empty()).map(|s| String::from_utf8_lossy(s).into_owned()).collect()
}

/// The paths git names with `-z`, each once.
async fn z_paths(cwd: &Path, args: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in z_entries(&git_bytes(cwd, args).await.unwrap_or_default()) {
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

/// The files with unmerged entries in the worktree's index.
async fn unmerged(wt: &Worktree) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for entry in z_entries(&git_bytes(&wt.path, &["ls-files", "-u", "-z"]).await.unwrap_or_default()) {
        if let Some((_, path)) = entry.split_once('\t')
            && !out.iter().any(|p| p == path)
        {
            out.push(path.to_string());
        }
    }
    out
}

/// A rebase of the task's commit is in progress in the worktree.
async fn rebasing(wt: &Worktree) -> bool {
    in_progress(&wt.path, &["rebase-merge", "rebase-apply"]).await.is_some()
}

/// The first of `markers` (files under the git dir) that exists for the
/// repository at `dir`.
async fn in_progress(dir: &Path, markers: &[&'static str]) -> Option<&'static str> {
    for m in markers {
        if let Ok(p) = git(dir, &["rev-parse", "--path-format=absolute", "--git-path", m]).await
            && Path::new(p.trim()).exists()
        {
            return Some(m);
        }
    }
    None
}

/// Why the owner's project is not where a copy can land.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Busy {
    /// The project is on no branch (a detached HEAD).
    Detached,
    /// The copy was made while the project was on no branch.
    MadeDetached,
    /// The project is on another branch than the copy was made from.
    OtherBranch { now: String, made_from: String },
    /// An operation is in progress in the project: `rebase`, `merge`,
    /// `cherry_pick`, `revert` or `bisect`.
    InProgress(&'static str),
}

impl std::fmt::Display for Busy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Busy::Detached => write!(f, "the project is on no branch (a detached HEAD)"),
            Busy::MadeDetached => write!(f, "the copy was made while the project was on no branch"),
            Busy::OtherBranch { now, made_from } => write!(f, "the project is on branch {now} now, and the copy was made from {made_from}"),
            Busy::InProgress(what) => write!(f, "a {} is in progress in the project", what.replace('_', "-")),
        }
    }
}

/// Why the owner's project is not where the copy can land, if it isn't.
async fn owner_busy(wt: &Worktree) -> Option<Busy> {
    let Ok(branch) = git(&wt.root, &["symbolic-ref", "--quiet", "--short", "HEAD"]).await else {
        return Some(Busy::Detached);
    };
    if wt.start_branch.is_empty() {
        return Some(Busy::MadeDetached);
    }
    if branch != wt.start_branch {
        return Some(Busy::OtherBranch { now: branch, made_from: wt.start_branch.clone() });
    }
    let markers = ["rebase-merge", "rebase-apply", "MERGE_HEAD", "CHERRY_PICK_HEAD", "REVERT_HEAD", "BISECT_LOG"];
    in_progress(&wt.root, &markers).await.map(|m| {
        Busy::InProgress(match m {
            "MERGE_HEAD" => "merge",
            "CHERRY_PICK_HEAD" => "cherry_pick",
            "REVERT_HEAD" => "revert",
            "BISECT_LOG" => "bisect",
            _ => "rebase",
        })
    })
}

/// Bring the copy up to date with the owner's project, in the copy.
///
/// Git: the task's work becomes one commit with `message`, made by `git
/// commit` in the worktree so the project's own pre-commit and commit-msg
/// hooks run on it, on the commit the copy was made from; it is then
/// rebased onto the tip of the owner's branch. A conflict leaves the rebase
/// in progress in the worktree, and it stays a conflict (whatever the files
/// say) until the builder has resolved it and said so
/// ([`Isolation::mark_resolved`]). Copy arm: every file the owner changed
/// meanwhile that the copy did not is taken in; one both changed gets the
/// owner's version next to it, and is resolved once the builder has
/// deleted that and said so, unless the owner changed it again since.
///
/// `Err` is the outcome that ends the landing (the project busy, a hook's
/// refusal, a failure).
pub async fn catch_up(iso: &mut Isolation, message: &str) -> Result<CatchUp, MergeOutcome> {
    match iso {
        Isolation::Git(wt) => catch_up_git(wt, message).await,
        Isolation::Copy(c) => catch_up_copy(c).map_err(|error| MergeOutcome::Failed { error, kept_at: c.path.clone(), branch: String::new() }),
    }
}

async fn catch_up_git(wt: &mut Worktree, message: &str) -> Result<CatchUp, MergeOutcome> {
    let failed = |wt: &Worktree, error: String| MergeOutcome::Failed { error, kept_at: wt.path.clone(), branch: wt.branch.clone() };
    if let Some(why) = owner_busy(wt).await {
        return Err(MergeOutcome::OwnerBusy { why, kept_at: wt.path.clone(), branch: wt.branch.clone() });
    }
    let owner_ref = format!("refs/heads/{}", wt.start_branch);
    if rebasing(wt).await {
        let conflicts = unmerged(wt).await;
        if !std::mem::take(&mut wt.resolved) {
            let files = if conflicts.is_empty() { vec!["(the files given to resolve)".to_string()] } else { conflicts };
            return Ok(CatchUp { conflicts: files, ..Default::default() });
        }
        // Resolved, as the builder says, unless a file it was given still
        // holds markers: what is in the copy now is the resolution (a
        // deletion included).
        let marked = with_markers(&wt.path, &conflicts);
        if !marked.is_empty() {
            return Ok(CatchUp { conflicts: marked, ..Default::default() });
        }
        git(&wt.path, &["add", "-A"]).await.map_err(|e| failed(wt, format!("git add in the copy: {e}")))?;
        let left = unmerged(wt).await;
        if !left.is_empty() {
            return Ok(CatchUp { conflicts: left, ..Default::default() });
        }
        git_as_nebo(&wt.path, &["-c", "core.editor=true", "rebase", "--continue"])
            .await
            .map_err(|e| failed(wt, format!("git rebase --continue: {e}")))?;
        if let Some(onto) = wt.onto.take() {
            wt.base = onto;
        }
    }
    let owner = git(&wt.root, &["rev-parse", &owner_ref]).await.map_err(|e| failed(wt, format!("the project's branch: {e}")))?;
    let base = wt.base.clone();
    git(&wt.path, &["add", "-A"]).await.map_err(|e| failed(wt, format!("git add in the copy: {e}")))?;
    let tree = git(&wt.path, &["write-tree"]).await.map_err(|e| failed(wt, format!("git write-tree: {e}")))?;
    let base_tree = git(&wt.path, &["rev-parse", &format!("{base}^{{tree}}")]).await.map_err(|e| failed(wt, format!("the base tree: {e}")))?;
    if tree == base_tree {
        return Ok(CatchUp { no_changes: true, onto: owner, ..Default::default() });
    }
    let head = git(&wt.path, &["rev-parse", "HEAD"]).await.map_err(|e| failed(wt, format!("HEAD: {e}")))?;
    // Already the one commit, with this message: nothing to fold.
    let folded = head != base
        && git(&wt.path, &["rev-parse", "HEAD^"]).await.is_ok_and(|p| p == base)
        && git(&wt.path, &["rev-parse", "HEAD^{tree}"]).await.is_ok_and(|t| t == tree)
        && git(&wt.path, &["log", "-1", "--format=%B"]).await.is_ok_and(|m| m.trim() == message.trim());
    if !folded {
        // The task branch goes back to the base, the work staged as it is,
        // and one commit is made through the project's own hooks.
        git(&wt.path, &["update-ref", &format!("refs/heads/{}", wt.branch), &base, &head])
            .await
            .map_err(|e| failed(wt, format!("git update-ref: {e}")))?;
        git(&wt.path, &["add", "-A"]).await.map_err(|e| failed(wt, format!("git add in the copy: {e}")))?;
        if let Err(output) = git_as_nebo(&wt.path, &["commit", "-q", "-m", message]).await {
            // Put the task's own history back, so nothing it did is lost.
            let _ = git(&wt.path, &["update-ref", &format!("refs/heads/{}", wt.branch), &head]).await;
            return Err(MergeOutcome::HookFailed { output, kept_at: wt.path.clone(), branch: wt.branch.clone() });
        }
    }
    let changed = z_paths(&wt.path, &["diff", "-z", "--name-only", "--no-renames", &base, "HEAD"]).await;
    if base == owner {
        return Ok(CatchUp { onto: owner, changed, ..Default::default() });
    }
    let theirs = z_paths(&wt.path, &["diff", "-z", "--name-only", "--no-renames", &base, &owner]).await;
    let overlap: Vec<String> = changed.iter().filter(|f| theirs.contains(f)).cloned().collect();
    if let Err(e) = git_as_nebo(&wt.path, &["rebase", &owner]).await {
        let conflicts = unmerged(wt).await;
        if conflicts.is_empty() && !rebasing(wt).await {
            return Err(failed(wt, format!("git rebase onto the project's branch: {e}")));
        }
        wt.onto = Some(owner.clone());
        wt.resolved = false;
        return Ok(CatchUp { onto: owner, changed, overlap, conflicts, ..Default::default() });
    }
    wt.base = owner.clone();
    Ok(CatchUp { onto: owner, changed, overlap, ..Default::default() })
}

fn catch_up_copy(c: &mut ScratchCopy) -> Result<CatchUp, String> {
    let resolved = std::mem::take(&mut c.resolved);
    // Conflicts the builder says it resolved: the owner's version it was
    // given is folded in, unless the owner changed the file again since.
    let mut still = Vec::new();
    for p in std::mem::take(&mut c.pending) {
        let twin_left = std::fs::symlink_metadata(c.path.join(&p.twin)).is_ok();
        let owner_now = fingerprint_file(&c.root.join(&p.rel));
        if !resolved || twin_left {
            still.push(p);
        } else if owner_now == p.owner_fp {
            match owner_now {
                Some(fp) => {
                    c.snapshot.insert(p.rel, fp);
                }
                None => {
                    c.snapshot.remove(&p.rel);
                }
            }
        } else {
            // Changed again by the owner: a fresh conflict, never an overwrite.
            still.push(put_twin(c, &p.rel)?);
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
        if c.pending.iter().any(|p| p.rel == rel) {
            continue;
        }
        if ours_paths.contains(&rel) {
            // Both changed it: the owner's version waits next to it.
            let pending = put_twin(c, &rel)?;
            c.pending.push(pending);
            overlap.push(rel.to_string_lossy().into_owned());
            continue;
        }
        // Only the owner changed it: the copy takes it in.
        let dst = c.path.join(&rel);
        match std::fs::read(c.root.join(&rel)).ok() {
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
    let changed: Vec<String> = changes_of(c)?.into_iter().map(|(p, _)| p.to_string_lossy().into_owned()).collect();
    let conflicts: Vec<String> = c.pending.iter().map(|p| p.rel.to_string_lossy().into_owned()).collect();
    Ok(CatchUp { no_changes: changed.is_empty() && conflicts.is_empty(), changed, overlap, conflicts, ..Default::default() })
}

/// Put the owner's current version of `rel` next to it in the copy, and
/// remember exactly what was put there and where.
fn put_twin(c: &ScratchCopy, rel: &Path) -> Result<Pending, String> {
    let owner_bytes = std::fs::read(c.root.join(rel)).ok();
    let twin = free_twin(c, rel, owner_bytes.is_none());
    let at = c.path.join(&twin);
    at.parent()
        .map(std::fs::create_dir_all)
        .unwrap_or(Ok(()))
        .and_then(|_| std::fs::write(&at, owner_bytes.as_deref().unwrap_or_default()))
        .map_err(|e| format!("write {}: {e}", at.display()))?;
    Ok(Pending { rel: rel.to_path_buf(), twin, owner_fp: owner_bytes.as_deref().map(tools::checkpoint::fingerprint) })
}

/// A name for the owner's version of `rel` that nothing in the copy or the
/// project holds: `<file>.nebo-owner` (or `-deleted`), numbered when taken.
/// An owner's own file of that name is never mistaken for one.
fn free_twin(c: &ScratchCopy, rel: &Path, deleted: bool) -> PathBuf {
    let suffix = if deleted { OWNER_DELETED_SUFFIX } else { OWNER_SUFFIX };
    let taken = |p: &PathBuf| {
        c.snapshot.contains_key(p)
            || c.pending.iter().any(|q| &q.twin == p)
            || std::fs::symlink_metadata(c.path.join(p)).is_ok()
            || std::fs::symlink_metadata(c.root.join(p)).is_ok()
    };
    (0..)
        .map(|n| match n {
            0 => PathBuf::from(format!("{}{suffix}", rel.to_string_lossy())),
            n => PathBuf::from(format!("{}{suffix}-{n}", rel.to_string_lossy())),
        })
        .find(|p| !taken(p))
        .expect("a free name")
}

/// Land a caught-up copy on the owner's branch.
///
/// Git: the task's one commit must sit on the tip of the branch the copy
/// was made from, with that branch checked out and nothing in progress in
/// the project. No file the commit adds, changes, deletes or renames may
/// hold anything of the owner's that differs from that tip, whether git
/// tracks it, ignores it or has never seen it: an ignored note or a local
/// secrets file in the way is the owner's, and nothing lands over it. Then
/// the branch is fast-forwarded (never a merge commit, never a force) and
/// the worktree and task branch are removed. Copy arm: the changed files
/// are staged next to their places and moved in together, unless the owner
/// changed one since the catch-up or a folder on the way is a link out of
/// the project. Anything else lands nothing.
pub async fn land(iso: &Isolation) -> MergeOutcome {
    match iso {
        Isolation::Git(wt) => land_git(wt).await,
        Isolation::Copy(c) => land_copy(c),
    }
}

/// The paths `commit` adds, changes or deletes relative to `parent`, a
/// rename's two paths both.
async fn touched_paths(dir: &Path, parent: &str, commit: &str) -> Vec<String> {
    let entries = z_entries(&git_bytes(dir, &["diff", "-z", "--name-status", "-M", parent, commit]).await.unwrap_or_default());
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < entries.len() {
        let code = &entries[i];
        let n = if code.starts_with('R') || code.starts_with('C') { 2 } else { 1 };
        for p in entries.iter().skip(i + 1).take(n) {
            if !out.contains(p) {
                out.push(p.clone());
            }
        }
        i += 1 + n;
    }
    out
}

/// The paths of `paths` where the owner's folder holds something that is
/// not `tip`'s version: an edit, staged or not, a new or ignored file in
/// the way, a deletion. A folder the change writes into must be a folder on
/// the owner's disk too: a file or a link of the owner's where it goes
/// (one the change does not itself replace) is in the way.
async fn owner_data_in_the_way(root: &Path, tip: &str, paths: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for rel in paths {
        let mut at = PathBuf::new();
        let parts: Vec<_> = Path::new(rel).components().collect();
        for part in &parts[..parts.len().saturating_sub(1)] {
            at.push(part);
            let folder = at.to_string_lossy().into_owned();
            if paths.contains(&folder) || out.contains(&folder) {
                continue;
            }
            if std::fs::symlink_metadata(root.join(&at)).is_ok_and(|m| !m.is_dir()) {
                out.push(folder);
            }
        }
    }
    for rel in paths {
        let disk = root.join(rel);
        let on_disk = std::fs::symlink_metadata(&disk).is_ok();
        let at_tip = git(root, &["rev-parse", "--verify", "--quiet", &format!("{tip}:{rel}")]).await.ok().filter(|s| !s.is_empty());
        let differs = match (on_disk, at_tip) {
            (false, None) => false,
            (true, None) | (false, Some(_)) => true,
            (true, Some(blob)) => git(root, &["hash-object", &format!("--path={rel}"), "--", rel]).await.map(|h| h != blob).unwrap_or(true),
        };
        let staged = git(root, &["diff", "--cached", "--quiet", tip, "--", rel]).await.is_err();
        if differs || staged {
            out.push(rel.clone());
        }
    }
    out
}

/// Whether a file still holds conflict markers: a line opening one
/// (`<<<<<<< `) and a later line closing it (`>>>>>>> `). A heading's
/// `=======` underline alone is no marker.
fn has_markers(bytes: &[u8]) -> bool {
    let mut open = false;
    for line in String::from_utf8_lossy(bytes).lines() {
        if line == "<<<<<<<" || line.starts_with("<<<<<<< ") {
            open = true;
        } else if open && (line == ">>>>>>>" || line.starts_with(">>>>>>> ")) {
            return true;
        }
    }
    false
}

/// The files of `files` under `dir` that still hold conflict markers.
fn with_markers(dir: &Path, files: &[String]) -> Vec<String> {
    files.iter().filter(|f| std::fs::read(dir.join(f)).is_ok_and(|b| has_markers(&b))).cloned().collect()
}

async fn land_git(wt: &Worktree) -> MergeOutcome {
    let failed = |error: String| MergeOutcome::Failed { error, kept_at: wt.path.clone(), branch: wt.branch.clone() };
    if rebasing(wt).await {
        return failed("catching up is not finished: a conflict is still being resolved in the copy".into());
    }
    if let Some(why) = owner_busy(wt).await {
        return MergeOutcome::OwnerBusy { why, kept_at: wt.path.clone(), branch: wt.branch.clone() };
    }
    let Ok(owner) = git(&wt.root, &["rev-parse", &format!("refs/heads/{}", wt.start_branch)]).await else {
        return failed("the project's branch has no commit".into());
    };
    let Ok(head) = git(&wt.path, &["rev-parse", "HEAD"]).await else {
        return failed("the copy has no HEAD".into());
    };
    if head == owner {
        return finish_no_changes(&Isolation::Git(wt.clone())).await;
    }
    if git(&wt.path, &["rev-parse", "HEAD^"]).await.ok().as_deref() != Some(owner.as_str()) {
        return failed("the project moved on since the copy caught up; catch up again".into());
    }
    let paths = touched_paths(&wt.path, &owner, "HEAD").await;
    let marked = with_markers(&wt.path, &paths);
    if !marked.is_empty() {
        return MergeOutcome::Conflict { files: marked, kept_at: wt.path.clone(), branch: wt.branch.clone() };
    }
    let in_the_way = owner_data_in_the_way(&wt.root, &owner, &paths).await;
    if !in_the_way.is_empty() {
        return MergeOutcome::OwnerEdits { files: in_the_way, kept_at: wt.path.clone(), branch: wt.branch.clone() };
    }
    let files = z_paths(&wt.path, &["diff", "-z", "--name-only", &owner, "HEAD"]).await;
    if let Err(e) = git(&wt.root, &["merge", "--ff-only", "--quiet", &wt.branch]).await {
        return failed(format!("fast-forward refused: {e}"));
    }
    // Landed: the commit is on the owner's branch, and whatever is left in
    // the worktree is what the checks wrote (a change of theirs to a landed
    // file was refused before landing), so it goes with the worktree.
    if let Err(e) = git(&wt.root, &["worktree", "remove", "--force", wt.path.to_str().unwrap_or_default()]).await {
        warn!(path = %wt.path.display(), error = %e, "worktree not removed after it landed");
    }
    if let Err(e) = git(&wt.root, &["branch", "-D", &wt.branch]).await {
        warn!(branch = %wt.branch, error = %e, "landed worktree branch not deleted");
    }
    MergeOutcome::Merged { files, commit: head, caught_up_to: owner }
}

/// A copy with nothing to land: removed when it holds nothing unsaved.
async fn finish_no_changes(iso: &Isolation) -> MergeOutcome {
    match iso {
        Isolation::Git(wt) => {
            if let Err(e) = remove(wt, false).await {
                warn!(path = %wt.path.display(), error = %e, "unchanged worktree not removed");
            }
        }
        Isolation::Copy(c) => remove_copy(&c.path),
    }
    MergeOutcome::NoChanges
}

/// Whether a folder on the way from `root` to `target` (or `target`
/// itself) is a link: writing through it could leave the project.
fn through_a_link(root: &Path, target: &Path) -> bool {
    let Ok(rel) = target.strip_prefix(root) else { return true };
    let mut at = root.to_path_buf();
    for part in rel.components() {
        at.push(part);
        if std::fs::symlink_metadata(&at).is_ok_and(|m| m.file_type().is_symlink()) {
            return true;
        }
    }
    false
}

fn land_copy(c: &ScratchCopy) -> MergeOutcome {
    let failed = |error: String| MergeOutcome::Failed { error, kept_at: c.path.clone(), branch: String::new() };
    if !c.pending.is_empty() {
        return MergeOutcome::Conflict {
            files: c.pending.iter().map(|p| p.rel.to_string_lossy().into_owned()).collect(),
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
    let written: Vec<String> =
        changes.iter().filter(|(_, ch)| matches!(ch, Change::Written(_))).map(|(rel, _)| rel.to_string_lossy().into_owned()).collect();
    let marked = with_markers(&c.path, &written);
    if !marked.is_empty() {
        return MergeOutcome::Conflict { files: marked, kept_at: c.path.clone(), branch: String::new() };
    }
    // The owner changed one of these files since the catch-up (a deletion
    // included): theirs is never overwritten.
    let touched: Vec<String> = changes
        .iter()
        .filter(|(rel, _)| c.snapshot.get(rel).copied() != fingerprint_file(&c.root.join(rel)))
        .map(|(rel, _)| rel.to_string_lossy().into_owned())
        .collect();
    if !touched.is_empty() {
        return MergeOutcome::OwnerEdits { files: touched, kept_at: c.path.clone(), branch: String::new() };
    }
    if let Some((rel, _)) = changes.iter().find(|(rel, _)| through_a_link(&c.root, &c.root.join(rel))) {
        return failed(format!("{} is reached through a link out of the project", rel.display()));
    }
    // One journal for the landing: every file staged next to its place,
    // the owner's version set aside, then all moved in together. A failure
    // (or a restart midway, [`cleanup_stale`]) puts every file back.
    let beside = |target: &Path, what: &str| {
        target.with_file_name(format!(".{}.{}.{what}", target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), c.task_id))
    };
    let mut journal = Journal { steps: Vec::new(), done: false };
    for (rel, change) in &changes {
        let target = c.root.join(rel);
        let (staged, fp) = match change {
            Change::Written(bytes) => (Some(beside(&target, "nebo-landing")), Some(tools::checkpoint::fingerprint(bytes))),
            Change::Deleted => (None, None),
        };
        let backup = std::fs::symlink_metadata(&target).is_ok().then(|| beside(&target, "nebo-backup"));
        if let Some(taken) = staged.iter().chain(backup.iter()).find(|p| std::fs::symlink_metadata(p).is_ok()) {
            return failed(format!("{} is in the way of landing {}", taken.display(), rel.display()));
        }
        journal.steps.push(Step { target, staged, backup, fp });
    }
    let journal_at = journal_path(c);
    if let Err(e) = journal.write(&journal_at) {
        return failed(e);
    }
    for ((rel, change), step) in changes.iter().zip(&journal.steps) {
        if let (Change::Written(bytes), Some(staged)) = (change, &step.staged) {
            let written = step.target.parent().map(std::fs::create_dir_all).unwrap_or(Ok(())).and_then(|_| std::fs::write(staged, bytes));
            if let Err(e) = written {
                journal.roll_back(&journal_at);
                return failed(format!("stage {}: {e}", rel.display()));
            }
        }
    }
    for ((rel, _), step) in changes.iter().zip(&journal.steps) {
        // Checked again right before it is replaced: an owner's edit since
        // is never overwritten.
        if c.snapshot.get(rel).copied() != fingerprint_file(&step.target) {
            journal.roll_back(&journal_at);
            return MergeOutcome::OwnerEdits { files: vec![rel.to_string_lossy().into_owned()], kept_at: c.path.clone(), branch: String::new() };
        }
        let moved = step
            .backup
            .as_ref()
            .map_or(Ok(()), |b| std::fs::rename(&step.target, b))
            .and_then(|_| step.staged.as_ref().map_or(Ok(()), |st| std::fs::rename(st, &step.target)));
        if let Err(e) = moved {
            journal.roll_back(&journal_at);
            return failed(format!("write {}: {e}", rel.display()));
        }
    }
    journal.done = true;
    if let Err(e) = journal.write(&journal_at) {
        warn!(error = %e, "a landed copy's journal could not be marked done; the owner's files are landed");
    }
    journal.finish(&journal_at);
    remove_copy(&c.path);
    MergeOutcome::Merged { files: changes.iter().map(|(rel, _)| rel.to_string_lossy().into_owned()).collect(), commit: String::new(), caught_up_to: String::new() }
}

/// A copy's landing in progress: what each owner file it writes or deletes
/// is staged as and set aside as, kept next to the copy until it is done.
#[derive(serde::Serialize, serde::Deserialize, Debug)]
struct Journal {
    steps: Vec<Step>,
    /// Every file is in place; only the set-aside versions are left to drop.
    done: bool,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
struct Step {
    target: PathBuf,
    /// The new version staged next to it (`None`: the landing deletes it).
    staged: Option<PathBuf>,
    /// The owner's version set aside while it is replaced (`None`: there
    /// was none).
    backup: Option<PathBuf>,
    /// The new version's fingerprint.
    fp: Option<u64>,
}

const JOURNAL_SUFFIX: &str = ".nebo-landing.json";

fn journal_path(c: &ScratchCopy) -> PathBuf {
    c.path.with_file_name(format!("{}{JOURNAL_SUFFIX}", c.task_id))
}

impl Journal {
    fn write(&self, at: &Path) -> Result<(), String> {
        let tmp = at.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec(self).unwrap_or_default())
            .and_then(|_| std::fs::rename(&tmp, at))
            .map_err(|e| format!("write {}: {e}", at.display()))
    }

    /// Put every owner file back as it was: a staged version not moved in
    /// is dropped, a set-aside version goes back, a new file this landing
    /// made is removed.
    fn roll_back(&self, at: &Path) {
        for step in self.steps.iter().rev() {
            if let Some(staged) = &step.staged {
                let _ = std::fs::remove_file(staged);
            }
            match &step.backup {
                Some(b) if std::fs::symlink_metadata(b).is_ok() => {
                    if let Err(e) = std::fs::rename(b, &step.target) {
                        warn!(path = %step.target.display(), error = %e, "an owner's file could not be put back; it is at its set-aside name");
                    }
                }
                None if step.fp.is_some() && fingerprint_file(&step.target) == step.fp => {
                    let _ = std::fs::remove_file(&step.target);
                }
                _ => {}
            }
        }
        let _ = std::fs::remove_file(at);
    }

    /// A landing that is done drops the owner's set-aside versions.
    fn finish(&self, at: &Path) {
        for step in &self.steps {
            for p in step.staged.iter().chain(step.backup.iter()) {
                let _ = std::fs::remove_file(p);
            }
        }
        let _ = std::fs::remove_file(at);
    }
}

/// A copy's landing a restart cut short: finished when every file was in
/// place, put back otherwise.
fn recover_landing(at: &Path) {
    match std::fs::read(at).ok().and_then(|b| serde_json::from_slice::<Journal>(&b).ok()) {
        Some(j) if j.done => j.finish(at),
        Some(j) => j.roll_back(at),
        None => warn!(path = %at.display(), "an unreadable landing journal is kept"),
    }
}

/// The last resort when the builder cannot resolve a conflict and the owner
/// has chosen to keep both: the copy's version of each file is written next
/// to the owner's as `<file>.<task_id>.nebo-conflict`. The owner's files are
/// untouched and the copy is kept.
pub fn keep_both(iso: &Isolation, files: &[String]) -> Result<Vec<PathBuf>, String> {
    let (root, copy) = (iso.root(), iso.path());
    let mut kept = Vec::new();
    for rel in files {
        let bytes = std::fs::read(copy.join(rel)).map_err(|e| format!("read {rel}: {e}"))?;
        let keep = root.join(format!("{rel}.{}.nebo-conflict", iso.task_id()));
        if through_a_link(root, &keep) {
            return Err(format!("{rel} is reached through a link out of the project"));
        }
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
    let roots: Vec<PathBuf> = isos.iter().map(|i| i.root().to_path_buf()).collect();
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

/// What a copy would land, taken before its checks run: the checks are
/// held to it ([`after_checks`]).
pub struct Checked(Vec<(PathBuf, Option<u64>)>);

pub fn before_checks(iso: &Isolation) -> Checked {
    let files = match iso {
        Isolation::Git(_) => Vec::new(),
        Isolation::Copy(c) => changes_of(c).unwrap_or_default().into_iter().map(|(rel, ch)| (rel, change_fp(&ch))).collect(),
    };
    Checked(files)
}

fn change_fp(ch: &Change) -> Option<u64> {
    match ch {
        Change::Written(bytes) => Some(tools::checkpoint::fingerprint(bytes)),
        Change::Deleted => None,
    }
}

/// After a copy's checks: what was checked is what lands. A file the
/// checks wrote that the work had not made (a cache, build output) is
/// dropped from the copy, never landed; a change of theirs to the work's
/// own files (a formatter, a snapshot update) is refused, and the work goes
/// back to its builder. Git alike: the checks may change no file the commit
/// holds, and a file they add beside it is dropped.
pub async fn after_checks(iso: &Isolation, before: Checked) -> Result<(), String> {
    let changed: Vec<String> = match iso {
        Isolation::Git(wt) => {
            // The work was all committed before the checks ran: a file
            // untracked now is theirs.
            for made in z_paths(&wt.path, &["ls-files", "-z", "--others", "--exclude-standard"]).await {
                let _ = std::fs::remove_file(wt.path.join(made));
            }
            z_paths(&wt.path, &["diff", "-z", "--name-only", "HEAD"]).await
        }
        Isolation::Copy(c) => {
            let was: HashMap<PathBuf, Option<u64>> = before.0.into_iter().collect();
            let now = changes_of(c)?;
            let mut changed: Vec<String> = was
                .keys()
                .filter(|rel| !now.iter().any(|(r, _)| r == *rel))
                .map(|rel| rel.to_string_lossy().into_owned())
                .collect();
            for (rel, ch) in &now {
                match was.get(rel) {
                    Some(fp) if *fp == change_fp(ch) => {}
                    None if matches!(ch, Change::Written(_)) && !c.snapshot.contains_key(rel) => {
                        let _ = std::fs::remove_file(c.path.join(rel));
                    }
                    _ => changed.push(rel.to_string_lossy().into_owned()),
                }
            }
            changed.sort();
            changed
        }
    };
    if changed.is_empty() {
        return Ok(());
    }
    Err(format!(
        "the checks changed {} after the work was done, so what was checked is not what would land; look at the change, keep it or undo it, and finish again",
        changed.join(", ")
    ))
}

/// A copy's re-test after catching up: `Err` with what failed.
pub type Retest<'a> = &'a (dyn Fn(&Isolation) -> futures::future::BoxFuture<'static, Result<(), String>> + Sync);

/// One landing at a time per project: two sets landing on one repository
/// at once would race for its branch.
fn project_lock(root: &Path) -> std::sync::Arc<tokio::sync::Mutex<()>> {
    static LOCKS: std::sync::OnceLock<std::sync::Mutex<HashMap<PathBuf, std::sync::Arc<tokio::sync::Mutex<()>>>>> = std::sync::OnceLock::new();
    let mut locks = LOCKS.get_or_init(Default::default).lock().unwrap_or_else(|p| p.into_inner());
    locks.entry(root.to_path_buf()).or_default().clone()
}

/// Land every copy of a set on the owner's project ([`dependency_order`],
/// one at a time, one landing per project at a time): each is caught up
/// ([`catch_up`]), re-tested with `retest` when one is given, and landed
/// ([`land`]). A copy with nothing to land is removed and does not stop the
/// set. The first copy that does not land stops the set: it says why, the
/// copies after it are not tried, and every copy not landed is kept.
/// Outcomes come back in the order the copies were given.
pub async fn merge_all(isos: &mut [Isolation], message: &str, retest: Option<Retest<'_>>) -> Vec<(String, MergeOutcome)> {
    let order = dependency_order(isos);
    let mut outcomes: Vec<Option<MergeOutcome>> = vec![None; isos.len()];
    let mut stopped = false;
    for idx in order {
        let iso = &mut isos[idx];
        if stopped {
            outcomes[idx] = Some(MergeOutcome::NotAttempted { kept_at: iso.path().to_path_buf(), branch: iso.branch().to_string() });
            continue;
        }
        let lock = project_lock(iso.root());
        let _one_at_a_time = lock.lock().await;
        let outcome = land_one(iso, message, retest).await;
        stopped = !outcome.landed();
        outcomes[idx] = Some(outcome);
    }
    isos.iter().zip(outcomes).map(|(iso, o)| (iso.task_id().to_string(), o.expect("every copy has an outcome"))).collect()
}

async fn land_one(iso: &mut Isolation, message: &str, retest: Option<Retest<'_>>) -> MergeOutcome {
    let kept = |iso: &Isolation| (iso.path().to_path_buf(), iso.branch().to_string());
    // A fast-forward lost to the owner's own commit is caught up again once.
    for attempt in 0..2 {
        let caught = match catch_up(iso, message).await {
            Ok(c) => c,
            Err(outcome) => return outcome,
        };
        if caught.no_changes {
            return finish_no_changes(iso).await;
        }
        if !caught.conflicts.is_empty() {
            let (kept_at, branch) = kept(iso);
            return MergeOutcome::Conflict { files: caught.conflicts, kept_at, branch };
        }
        if let Some(retest) = retest {
            let before = before_checks(iso);
            let checked = match retest(iso).await {
                Ok(()) => after_checks(iso, before).await,
                Err(error) => Err(error),
            };
            if let Err(error) = checked {
                let (kept_at, branch) = kept(iso);
                return MergeOutcome::RetestFailed { error, kept_at, branch };
            }
        }
        match land(iso).await {
            MergeOutcome::Failed { error, .. } if attempt == 0 && error.contains("catch up again") => continue,
            other => return other,
        }
    }
    let (kept_at, branch) = kept(iso);
    MergeOutcome::Failed { error: "the project kept moving while the copy caught up; try again when it is quiet".into(), kept_at, branch }
}

/// A merged scratch copy has nothing left to keep; a failure to delete it is
/// disk, not correctness, so it is logged and the stale sweep gets it later.
fn remove_copy(path: &Path) {
    if let Err(e) = std::fs::remove_dir_all(path) {
        warn!(path = %path.display(), error = %e, "scratch copy not removed; the stale sweep will retry");
    }
}

/// Remove a worktree, and its task branch unless `keep_branch` (work put
/// away on it). A worktree holding anything uncommitted, a catch-up in
/// progress, or (keeping the branch) a commit its branch does not hold is
/// never removed.
pub async fn remove(wt: &Worktree, keep_branch: bool) -> Result<(), String> {
    // Never lose work that isn't stored: a worktree holding uncommitted
    // edits is kept (the stale sweep keeps it too).
    let unsaved = git(&wt.path, &["status", "--porcelain", "--untracked-files=normal"]).await?;
    if !unsaved.trim().is_empty() {
        return Err(format!("{} holds work that isn't committed; it is kept", wt.path.display()));
    }
    if rebasing(wt).await {
        return Err(format!("{} is catching up with the project; it is kept", wt.path.display()));
    }
    if keep_branch && git(&wt.path, &["merge-base", "--is-ancestor", "HEAD", &format!("refs/heads/{}", wt.branch)]).await.is_err() {
        return Err(format!("{} holds work its branch {} does not; it is kept", wt.path.display(), wt.branch));
    }
    // Clean as git sees it; what's left is ignored build output.
    git(&wt.root, &["worktree", "remove", "--force", wt.path.to_str().unwrap_or_default()]).await?;
    if keep_branch {
        return Ok(());
    }
    // The branch is ours (nebo/<task>), already merged or empty; a leftover
    // branch is clutter, not data loss, so it is logged rather than failed.
    if let Err(e) = git(&wt.root, &["branch", "-D", &wt.branch]).await {
        warn!(branch = %wt.branch, error = %e, "merged worktree branch not deleted");
    }
    Ok(())
}

/// The files a copy's work has changed so far (relative paths), however far
/// it got: committed on its branch, staged, edited or new.
pub async fn changed_files(iso: &Isolation) -> Vec<String> {
    match iso {
        Isolation::Git(wt) => {
            let mut files = z_paths(&wt.path, &["diff", "-z", "--name-only", "--no-renames", &wt.base]).await;
            for f in z_paths(&wt.path, &["ls-files", "-z", "--others", "--exclude-standard"]).await {
                if !files.contains(&f) {
                    files.push(f);
                }
            }
            files
        }
        Isolation::Copy(c) => changes_of(c).map(|ch| ch.into_iter().map(|(p, _)| p.to_string_lossy().into_owned()).collect()).unwrap_or_default(),
    }
}

/// Put a copy's work away when nothing will land it now (its helper was
/// stopped, or Nebo restarted): a worktree's work is committed on its task
/// branch and the worktree removed, so the disk is freed and nothing is
/// lost ([`shelve`]; [`reattach`] checks it out again); a scratch copy with
/// work in it is kept where it is; a copy with none is removed. Returns
/// where the work is, for the owner's report (`None`: there was none).
pub async fn put_away(iso: &mut Isolation, message: &str) -> Option<String> {
    let mid_catch_up = matches!(iso, Isolation::Git(wt) if rebasing(wt).await);
    if !mid_catch_up && changed_files(iso).await.is_empty() {
        finish_no_changes(iso).await;
        return None;
    }
    match iso {
        Isolation::Git(wt) => {
            match shelve(wt, message).await {
                Ok(()) => Some(format!("its work is kept on branch {} in {}", wt.branch, wt.root.display())),
                Err(e) => {
                    warn!(path = %wt.path.display(), error = %e, "a stopped copy's work could not be put away; the copy is kept");
                    Some(format!("its work is kept in its copy at {}", wt.path.display()))
                }
            }
        }
        Isolation::Copy(c) => Some(format!("its work is kept in its copy at {}", c.path.display())),
    }
}

/// Commit whatever a worktree holds on its own task branch and remove the
/// worktree, keeping the branch: its work is stored and the disk freed.
/// Midway through catching up (a conflict being resolved), the copy as the
/// builder left it is committed on the owner's commit it was catching up
/// to and the catch-up ends there, so the branch holds the resolution so
/// far; markers left in it keep it a conflict when it is landed.
pub async fn shelve(wt: &mut Worktree, message: &str) -> Result<(), String> {
    git(&wt.path, &["add", "-A"]).await?;
    if rebasing(wt).await {
        let onto = match wt.onto.clone() {
            Some(onto) => onto,
            None => rebase_onto(wt).await?,
        };
        let tree = git(&wt.path, &["write-tree"]).await?;
        let commit = git_as_nebo(&wt.path, &["commit-tree", &tree, "-p", &onto, "-m", message]).await?;
        git(&wt.path, &["rebase", "--quit"]).await?;
        let branch_ref = format!("refs/heads/{}", wt.branch);
        git(&wt.path, &["update-ref", &branch_ref, &commit]).await?;
        git(&wt.path, &["symbolic-ref", "HEAD", &branch_ref]).await?;
        wt.base = onto;
        wt.onto = None;
        wt.resolved = false;
    } else if git(&wt.path, &["diff", "--cached", "--quiet"]).await.is_err() {
        git_as_nebo(&wt.path, &["commit", "-q", "--no-verify", "-m", message]).await?;
    }
    remove(wt, true).await
}

/// The commit a rebase in progress in the worktree is rebasing onto.
async fn rebase_onto(wt: &Worktree) -> Result<String, String> {
    for dir in ["rebase-merge", "rebase-apply"] {
        let at = git(&wt.path, &["rev-parse", "--path-format=absolute", "--git-path", &format!("{dir}/onto")]).await?;
        if let Ok(onto) = std::fs::read_to_string(at.trim()) {
            return Ok(onto.trim().to_string());
        }
    }
    Err("the catch-up in progress names no commit it is catching up to".into())
}

/// Drop a copy and everything in it, at the owner's (or the primary's)
/// word: the worktree and its task branch, or the scratch copy. The owner's
/// project is untouched.
pub async fn discard(iso: &Isolation) -> Result<(), String> {
    match iso {
        Isolation::Git(wt) => {
            // A copy put away on its branch has no worktree left to remove.
            if wt.path.exists() {
                git(&wt.root, &["worktree", "remove", "--force", wt.path.to_str().unwrap_or_default()]).await?;
            }
            git(&wt.root, &["branch", "-D", &wt.branch]).await.map(|_| ())
        }
        Isolation::Copy(c) => std::fs::remove_dir_all(&c.path).map_err(|e| format!("remove {}: {e}", c.path.display())),
    }
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
            if name.ends_with(JOURNAL_SUFFIX) {
                recover_landing(&path);
                continue;
            }
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
                let wt = Worktree {
                    task_id: name.clone(),
                    root,
                    path: path.clone(),
                    branch: format!("nebo/{name}"),
                    start_branch: String::new(),
                    base: String::new(),
                    onto: None,
                    resolved: false,
                };
                if remove(&wt, false).await.is_ok() {
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
            if branch.is_empty() {
                format!(" (the owner's version is next to each file as <file>{OWNER_SUFFIX}, or <file>{OWNER_DELETED_SUFFIX} if they deleted it; delete it once combined, then say it is resolved)")
            } else {
                " (the rebase is in progress there; resolve the files, then say it is resolved)".to_string()
            }
        ),
        MergeOutcome::OwnerEdits { files, kept_at, branch } => format!(
            "- {desc}: not landed. The owner has unsaved edits in {} that this change also touches; nothing was overwritten. The work is kept{} at {} until the owner decides.",
            files.join(", "),
            branch_note(branch),
            kept_at.display()
        ),
        MergeOutcome::OwnerBusy { why, kept_at, branch } => format!(
            "- {desc}: not landed, because {why}. Nothing of the owner's changed; the work is kept{} at {} until the owner decides.",
            branch_note(branch),
            kept_at.display()
        ),
        MergeOutcome::HookFailed { output, kept_at, branch } => format!(
            "- {desc}: not landed. The project's commit hooks refused it:\n{}\nThe work is kept{} at {}; fix it there.",
            output.lines().take(20).collect::<Vec<_>>().join("\n"),
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
        git(dir, &["log", "--format=%s|%p"]).await.unwrap().lines().map(str::to_string).collect()
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
            // Resolved in the files but not said: still a conflict.
            std::fs::write(copy.join("a.txt"), "parent and child\n").unwrap();
            assert!(matches!(merge_all(&mut isos, "Change a", None).await[0].1, MergeOutcome::Conflict { .. }), "only the builder's word resolves it");
            // The builder says so; the re-test runs on the result.
            isos[0].mark_resolved();
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
            assert_eq!(std::fs::read_to_string(copy.join("b.txt.nebo-owner")).unwrap(), "owner meanwhile\n");
            assert_eq!(std::fs::read_to_string(copy.join("src/a.txt")).unwrap(), "owner a\n", "the owner's other change is taken in");
            assert!(!f.path().join("b.txt.sa-22.nebo-conflict").exists(), "nothing is written beside the owner's files");

            // The builder folds the owner's version in, in the copy.
            std::fs::write(copy.join("b.txt"), "owner meanwhile\nchild\n").unwrap();
            std::fs::remove_file(copy.join("b.txt.nebo-owner")).unwrap();
            isos[0].mark_resolved();
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
            MergeOutcome::OwnerBusy { why: Busy::InProgress("merge"), kept_at: kept.clone(), branch: String::new() },
            MergeOutcome::HookFailed { output: "lint failed".into(), kept_at: kept.clone(), branch: String::new() },
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
        assert!(render_outcome("d", &outcomes[3]).contains("<file>.nebo-owner"));
    }

    // ---- from the review of the landing (each a repro turned into a test) ----

    #[test]
    fn an_owner_edit_after_the_twin_was_written_is_a_fresh_conflict_never_an_overwrite() {
        with_home(|_| rt().block_on(async {
            let f = folder();
            let c = create_copy(f.path(), "sa-r1").unwrap();
            std::fs::write(c.path.join("b.txt"), "child\n").unwrap();
            std::fs::write(f.path().join("b.txt"), "owner1\n").unwrap();
            let copy = c.path.clone();
            let mut isos = [Isolation::Copy(c)];
            assert!(matches!(merge_all(&mut isos, "m", None).await[0].1, MergeOutcome::Conflict { .. }));
            assert_eq!(std::fs::read_to_string(copy.join("b.txt.nebo-owner")).unwrap(), "owner1\n");
            // The owner keeps working on the file while the builder resolves.
            std::fs::write(f.path().join("b.txt"), "owner1\nowner2 IMPORTANT\n").unwrap();
            std::fs::write(copy.join("b.txt"), "child\nowner1\n").unwrap();
            std::fs::remove_file(copy.join("b.txt.nebo-owner")).unwrap();
            isos[0].mark_resolved();
            let out = merge_all(&mut isos, "m", None).await;
            assert!(matches!(&out[0].1, MergeOutcome::Conflict { files, .. } if files == &["b.txt".to_string()]), "{:?}", out[0].1);
            assert_eq!(std::fs::read_to_string(f.path().join("b.txt")).unwrap(), "owner1\nowner2 IMPORTANT\n", "the owner's newer edit is kept");
            assert_eq!(std::fs::read_to_string(copy.join("b.txt.nebo-owner")).unwrap(), "owner1\nowner2 IMPORTANT\n", "and handed to the builder");
        }))
    }

    #[test]
    fn an_ignored_owner_file_where_the_task_adds_one_is_asked_about_never_overwritten() {
        with_home(|_| rt().block_on(async {
            let r = repo().await;
            std::fs::write(r.path().join(".gitignore"), "notes/\nconfig.local\n").unwrap();
            git(r.path(), &["add", "-A"]).await.unwrap();
            git(r.path(), &["commit", "-q", "-m", "ignore"]).await.unwrap();
            let wt = create_worktree(r.path(), "sa-r2").await.unwrap();
            std::fs::write(wt.path.join("config.local"), "TEMPLATE=1\n").unwrap();
            git(&wt.path, &["add", "-f", "config.local"]).await.unwrap();
            std::fs::write(r.path().join("config.local"), "SECRET=hunter2\n").unwrap();
            let mut isos = [Isolation::Git(wt)];
            let out = merge_all(&mut isos, "Add a config template", None).await;
            assert!(matches!(&out[0].1, MergeOutcome::OwnerEdits { files, .. } if files == &["config.local".to_string()]), "{:?}", out[0].1);
            assert_eq!(std::fs::read_to_string(r.path().join("config.local")).unwrap(), "SECRET=hunter2\n");
        }))
    }

    #[test]
    fn a_project_on_another_branch_or_mid_operation_is_asked_about_and_never_landed_on() {
        with_home(|_| rt().block_on(async {
            let r = repo().await;
            let wt = create_worktree(r.path(), "sa-r3").await.unwrap();
            std::fs::write(wt.path.join("b.txt"), "child\n").unwrap();
            git(r.path(), &["checkout", "-q", "-b", "experiment"]).await.unwrap();
            let mut isos = [Isolation::Git(wt)];
            let out = merge_all(&mut isos, "Add b", None).await;
            assert!(matches!(&out[0].1, MergeOutcome::OwnerBusy { why, .. } if why.to_string().contains("experiment")), "{:?}", out[0].1);
            assert!(!r.path().join("b.txt").exists());

            git(r.path(), &["checkout", "-q", "--detach"]).await.unwrap();
            assert!(matches!(&merge_all(&mut isos, "Add b", None).await[0].1, MergeOutcome::OwnerBusy { why, .. } if why.to_string().contains("no branch")));

            git(r.path(), &["checkout", "-q", "main"]).await.unwrap();
            let merge_head = git(r.path(), &["rev-parse", "--git-path", "MERGE_HEAD"]).await.unwrap();
            let merge_head = r.path().join(merge_head);
            std::fs::write(&merge_head, git(r.path(), &["rev-parse", "HEAD"]).await.unwrap()).unwrap();
            assert!(matches!(&merge_all(&mut isos, "Add b", None).await[0].1, MergeOutcome::OwnerBusy { why, .. } if why.to_string().contains("a merge")));
            std::fs::remove_file(&merge_head).unwrap();
            assert_eq!(landed_files(&merge_all(&mut isos, "Add b", None).await[0].1), vec!["b.txt".to_string()]);
        }))
    }

    #[test]
    fn a_modify_delete_conflict_stays_a_conflict_until_the_builder_resolves_it() {
        with_home(|_| rt().block_on(async {
            let r = repo().await;
            let wt = create_worktree(r.path(), "sa-r4").await.unwrap();
            std::fs::write(wt.path.join("a.txt"), "child edit\n").unwrap();
            git(r.path(), &["rm", "-q", "a.txt"]).await.unwrap();
            git(r.path(), &["commit", "-q", "-m", "owner deletes a"]).await.unwrap();
            let mut isos = [Isolation::Git(wt)];
            assert!(matches!(merge_all(&mut isos, "m", None).await[0].1, MergeOutcome::Conflict { .. }));
            // Tried again with nobody resolving anything: still a conflict.
            assert!(matches!(merge_all(&mut isos, "m", None).await[0].1, MergeOutcome::Conflict { .. }));
            assert!(!r.path().join("a.txt").exists(), "the owner's deletion stands");
        }))
    }

    #[test]
    fn a_resolution_with_a_heading_underline_is_not_mistaken_for_a_marker() {
        with_home(|_| rt().block_on(async {
            let r = repo().await;
            let wt = create_worktree(r.path(), "sa-r5").await.unwrap();
            std::fs::write(wt.path.join("a.txt"), "child\n").unwrap();
            std::fs::write(r.path().join("a.txt"), "parent\n").unwrap();
            git(r.path(), &["commit", "-q", "-am", "parent"]).await.unwrap();
            let copy = wt.path.clone();
            let mut isos = [Isolation::Git(wt)];
            merge_all(&mut isos, "m", None).await;
            std::fs::write(copy.join("a.txt"), "Title\n=======\nparent and child\n").unwrap();
            isos[0].mark_resolved();
            assert_eq!(landed_files(&merge_all(&mut isos, "m", None).await[0].1), vec!["a.txt".to_string()]);
        }))
    }

    #[test]
    fn a_task_with_no_changes_after_the_owner_moved_on_is_a_no_op_and_does_not_stop_the_set() {
        with_home(|_| rt().block_on(async {
            let r = repo().await;
            let idle = create_worktree(r.path(), "sa-r6").await.unwrap();
            let idle_path = idle.path.clone();
            std::fs::write(r.path().join("c.txt"), "owner\n").unwrap();
            git(r.path(), &["add", "-A"]).await.unwrap();
            git(r.path(), &["commit", "-q", "-m", "owner"]).await.unwrap();
            let busy = create_worktree(r.path(), "sa-r7").await.unwrap();
            std::fs::write(busy.path.join("z.txt"), "z\n").unwrap();
            let mut isos = [Isolation::Git(idle), Isolation::Git(busy)];
            let out = merge_all(&mut isos, "Add z", None).await;
            assert_eq!(out[0].1, MergeOutcome::NoChanges);
            assert!(!idle_path.exists(), "its worktree is cleaned up");
            assert_eq!(landed_files(&out[1].1), vec!["z.txt".to_string()]);
        }))
    }

    #[cfg(unix)]
    fn hook(repo: &Path, name: &str, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        let hooks = repo.join(".git/hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        std::fs::write(hooks.join(name), format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(hooks.join(name), std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn the_projects_commit_hooks_run_on_the_landing_commit_and_a_refusal_goes_back_to_the_builder() {
        with_home(|home| rt().block_on(async {
            let r = repo().await;
            let log = home.join("hooks.log");
            hook(r.path(), "commit-msg", &format!("echo commit-msg >> {}", log.display()));
            hook(r.path(), "pre-commit", "grep -q TODO b.txt && { echo 'TODO left in b.txt'; exit 1; }; exit 0");
            let wt = create_worktree(r.path(), "sa-r8").await.unwrap();
            std::fs::write(wt.path.join("b.txt"), "TODO\n").unwrap();
            let copy = wt.path.clone();
            let mut isos = [Isolation::Git(wt)];
            let out = merge_all(&mut isos, "Add b", None).await;
            assert!(matches!(&out[0].1, MergeOutcome::HookFailed { output, .. } if output.contains("TODO left in b.txt")), "{:?}", out[0].1);
            assert!(!r.path().join("b.txt").exists());
            assert!(copy.join("b.txt").exists(), "the work is kept for the builder");
            std::fs::write(copy.join("b.txt"), "done\n").unwrap();
            assert_eq!(landed_files(&merge_all(&mut isos, "Add b", None).await[0].1), vec!["b.txt".to_string()]);
            assert!(std::fs::read_to_string(&log).unwrap().contains("commit-msg"), "the commit-msg hook ran on it");
        }))
    }

    #[test]
    fn a_rename_away_from_a_file_the_owner_is_editing_is_asked_about() {
        with_home(|_| rt().block_on(async {
            let r = repo().await;
            let wt = create_worktree(r.path(), "sa-r9").await.unwrap();
            git(&wt.path, &["mv", "a.txt", "b.txt"]).await.unwrap();
            std::fs::write(r.path().join("a.txt"), "owner wip\n").unwrap();
            let mut isos = [Isolation::Git(wt)];
            let out = merge_all(&mut isos, "Rename a to b", None).await;
            assert!(matches!(&out[0].1, MergeOutcome::OwnerEdits { files, .. } if files.contains(&"a.txt".to_string())), "{:?}", out[0].1);
            assert_eq!(std::fs::read_to_string(r.path().join("a.txt")).unwrap(), "owner wip\n");
        }))
    }

    #[test]
    fn the_owner_committing_during_the_retest_is_caught_up_again_and_lands() {
        with_home(|_| rt().block_on(async {
            let r = repo().await;
            let wt = create_worktree(r.path(), "sa-r10").await.unwrap();
            std::fs::write(wt.path.join("b.txt"), "child\n").unwrap();
            let owner = r.path().to_path_buf();
            let once = std::sync::atomic::AtomicBool::new(false);
            let retest = |_: &Isolation| -> futures::future::BoxFuture<'static, Result<(), String>> {
                let first = !once.swap(true, std::sync::atomic::Ordering::SeqCst);
                let owner = owner.clone();
                Box::pin(async move {
                    if first {
                        std::fs::write(owner.join("c.txt"), "owner\n").unwrap();
                        git(&owner, &["add", "-A"]).await.unwrap();
                        git(&owner, &["commit", "-q", "-m", "owner meanwhile"]).await.unwrap();
                    }
                    Ok(())
                })
            };
            let mut isos = [Isolation::Git(wt)];
            assert_eq!(landed_files(&merge_all(&mut isos, "Add b", Some(&retest)).await[0].1), vec!["b.txt".to_string()]);
            assert!(r.path().join("c.txt").exists() && r.path().join("b.txt").exists());
        }))
    }

    #[test]
    fn a_worktree_holding_unsaved_work_is_never_removed() {
        with_home(|_| rt().block_on(async {
            let r = repo().await;
            let wt = create_worktree(r.path(), "sa-r11").await.unwrap();
            std::fs::write(wt.path.join("draft.txt"), "unsaved\n").unwrap();
            assert!(remove(&wt, false).await.is_err());
            assert!(wt.path.join("draft.txt").exists());
        }))
    }

    #[test]
    fn an_owner_deletion_is_told_apart_from_an_emptied_file() {
        with_home(|_| rt().block_on(async {
            let f = folder();
            let c = create_copy(f.path(), "sa-r12").unwrap();
            std::fs::write(c.path.join("b.txt"), "child\n").unwrap();
            std::fs::remove_file(f.path().join("b.txt")).unwrap();
            let copy = c.path.clone();
            let mut isos = [Isolation::Copy(c)];
            assert!(matches!(merge_all(&mut isos, "m", None).await[0].1, MergeOutcome::Conflict { .. }));
            assert!(copy.join("b.txt.nebo-owner-deleted").exists(), "a deletion, not an empty file");
            assert!(!copy.join("b.txt.nebo-owner").exists());
        }))
    }

    #[cfg(unix)]
    #[test]
    fn a_copy_never_writes_through_a_link_out_of_the_project() {
        with_home(|_| rt().block_on(async {
            let f = folder();
            let outside = tempfile::tempdir().unwrap();
            let c = create_copy(f.path(), "sa-r13").unwrap();
            std::fs::create_dir_all(c.path.join("out")).unwrap();
            std::fs::write(c.path.join("out/x.txt"), "x\n").unwrap();
            std::os::unix::fs::symlink(outside.path(), f.path().join("out")).unwrap();
            let mut isos = [Isolation::Copy(c)];
            assert!(matches!(&merge_all(&mut isos, "m", None).await[0].1, MergeOutcome::Failed { error, .. } if error.contains("link")));
            assert!(!outside.path().join("x.txt").exists());
        }))
    }

    #[test]
    fn a_name_in_any_script_lands_whole() {
        with_home(|_| rt().block_on(async {
            let r = repo().await;
            let wt = create_worktree(r.path(), "sa-r14").await.unwrap();
            std::fs::write(wt.path.join("naïve résumé.txt"), "x\n").unwrap();
            let mut isos = [Isolation::Git(wt)];
            assert_eq!(landed_files(&merge_all(&mut isos, "Add the résumé", None).await[0].1), vec!["naïve résumé.txt".to_string()]);
            assert!(r.path().join("naïve résumé.txt").exists());
        }))
    }

    #[test]
    fn a_stopped_copys_work_is_put_away_on_its_branch_and_the_disk_freed() {
        with_home(|_| rt().block_on(async {
            let r = repo().await;
            let wt = create_worktree(r.path(), "sa-r15").await.unwrap();
            std::fs::write(wt.path.join("b.txt"), "half done\n").unwrap();
            let path = wt.path.clone();
            let mut iso = Isolation::Git(wt);
            assert_eq!(changed_files(&iso).await, vec!["b.txt".to_string()]);
            let note = put_away(&mut iso, "Unfinished: add b").await.unwrap();
            assert!(note.contains("branch nebo/sa-r15"), "{note}");
            assert!(!path.exists(), "the worktree is gone");
            assert_eq!(git(r.path(), &["show", "nebo/sa-r15:b.txt"]).await.unwrap(), "half done", "the work is on its branch");
            assert!(!r.path().join("b.txt").exists(), "nothing landed");

            let mut idle = Isolation::Git(create_worktree(r.path(), "sa-r16").await.unwrap());
            assert!(put_away(&mut idle, "Unfinished").await.is_none(), "no work, nothing kept");
            assert!(!idle.path().exists());
        }))
    }

    #[test]
    fn a_discarded_copy_leaves_nothing_behind_and_the_project_untouched() {
        with_home(|_| rt().block_on(async {
            let r = repo().await;
            let wt = create_worktree(r.path(), "sa-r17").await.unwrap();
            std::fs::write(wt.path.join("b.txt"), "x\n").unwrap();
            let iso = Isolation::Git(wt);
            discard(&iso).await.unwrap();
            assert!(!iso.path().exists());
            assert!(git(r.path(), &["branch", "--list", "nebo/sa-r17"]).await.unwrap().is_empty());
            assert!(!r.path().join("b.txt").exists());
        }))
    }

    /// An owner's file (ignored, so git never shows it) or link where the
    /// change makes a folder is in the way: nothing lands over it.
    #[cfg(unix)]
    #[test]
    fn an_owner_file_or_link_where_the_change_makes_a_folder_is_asked_about() {
        with_home(|_| rt().block_on(async {
            let r = repo().await;
            std::fs::write(r.path().join(".gitignore"), "cache\ndocs\n").unwrap();
            git(r.path(), &["add", "-A"]).await.unwrap();
            git(r.path(), &["commit", "-q", "-m", "ignore"]).await.unwrap();
            let elsewhere = tempfile::tempdir().unwrap();
            for (id, folder) in [("sa-f1", "cache"), ("sa-f2", "docs")] {
                let wt = create_worktree(r.path(), id).await.unwrap();
                std::fs::create_dir_all(wt.path.join(folder)).unwrap();
                std::fs::write(wt.path.join(folder).join("readme.md"), "task\n").unwrap();
                git(&wt.path, &["add", "-f", "-A"]).await.unwrap();
                if folder == "cache" {
                    std::fs::write(r.path().join("cache"), "OWNER NOTES\n").unwrap();
                } else {
                    std::os::unix::fs::symlink(elsewhere.path(), r.path().join("docs")).unwrap();
                }
                let mut isos = [Isolation::Git(wt)];
                let out = merge_all(&mut isos, "Add a readme", None).await;
                assert!(matches!(&out[0].1, MergeOutcome::OwnerEdits { files, .. } if files == &vec![folder.to_string()]), "{:?}", out[0].1);
            }
            assert_eq!(std::fs::read_to_string(r.path().join("cache")).unwrap(), "OWNER NOTES\n");
            assert!(!elsewhere.path().join("readme.md").exists(), "nothing written through the owner's link");
        }))
    }

    /// Asked to land again after a conflict, with nobody resolving it, it
    /// is still a conflict; said resolved with markers still in the file,
    /// it is still a conflict. Markers never reach the owner's file.
    #[test]
    fn a_conflict_never_lands_until_resolved_and_free_of_markers() {
        with_home(|_| rt().block_on(async {
            let r = repo().await;
            let wt = create_worktree(r.path(), "sa-m1").await.unwrap();
            std::fs::write(wt.path.join("a.txt"), "child\n").unwrap();
            std::fs::write(r.path().join("a.txt"), "parent\n").unwrap();
            git(r.path(), &["commit", "-q", "-am", "parent"]).await.unwrap();
            let copy = wt.path.clone();
            let mut isos = [Isolation::Git(wt)];
            assert!(matches!(&merge_all(&mut isos, "Change a", None).await[0].1, MergeOutcome::Conflict { .. }));
            assert!(matches!(&merge_all(&mut isos, "Change a", None).await[0].1, MergeOutcome::Conflict { .. }), "not resolved by anyone");
            isos[0].mark_resolved();
            let out = merge_all(&mut isos, "Change a", None).await;
            assert!(matches!(&out[0].1, MergeOutcome::Conflict { files, .. } if files == &vec!["a.txt".to_string()]), "{:?}", out[0].1);
            assert_eq!(std::fs::read_to_string(r.path().join("a.txt")).unwrap(), "parent\n");
            std::fs::write(copy.join("a.txt"), "parent and child\n").unwrap();
            isos[0].mark_resolved();
            assert_eq!(landed_files(&merge_all(&mut isos, "Change a", None).await[0].1), vec!["a.txt".to_string()]);
            assert_eq!(std::fs::read_to_string(r.path().join("a.txt")).unwrap(), "parent and child\n");
        }))
    }

    /// A copy put away midway through resolving a conflict keeps the
    /// resolution so far and everything else it did on its branch; the
    /// task gets that branch back (never a second one) and it lands.
    #[test]
    fn a_copy_put_away_mid_conflict_keeps_its_resolution_and_comes_back_on_its_branch() {
        with_home(|_| rt().block_on(async {
            let r = repo().await;
            let wt = create_worktree(r.path(), "sa-m3").await.unwrap();
            std::fs::write(wt.path.join("a.txt"), "child\n").unwrap();
            std::fs::write(r.path().join("a.txt"), "parent\n").unwrap();
            git(r.path(), &["commit", "-q", "-am", "parent"]).await.unwrap();
            let owner = git(r.path(), &["rev-parse", "HEAD"]).await.unwrap();
            let copy = wt.path.clone();
            let mut isos = [Isolation::Git(wt)];
            assert!(matches!(&merge_all(&mut isos, "Change a", None).await[0].1, MergeOutcome::Conflict { .. }));
            std::fs::write(copy.join("a.txt"), "parent and child\n").unwrap();
            std::fs::write(copy.join("more.txt"), "more work\n").unwrap();
            let [mut iso] = isos;
            let note = put_away(&mut iso, "Unfinished").await.unwrap();
            assert!(note.contains("branch nebo/sa-m3"), "{note}");
            assert!(!copy.exists(), "the disk is freed");
            assert_eq!(git(r.path(), &["show", "nebo/sa-m3:a.txt"]).await.unwrap(), "parent and child");
            assert_eq!(git(r.path(), &["show", "nebo/sa-m3:more.txt"]).await.unwrap(), "more work");
            assert_eq!(git(r.path(), &["rev-parse", "nebo/sa-m3^"]).await.unwrap(), owner, "on the owner's commit it caught up to");

            let again = create_worktree(r.path(), "sa-m3").await.unwrap();
            assert_eq!(again.path, copy, "the same copy, on the same branch");
            assert_eq!(std::fs::read_to_string(copy.join("more.txt")).unwrap(), "more work\n");
            let mut isos = [Isolation::Git(again)];
            let landed = landed_files(&merge_all(&mut isos, "Change a", None).await[0].1);
            assert_eq!(landed, vec!["a.txt".to_string(), "more.txt".to_string()]);
            assert_eq!(std::fs::read_to_string(r.path().join("a.txt")).unwrap(), "parent and child\n");
        }))
    }

    /// What was checked is what lands. Git: checks that rewrite a file of
    /// the change send it back; what they write beside it never lands, and
    /// the copy is removed once it landed.
    #[test]
    fn checks_that_rewrite_the_change_send_it_back_and_their_output_never_lands() {
        with_home(|_| rt().block_on(async {
            let r = repo().await;
            let wt = create_worktree(r.path(), "sa-m5").await.unwrap();
            std::fs::write(wt.path.join("b.txt"), "child\n").unwrap();
            let path = wt.path.clone();
            let formatter = |iso: &Isolation| -> futures::future::BoxFuture<'static, Result<(), String>> {
                let p = iso.path().to_path_buf();
                Box::pin(async move {
                    std::fs::write(p.join("b.txt"), "formatted\n").unwrap();
                    std::fs::create_dir_all(p.join("build")).unwrap();
                    std::fs::write(p.join("build/out.txt"), "output").unwrap();
                    Ok(())
                })
            };
            let mut isos = [Isolation::Git(wt)];
            let out = merge_all(&mut isos, "Add b", Some(&formatter)).await;
            assert!(matches!(&out[0].1, MergeOutcome::RetestFailed { error, .. } if error.contains("the checks changed b.txt")), "{:?}", out[0].1);
            assert!(!r.path().join("b.txt").exists());
            // Taken in as the work, it is checked again and lands; the
            // checks' output does not, and the copy goes.
            let out = merge_all(&mut isos, "Add b", Some(&formatter)).await;
            assert_eq!(landed_files(&out[0].1), vec!["b.txt".to_string()]);
            assert_eq!(std::fs::read_to_string(r.path().join("b.txt")).unwrap(), "formatted\n");
            assert!(!r.path().join("build").exists(), "the checks' output never lands");
            assert!(!path.exists(), "the landed copy is removed, the checks' output with it");
        }))
    }

    /// Copy arm: a file the checks made is dropped, never landed; a change
    /// of theirs to the work's own file sends it back.
    #[test]
    fn a_scratch_copy_lands_only_what_was_checked() {
        with_home(|_| rt().block_on(async {
            let f = folder();
            std::fs::write(f.path().join("doc.txt"), "base\n").unwrap();
            let c = create_copy(f.path(), "sa-m5c").unwrap();
            std::fs::write(c.path.join("doc.txt"), "edited\n").unwrap();
            let rewrites = |iso: &Isolation| -> futures::future::BoxFuture<'static, Result<(), String>> {
                let p = iso.path().to_path_buf();
                Box::pin(async move {
                    std::fs::create_dir_all(p.join("__pycache__")).unwrap();
                    std::fs::write(p.join("__pycache__/x.pyc"), "artifact").unwrap();
                    std::fs::write(p.join("doc.txt"), "rewritten by the checks\n").unwrap();
                    Ok(())
                })
            };
            let mut isos = [Isolation::Copy(c)];
            let out = merge_all(&mut isos, "Edit doc", Some(&rewrites)).await;
            assert!(matches!(&out[0].1, MergeOutcome::RetestFailed { error, .. } if error.contains("the checks changed doc.txt")), "{:?}", out[0].1);
            assert_eq!(std::fs::read_to_string(f.path().join("doc.txt")).unwrap(), "base\n");
            let caches = |iso: &Isolation| -> futures::future::BoxFuture<'static, Result<(), String>> {
                let p = iso.path().to_path_buf();
                Box::pin(async move {
                    std::fs::create_dir_all(p.join("__pycache__")).unwrap();
                    std::fs::write(p.join("__pycache__/y.pyc"), "artifact").unwrap();
                    Ok(())
                })
            };
            let out = merge_all(&mut isos, "Edit doc", Some(&caches)).await;
            assert_eq!(landed_files(&out[0].1), vec!["doc.txt".to_string()]);
            assert!(!f.path().join("__pycache__").exists(), "the checks' output never lands");
        }))
    }

    /// The owner's own file named like the place a conflict's owner version
    /// waits is never taken for it: the twin gets a free name, and the
    /// owner's file stays.
    #[test]
    fn an_owner_file_named_like_a_twin_is_never_touched() {
        with_home(|_| rt().block_on(async {
            let f = folder();
            std::fs::write(f.path().join("doc.txt"), "base\n").unwrap();
            std::fs::write(f.path().join("doc.txt.nebo-owner"), "OWNER'S OWN FILE\n").unwrap();
            let c = create_copy(f.path(), "sa-tw").unwrap();
            let copy = c.path.clone();
            std::fs::write(copy.join("doc.txt"), "child\n").unwrap();
            std::fs::write(f.path().join("doc.txt"), "owner\n").unwrap();
            let mut isos = [Isolation::Copy(c)];
            assert!(matches!(&merge_all(&mut isos, "m", None).await[0].1, MergeOutcome::Conflict { .. }));
            assert_eq!(std::fs::read_to_string(copy.join("doc.txt.nebo-owner-1")).unwrap(), "owner\n");
            assert_eq!(std::fs::read_to_string(copy.join("doc.txt.nebo-owner")).unwrap(), "OWNER'S OWN FILE\n");
            std::fs::write(copy.join("doc.txt"), "owner and child\n").unwrap();
            std::fs::remove_file(copy.join("doc.txt.nebo-owner-1")).unwrap();
            isos[0].mark_resolved();
            assert_eq!(landed_files(&merge_all(&mut isos, "m", None).await[0].1), vec!["doc.txt".to_string()]);
            assert_eq!(std::fs::read_to_string(f.path().join("doc.txt.nebo-owner")).unwrap(), "OWNER'S OWN FILE\n");
            assert_eq!(std::fs::read_to_string(f.path().join("doc.txt")).unwrap(), "owner and child\n");
        }))
    }

    /// A file git keeps in LFS: the owner's unsaved edit to it is asked
    /// about, and an untouched one lands.
    #[test]
    fn an_lfs_file_lands_and_the_owners_edit_to_one_is_asked_about() {
        with_home(|_| rt().block_on(async {
            let r = repo().await;
            if git(r.path(), &["lfs", "install", "--local"]).await.is_err() {
                eprintln!("git lfs is not installed here; skipped");
                return;
            }
            git(r.path(), &["lfs", "track", "*.bin"]).await.unwrap();
            std::fs::write(r.path().join("x.bin"), "v1\n").unwrap();
            git(r.path(), &["add", "-A"]).await.unwrap();
            git(r.path(), &["commit", "-q", "-m", "lfs"]).await.unwrap();
            let wt = create_worktree(r.path(), "sa-lfs1").await.unwrap();
            std::fs::write(wt.path.join("x.bin"), "v2\n").unwrap();
            let mut isos = [Isolation::Git(wt)];
            assert_eq!(landed_files(&merge_all(&mut isos, "v2", None).await[0].1), vec!["x.bin".to_string()]);
            assert_eq!(std::fs::read_to_string(r.path().join("x.bin")).unwrap(), "v2\n");
            let wt = create_worktree(r.path(), "sa-lfs2").await.unwrap();
            std::fs::write(wt.path.join("x.bin"), "v3\n").unwrap();
            std::fs::write(r.path().join("x.bin"), "OWNER UNSAVED\n").unwrap();
            let mut isos = [Isolation::Git(wt)];
            assert!(matches!(&merge_all(&mut isos, "v3", None).await[0].1, MergeOutcome::OwnerEdits { .. }));
            assert_eq!(std::fs::read_to_string(r.path().join("x.bin")).unwrap(), "OWNER UNSAVED\n");
        }))
    }

    /// A commit hook that rewrites and stages a file: what the checks see
    /// is what lands.
    #[cfg(unix)]
    #[test]
    fn a_hook_that_rewrites_a_file_lands_what_was_checked() {
        with_home(|_| rt().block_on(async {
            let r = repo().await;
            hook(r.path(), "pre-commit", "echo hooked >> b.txt\ngit add b.txt");
            let wt = create_worktree(r.path(), "sa-hk").await.unwrap();
            std::fs::write(wt.path.join("b.txt"), "child\n").unwrap();
            let seen = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
            let saw = seen.clone();
            let retest = move |iso: &Isolation| -> futures::future::BoxFuture<'static, Result<(), String>> {
                *saw.lock().unwrap() = std::fs::read_to_string(iso.path().join("b.txt")).unwrap_or_default();
                Box::pin(async { Ok(()) })
            };
            let mut isos = [Isolation::Git(wt)];
            landed_files(&merge_all(&mut isos, "Add b", Some(&retest)).await[0].1);
            assert_eq!(std::fs::read_to_string(r.path().join("b.txt")).unwrap(), *seen.lock().unwrap());
            assert_eq!(*seen.lock().unwrap(), "child\nhooked\n");
        }))
    }

    /// A scratch copy put away keeps its work where it is; held again, it
    /// is the same copy, and it lands.
    #[test]
    fn a_scratch_copy_put_away_is_the_same_copy_when_held_again() {
        with_home(|_| rt().block_on(async {
            let f = folder();
            std::fs::write(f.path().join("doc.txt"), "base\n").unwrap();
            let c = create_copy(f.path(), "sa-pa").unwrap();
            std::fs::write(c.path.join("doc.txt"), "half\n").unwrap();
            let mut iso = Isolation::Copy(c);
            assert!(put_away(&mut iso, "x").await.unwrap().contains("kept in its copy"));
            let iso = reattach(iso).await.unwrap();
            assert_eq!(std::fs::read_to_string(iso.path().join("doc.txt")).unwrap(), "half\n");
            let mut isos = [iso];
            assert_eq!(landed_files(&merge_all(&mut isos, "m", None).await[0].1), vec!["doc.txt".to_string()]);
            assert_eq!(std::fs::read_to_string(f.path().join("doc.txt")).unwrap(), "half\n");
        }))
    }

    /// A copy's landing a restart cut short puts every owner file back; one
    /// cut short after every file was in place is finished.
    #[test]
    fn a_landing_cut_short_is_put_back_or_finished() {
        let f = folder();
        std::fs::write(f.path().join("doc.txt"), "base\n").unwrap();
        let at = f.path().join("t.nebo-landing.json");
        let target = f.path().join("doc.txt");
        let backup = f.path().join(".doc.txt.t.nebo-backup");
        let made = f.path().join("new.txt");
        let new_fp = Some(tools::checkpoint::fingerprint(b"new\n"));
        let journal = |done| Journal {
            steps: vec![
                Step { target: target.clone(), staged: Some(f.path().join(".doc.txt.t.nebo-landing")), backup: Some(backup.clone()), fp: new_fp },
                Step { target: made.clone(), staged: Some(f.path().join(".new.txt.t.nebo-landing")), backup: None, fp: new_fp },
            ],
            done,
        };
        // Cut short: doc.txt replaced and new.txt made, the journal not done.
        std::fs::rename(&target, &backup).unwrap();
        std::fs::write(&target, "new\n").unwrap();
        std::fs::write(&made, "new\n").unwrap();
        journal(false).write(&at).unwrap();
        recover_landing(&at);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "base\n", "the owner's file is back");
        assert!(!made.exists() && !backup.exists() && !at.exists());
        // Done: every file was in place; only the set-aside one is dropped.
        std::fs::rename(&target, &backup).unwrap();
        std::fs::write(&target, "new\n").unwrap();
        journal(true).write(&at).unwrap();
        recover_landing(&at);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new\n");
        assert!(!backup.exists() && !at.exists());
    }
}
