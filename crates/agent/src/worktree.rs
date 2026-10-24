//! Isolation for mutating fan-out (coding harness P5.3).
//!
//! When helpers fan out with `isolate: "worktree"`, each gets its own git
//! worktree of the project, at `<repo>/.nebo/worktrees/<id>` on its own
//! branch (`.nebo/worktrees/` is excluded through the repository's
//! `info/exclude`; it survives `git clean -fdx` as an excluded folder
//! holding a repository, though not `git clean -ffdx`),
//! fenced by its grant's `fence` and `cwd`, and the parent merges the
//! copies back when they are reviewed. Read-only fan-out never comes here:
//! it shares the tree. A project that is no repository, or a worktree git
//! can't make, gets no copy: the work goes on in the project itself.
//!
//! Landing a copy is one way ([`merge_all`]), in order:
//! 1. Catch up in the copy ([`catch_up`]): git squashes the task's work into
//!    one commit and rebases it onto the owner's current HEAD. A conflict
//!    stays in the copy for the builder to resolve there (a rebase in
//!    progress), never in the owner's tree.
//! 2. Re-test in the copy (the caller's check), so what was verified is
//!    what lands.
//! 3. Land ([`land`]): one commit, fast-forwarded onto the owner's branch,
//!    never a merge commit, never a force. When the owner has uncommitted
//!    edits in a file the change touches, nothing lands: the owner decides.
//!
//! Several copies land in dependency order, one at a time, each caught up
//! and re-tested on top of the one before; the first that does not land
//! stops the set, and the copies not landed are kept. A copy is removed
//! once it has landed, or when it holds no change.
//!
//! House git rules hold throughout: no stash, no reset, no checkout of the
//! owner's files, nothing forced. The owner's tree is only ever moved
//! forward by a fast-forward.
//! Every git subprocess runs with prompts disabled — an unattended agent can
//! never answer one.

use std::path::{Path, PathBuf};
use tokio::process::Command;
use tracing::warn;

/// Worktrees older than this with nothing left in them are swept at
/// startup (a parent killed mid-batch leaves them behind).
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
    /// The files that catch-up left in conflict. Each is resolved only when
    /// the builder stages its resolution (`git add`, or `git rm` to keep a
    /// deletion), the explicit word git itself reads; one still holding
    /// conflict markers never lands.
    #[serde(default)]
    pub conflicted: Vec<String>,
}

/// A helper's own copy of the project: a git worktree on its own branch.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub enum Isolation {
    Git(Worktree),
}

impl Isolation {
    pub fn path(&self) -> &Path {
        match self {
            Isolation::Git(w) => &w.path,
        }
    }
    pub fn task_id(&self) -> &str {
        match self {
            Isolation::Git(w) => &w.task_id,
        }
    }

    /// The owner's project folder the copy was made from.
    pub fn root(&self) -> &Path {
        match self {
            Isolation::Git(w) => &w.root,
        }
    }

    /// The task branch (git), empty for a scratch copy.
    pub fn branch(&self) -> &str {
        match self {
            Isolation::Git(w) => &w.branch,
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
    /// The copy holds changes outside what was reviewed (`Landing::only`):
    /// a file the review never saw (too big to snapshot, a nested
    /// repository, one added after the snapshot). Nothing landed; it goes
    /// back to the builder, or the owner decides.
    Unreviewed { files: Vec<String>, kept_at: PathBuf, branch: String },
    /// Files the change touches hold something other than what was
    /// reviewed (catching up with the owner's project changed them, or a
    /// commit hook rewrote them). Nothing landed; it is reviewed again
    /// first.
    NeedsReview { files: Vec<String>, kept_at: PathBuf, branch: String },
    /// Refused or failed otherwise; nothing of the owner's changed.
    Failed { error: String, kept_at: PathBuf, branch: String },
}

/// What was reviewed: every path the change touches, with what the review
/// saw there ([`content_id`]; `None`: no file).
pub type Reviewed = std::collections::BTreeMap<String, Option<String>>;

/// What a landing may land, and how it waits its turn.
#[derive(Clone, Default)]
pub struct Landing {
    /// What was reviewed. Exactly that lands: a change to any other path
    /// is refused ([`MergeOutcome::Unreviewed`]) unless the folder ignores
    /// it (build output, which never lands), and a reviewed path holding
    /// anything else now (catching up with the owner's changes, a commit
    /// hook's rewrite) goes back to review ([`MergeOutcome::NeedsReview`]).
    /// `None`: the owner chose to land the copy as it is.
    pub only: Option<Reviewed>,
    /// Stops a landing that waits for another one on the same project.
    pub stop: tokio_util::sync::CancellationToken,
    /// Told once when the landing has to wait for another one on the same
    /// project.
    pub waiting: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
}

impl std::fmt::Debug for Landing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Landing").field("only", &self.only).field("stopped", &self.stop.is_cancelled()).finish()
    }
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
    git_io(cwd, args, None).await
}

/// [`git_bytes`] with `input` on git's standard input (`--stdin -z`: paths
/// cross to git NUL-separated, never quoted or split).
pub(crate) async fn git_io(cwd: &Path, args: &[&str], input: Option<&[u8]>) -> Result<Vec<u8>, String> {
    let mut cmd = command::new::<Command>("git", command::Console::Hidden);
    cmd.args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "")
        .env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes")
        .stdin(if input.is_some() { std::process::Stdio::piped() } else { std::process::Stdio::null() })
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    for var in GIT_REDIRECTS {
        cmd.env_remove(var);
    }
    let name = args.iter().find(|a| !a.starts_with('-') && !a.contains('=')).copied().unwrap_or("");
    let run = async {
        let mut child = cmd.spawn()?;
        if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
            use tokio::io::AsyncWriteExt;
            // Written while git reads; closed after, so git sees the end.
            let input = input.to_vec();
            tokio::spawn(async move {
                let _ = stdin.write_all(&input).await;
            });
        }
        child.wait_with_output().await
    };
    let out = match tokio::time::timeout(GIT_TIMEOUT, run).await {
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

/// The oldest git Nebo works on code with: 2.31 brought
/// `git rev-parse --path-format=absolute`, which finding a copy's own git
/// files (`--git-path`) and a project's repository (`--git-common-dir`)
/// rely on.
pub const MIN_GIT: (u32, u32) = (2, 31);

/// What is wrong with this computer's git for working on code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitProblem {
    /// No git (on a Mac: no command-line tools, found without asking macOS
    /// to install them).
    Missing,
    /// Older than [`MIN_GIT`]: the version it has.
    TooOld(String),
}

impl GitProblem {
    /// What the owner reads: what to do, in plain words.
    pub fn words(&self) -> (&'static str, String) {
        match self {
            GitProblem::Missing => (
                "Install git to let your employees work on code",
                "Your employees use git to work on code in copies of their own and land it in your project, and this \
                 computer doesn't have it. Install git (on a Mac: the command-line tools), then ask again."
                    .into(),
            ),
            GitProblem::TooOld(have) => (
                "Update git to let your employees work on code",
                format!(
                    "Your employees need git {}.{} or newer to work on code in copies of their own; this computer has git \
                     {have}. Update git, then ask again.",
                    MIN_GIT.0, MIN_GIT.1
                ),
            ),
        }
    }
}

/// Where `program` runs from: itself when it names a path, else the first
/// of it on PATH.
fn resolve(program: &Path) -> Option<PathBuf> {
    if program.components().count() > 1 {
        return Some(program.to_path_buf());
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(program)).find(|g| g.is_file())
}

/// `program` (git) checked for working on code: `None` when it is
/// [`MIN_GIT`] or newer. On a Mac, macOS's own `/usr/bin/git` is only a
/// stub until the command-line tools are installed: they are looked for
/// first (`xcode-select -p`), so the check never pops up macOS's install
/// dialog. Callers pass `Path::new("git")`, the git every copy runs.
pub async fn git_problem(program: &Path) -> Option<GitProblem> {
    let found = resolve(program)?;
    if cfg!(target_os = "macos") && found == Path::new("/usr/bin/git") {
        let tools = command::new::<Command>("xcode-select", command::Console::Hidden)
            .arg("-p")
            .stdin(std::process::Stdio::null())
            .output()
            .await;
        if !tools.is_ok_and(|o| o.status.success()) {
            return Some(GitProblem::Missing);
        }
    }
    let said = command::new::<Command>(&found, command::Console::Hidden)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    let Some(said) = said else { return Some(GitProblem::Missing) };
    let version = said.split_whitespace().nth(2).unwrap_or_default().to_string();
    let mut parts = version.split('.').map(|n| n.parse::<u32>().unwrap_or(0));
    let have = (parts.next().unwrap_or(0), parts.next().unwrap_or(0));
    (have < MIN_GIT).then_some(GitProblem::TooOld(version))
}

/// The owner's notice about git (`program`, [`git_problem`]), when code
/// work is attempted (an employee's own copy, a landing): in the Inbox while
/// git is missing or too old, said again in new words when that changes and
/// raised again when it breaks after it was fixed, and put away once git
/// works. Whether git is ready.
pub async fn git_notice(store: &db::Store, program: &Path) -> bool {
    const ID: &str = "git-needed";
    let user = store.ensure_local_user_id().unwrap_or_default();
    let open = store.get_notification(ID, &user).ok().flatten().filter(|n| n.archived_at.is_none());
    match git_problem(program).await {
        None => {
            if open.is_some()
                && let Err(e) = store.archive_notification(ID, &user)
            {
                warn!(error = %e, "the git notice was not put away");
            }
            true
        }
        Some(problem) => {
            let (title, body) = problem.words();
            // Already said, in these words: not again.
            if open.is_some_and(|n| n.title == title) {
                return false;
            }
            warn!(?problem, "git is missing or too old for code work");
            tools::owner_notify::emit(
                store,
                None,
                &tools::owner_notify::OwnerNotification { id: ID, kind: "warning", title, body: Some(&body), latest: true, ..Default::default() },
            );
            false
        }
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

/// The folder a project keeps Nebo's working files in, beside the owner's
/// own Nebo files there (`.nebo/hooks.yaml`, which he commits):
/// [`NEBO_OWN`].
pub const NEBO_DIR: &str = ".nebo";

/// Nebo's own working folders in a project's `.nebo`: helpers' copies
/// (`worktrees`), the reviewers' snapshots of a change (`verify`) and the
/// one build folder its checks share (`build`). Only these are hidden from
/// git and left out of a change; anything else in `.nebo` is the owner's.
pub const NEBO_OWN: &[&str] = &["worktrees", "verify", "build"];

/// `<project>/.nebo`, made when missing, and in a git repository each of
/// Nebo's own folders in it ([`NEBO_OWN`]) excluded through the
/// repository's own `info/exclude`, so `git status` never shows them and
/// nothing in them is ever committed; the owner's own files there stay his.
/// The one place Nebo's working files in a project are: a helper's copy
/// (inside the project's own `.nebo`) keeps them in the project's, never a
/// `.nebo` of its own.
pub fn nebo_dir(project: &Path) -> Result<PathBuf, String> {
    let parts: Vec<_> = project.components().collect();
    let project: PathBuf = match parts.iter().position(|c| c.as_os_str() == NEBO_DIR) {
        Some(at) => parts[..at].iter().collect(),
        None => project.to_path_buf(),
    };
    let dir = project.join(NEBO_DIR);
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    if let Some(common) = git_common_dir(&project) {
        let exclude = common.join("info").join("exclude");
        let rules = std::fs::read_to_string(&exclude).unwrap_or_default();
        let missing: Vec<String> =
            NEBO_OWN.iter().map(|own| format!("{NEBO_DIR}/{own}/")).filter(|line| !rules.lines().any(|l| l.trim() == line)).collect();
        if !missing.is_empty() {
            let _ = std::fs::create_dir_all(common.join("info"));
            let mut text = rules;
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            for line in missing {
                text.push_str(&line);
                text.push('\n');
            }
            std::fs::write(&exclude, text).map_err(|e| format!("write {}: {e}", exclude.display()))?;
        }
    }
    Ok(dir)
}

/// The git directory every worktree of the repository at `dir` shares (its
/// `.git` folder, followed from a worktree's `.git` file), read from disk.
fn git_common_dir(dir: &Path) -> Option<PathBuf> {
    let dot = dir.join(".git");
    if dot.is_dir() {
        return Some(dot);
    }
    let pointer = std::fs::read_to_string(&dot).ok()?;
    let gitdir = PathBuf::from(pointer.trim().strip_prefix("gitdir:")?.trim());
    let gitdir = if gitdir.is_absolute() { gitdir } else { dir.join(gitdir) };
    // A linked worktree's git dir is `<common>/worktrees/<name>`, and says so.
    match std::fs::read_to_string(gitdir.join("commondir")) {
        Ok(common) => {
            let common = PathBuf::from(common.trim());
            Some(if common.is_absolute() { common } else { gitdir.join(common) })
        }
        Err(_) => Some(gitdir),
    }
}

/// Where a helper's copies of the repository at `root` live: in its
/// canonical root's Nebo folder (a worktree's copy beside the main
/// checkout's, never nested in another copy), `<repo>/.nebo/worktrees`.
async fn worktrees_dir(root: &Path) -> Result<PathBuf, String> {
    let canonical = match git(root, &["rev-parse", "--path-format=absolute", "--git-common-dir"]).await {
        Ok(common) => PathBuf::from(common).parent().map(Path::to_path_buf).unwrap_or_else(|| root.to_path_buf()),
        Err(_) => root.to_path_buf(),
    };
    nebo_dir(&canonical).map(|d| d.join("worktrees"))
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

/// Isolate `task_id` for the project at `workspace`: a git worktree of its
/// repository. `Err` when it is no repository, or git can't make one: the
/// work then goes on in the project itself.
pub async fn create(workspace: &Path, task_id: &str) -> Result<Isolation, String> {
    validate_task_id(task_id)?;
    let root = repo_root(workspace).await?;
    create_worktree(&root, task_id).await.map(Isolation::Git)
}

/// Create a worktree on a fresh branch from the repo's current HEAD. A task
/// whose work was put away on its branch (`nebo/<task_id>`) gets that
/// branch back, its work on it, never a second branch or copy.
///
/// A project on a branch with no commit yet (`git init` and nothing after)
/// is copied from git's empty tree: the copy is a worktree on a branch
/// with no commit either (`git worktree add --orphan`), its base the empty
/// tree, and its reviewed commit lands as the project's first by the same
/// fast-forward as any other. Nothing is committed in the owner's project
/// to make the copy, and what the owner's folder holds uncommitted is not
/// in it.
pub async fn create_worktree(root: &Path, task_id: &str) -> Result<Worktree, String> {
    validate_task_id(task_id)?;
    let dir = worktrees_dir(root).await?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let path = dir.join(task_id);
    let branch = format!("nebo/{task_id}");
    let start_branch = git(root, &["symbolic-ref", "--quiet", "--short", "HEAD"]).await.unwrap_or_default();
    let (head, unborn) = match git(root, &["rev-parse", "--verify", "--quiet", "HEAD"]).await {
        Ok(head) => (head, false),
        Err(_) if !start_branch.is_empty() => (empty_tree(root).await?, true),
        Err(e) => return Err(format!("the project has no commit to copy: {e}")),
    };
    if git(root, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")]).await.is_ok() {
        // The task's work sits on the owner's commit it shares with them,
        // or on the empty tree when it was made before the project's first
        // commit (it then shares no history with the project).
        let base = match git(root, &["merge-base", &branch, &head]).await {
            Ok(base) => base,
            Err(_) => empty_tree(root).await?,
        };
        let wt = Worktree { task_id: task_id.to_string(), root: root.to_path_buf(), path, branch, start_branch, base, onto: None, conflicted: Vec::new() };
        return reattach_worktree(wt).await;
    }
    let at = path.to_str().unwrap_or_default();
    if unborn {
        git(root, &["worktree", "add", "--orphan", "-b", &branch, at])
            .await
            .map_err(|e| format!("git worktree add --orphan (a project with no commit yet is copied by git 2.42 or newer): {e}"))?;
    } else {
        git(root, &["worktree", "add", "-b", &branch, at, &head]).await.map_err(|e| format!("git worktree add: {e}"))?;
    }
    Ok(Worktree { task_id: task_id.to_string(), root: root.to_path_buf(), path, branch, start_branch, base: head, onto: None, conflicted: Vec::new() })
}

/// Git's empty tree, as the repository at `dir` names it: the base of a
/// copy of a project that had no commit when it was made.
async fn empty_tree(dir: &Path) -> Result<String, String> {
    git_io(dir, &["hash-object", "-t", "tree", "--stdin"], Some(&[]))
        .await
        .map(|out| String::from_utf8_lossy(&out).trim().to_string())
        .map_err(|e| format!("git's empty tree: {e}"))
}

/// The tip of the owner's branch a copy lands on: its commit, or the empty
/// tree while the branch still has no commit and the copy was made then.
async fn owner_tip(wt: &Worktree) -> Result<String, String> {
    match git(&wt.root, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{}", wt.start_branch)]).await {
        Ok(commit) => Ok(commit),
        Err(_) if wt.base == empty_tree(&wt.root).await? => Ok(wt.base.clone()),
        Err(_) => Err(format!("the project's branch {} has no commit", wt.start_branch)),
    }
}

/// Whether `commit` is one commit on `base`: its parent is `base`, or it
/// has no parent and `base` is the empty tree (the project had no commit).
async fn sits_on(dir: &Path, commit: &str, base: &str) -> bool {
    match git(dir, &["rev-parse", "--verify", "--quiet", &format!("{commit}^")]).await {
        Ok(parent) => parent == base,
        Err(_) => empty_tree(dir).await.is_ok_and(|t| t == base),
    }
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
/// put away on its branch is checked out again. `Err` when its work is gone.
pub async fn reattach(iso: Isolation) -> Result<Isolation, String> {
    match iso {
        Isolation::Git(wt) => reattach_worktree(wt).await.map(Isolation::Git),
    }
}

/// Left out of review and landing in a folder that is no repository and
/// has no `.gitignore` of its own: installed packages, build output and
/// caches, rebuilt from their sources.
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

/// Set `git_dir`'s own exclude file to the rule of a folder that is no
/// repository: [`EXCLUDED`] when the folder has no `.gitignore` at its top,
/// nothing otherwise (its `.gitignore` files then decide).
fn folder_rule(folder: &Path, git_dir: &Path) -> Result<(), String> {
    let info = git_dir.join("info");
    std::fs::create_dir_all(&info).map_err(|e| format!("create {}: {e}", info.display()))?;
    let rule = if folder.join(".gitignore").is_file() { String::new() } else { EXCLUDED.join("\n") + "\n" };
    std::fs::write(info.join("exclude"), rule).map_err(|e| format!("write {}: {e}", info.display()))
}

/// Run git over a folder by what the folder ignores: the one rule of what
/// is build output, for a review's snapshot and a landing alike. In a
/// repository (`git_dir` `None`) git reads the repository's own ignore
/// rules; a folder that is no repository is read through `git_dir`, by its
/// own `.gitignore` files, or by [`EXCLUDED`] when it has none at its top.
async fn over_folder(folder: &Path, git_dir: Option<&Path>, args: &[&str]) -> Result<Vec<u8>, String> {
    over_folder_io(folder, git_dir, args, None).await
}

/// [`over_folder`] with `input` on git's standard input. A folder that is no
/// repository is read with its names exactly as the disk holds them
/// (`core.precomposeunicode` off), as the copy itself reads them: a name
/// macOS stores decomposed is the same name on both sides.
async fn over_folder_io(folder: &Path, git_dir: Option<&Path>, args: &[&str], input: Option<&[u8]>) -> Result<Vec<u8>, String> {
    let Some(git_dir) = git_dir else { return git_io(folder, args, input).await };
    folder_rule(folder, git_dir)?;
    let mut all = vec![
        format!("--git-dir={}", git_dir.display()),
        format!("--work-tree={}", folder.display()),
        "-c".into(),
        "core.precomposeunicode=false".into(),
    ];
    all.extend(args.iter().map(|a| a.to_string()));
    let all: Vec<&str> = all.iter().map(String::as_str).collect();
    git_io(folder, &all, input).await
}

/// `git ls-files` over a folder, by what it ignores ([`over_folder`]).
pub async fn ls_folder(folder: &Path, git_dir: Option<&Path>, args: &[&str]) -> Result<Vec<u8>, String> {
    over_folder(folder, git_dir, args).await
}

/// The paths of `paths` a folder ignores ([`over_folder`]), decided by the
/// path alone: one that is gone is ignored as much as one that is there.
pub async fn ignored_paths(folder: &Path, git_dir: Option<&Path>, paths: &[String]) -> Result<Vec<String>, String> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let mut input = Vec::new();
    for p in paths {
        input.extend_from_slice(p.as_bytes());
        input.push(0);
    }
    match over_folder_io(folder, git_dir, &["check-ignore", "--no-index", "-z", "--stdin"], Some(&input)).await {
        Ok(listed) => Ok(z_entries(&listed)),
        // `check-ignore` exits 1, saying nothing, when none is ignored.
        Err(e) if e.is_empty() => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

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
/// in progress in the worktree. It stays a conflict until the builder has
/// staged each file's resolution (`git add`, or `git rm` to keep a
/// deletion: git's own explicit word, binary and modify/delete conflicts
/// included) and no file it was given still holds conflict markers. Copy
/// arm: every file the owner changed meanwhile that the copy did not is
/// taken in; one both changed gets the owner's version next to it, and is
/// resolved once the builder has deleted that, unless the owner changed it
/// again since.
///
/// With `landing.only` set, a change outside what was reviewed is refused
/// before anything is committed ([`MergeOutcome::Unreviewed`]), and what
/// is committed must be what was reviewed ([`held_to_review`]).
///
/// `Err` is the outcome that ends the landing (the project busy, a hook's
/// refusal, unreviewed changes, a failure).
pub async fn catch_up(iso: &mut Isolation, message: &str, landing: &Landing) -> Result<CatchUp, MergeOutcome> {
    match iso {
        Isolation::Git(wt) => catch_up_git(wt, message, landing).await,
    }
}

/// The changed paths outside what `landing` reviewed.
fn outside_review(changed: &[String], landing: &Landing) -> Vec<String> {
    match &landing.only {
        None => Vec::new(),
        Some(only) => changed.iter().filter(|p| !only.contains_key(*p)).cloned().collect(),
    }
}

/// Whether what a copy would land is exactly what was reviewed
/// (`landing.only`): every path it changes was reviewed, and each reviewed
/// path holds what the review saw, compared by content. A worktree's
/// commit must also be what its folder holds: a commit hook that rewrote a
/// file, or committed something else, is caught. `Err` is the outcome:
/// [`MergeOutcome::Unreviewed`] for changes nobody reviewed,
/// [`MergeOutcome::NeedsReview`] for reviewed paths holding anything else.
pub async fn held_to_review(iso: &Isolation, landing: &Landing) -> Result<(), MergeOutcome> {
    let Some(only) = &landing.only else { return Ok(()) };
    let (kept_at, branch) = (iso.path().to_path_buf(), iso.branch().to_string());
    let failed = |error: String| MergeOutcome::Failed { error, kept_at: kept_at.clone(), branch: branch.clone() };
    let (changed, mut differs) = match iso {
        Isolation::Git(wt) => {
            let changed = touched_paths(&wt.path, &wt.base, "HEAD").await;
            let mut all: Vec<String> = changed.clone();
            all.extend(only.keys().filter(|p| !changed.contains(p)).cloned());
            // What is on disk is what is committed, for every path.
            let mut args = vec!["--literal-pathspecs", "status", "-z", "--porcelain", "--untracked-files=all", "--no-renames", "--"];
            args.extend(all.iter().map(String::as_str));
            let dirty: Vec<String> = z_entries(&git_bytes(&wt.path, &args).await.map_err(|e| failed(format!("git status in the copy: {e}")))?)
                .into_iter()
                .filter_map(|e| e.get(3..).map(str::to_string))
                .collect();
            (changed, dirty)
        }
    };
    let unreviewed = outside_review(&changed, landing);
    if !unreviewed.is_empty() {
        return Err(MergeOutcome::Unreviewed { files: unreviewed, kept_at, branch });
    }
    for (rel, seen) in only {
        if !differs.contains(rel) && content_id(iso.path(), rel).await != Ok(seen.clone()) {
            differs.push(rel.clone());
        }
    }
    if differs.is_empty() {
        return Ok(());
    }
    differs.sort();
    Err(MergeOutcome::NeedsReview { files: differs, kept_at, branch })
}

async fn catch_up_git(wt: &mut Worktree, message: &str, landing: &Landing) -> Result<CatchUp, MergeOutcome> {
    let failed = |wt: &Worktree, error: String| MergeOutcome::Failed { error, kept_at: wt.path.clone(), branch: wt.branch.clone() };
    if let Some(why) = owner_busy(wt).await {
        return Err(MergeOutcome::OwnerBusy { why, kept_at: wt.path.clone(), branch: wt.branch.clone() });
    }
    if rebasing(wt).await {
        // Resolved only as the builder staged it, and with no markers left.
        let left = unmerged(wt).await;
        if !left.is_empty() {
            return Ok(CatchUp { conflicts: left, ..Default::default() });
        }
        let marked = with_markers(&wt.path, &wt.conflicted).await;
        if !marked.is_empty() {
            return Ok(CatchUp { conflicts: marked, ..Default::default() });
        }
        git(&wt.path, &["add", "-A"]).await.map_err(|e| failed(wt, format!("git add in the copy: {e}")))?;
        git_as_nebo(&wt.path, &["-c", "core.editor=true", "rebase", "--continue"])
            .await
            .map_err(|e| failed(wt, format!("git rebase --continue: {e}")))?;
        if let Some(onto) = wt.onto.take() {
            wt.base = onto;
        }
        wt.conflicted.clear();
    }
    let owner = owner_tip(wt).await.map_err(|e| failed(wt, e))?;
    let base = wt.base.clone();
    // Nothing outside what was reviewed is ever staged: a file the review
    // never saw (too big to snapshot, a repository of its own, made after
    // it) is refused first. What the project ignores git never stages.
    let mut changed = z_paths(&wt.path, &["diff", "-z", "--name-only", "--no-renames", &base]).await;
    changed.extend(z_paths(&wt.path, &["ls-files", "-z", "--others", "--exclude-standard"]).await);
    let unreviewed = outside_review(&changed, landing);
    if !unreviewed.is_empty() {
        return Err(MergeOutcome::Unreviewed { files: unreviewed, kept_at: wt.path.clone(), branch: wt.branch.clone() });
    }
    git(&wt.path, &["add", "-A"]).await.map_err(|e| failed(wt, format!("git add in the copy: {e}")))?;
    let tree = git(&wt.path, &["write-tree"]).await.map_err(|e| failed(wt, format!("git write-tree: {e}")))?;
    let base_tree = git(&wt.path, &["rev-parse", &format!("{base}^{{tree}}")]).await.map_err(|e| failed(wt, format!("the base tree: {e}")))?;
    if tree == base_tree {
        return Ok(CatchUp { no_changes: true, onto: owner, ..Default::default() });
    }
    // `None`: nothing committed yet in a copy of a project that had no
    // commit, whose branch has none either.
    let head = git(&wt.path, &["rev-parse", "--verify", "--quiet", "HEAD"]).await.ok();
    // Already the one commit, with this message: nothing to fold.
    let folded = head.is_some()
        && sits_on(&wt.path, "HEAD", &base).await
        && git(&wt.path, &["rev-parse", "HEAD^{tree}"]).await.is_ok_and(|t| t == tree)
        && git(&wt.path, &["log", "-1", "--format=%B"]).await.is_ok_and(|m| m.trim() == message.trim());
    if !folded {
        // The task branch goes back to the base (to no commit at all when
        // the base is the empty tree), the work staged as it is, and one
        // commit is made through the project's own hooks.
        let branch_ref = format!("refs/heads/{}", wt.branch);
        let unborn = base == empty_tree(&wt.path).await.map_err(|e| failed(wt, e))?;
        if let Some(head) = &head {
            let back = if unborn { git(&wt.path, &["update-ref", "-d", &branch_ref, head]).await } else { git(&wt.path, &["update-ref", &branch_ref, &base, head]).await };
            back.map_err(|e| failed(wt, format!("git update-ref: {e}")))?;
        }
        git(&wt.path, &["add", "-A"]).await.map_err(|e| failed(wt, format!("git add in the copy: {e}")))?;
        if let Err(output) = git_as_nebo(&wt.path, &["commit", "-q", "-m", message]).await {
            // Put the task's own history back, so nothing it did is lost.
            if let Some(head) = &head {
                let _ = git(&wt.path, &["update-ref", &branch_ref, head]).await;
            }
            return Err(MergeOutcome::HookFailed { output, kept_at: wt.path.clone(), branch: wt.branch.clone() });
        }
        // A hook may have rewritten or staged files of its own: what is
        // committed is still only what was reviewed, byte for byte.
        held_to_review(&Isolation::Git(wt.clone()), landing).await?;
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
        wt.conflicted = conflicts.clone();
        return Ok(CatchUp { onto: owner, changed, overlap, conflicts, ..Default::default() });
    }
    wt.base = owner.clone();
    Ok(CatchUp { onto: owner, changed, overlap, ..Default::default() })
}

/// Land a caught-up copy on the owner's branch.
///
/// Git: the task's one commit must sit on the tip of the branch the copy
/// was made from, with that branch checked out and nothing in progress in
/// the project. No file the commit adds, changes, deletes or renames may
/// hold anything of the owner's that differs from that tip, whether git
/// tracks it, ignores it or has never seen it: an ignored note or a local
/// secrets file in the way is the owner's, and nothing lands over it. Then
/// the branch is fast-forwarded to that commit (never a merge commit, never
/// a force), the move is checked, and the worktree and task branch are
/// removed. Copy arm: the changed files are staged next to their places and
/// moved in together, unless the owner changed one since the catch-up or a
/// folder on the way is a link out of the project. With `landing.only`, a
/// change outside what was reviewed lands nothing. Anything else lands
/// nothing.
pub async fn land(iso: &Isolation, landing: &Landing) -> MergeOutcome {
    match iso {
        Isolation::Git(wt) => land_git(wt, landing).await,
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

/// What a path holds on the disk, read once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnDisk {
    Nothing,
    File(Vec<u8>),
    /// A link: its target's text, never what it points at.
    Link(Vec<u8>),
    /// A submodule: the commit it has checked out.
    Commit(String),
    /// A folder where a file goes.
    Folder,
}

/// Read what `root` holds at `rel`, once.
pub async fn read_disk(root: &Path, rel: &str) -> OnDisk {
    let disk = root.join(rel);
    let Ok(meta) = std::fs::symlink_metadata(&disk) else { return OnDisk::Nothing };
    if meta.file_type().is_symlink() {
        return match std::fs::read_link(&disk) {
            Ok(target) => OnDisk::Link(target.to_string_lossy().into_owned().into_bytes()),
            Err(_) => OnDisk::Folder,
        };
    }
    if meta.is_dir() {
        if disk.join(".git").exists() {
            return match git(&disk, &["rev-parse", "HEAD"]).await {
                Ok(commit) => OnDisk::Commit(commit),
                Err(_) => OnDisk::Folder,
            };
        }
        let empty = std::fs::read_dir(&disk).map(|mut d| d.next().is_none()).unwrap_or(false);
        return if empty { OnDisk::Nothing } else { OnDisk::Folder };
    }
    std::fs::read(&disk).map(OnDisk::File).unwrap_or(OnDisk::Folder)
}

/// The id git records for `what` at `rel`, hashed by git run with `prefix`
/// in `cwd` (a repository's own settings, or a caller's): a file's blob
/// through the attributes of `rel`, a link's target text unfiltered, a
/// submodule's commit. `None`: nothing there; `Err`: a folder where a file
/// goes, or git could not hash it.
pub async fn object_id(cwd: &Path, prefix: &[&str], rel: &str, what: &OnDisk) -> Result<Option<String>, ()> {
    let path = format!("--path={rel}");
    let (args, input): (Vec<&str>, &[u8]) = match what {
        OnDisk::Nothing => return Ok(None),
        OnDisk::Folder => return Err(()),
        OnDisk::Commit(c) => return Ok(Some(c.clone())),
        OnDisk::Link(target) => (prefix.iter().copied().chain(["hash-object", "--stdin", "--no-filters"]).collect(), target),
        OnDisk::File(bytes) => (prefix.iter().copied().chain(["hash-object", "--stdin", path.as_str()]).collect(), bytes),
    };
    match git_io(cwd, &args, Some(input)).await {
        Ok(out) => Ok(Some(String::from_utf8_lossy(&out).trim().to_string())),
        Err(_) => Err(()),
    }
}

/// How a review's snapshot hashes a file: line endings as they are on
/// disk (a snapshot never converts them).
pub const SNAPSHOT_HASH: &[&str] = &["-c", "core.autocrlf=false"];

/// What `rel` in `dir` holds as a review's snapshot records it
/// ([`SNAPSHOT_HASH`]). `None`: nothing there.
pub async fn content_id(dir: &Path, rel: &str) -> Result<Option<String>, ()> {
    object_id(dir, SNAPSHOT_HASH, rel, &read_disk(dir, rel).await).await
}

/// What the disk under `root` holds at `rel` as its repository would
/// record it. `None` when there is nothing there; `Err` when it can't be
/// read as any of them (a folder where a file goes).
async fn disk_object(root: &Path, rel: &str) -> Result<Option<String>, ()> {
    object_id(root, &[], rel, &read_disk(root, rel).await).await
}

/// The paths of `paths` where the owner's folder holds something that is
/// not `tip`'s version: an edit, staged or not, a new or ignored file in
/// the way, a deletion, a folder where the change puts a file. A folder the
/// change writes into must be a folder on the owner's disk too: a file or a
/// link of the owner's where it goes (one the change does not itself
/// replace) is in the way. A tracked link and a submodule compare as git
/// records them.
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
        let at_tip = git(root, &["rev-parse", "--verify", "--quiet", &format!("{tip}:{rel}")]).await.ok().filter(|s| !s.is_empty());
        let differs = match (disk_object(root, rel).await, at_tip) {
            (Ok(None), None) => false,
            (Ok(Some(on_disk)), Some(blob)) => on_disk != blob,
            _ => true,
        };
        let staged = git(root, &["--literal-pathspecs", "diff", "--cached", "--quiet", tip, "--", rel]).await.is_err();
        if differs || staged {
            out.push(rel.clone());
        }
    }
    out
}

/// Git's own conflict marker width; `conflict-marker-size` in a project's
/// `.gitattributes` may set another.
const DEFAULT_MARKER_SIZE: usize = 7;

/// Whether a file still holds conflict markers of `size`: a line opening
/// one (`<<<<<<<` and a space or the line's end) and a later line closing
/// it. A heading's `=======` underline alone is no marker.
fn has_markers(bytes: &[u8], size: usize) -> bool {
    let is = |line: &str, c: char| {
        line.len() >= size && line.chars().take(size).all(|x| x == c) && line[size..].chars().next().is_none_or(|n| n == ' ')
    };
    let mut open = false;
    for line in String::from_utf8_lossy(bytes).lines() {
        if is(line, '<') {
            open = true;
        } else if open && is(line, '>') {
            return true;
        }
    }
    false
}

/// The marker width git uses for `rel` in the repository at `dir`.
async fn marker_size(dir: &Path, rel: &str) -> usize {
    git(dir, &["check-attr", "conflict-marker-size", "--", rel])
        .await
        .ok()
        .and_then(|out| out.rsplit(": ").next().and_then(|n| n.trim().parse().ok()))
        .unwrap_or(DEFAULT_MARKER_SIZE)
}

/// The files of `files` in the repository at `dir` that still hold
/// conflict markers, each of the width git uses for it.
async fn with_markers(dir: &Path, files: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for f in files {
        let size = marker_size(dir, f).await;
        if std::fs::read(dir.join(f)).is_ok_and(|b| has_markers(&b, size)) {
            out.push(f.clone());
        }
    }
    out
}

/// Land a worktree's one commit by fast-forwarding the owner's branch.
///
/// A file lands under the name git records for it. In a repository with
/// `core.precomposeunicode` on (macOS's default), git records a name typed
/// decomposed (as Finder writes it) composed, and its checkout writes the
/// file under that composed name: a tracked file the change touches keeps
/// its name, but not its original bytes. That is git's own checkout, the same
/// one the owner's `git checkout` and `git pull` do, and the disk (APFS)
/// treats the two spellings as one file; landing without git's checkout
/// would be a second way to write the owner's tree, so it is not done.
async fn land_git(wt: &Worktree, landing: &Landing) -> MergeOutcome {
    let failed = |error: String| MergeOutcome::Failed { error, kept_at: wt.path.clone(), branch: wt.branch.clone() };
    if rebasing(wt).await {
        return failed("catching up is not finished: a conflict is still being resolved in the copy".into());
    }
    if let Some(why) = owner_busy(wt).await {
        return MergeOutcome::OwnerBusy { why, kept_at: wt.path.clone(), branch: wt.branch.clone() };
    }
    let owner_ref = format!("refs/heads/{}", wt.start_branch);
    let owner = match owner_tip(wt).await {
        Ok(owner) => owner,
        Err(e) => return failed(e),
    };
    let Ok(head) = git(&wt.path, &["rev-parse", "HEAD"]).await else {
        return failed("the copy has no HEAD".into());
    };
    // What lands is the copy's own branch, exactly.
    if git(&wt.path, &["rev-parse", &format!("refs/heads/{}", wt.branch)]).await.ok().as_deref() != Some(head.as_str()) {
        return failed(format!("the copy is not on its own branch {}", wt.branch));
    }
    if head == owner {
        return finish_no_changes(&Isolation::Git(wt.clone())).await;
    }
    if !sits_on(&wt.path, "HEAD", &owner).await {
        return failed("the project moved on since the copy caught up; catch up again".into());
    }
    let paths = touched_paths(&wt.path, &owner, "HEAD").await;
    if let Err(outcome) = held_to_review(&Isolation::Git(wt.clone()), landing).await {
        return outcome;
    }
    let marked = with_markers(&wt.path, &paths).await;
    if !marked.is_empty() {
        return MergeOutcome::Conflict { files: marked, kept_at: wt.path.clone(), branch: wt.branch.clone() };
    }
    let in_the_way = owner_data_in_the_way(&wt.root, &owner, &paths).await;
    if !in_the_way.is_empty() {
        return MergeOutcome::OwnerEdits { files: in_the_way, kept_at: wt.path.clone(), branch: wt.branch.clone() };
    }
    let files = z_paths(&wt.path, &["diff", "-z", "--name-only", &owner, "HEAD"]).await;
    // An ignored file of the owner's made since it was checked is never
    // overwritten: git refuses instead.
    if let Err(e) = git(&wt.root, &["merge", "--ff-only", "--no-overwrite-ignore", "--quiet", &head]).await {
        return failed(format!("fast-forward refused: {e}"));
    }
    // Landed only if the owner's branch is now that commit.
    if git(&wt.root, &["rev-parse", &owner_ref]).await.ok().as_deref() != Some(head.as_str()) {
        return failed("the project's branch did not move to the change".into());
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
    }
    MergeOutcome::NoChanges
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

/// After a copy's checks: what was checked is what lands. A file the
/// checks wrote that the work had not made (a cache, build output) is
/// dropped from the copy, never landed; a change of theirs to the work's
/// own files (a formatter, a snapshot update) is refused, and the work goes
/// back to its builder. Git alike: the checks may change no file the commit
/// holds, and a file they add beside it is dropped.
pub async fn after_checks(iso: &Isolation) -> Result<(), String> {
    let changed: Vec<String> = match iso {
        Isolation::Git(wt) => {
            // The work was all committed before the checks ran: a file
            // untracked now is theirs.
            for made in z_paths(&wt.path, &["ls-files", "-z", "--others", "--exclude-standard"]).await {
                let _ = std::fs::remove_file(wt.path.join(made));
            }
            z_paths(&wt.path, &["diff", "-z", "--name-only", "HEAD"]).await
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

/// One landing at a time per project, for every Nebo on this computer
/// whatever its data folder: a lock file in the repository's own git
/// directory (`nebo-land.lock`), or for a folder that is no repository in
/// the system's temp folder by the folder's path. Held while a copy lands,
/// released when the returned file is dropped. A landing that has to wait
/// says so once (`landing.waiting`), and `landing.stop` ends the wait.
async fn project_lock(root: &Path, landing: &Landing) -> Result<std::fs::File, String> {
    // The git directory as git names it, relative to `root` or absolute.
    let at = match git(root, &["rev-parse", "--git-common-dir"]).await {
        Ok(dir) => root.join(dir.trim()).join("nebo-land.lock"),
        Err(_) => {
            let canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
            std::env::temp_dir().join(format!("nebo-land-{:016x}.lock", tools::checkpoint::fingerprint(canonical.to_string_lossy().as_bytes())))
        }
    };
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&at)
        .map_err(|e| format!("open {}: {e}", at.display()))?;
    let mut told = false;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock) => {}
            Err(std::fs::TryLockError::Error(e)) => return Err(format!("lock {}: {e}", at.display())),
        }
        if !told {
            told = true;
            tracing::info!(project = %root.display(), "waiting for another landing on this project to finish");
            if let Some(waiting) = &landing.waiting {
                waiting();
            }
        }
        tokio::select! {
            _ = landing.stop.cancelled() => return Err("stopped while waiting for another landing on this project to finish".into()),
            _ = tokio::time::sleep(std::time::Duration::from_millis(200)) => {}
        }
    }
}

/// Land every copy of a set on the owner's project ([`dependency_order`],
/// one at a time, each through [`land_one`]). A copy with nothing to land
/// is removed and does not stop the set. The first copy that does not land
/// stops the set: it says why, the copies after it are not tried, and
/// every copy not landed is kept. Outcomes come back in the order the
/// copies were given.
pub async fn merge_all(isos: &mut [Isolation], message: &str, retest: Option<Retest<'_>>, landing: &Landing) -> Vec<(String, MergeOutcome)> {
    let order = dependency_order(isos);
    let mut outcomes: Vec<Option<MergeOutcome>> = vec![None; isos.len()];
    let mut stopped = false;
    for idx in order {
        let iso = &mut isos[idx];
        if stopped {
            outcomes[idx] = Some(MergeOutcome::NotAttempted { kept_at: iso.path().to_path_buf(), branch: iso.branch().to_string() });
            continue;
        }
        let (_, outcome) = land_one(iso, message, retest, landing).await;
        stopped = !outcome.landed();
        outcomes[idx] = Some(outcome);
    }
    isos.iter().zip(outcomes).map(|(iso, o)| (iso.task_id().to_string(), o.expect("every copy has an outcome"))).collect()
}

/// The one way a copy lands, for the verify loop and for a helper's
/// changes alike, under its project's landing lock: caught up with the
/// owner's project ([`catch_up`]); stopped for another review when what
/// would land is not what was reviewed ([`held_to_review`]); re-tested with `retest`, held to what was checked
/// ([`after_checks`]); landed ([`land`]). Returns the catch-up (when it
/// got that far) and the outcome.
pub async fn land_one(iso: &mut Isolation, message: &str, retest: Option<Retest<'_>>, landing: &Landing) -> (Option<CatchUp>, MergeOutcome) {
    let kept = |iso: &Isolation| (iso.path().to_path_buf(), iso.branch().to_string());
    let _one_at_a_time = match project_lock(iso.root(), landing).await {
        Ok(lock) => lock,
        Err(error) => {
            let (kept_at, branch) = kept(iso);
            return (None, MergeOutcome::Failed { error, kept_at, branch });
        }
    };
    // A fast-forward lost to the owner's own commit is caught up again once.
    let mut last = None;
    for attempt in 0..2 {
        let caught = match catch_up(iso, message, landing).await {
            Ok(c) => c,
            Err(outcome) => return (last, outcome),
        };
        last = Some(caught.clone());
        if caught.no_changes {
            return (last, finish_no_changes(iso).await);
        }
        if !caught.conflicts.is_empty() {
            let (kept_at, branch) = kept(iso);
            return (last, MergeOutcome::Conflict { files: caught.conflicts, kept_at, branch });
        }
        // Catching up may have changed what was reviewed: reviewed again
        // first, never re-tested and landed as it is.
        if let Err(outcome) = held_to_review(iso, landing).await {
            return (last, outcome);
        }
        if let Some(retest) = retest {
            let checked = match retest(iso).await {
                Ok(()) => after_checks(iso).await,
                Err(error) => Err(error),
            };
            if let Err(error) = checked {
                let (kept_at, branch) = kept(iso);
                return (last, MergeOutcome::RetestFailed { error, kept_at, branch });
            }
        }
        match land(iso, landing).await {
            MergeOutcome::Failed { error, .. } if attempt == 0 && error.contains("catch up again") => continue,
            other => return (last, other),
        }
    }
    let (kept_at, branch) = kept(iso);
    (last, MergeOutcome::Failed { error: "the project kept moving while the copy caught up; try again when it is quiet".into(), kept_at, branch })
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
    }
}

/// Commit whatever a worktree holds on its own task branch and remove the
/// worktree, keeping the branch: its work is stored and the disk freed.
/// Midway through catching up (a conflict being resolved), a copy whose
/// every conflict the builder has staged a resolution for is committed on
/// the owner's commit it was catching up to and the catch-up ends there, so
/// the branch holds the resolution; markers left in it keep it a conflict
/// when it is landed. One with a conflict not yet resolved is kept as it is
/// (`Err`): staging it here would resolve it without the builder's word.
pub async fn shelve(wt: &mut Worktree, message: &str) -> Result<(), String> {
    if rebasing(wt).await && !unmerged(wt).await.is_empty() {
        return Err(format!("{} holds a conflict not resolved yet; it is kept as it is", wt.path.display()));
    }
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
        wt.conflicted.clear();
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
/// Discarding what is already gone (the copy, its branch) succeeds.
pub async fn discard(iso: &Isolation) -> Result<(), String> {
    match iso {
        Isolation::Git(wt) => {
            // A copy put away on its branch has no worktree left to remove.
            if wt.path.exists() {
                git(&wt.root, &["worktree", "remove", "--force", wt.path.to_str().unwrap_or_default()]).await?;
            }
            let _ = git(&wt.root, &["worktree", "prune"]).await;
            if git(&wt.root, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{}", wt.branch)]).await.is_ok() {
                git(&wt.root, &["branch", "-D", &wt.branch]).await?;
            }
            Ok(())
        }
    }
}

/// Sweep the copies a crashed parent left behind in the projects `roots`
/// (`<repo>/.nebo/worktrees`). Only Nebo's own (a helper's `h-…`, a
/// verification's `v-…`, an older `sa-…`), untouched for `cutoff_secs`,
/// and holding no work that hasn't landed: nothing uncommitted or
/// untracked, and no commit the project's other branches and remotes don't
/// hold. Anything git can't answer for is kept. Returns what was removed.
pub async fn cleanup_stale(roots: &[PathBuf], cutoff_secs: u64) -> Vec<PathBuf> {
    let now = std::time::SystemTime::now();
    let mut removed = Vec::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(root.join(NEBO_DIR).join("worktrees")) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let nebos = ["h-", "v-", "sa-"].iter().any(|p| name.starts_with(p)) && validate_task_id(&name).is_ok();
            let old = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|m| now.duration_since(m).ok())
                .is_some_and(|age| age.as_secs() >= cutoff_secs);
            if !nebos || !old || !path.join(".git").is_file() {
                continue;
            }
            let branch = format!("nebo/{name}");
            let dirty = git(&path, &["status", "--porcelain", "--untracked-files=normal"]).await;
            let unlanded = git(&path, &["rev-list", "--count", "HEAD", "--not", &format!("--exclude={branch}"), "--branches", "--remotes"]).await;
            let clean = matches!((&dirty, &unlanded), (Ok(d), Ok(n)) if d.trim().is_empty() && n.trim() == "0");
            if !clean {
                continue;
            }
            let wt = Worktree {
                task_id: name.clone(),
                root: root.clone(),
                path: path.clone(),
                branch,
                start_branch: String::new(),
                base: String::new(),
                onto: None,
                conflicted: Vec::new(),
            };
            if remove(&wt, false).await.is_ok() {
                removed.push(path);
            }
        }
    }
    removed
}

/// The copies the verify loop made in Nebo's data folder
/// (`<data_dir>/worktrees`), before the builder worked in the project
/// itself: each verification row of kind `isolated` whose copy is still
/// there. A git worktree holding work that hasn't landed is moved beside
/// its project (`<repo>/.nebo/worktrees/<id>`, by git); one holding none is
/// removed. A copy of a folder that is no repository is never moved: one
/// identical to its project, file for file and link for link, is removed,
/// and one that differs stays exactly where it is. Either way a kept
/// copy's row says where it is (`kept_in_copy`). A copy whose work can't
/// be told is kept where it is. The folders they were in go once empty.
/// A check of an old copy still live (waiting on the owner's answer) ends
/// as stopped, its question withdrawn: the loop never takes one up again.
/// Run at boot, before a turn reads the rows. Returns how many it kept
/// and removed.
pub async fn adopt_old_copies(store: &db::Store) -> (usize, usize) {
    let Ok(old_root) = config::data_dir().map(|d| d.join("worktrees")) else { return (0, 0) };
    let rows = store.list_verifications(&db::VerificationQuery::default()).unwrap_or_default();
    let (mut kept, mut removed) = (0, 0);
    for v in rows.iter().filter(|v| v.work_kind == "isolated" && v.landed_ref.is_empty()) {
        let copy = PathBuf::from(&v.work_dir);
        if !copy.starts_with(&old_root) || !copy.is_dir() {
            continue;
        }
        let project = PathBuf::from(v.project_root.trim_end_matches('/'));
        let outcome = if copy.join(".git").is_file() {
            adopt_worktree(&copy, &v.id).await
        } else if types::Why::read(&v.reason).is_some_and(|w| w.code == "kept_in_copy") {
            // Kept where it is, and said so, at an earlier start.
            continue;
        } else {
            settle_folder_copy(&copy, &project)
        };
        let at = match outcome {
            Adopted::Moved(to) => {
                let _ = store.set_verification_work_dir(&v.id, &to.to_string_lossy(), &v.isolation, &v.branch);
                if let Some(mut state) = v.isolation_state.clone() {
                    if let Some(path) = state.get_mut("Git").and_then(|a| a.get_mut("path")) {
                        *path = serde_json::Value::String(to.to_string_lossy().into_owned());
                    }
                    let _ = store.set_verification_isolation(&v.id, &state);
                }
                Some(to)
            }
            Adopted::Stays => Some(copy.clone()),
            Adopted::Removed => {
                removed += 1;
                None
            }
            Adopted::Kept(why) => {
                warn!(copy = %copy.display(), error = %why, "an old working copy was kept where it is");
                None
            }
        };
        if let Some(at) = at {
            let reason = types::Why::new("kept_in_copy").with("path", at.to_string_lossy().into_owned()).text();
            let _ = store.set_verification_state(
                &v.id,
                &db::VerificationState {
                    state: &v.state,
                    reason: &reason,
                    rounds: v.rounds,
                    open_blockers: v.open_blockers,
                    open_majors: v.open_majors,
                    open_minors: v.open_minors,
                    open: v.open.clone(),
                    deferred: v.deferred.clone(),
                    summary: &v.summary,
                },
                v.finished_at.unwrap_or(v.created_at),
            );
            kept += 1;
        }
    }
    // The loop works in place now and never takes an old copy's check up
    // again: one still live (paused on a question for the owner, say) would
    // show as waiting on him forever, with a question nothing asks. It ends
    // as stopped, its question withdrawn, its row saying where its copy is.
    for v in rows.iter().filter(|v| v.work_kind == "isolated" && db::VERIFICATION_LIVE.contains(&v.state.as_str())) {
        let Some(now) = store.get_verification(&v.id).ok().flatten() else { continue };
        let reason = match types::Why::read(&now.reason) {
            Some(why) if why.code == "kept_in_copy" => now.reason.clone(),
            _ => types::Why::new("restarted").text(),
        };
        let _ = store.set_verification_pending_ask(&now.id, None);
        let _ = store.set_verification_state(
            &now.id,
            &db::VerificationState {
                state: "stopped",
                reason: &reason,
                rounds: now.rounds,
                open_blockers: now.open_blockers,
                open_majors: now.open_majors,
                open_minors: now.open_minors,
                open: now.open.clone(),
                deferred: now.deferred.clone(),
                summary: &now.summary,
            },
            chrono::Utc::now().timestamp(),
        );
        crate::harness::verify::work::Work::clean_up_reviewers(Path::new(&now.project_root), &now.id);
    }
    // The folders the copies were in, once nothing is left in them.
    for project in std::fs::read_dir(&old_root).into_iter().flatten().flatten() {
        let _ = std::fs::remove_dir(project.path());
    }
    let _ = std::fs::remove_dir(&old_root);
    (kept, removed)
}

/// What became of one old copy ([`adopt_old_copies`]).
enum Adopted {
    /// Moved beside its project, there.
    Moved(PathBuf),
    /// Holds work, and stays where it is.
    Stays,
    Removed,
    /// Its work couldn't be told: kept where it is, unnoted.
    Kept(String),
}

/// An old git worktree copy: moved by git itself into its repository's
/// Nebo folder when it holds work its project doesn't (anything
/// uncommitted or untracked, or a commit no other branch or remote holds),
/// removed with its branch otherwise.
async fn adopt_worktree(copy: &Path, id: &str) -> Adopted {
    let branch = format!("nebo/{id}");
    let (dirty, unlanded) = (
        git(copy, &["status", "--porcelain", "--untracked-files=normal"]).await,
        git(copy, &["rev-list", "--count", "HEAD", "--not", &format!("--exclude={branch}"), "--branches", "--remotes"]).await,
    );
    let (Ok(dirty), Ok(unlanded)) = (dirty, unlanded) else { return Adopted::Kept("git could not say what it holds".into()) };
    let root = match git(copy, &["rev-parse", "--path-format=absolute", "--git-common-dir"]).await {
        Ok(common) => match PathBuf::from(common).parent() {
            Some(root) => root.to_path_buf(),
            None => return Adopted::Kept("its repository could not be found".into()),
        },
        Err(e) => return Adopted::Kept(e),
    };
    if dirty.trim().is_empty() && unlanded.trim() == "0" {
        let wt = Worktree { task_id: id.to_string(), root, path: copy.to_path_buf(), branch, start_branch: String::new(), base: String::new(), onto: None, conflicted: Vec::new() };
        return match remove(&wt, false).await {
            Ok(()) => Adopted::Removed,
            Err(e) => Adopted::Kept(e),
        };
    }
    let to = match worktrees_dir(&root).await {
        Ok(dir) => match std::fs::create_dir_all(&dir) {
            Ok(()) => dir.join(id),
            Err(e) => return Adopted::Kept(e.to_string()),
        },
        Err(e) => return Adopted::Kept(e),
    };
    if to.exists() {
        return Adopted::Kept(format!("{} is taken", to.display()));
    }
    match git(&root, &["worktree", "move", &copy.to_string_lossy(), &to.to_string_lossy()]).await {
        Ok(_) => Adopted::Moved(to),
        Err(e) => Adopted::Kept(e),
    }
}

/// An old copy of a folder that was no repository: removed when it is
/// identical to its project (every file's bytes and whether it may be run,
/// every link's target, nothing more and nothing less; Nebo's own folders in the project's
/// `.nebo` aside), and otherwise left exactly where it is. It is never
/// moved: in the project's folder a later `git clean -fdx` would take it.
fn settle_folder_copy(copy: &Path, project: &Path) -> Adopted {
    let mut theirs = Vec::new();
    let mut ours = Vec::new();
    if let Err(e) = entries_under(copy, copy, &mut ours).and_then(|()| entries_under(project, project, &mut theirs)) {
        return Adopted::Kept(e);
    }
    theirs.retain(|(rel, _)| !NEBO_OWN.iter().any(|own| rel.starts_with(&format!("{NEBO_DIR}/{own}/"))));
    ours.sort();
    theirs.sort();
    if ours != theirs {
        return Adopted::Stays;
    }
    match std::fs::remove_dir_all(copy) {
        Ok(()) => Adopted::Removed,
        Err(e) => Adopted::Kept(e.to_string()),
    }
}

/// Every file and link under `dir`, relative to `base` ('/'-separated):
/// a file with its bytes' fingerprint and whether it may be run, a link
/// with its target as written (never followed).
fn entries_under(base: &Path, dir: &Path, out: &mut Vec<(String, String)>) -> Result<(), String> {
    for entry in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?.flatten() {
        let path = entry.path();
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        let rel = path.strip_prefix(base).unwrap_or(&path).components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
        if kind.is_symlink() {
            let target = std::fs::read_link(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            out.push((rel, format!("link:{}", target.display())));
        } else if kind.is_dir() {
            entries_under(base, &path, out)?;
        } else if kind.is_file() {
            let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            #[cfg(unix)]
            let runs = {
                use std::os::unix::fs::PermissionsExt;
                entry.metadata().map_err(|e| e.to_string())?.permissions().mode() & 0o111 != 0
            };
            #[cfg(not(unix))]
            let runs = false;
            out.push((rel, format!("file:{:016x}:{}", tools::checkpoint::fingerprint(&bytes), if runs { "x" } else { "-" })));
        } else {
            return Err(format!("{} is neither a file, a folder nor a link", path.display()));
        }
    }
    Ok(())
}

/// The lines the child sees at the top of its prompt.
pub fn preamble(iso: &Isolation) -> String {
    match iso {
        Isolation::Git(wt) => format!(
            "Working copy: {}\nThis is your own git worktree on branch {}. It is also your shell's working directory and where relative paths resolve. Do ALL file work under that path and nowhere else; the parent merges it when you finish. Commit as you like or not at all: uncommitted work is committed for you.\n\n",
            wt.path.display(),
            wt.branch
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
            "- {desc}: not landed yet. Catching up with the project met the owner's changes in {}; they wait in the copy{} at {} to be combined there (the rebase is in progress there: resolve each file, then stage it with `git add`, or `git rm` to keep a deletion; staging it is the word that it is resolved). Nothing of the owner's changed.",
            files.join(", "),
            branch_note(branch),
            kept_at.display(),
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
        MergeOutcome::Unreviewed { files, kept_at, branch } => format!(
            "- {desc}: not landed. The copy holds changes nobody reviewed ({}): too big to snapshot, a repository of its own, or made after the review. Nothing of the owner's changed; the work is kept{} at {}. Remove them, or the owner decides.",
            files.join(", "),
            branch_note(branch),
            kept_at.display()
        ),
        MergeOutcome::NeedsReview { files, kept_at, branch } => format!(
            "- {desc}: not landed yet. Catching up with the project changed files this change touches ({}), so it is reviewed again first; the work is kept{} at {}.",
            files.join(", "),
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

    /// The builder's word that a conflict is resolved: its resolution
    /// staged (`git add`).
    async fn stage_resolution(iso: &Isolation) {
        let Isolation::Git(wt) = iso;
        git(&wt.path, &["add", "-A"]).await.unwrap();
    }

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
    }


    async fn log(dir: &Path) -> Vec<String> {
        git(dir, &["log", "--format=%s|%p"]).await.unwrap().lines().map(str::to_string).collect()
    }

    /// A landing held to `paths` as `dir` holds them now: what a review of
    /// it saw.
    async fn reviewed(dir: &Path, paths: &[&str]) -> Landing {
        let mut only = Reviewed::new();
        for p in paths {
            only.insert(p.to_string(), content_id(dir, p).await.unwrap());
        }
        Landing { only: Some(only), ..Default::default() }
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
            let out = merge_all(&mut isos, "Add b and change a", None, &Landing::default()).await;
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
            let out = merge_all(&mut isos, "Add b", Some(&retest), &Landing::default()).await;
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
            let out = merge_all(&mut isos, "Change a", None, &Landing::default()).await;
            assert!(matches!(&out[0].1, MergeOutcome::Conflict { files, .. } if files == &["a.txt".to_string()]), "{:?}", out[0].1);
            assert_eq!(std::fs::read_to_string(r.path().join("a.txt")).unwrap(), "parent\n", "the owner's tree is as it was");
            assert!(git(r.path(), &["rev-parse", "-q", "--verify", "MERGE_HEAD"]).await.is_err());
            assert!(std::fs::read_to_string(copy.join("a.txt")).unwrap().contains("<<<<<<< "), "the conflict waits in the copy");

            // Still unresolved: it stays a conflict.
            assert!(matches!(merge_all(&mut isos, "Change a", None, &Landing::default()).await[0].1, MergeOutcome::Conflict { .. }));
            // Resolved in the files but not said: still a conflict.
            std::fs::write(copy.join("a.txt"), "parent and child\n").unwrap();
            assert!(matches!(merge_all(&mut isos, "Change a", None, &Landing::default()).await[0].1, MergeOutcome::Conflict { .. }), "only the builder's word resolves it");
            // The builder says so; the re-test runs on the result.
            stage_resolution(&isos[0]).await;
            let retest = |iso: &Isolation| -> futures::future::BoxFuture<'static, Result<(), String>> {
                let text = std::fs::read_to_string(iso.path().join("a.txt")).unwrap_or_default();
                Box::pin(async move { if text == "parent and child\n" { Ok(()) } else { Err(text) } })
            };
            let out = merge_all(&mut isos, "Change a", Some(&retest), &Landing::default()).await;
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
            assert_eq!(merge_all(&mut isos, "m", None, &Landing::default()).await[0].1, MergeOutcome::NoChanges);
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
            match &merge_all(&mut isos, "Change a", None, &Landing::default()).await[0].1 {
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
            assert_eq!(landed_files(&merge_all(&mut isos, "Change a", None, &Landing::default()).await[0].1), vec!["a.txt".to_string()]);
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
            assert!(matches!(&merge_all(&mut isos, "Add b", Some(&retest), &Landing::default()).await[0].1, MergeOutcome::RetestFailed { error, .. } if error == "2 tests failed"));
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
            let out = merge_all(&mut isos, "Use f", Some(&retest), &Landing::default()).await;
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
            let out = merge_all(&mut isos, "Use g", Some(&fail_first), &Landing::default()).await;
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

    /// A brand-new repository: `git init` and no commit, as an employee
    /// starting a project leaves it.
    async fn unborn() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for args in [vec!["init", "-q", "-b", "main"], vec!["config", "user.email", "t@t"], vec!["config", "user.name", "t"]] {
            git(dir.path(), &args).await.unwrap();
        }
        dir
    }

    #[test]
    fn a_repository_with_no_commit_gets_a_copy_whose_reviewed_commit_becomes_its_first() {
        with_home(|_| rt().block_on(async {
            let r = unborn().await;
            let wt = create_worktree(r.path(), "sa-u1").await.unwrap();
            assert_eq!(wt.start_branch, "main");
            assert!(git(r.path(), &["rev-parse", "--verify", "--quiet", "HEAD"]).await.is_err(), "nothing committed in the owner's project");
            // The builder writes the project's first files, commits one of
            // them itself, and the review sees both.
            std::fs::write(wt.path.join(".gitignore"), "target/\n").unwrap();
            git(&wt.path, &["add", "-A"]).await.unwrap();
            git(&wt.path, &["commit", "-q", "-m", "wip"]).await.unwrap();
            std::fs::create_dir_all(wt.path.join("src")).unwrap();
            std::fs::write(wt.path.join("src/main.rs"), "fn main() {}\n").unwrap();
            let landing = reviewed(&wt.path, &[".gitignore", "src/main.rs"]).await;
            let path = wt.path.clone();
            let mut isos = [Isolation::Git(wt)];
            let out = merge_all(&mut isos, "Start the project", None, &landing).await;
            assert_eq!(landed_files(&out[0].1), vec![".gitignore".to_string(), "src/main.rs".to_string()]);
            assert_eq!(log(r.path()).await, ["Start the project|"], "exactly the reviewed commit, with no parent");
            let MergeOutcome::Merged { commit, .. } = &out[0].1 else { unreachable!() };
            assert_eq!(&git(r.path(), &["rev-parse", "refs/heads/main"]).await.unwrap(), commit);
            assert_eq!(std::fs::read_to_string(r.path().join("src/main.rs")).unwrap(), "fn main() {}\n");
            assert!(git(r.path(), &["status", "--porcelain"]).await.unwrap().is_empty(), "the owner's folder is that commit");
            assert!(!path.exists(), "the copy is removed after it landed");
        }))
    }

    #[test]
    fn a_first_commit_the_owner_makes_while_the_copy_is_open_is_caught_up_to() {
        with_home(|_| rt().block_on(async {
            let r = unborn().await;
            let wt = create_worktree(r.path(), "sa-u2").await.unwrap();
            std::fs::write(wt.path.join("app.js"), "start()\n").unwrap();
            // The owner makes the project's first commit meanwhile.
            std::fs::write(r.path().join("README.md"), "mine\n").unwrap();
            git(r.path(), &["add", "-A"]).await.unwrap();
            git(r.path(), &["commit", "-q", "-m", "owner's first"]).await.unwrap();
            let owner = git(r.path(), &["rev-parse", "HEAD"]).await.unwrap();
            let mut isos = [Isolation::Git(wt)];
            let out = merge_all(&mut isos, "Add the app", None, &Landing::default()).await;
            let MergeOutcome::Merged { files, caught_up_to, .. } = &out[0].1 else { panic!("{:?}", out[0].1) };
            assert_eq!(files, &vec!["app.js".to_string()]);
            assert_eq!(caught_up_to, &owner);
            assert_eq!(git(r.path(), &["log", "--format=%s"]).await.unwrap(), "Add the app\nowner's first");
            assert_eq!(git(r.path(), &["rev-parse", "HEAD^"]).await.unwrap(), owner, "on the owner's first commit");
            assert_eq!(std::fs::read_to_string(r.path().join("README.md")).unwrap(), "mine\n");
        }))
    }

    #[test]
    fn a_file_the_owner_left_uncommitted_in_a_new_repository_is_never_overwritten() {
        with_home(|_| rt().block_on(async {
            let r = unborn().await;
            let wt = create_worktree(r.path(), "sa-u3").await.unwrap();
            assert!(!wt.path.join("notes.txt").exists(), "what the owner hasn't committed is not in the copy");
            std::fs::write(wt.path.join("notes.txt"), "the copy's\n").unwrap();
            std::fs::write(r.path().join("notes.txt"), "the owner's\n").unwrap();
            let mut isos = [Isolation::Git(wt)];
            let out = merge_all(&mut isos, "Add notes", None, &Landing::default()).await;
            assert!(!out[0].1.landed(), "{:?}", out[0].1);
            assert_eq!(std::fs::read_to_string(r.path().join("notes.txt")).unwrap(), "the owner's\n");
            assert!(git(r.path(), &["rev-parse", "--verify", "--quiet", "HEAD"]).await.is_err(), "still no commit");
        }))
    }

    #[test]
    fn a_copy_of_a_new_repository_put_away_comes_back_on_its_own_branch() {
        with_home(|_| rt().block_on(async {
            let r = unborn().await;
            let mut iso = Isolation::Git(create_worktree(r.path(), "sa-u4").await.unwrap());
            std::fs::write(iso.path().join("a.txt"), "kept\n").unwrap();
            assert!(put_away(&mut iso, "Kept").await.is_some());
            assert!(!iso.path().exists());
            let wt = create_worktree(r.path(), "sa-u4").await.unwrap();
            assert_eq!(std::fs::read_to_string(wt.path.join("a.txt")).unwrap(), "kept\n");
            let mut isos = [Isolation::Git(wt)];
            let out = merge_all(&mut isos, "Add a", None, &Landing::default()).await;
            assert_eq!(landed_files(&out[0].1), vec!["a.txt".to_string()]);
            assert_eq!(log(r.path()).await, ["Add a|"]);
        }))
    }



    #[test]
    fn every_outcome_renders_one_line_for_its_parent() {
        let kept: PathBuf = "/w/sa-1".into();
        let outcomes = [
            MergeOutcome::NoChanges,
            MergeOutcome::Merged { files: vec!["a".into()], commit: "0123456789abcdef".into(), caught_up_to: "f".into() },
            MergeOutcome::Conflict { files: vec!["a".into()], kept_at: kept.clone(), branch: "nebo/sa-1".into() },
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
    }

    // ---- from the review of the landing (each a repro turned into a test) ----

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
            let out = merge_all(&mut isos, "Add a config template", None, &Landing::default()).await;
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
            let out = merge_all(&mut isos, "Add b", None, &Landing::default()).await;
            assert!(matches!(&out[0].1, MergeOutcome::OwnerBusy { why, .. } if why.to_string().contains("experiment")), "{:?}", out[0].1);
            assert!(!r.path().join("b.txt").exists());

            git(r.path(), &["checkout", "-q", "--detach"]).await.unwrap();
            assert!(matches!(&merge_all(&mut isos, "Add b", None, &Landing::default()).await[0].1, MergeOutcome::OwnerBusy { why, .. } if why.to_string().contains("no branch")));

            git(r.path(), &["checkout", "-q", "main"]).await.unwrap();
            let merge_head = git(r.path(), &["rev-parse", "--git-path", "MERGE_HEAD"]).await.unwrap();
            let merge_head = r.path().join(merge_head);
            std::fs::write(&merge_head, git(r.path(), &["rev-parse", "HEAD"]).await.unwrap()).unwrap();
            assert!(matches!(&merge_all(&mut isos, "Add b", None, &Landing::default()).await[0].1, MergeOutcome::OwnerBusy { why, .. } if why.to_string().contains("a merge")));
            std::fs::remove_file(&merge_head).unwrap();
            assert_eq!(landed_files(&merge_all(&mut isos, "Add b", None, &Landing::default()).await[0].1), vec!["b.txt".to_string()]);
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
            assert!(matches!(merge_all(&mut isos, "m", None, &Landing::default()).await[0].1, MergeOutcome::Conflict { .. }));
            // Tried again with nobody resolving anything: still a conflict.
            assert!(matches!(merge_all(&mut isos, "m", None, &Landing::default()).await[0].1, MergeOutcome::Conflict { .. }));
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
            merge_all(&mut isos, "m", None, &Landing::default()).await;
            std::fs::write(copy.join("a.txt"), "Title\n=======\nparent and child\n").unwrap();
            stage_resolution(&isos[0]).await;
            assert_eq!(landed_files(&merge_all(&mut isos, "m", None, &Landing::default()).await[0].1), vec!["a.txt".to_string()]);
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
            let out = merge_all(&mut isos, "Add z", None, &Landing::default()).await;
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
            let out = merge_all(&mut isos, "Add b", None, &Landing::default()).await;
            assert!(matches!(&out[0].1, MergeOutcome::HookFailed { output, .. } if output.contains("TODO left in b.txt")), "{:?}", out[0].1);
            assert!(!r.path().join("b.txt").exists());
            assert!(copy.join("b.txt").exists(), "the work is kept for the builder");
            std::fs::write(copy.join("b.txt"), "done\n").unwrap();
            assert_eq!(landed_files(&merge_all(&mut isos, "Add b", None, &Landing::default()).await[0].1), vec!["b.txt".to_string()]);
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
            let out = merge_all(&mut isos, "Rename a to b", None, &Landing::default()).await;
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
            assert_eq!(landed_files(&merge_all(&mut isos, "Add b", Some(&retest), &Landing::default()).await[0].1), vec!["b.txt".to_string()]);
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
    fn a_name_in_any_script_lands_whole() {
        with_home(|_| rt().block_on(async {
            let r = repo().await;
            let wt = create_worktree(r.path(), "sa-r14").await.unwrap();
            std::fs::write(wt.path.join("naïve résumé.txt"), "x\n").unwrap();
            let mut isos = [Isolation::Git(wt)];
            assert_eq!(landed_files(&merge_all(&mut isos, "Add the résumé", None, &Landing::default()).await[0].1), vec!["naïve résumé.txt".to_string()]);
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
                let out = merge_all(&mut isos, "Add a readme", None, &Landing::default()).await;
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
            assert!(matches!(&merge_all(&mut isos, "Change a", None, &Landing::default()).await[0].1, MergeOutcome::Conflict { .. }));
            assert!(matches!(&merge_all(&mut isos, "Change a", None, &Landing::default()).await[0].1, MergeOutcome::Conflict { .. }), "not resolved by anyone");
            stage_resolution(&isos[0]).await;
            let out = merge_all(&mut isos, "Change a", None, &Landing::default()).await;
            assert!(matches!(&out[0].1, MergeOutcome::Conflict { files, .. } if files == &vec!["a.txt".to_string()]), "{:?}", out[0].1);
            assert_eq!(std::fs::read_to_string(r.path().join("a.txt")).unwrap(), "parent\n");
            std::fs::write(copy.join("a.txt"), "parent and child\n").unwrap();
            stage_resolution(&isos[0]).await;
            assert_eq!(landed_files(&merge_all(&mut isos, "Change a", None, &Landing::default()).await[0].1), vec!["a.txt".to_string()]);
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
            assert!(matches!(&merge_all(&mut isos, "Change a", None, &Landing::default()).await[0].1, MergeOutcome::Conflict { .. }));
            std::fs::write(copy.join("a.txt"), "parent and child\n").unwrap();
            std::fs::write(copy.join("more.txt"), "more work\n").unwrap();
            let [mut iso] = isos;
            // Not resolved yet: kept as it is, nothing staged for the builder.
            let note = put_away(&mut iso, "Unfinished").await.unwrap();
            assert!(note.contains("kept in its copy") && copy.exists(), "{note}");
            assert!(!unmerged(match &iso { Isolation::Git(w) => w }).await.is_empty(), "the conflict is still the builder's");
            // Resolved (staged): put away on its branch.
            stage_resolution(&iso).await;
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
            let landed = landed_files(&merge_all(&mut isos, "Change a", None, &Landing::default()).await[0].1);
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
            let out = merge_all(&mut isos, "Add b", Some(&formatter), &Landing::default()).await;
            assert!(matches!(&out[0].1, MergeOutcome::RetestFailed { error, .. } if error.contains("the checks changed b.txt")), "{:?}", out[0].1);
            assert!(!r.path().join("b.txt").exists());
            // The rewrite waits in the copy for the builder; nothing of it
            // landed. The builder undoes it, and checks that only write
            // output beside the change let it land; their output does not.
            assert_eq!(std::fs::read_to_string(path.join("b.txt")).unwrap(), "formatted\n");
            std::fs::write(path.join("b.txt"), "child\n").unwrap();
            let output_only = |iso: &Isolation| -> futures::future::BoxFuture<'static, Result<(), String>> {
                let p = iso.path().to_path_buf();
                Box::pin(async move {
                    std::fs::create_dir_all(p.join("build")).unwrap();
                    std::fs::write(p.join("build/out.txt"), "output").unwrap();
                    Ok(())
                })
            };
            let out = merge_all(&mut isos, "Add b", Some(&output_only), &Landing::default()).await;
            assert_eq!(landed_files(&out[0].1), vec!["b.txt".to_string()]);
            assert_eq!(std::fs::read_to_string(r.path().join("b.txt")).unwrap(), "child\n");
            assert!(!r.path().join("build").exists(), "the checks' output never lands");
            assert!(!path.exists(), "the landed copy is removed, the checks' output with it");
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
            assert_eq!(landed_files(&merge_all(&mut isos, "v2", None, &Landing::default()).await[0].1), vec!["x.bin".to_string()]);
            assert_eq!(std::fs::read_to_string(r.path().join("x.bin")).unwrap(), "v2\n");
            let wt = create_worktree(r.path(), "sa-lfs2").await.unwrap();
            std::fs::write(wt.path.join("x.bin"), "v3\n").unwrap();
            std::fs::write(r.path().join("x.bin"), "OWNER UNSAVED\n").unwrap();
            let mut isos = [Isolation::Git(wt)];
            assert!(matches!(&merge_all(&mut isos, "v3", None, &Landing::default()).await[0].1, MergeOutcome::OwnerEdits { .. }));
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
            landed_files(&merge_all(&mut isos, "Add b", Some(&retest), &Landing::default()).await[0].1);
            assert_eq!(std::fs::read_to_string(r.path().join("b.txt")).unwrap(), *seen.lock().unwrap());
            assert_eq!(*seen.lock().unwrap(), "child\nhooked\n");
        }))
    }

    /// Review round 3 M1: conflict markers of the width a project's
    /// `.gitattributes` sets are markers too: staged with them still in,
    /// it is still a conflict.
    #[test]
    fn markers_of_any_width_keep_a_conflict() {
        with_home(|_| rt().block_on(async {
            let r = repo().await;
            std::fs::write(r.path().join(".gitattributes"), "a.txt conflict-marker-size=32\n").unwrap();
            git(r.path(), &["add", "-A"]).await.unwrap();
            git(r.path(), &["commit", "-q", "-m", "attrs"]).await.unwrap();
            let wt = create_worktree(r.path(), "sa-mk").await.unwrap();
            std::fs::write(wt.path.join("a.txt"), "child\n").unwrap();
            std::fs::write(r.path().join("a.txt"), "parent\n").unwrap();
            git(r.path(), &["commit", "-q", "-am", "parent"]).await.unwrap();
            let copy = wt.path.clone();
            let mut isos = [Isolation::Git(wt)];
            assert!(matches!(&merge_all(&mut isos, "m", None, &Landing::default()).await[0].1, MergeOutcome::Conflict { .. }));
            let text = std::fs::read_to_string(copy.join("a.txt")).unwrap();
            assert!(text.contains(&"<".repeat(32)), "git wrote wide markers: {text}");
            stage_resolution(&isos[0]).await;
            assert!(matches!(&merge_all(&mut isos, "m", None, &Landing::default()).await[0].1, MergeOutcome::Conflict { files, .. } if files == &vec!["a.txt".to_string()]));
            assert_eq!(std::fs::read_to_string(r.path().join("a.txt")).unwrap(), "parent\n");
        }))
    }

    /// Review round 3 M3: what lands is the copy's own branch, and the
    /// owner's branch must have moved to it.
    #[test]
    fn a_copy_off_its_branch_never_lands() {
        with_home(|_| rt().block_on(async {
            let r = repo().await;
            let wt = create_worktree(r.path(), "sa-off").await.unwrap();
            std::fs::write(wt.path.join("b.txt"), "b\n").unwrap();
            let mut iso = Isolation::Git(wt);
            let caught = catch_up(&mut iso, "Add b", &Landing::default()).await.unwrap();
            assert!(!caught.no_changes);
            git(iso.path(), &["checkout", "-q", "--detach"]).await.unwrap();
            std::fs::write(iso.path().join("c.txt"), "c\n").unwrap();
            git(iso.path(), &["add", "-A"]).await.unwrap();
            git(iso.path(), &["commit", "-q", "-m", "off the branch"]).await.unwrap();
            let out = land(&iso, &Landing::default()).await;
            assert!(matches!(&out, MergeOutcome::Failed { error, .. } if error.contains("not on its own branch")), "{out:?}");
            assert!(!r.path().join("c.txt").exists());
            assert!(iso.path().exists(), "never force-removed");
        }))
    }

    /// A tracked link and a submodule are compared as git records them: a
    /// change to either lands, and nothing of the owner's is mistaken for
    /// an edit.
    #[cfg(unix)]
    #[test]
    fn a_tracked_link_and_a_submodule_bump_land() {
        with_home(|_| rt().block_on(async {
            let r = repo().await;
            std::os::unix::fs::symlink("a.txt", r.path().join("link")).unwrap();
            let sub = repo().await;
            git(r.path(), &["-c", "protocol.file.allow=always", "submodule", "add", "-q", &sub.path().to_string_lossy(), "sub"]).await.unwrap();
            git(r.path(), &["add", "-A"]).await.unwrap();
            git(r.path(), &["commit", "-q", "-m", "link and sub"]).await.unwrap();
            std::fs::write(sub.path().join("s.txt"), "s\n").unwrap();
            git(sub.path(), &["add", "-A"]).await.unwrap();
            git(sub.path(), &["commit", "-q", "-m", "next"]).await.unwrap();
            let next = git(sub.path(), &["rev-parse", "HEAD"]).await.unwrap();
            let wt = create_worktree(r.path(), "sa-ln").await.unwrap();
            std::fs::remove_file(wt.path.join("link")).unwrap();
            std::os::unix::fs::symlink("b.txt", wt.path.join("link")).unwrap();
            git(&wt.path, &["update-index", "--add", "--cacheinfo", &format!("160000,{next},sub")]).await.unwrap();
            let landing = reviewed(&wt.path, &["link", "sub"]).await;
            let mut isos = [Isolation::Git(wt)];
            let out = merge_all(&mut isos, "Point the link at b; bump sub", None, &landing).await;
            let mut files = landed_files(&out[0].1);
            files.sort();
            assert_eq!(files, vec!["link".to_string(), "sub".to_string()]);
            assert_eq!(git(r.path(), &["rev-parse", "HEAD:sub"]).await.unwrap(), next);
        }))
    }

    #[test]
    fn unreviewed_build_output_never_lands_from_a_worktree() {
        with_home(|_| rt().block_on(async {
            let r = repo().await;
            let wt = create_worktree(r.path(), "sa-unrev-build").await.unwrap();
            std::fs::write(wt.path.join("b.txt"), "reviewed\n").unwrap();
            std::fs::create_dir_all(wt.path.join("dist")).unwrap();
            std::fs::write(wt.path.join("dist/app.js"), "never reviewed").unwrap();
            let landing = reviewed(&wt.path, &["b.txt"]).await;
            let mut isos = [Isolation::Git(wt)];
            let out = merge_all(&mut isos, "Add b", None, &landing).await;
            assert!(matches!(&out[0].1, MergeOutcome::Unreviewed { files, .. } if files == &vec!["dist/app.js".to_string()]), "{:?}", out[0].1);
            assert!(!r.path().join("dist/app.js").exists(), "unreviewed build output landed on the owner's branch");
        }))
    }

    #[cfg(unix)]
    #[test]
    fn a_hook_rewrite_goes_back_to_review() {
        with_home(|_| rt().block_on(async {
            let r = repo().await;
            hook(r.path(), "pre-commit", "echo injected >> b.txt\ngit add b.txt");
            let wt = create_worktree(r.path(), "sa-hook-rewrite").await.unwrap();
            std::fs::write(wt.path.join("b.txt"), "reviewed\n").unwrap();
            let landing = reviewed(&wt.path, &["b.txt"]).await;
            let mut isos = [Isolation::Git(wt)];
            let out = merge_all(&mut isos, "Add b", None, &landing).await;
            // The rewrite goes back to review; nothing of it lands.
            assert!(matches!(&out[0].1, MergeOutcome::NeedsReview { files, .. } if files == &vec!["b.txt".to_string()]), "{:?}", out[0].1);
            assert!(!std::fs::read_to_string(r.path().join("b.txt")).unwrap_or_default().contains("injected"));
        }))
    }

    /// Review round 4: landings on one project take turns through a lock in
    /// the repository's own git directory, whatever Nebo's data folder; one
    /// that waits says so, and Stop ends the wait with nothing landed.
    #[test]
    fn a_landing_waits_its_turn_and_stop_ends_the_wait() {
        with_home(|_| rt().block_on(async {
            let r = repo().await;
            let held = project_lock(r.path(), &Landing::default()).await.unwrap();
            assert!(r.path().join(".git/nebo-land.lock").is_file());
            let wt = create_worktree(r.path(), "sa-wait").await.unwrap();
            std::fs::write(wt.path.join("b.txt"), "b\n").unwrap();
            let told = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let saw = told.clone();
            let landing = Landing {
                waiting: Some(std::sync::Arc::new(move || {
                    saw.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                })),
                ..Default::default()
            };
            let stop = landing.stop.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                stop.cancel();
            });
            let mut iso = Isolation::Git(wt);
            let (_, out) = land_one(&mut iso, "Add b", None, &landing).await;
            assert!(matches!(&out, MergeOutcome::Failed { error, .. } if error.contains("waiting for another landing")), "{out:?}");
            assert_eq!(told.load(std::sync::atomic::Ordering::SeqCst), 1, "said once");
            assert!(!r.path().join("b.txt").exists());
            drop(held);
            assert_eq!(landed_files(&land_one(&mut iso, "Add b", None, &Landing::default()).await.1), vec!["b.txt".to_string()]);
        }))
    }

    /// Review round 5: the git Nebo needs is checked by version, and the
    /// owner reads what to do: install when it is missing, update when old.
    #[test]
    fn this_computers_git_is_new_enough() {
        rt().block_on(async { assert_eq!(git_problem(Path::new("git")).await, None) });
        assert!(GitProblem::Missing.words().0.starts_with("Install git"));
        assert!(GitProblem::TooOld("2.20.1".into()).words().1.contains("git 2.20.1"));
        assert!(GitProblem::TooOld("2.20.1".into()).words().0.starts_with("Update git"));
    }



    /// 50,000 paths go to git on its input while it writes as many back,
    /// and a big file is hashed from its bytes, with no deadlock.
    #[test]
    fn fifty_thousand_paths_and_a_big_file_cross_to_git_without_a_deadlock() {
        with_home(|_| rt().block_on(async {
            let f = folder();
            std::fs::write(f.path().join(".gitignore"), "*.log\n").unwrap();
            let paths: Vec<String> = (0..50_000).map(|i| format!("deep/dir/number/{i:06}-a-fairly-long-name-to-fill-pipes.log")).collect();
            let g = tempfile::tempdir().unwrap();
            git(g.path(), &["init", "-q", "--bare", "s.git"]).await.unwrap();
            let got = ignored_paths(f.path(), Some(&g.path().join("s.git")), &paths).await;
            assert_eq!(got.unwrap().len(), 50_000);
            let r = repo().await;
            std::fs::write(r.path().join(".gitignore"), "*.log\n").unwrap();
            let got = ignored_paths(r.path(), None, &paths).await;
            assert_eq!(got.unwrap().len(), 50_000);
            // 8 MB through hash-object --stdin.
            let big = vec![b'x'; 8 << 20];
            std::fs::write(r.path().join("big.bin"), &big).unwrap();
            let id = content_id(r.path(), "big.bin").await;
            assert!(matches!(id, Ok(Some(_))));
        }))
    }


    /// Review round 7: the owner hears about git once, when code work is
    /// attempted, in words that follow what is wrong (missing, then too
    /// old); the notice is put away once git works, and raised again when
    /// it breaks after that. The git checked is passed in: nothing of the
    /// process (its PATH) changes under other tests.
    #[cfg(unix)]
    #[test]
    fn the_git_notice_follows_what_is_wrong_and_comes_back_when_git_breaks_again() {
        with_home(|home| rt().block_on(async {
            let store = db::Store::new(&home.join("t.db").to_string_lossy()).unwrap();
            let user = store.ensure_local_user_id().unwrap();
            let notice = || store.get_notification("git-needed", &user).unwrap();
            let old = home.join("old-git");
            std::fs::write(&old, "#!/bin/sh\necho 'git version 2.20.1'\n").unwrap();
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&old, std::fs::Permissions::from_mode(0o755)).unwrap();
            let missing = home.join("no-such-git");

            assert!(!git_notice(&store, &missing).await);
            assert!(notice().unwrap().title.starts_with("Install git"));
            assert!(!git_notice(&store, &old).await);
            let n = notice().unwrap();
            assert!(n.title.starts_with("Update git") && n.body.as_deref().is_some_and(|b| b.contains("2.20.1")), "{n:?}");
            assert!(git_notice(&store, Path::new("git")).await, "git works");
            assert!(notice().unwrap().archived_at.is_some(), "put away once git works");
            assert!(!git_notice(&store, &old).await);
            let n = notice().unwrap();
            assert!(n.archived_at.is_none() && n.read_at.is_none(), "raised again when git breaks: {n:?}");
        }))
    }


    /// Review round 6: names git would quote or rewrite (a quote, a
    /// backslash, a tab, a newline, decomposed and composed accents, a
    /// leading dash, spaces, glob characters) land whole, held to what was
    /// reviewed.
    #[test]
    fn names_git_would_quote_or_rewrite_land() {
        let names = ["a\"b.txt", "back\\slash.txt", "tab\there.txt", "new\nline.txt", "caf\u{e9}.txt", "cafe\u{301}x.txt", "-dash.txt", "with space.txt", "routes/[id]/page.txt"];
        with_home(|_| rt().block_on(async {
            let r = repo().await;
            let wt = create_worktree(r.path(), "sa-r6-names-g").await.unwrap();
            for n in names {
                let p = wt.path.join(n);
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(p, format!("{n}\n")).unwrap();
            }
            let paths: Vec<String> = z_paths(&wt.path, &["ls-files", "-z", "--others", "--exclude-standard"]).await;
            let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
            let landing = reviewed(&wt.path, &refs).await;
            let mut isos = [Isolation::Git(wt)];
            let out = merge_all(&mut isos, "m", None, &landing).await;
            assert!(matches!(&out[0].1, MergeOutcome::Merged { .. }), "{:?}", out[0].1);
            for n in names {
                assert!(r.path().join(n).exists(), "{n:?}");
            }
        }))
    }

    /// Review round 3 B3: only what was reviewed lands. A file the review
    /// never saw (too big to snapshot, added after it) is refused.
    #[test]
    fn only_the_reviewed_paths_land() {
        with_home(|_| rt().block_on(async {
            let r = repo().await;
            let wt = create_worktree(r.path(), "sa-only").await.unwrap();
            std::fs::write(wt.path.join("b.txt"), "reviewed\n").unwrap();
            std::fs::write(wt.path.join("video.mov"), "never reviewed").unwrap();
            let landing = reviewed(&wt.path, &["b.txt"]).await;
            let mut isos = [Isolation::Git(wt)];
            let out = merge_all(&mut isos, "Add b", None, &landing).await;
            assert!(matches!(&out[0].1, MergeOutcome::Unreviewed { files, .. } if files == &vec!["video.mov".to_string()]), "{:?}", out[0].1);
            assert!(!r.path().join("b.txt").exists() && !r.path().join("video.mov").exists());
        }))
    }

    /// Review round 3 M10: discarding what is already gone succeeds.
    #[test]
    fn discarding_twice_succeeds() {
        with_home(|_| rt().block_on(async {
            let r = repo().await;
            let iso = Isolation::Git(create_worktree(r.path(), "sa-dd").await.unwrap());
            discard(&iso).await.unwrap();
            discard(&iso).await.unwrap();
        }))
    }

    /// A helper's copy is a git worktree in its repository's own Nebo
    /// folder, which git is told to ignore, so the owner's `git status`
    /// never shows it; a folder that is no repository gets no copy.
    #[test]
    fn a_helpers_copy_lives_in_the_repositorys_nebo_folder() {
        with_home(|_| rt().block_on(async {
            let r = repo().await;
            let iso = create(r.path(), "h-place").await.unwrap();
            let root = std::fs::canonicalize(r.path()).unwrap();
            assert_eq!(std::fs::canonicalize(iso.path()).unwrap(), root.join(".nebo/worktrees/h-place"));
            let exclude = std::fs::read_to_string(r.path().join(".git/info/exclude")).unwrap();
            for own in [".nebo/worktrees/", ".nebo/verify/", ".nebo/build/"] {
                assert!(exclude.lines().any(|l| l == own), "{exclude}");
            }
            assert!(!exclude.lines().any(|l| l == ".nebo/"), "the owner's own Nebo files are never hidden: {exclude}");
            assert_eq!(git(r.path(), &["status", "--porcelain"]).await.unwrap(), "", "the owner's status shows nothing of it");
            // A copy of that copy is made beside it, never inside it.
            let again = create(iso.path(), "h-inner").await.unwrap();
            assert_eq!(std::fs::canonicalize(again.path()).unwrap(), root.join(".nebo/worktrees/h-inner"));
            assert!(create(folder().path(), "h-none").await.is_err(), "no repository: no copy");
        }))
    }

    /// The stale sweep removes only Nebo's own copies, old enough, holding
    /// nothing that hasn't landed; a copy with uncommitted work or a commit
    /// only it holds is kept, and so is a folder Nebo didn't name.
    #[test]
    fn the_stale_sweep_removes_only_nebos_copies_with_nothing_unlanded() {
        with_home(|_| rt().block_on(async {
            let r = repo().await;
            let clean = create(r.path(), "h-clean").await.unwrap();
            let dirty = create(r.path(), "h-dirty").await.unwrap();
            std::fs::write(dirty.path().join("work.txt"), "unsaved\n").unwrap();
            let ahead = create(r.path(), "h-ahead").await.unwrap();
            std::fs::write(ahead.path().join("done.txt"), "done\n").unwrap();
            git(ahead.path(), &["add", "-A"]).await.unwrap();
            git(ahead.path(), &["commit", "-q", "-m", "done"]).await.unwrap();
            let theirs = r.path().join(".nebo/worktrees/notes");
            std::fs::create_dir_all(&theirs).unwrap();
            let removed = cleanup_stale(&[r.path().to_path_buf()], 0).await;
            assert_eq!(removed.len(), 1, "{removed:?}");
            assert!(!clean.path().exists());
            assert!(dirty.path().join("work.txt").exists() && ahead.path().join("done.txt").exists() && theirs.exists());
        }))
    }

    /// The copies the verify loop made in Nebo's data folder before it
    /// worked in place: a worktree holding work is moved beside its project
    /// by git, a folder's copy that differs from its project stays where it
    /// is, and each one's row says where; one holding nothing is removed.
    #[test]
    fn old_copies_with_work_move_beside_their_project_and_empty_ones_go() {
        with_home(|home| rt().block_on(async {
            let store = db::Store::new(&home.join("t.db").to_string_lossy()).unwrap();
            let old = config::data_dir().unwrap().join("worktrees");
            let r = repo().await;
            let mut rows = Vec::new();
            for id in ["v-oldwork", "v-oldclean"] {
                let path = old.join("site").join(id);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                git(r.path(), &["worktree", "add", "-q", "-b", &format!("nebo/{id}"), &path.to_string_lossy()]).await.unwrap();
                rows.push((id, path));
            }
            std::fs::write(rows[0].1.join("feature.txt"), "the owner's real work\n").unwrap();
            let f = folder();
            let copy = old.join("letters").join("v-oldcopy");
            std::fs::create_dir_all(copy.join("src")).unwrap();
            std::fs::write(copy.join("src/a.txt"), "a0\n").unwrap();
            std::fs::write(copy.join("b.txt"), "b1, changed\n").unwrap();
            let unchanged = old.join("letters").join("v-oldsame");
            std::fs::create_dir_all(unchanged.join("src")).unwrap();
            std::fs::write(unchanged.join("src/a.txt"), "a0\n").unwrap();
            std::fs::write(unchanged.join("b.txt"), "b0\n").unwrap();
            let fp = |t: &str| tools::checkpoint::fingerprint(t.as_bytes());
            let snapshot = serde_json::json!({"src/a.txt": fp("a0\n"), "b.txt": fp("b0\n")});
            for (id, root, path, isolation) in [
                (rows[0].0, r.path().to_path_buf(), rows[0].1.clone(), "worktree"),
                (rows[1].0, r.path().to_path_buf(), rows[1].1.clone(), "worktree"),
                ("v-oldcopy", f.path().to_path_buf(), copy.clone(), "copy"),
                ("v-oldsame", f.path().to_path_buf(), unchanged.clone(), "copy"),
            ] {
                let state = if isolation == "copy" {
                    serde_json::json!({"Copy": {"task_id": id, "root": root, "path": path, "snapshot": snapshot, "skipped": [], "pending": []}})
                } else {
                    serde_json::json!({"Git": {"path": path}})
                };
                store
                    .create_verification(
                        &db::NewVerification {
                            id,
                            session_key: "agent:dev:web",
                            project_root: &root.to_string_lossy(),
                            work_dir: &path.to_string_lossy(),
                            isolation,
                            work_kind: "isolated",
                            isolation_state: Some(&state),
                            ..Default::default()
                        },
                        1,
                    )
                    .unwrap();
            }
            let done = adopt_old_copies(&store).await;
            let said: Vec<String> = ["v-oldwork", "v-oldclean", "v-oldcopy", "v-oldsame"].iter().map(|id| { let v = store.get_verification(id).unwrap().unwrap(); format!("{id} {} {}", v.work_dir, v.reason) }).collect();
            assert_eq!(done, (2, 2), "{said:#?}");
            let moved = store.get_verification("v-oldwork").unwrap().unwrap();
            let to = std::fs::canonicalize(r.path()).unwrap().join(".nebo/worktrees/v-oldwork");
            assert_eq!(std::fs::canonicalize(&moved.work_dir).unwrap(), to);
            assert_eq!(std::fs::read_to_string(to.join("feature.txt")).unwrap(), "the owner's real work\n");
            assert!(moved.reason.contains("kept_in_copy"), "{}", moved.reason);
            let kept = store.get_verification("v-oldcopy").unwrap().unwrap();
            assert_eq!(PathBuf::from(&kept.work_dir), copy, "a folder's copy is never moved");
            assert_eq!(std::fs::read_to_string(copy.join("b.txt")).unwrap(), "b1, changed\n");
            assert!(kept.reason.contains("kept_in_copy") && kept.reason.contains("v-oldcopy"), "{}", kept.reason);
            assert!(!f.path().join(".nebo/copies").exists(), "no new home for it");
            assert!(!rows[1].1.exists() && !unchanged.exists() && !rows[0].1.exists(), "the empty ones went, and the worktree with work moved");
            // Run again: nothing is left to do.
            assert_eq!(adopt_old_copies(&store).await, (0, 0));
        }))
    }

    /// Live 2026-10-24: a check of an old copy was paused on a reviewer's
    /// question when Nebo updated; its copy was moved beside the project,
    /// but the row stayed paused with the question. The loop never takes an
    /// old copy's check up again, so the phone said "Waiting on you" with
    /// nothing to answer. A live one ends as stopped, its question
    /// withdrawn and its copy's place kept, at the next start too.
    #[test]
    fn an_old_copys_check_left_waiting_on_the_owner_ends_with_its_question() {
        with_home(|home| rt().block_on(async {
            let store = db::Store::new(&home.join("t.db").to_string_lossy()).unwrap();
            let kept = home.join("site/.nebo/worktrees/v-oldask");
            std::fs::create_dir_all(&kept).unwrap();
            let reason = types::Why::new("kept_in_copy").with("path", kept.to_string_lossy().into_owned()).text();
            store
                .create_verification(
                    &db::NewVerification {
                        id: "v-oldask",
                        session_key: "agent:dev:web",
                        project_root: &home.join("site").to_string_lossy(),
                        work_dir: &kept.to_string_lossy(),
                        isolation: "worktree",
                        work_kind: "isolated",
                        ..Default::default()
                    },
                    1,
                )
                .unwrap();
            let ask = serde_json::json!({"question": "Match by code or city?", "options": ["Code", "City"], "purpose": {"kind": "decision", "title": "Match"}});
            store.set_verification_pending_ask("v-oldask", Some(&ask)).unwrap();
            store.set_verification_state("v-oldask", &db::VerificationState { state: "paused", reason: &reason, ..Default::default() }, 2).unwrap();
            assert_eq!(adopt_old_copies(&store).await, (0, 0), "its copy was moved at an earlier start");
            let row = store.get_verification("v-oldask").unwrap().unwrap();
            assert_eq!((row.state.as_str(), row.pending_ask.clone()), ("stopped", None), "{row:?}");
            assert_eq!(row.reason, reason, "its row still says where its copy is");
            assert!(kept.is_dir(), "the work is kept");
        }))
    }

    /// Review dl1055 A: an old copy of a folder that was no repository
    /// then, whose project is one now, is never moved into the project
    /// (where the owner's `git clean -fdx` would take it): it stays in
    /// Nebo's data folder, its row says where, and nothing the owner runs in
    /// the project touches it.
    #[test]
    fn an_old_folder_copy_is_never_moved_where_git_clean_takes_it() {
        with_home(|home| rt().block_on(async {
            let store = db::Store::new(&home.join("t.db").to_string_lossy()).unwrap();
            let old = config::data_dir().unwrap().join("worktrees");
            let f = folder();
            let copy = old.join("letters").join("v-dlcopy");
            std::fs::create_dir_all(copy.join("src")).unwrap();
            std::fs::write(copy.join("src/a.txt"), "a0\n").unwrap();
            std::fs::write(copy.join("b.txt"), "b1, the unlanded work\n").unwrap();
            for args in [vec!["init", "-q", "-b", "main"], vec!["config", "user.email", "t@t"], vec!["config", "user.name", "t"]] {
                git(f.path(), &args).await.unwrap();
            }
            git(f.path(), &["add", "-A"]).await.unwrap();
            git(f.path(), &["commit", "-q", "-m", "base"]).await.unwrap();
            let state = serde_json::json!({"Copy": {"task_id": "v-dlcopy", "root": f.path(), "path": copy, "snapshot": {}, "skipped": [], "pending": []}});
            store.create_verification(&db::NewVerification { id: "v-dlcopy", session_key: "agent:dev:web", project_root: &f.path().to_string_lossy(), work_dir: &copy.to_string_lossy(), isolation: "copy", work_kind: "isolated", isolation_state: Some(&state), ..Default::default() }, 1).unwrap();
            assert_eq!(adopt_old_copies(&store).await, (1, 0));
            assert_eq!(std::fs::read_to_string(copy.join("b.txt")).unwrap(), "b1, the unlanded work\n", "kept where it was");
            let row = store.get_verification("v-dlcopy").unwrap().unwrap();
            assert!(row.reason.contains("kept_in_copy") && row.reason.contains(&copy.to_string_lossy().into_owned()), "{}", row.reason);
            git(f.path(), &["clean", "-fdx"]).await.unwrap();
            assert!(copy.join("b.txt").exists(), "the owner's clean never reaches it");
            assert_eq!(adopt_old_copies(&store).await, (0, 0), "settled once");
        }))
    }

    /// Review dl1055 B: a folder's copy is compared with its project link
    /// for link, never followed or skipped: one that differs only by a
    /// link is kept with the link as it was; one identical to its project,
    /// links included, is removed.
    #[test]
    fn an_old_folder_copy_is_compared_link_for_link() {
        with_home(|home| rt().block_on(async {
            let store = db::Store::new(&home.join("t.db").to_string_lossy()).unwrap();
            let old = config::data_dir().unwrap().join("worktrees");
            let f = folder();
            std::os::unix::fs::symlink("b.txt", f.path().join("same")).unwrap();
            let mut rows = Vec::new();
            for (id, extra) in [("v-dllink", Some("current")), ("v-dlsame", None)] {
                let copy = old.join("letters").join(id);
                std::fs::create_dir_all(copy.join("src")).unwrap();
                std::fs::write(copy.join("src/a.txt"), "a0\n").unwrap();
                std::fs::write(copy.join("b.txt"), "b0\n").unwrap();
                std::os::unix::fs::symlink("b.txt", copy.join("same")).unwrap();
                if let Some(link) = extra {
                    std::os::unix::fs::symlink("b.txt", copy.join(link)).unwrap();
                }
                let state = serde_json::json!({"Copy": {"task_id": id, "root": f.path(), "path": copy, "snapshot": {}, "skipped": [], "pending": []}});
                store.create_verification(&db::NewVerification { id, session_key: "agent:dev:web", project_root: &f.path().to_string_lossy(), work_dir: &copy.to_string_lossy(), isolation: "copy", work_kind: "isolated", isolation_state: Some(&state), ..Default::default() }, 1).unwrap();
                rows.push(copy);
            }
            assert_eq!(adopt_old_copies(&store).await, (1, 1));
            assert_eq!(std::fs::read_link(rows[0].join("current")).unwrap(), PathBuf::from("b.txt"), "the link is kept as it was");
            assert!(!rows[1].exists(), "identical, links included: removed");
        }))
    }

    /// Review dl1055 D: `.nebo/hooks.yaml` is the owner's own, committed
    /// project file (`shell_hooks`). Only Nebo's own folders in `.nebo` are
    /// hidden from git: his file shows and is staged, and a change's
    /// snapshot holds it.
    #[test]
    fn the_owners_own_nebo_files_are_never_hidden_from_git() {
        with_home(|_| rt().block_on(async {
            let r = repo().await;
            nebo_dir(r.path()).unwrap();
            std::fs::create_dir_all(r.path().join(".nebo/verify/v-1")).unwrap();
            std::fs::write(r.path().join(".nebo/verify/v-1/change.patch"), "+x\n").unwrap();
            std::fs::write(r.path().join(".nebo/hooks.yaml"), "post_tool:\n  - name: fmt\n    command: cargo fmt\n").unwrap();
            git(r.path(), &["add", "-A"]).await.unwrap();
            let staged = git(r.path(), &["diff", "--cached", "--name-only"]).await.unwrap();
            assert_eq!(staged, ".nebo/hooks.yaml", "his file is staged, Nebo's are not");
            let shadows = tempfile::tempdir().unwrap();
            let shadow = crate::harness::verify::shadow::Shadow::open(&shadows.path().join("s.git"), r.path()).await.unwrap();
            let base = shadow.snapshot().await.unwrap();
            std::fs::write(r.path().join(".nebo/hooks.yaml"), "post_tool: []\n").unwrap();
            std::fs::write(r.path().join(".nebo/verify/v-1/change.patch"), "+y\n").unwrap();
            let change = shadow.diff(&base, &shadow.snapshot().await.unwrap()).await.unwrap();
            assert_eq!(change.paths(), [".nebo/hooks.yaml"], "the snapshot holds his file and none of Nebo's");
        }))
    }

    fn dl_row(store: &db::Store, id: &str, root: &Path, copy: &Path) {
        let state = serde_json::json!({"Copy": {"task_id": id, "root": root, "path": copy, "snapshot": {}, "skipped": [], "pending": []}});
        store.create_verification(&db::NewVerification { id, session_key: "agent:dev:web", project_root: &root.to_string_lossy(), work_dir: &copy.to_string_lossy(), isolation: "copy", work_kind: "isolated", isolation_state: Some(&state), ..Default::default() }, 1).unwrap();
    }

    /// Review dl1055b A: the old folder copy is kept where it is, and the
    /// owner's `git clean -ffdx` in the (now git) project never reaches it,
    /// at this start or the next.
    #[test]
    fn an_old_folder_copy_survives_any_git_clean_in_its_project() {
        with_home(|home| rt().block_on(async {
            let store = db::Store::new(&home.join("t.db").to_string_lossy()).unwrap();
            let old = config::data_dir().unwrap().join("worktrees");
            let f = folder();
            let copy = old.join("letters").join("v-dlcopy");
            std::fs::create_dir_all(copy.join("src")).unwrap();
            std::fs::write(copy.join("src/a.txt"), "a0\n").unwrap();
            std::fs::write(copy.join("b.txt"), "b1, the unlanded work\n").unwrap();
            for args in [vec!["init", "-q", "-b", "main"], vec!["config", "user.email", "t@t"], vec!["config", "user.name", "t"]] {
                git(f.path(), &args).await.unwrap();
            }
            git(f.path(), &["add", "-A"]).await.unwrap();
            git(f.path(), &["commit", "-q", "-m", "base"]).await.unwrap();
            dl_row(&store, "v-dlcopy", f.path(), &copy);
            adopt_old_copies(&store).await;
            git(f.path(), &["clean", "-ffdx"]).await.unwrap();
            assert_eq!(std::fs::read_to_string(copy.join("b.txt")).unwrap(), "b1, the unlanded work\n");
            adopt_old_copies(&store).await;
            assert!(copy.join("b.txt").exists(), "kept at a second boot");
        }))
    }

    /// Review dl1055b B: a copy that differs from its project only in a
    /// link's target, or only in a file's executable bit, holds work: it is
    /// never removed as identical.
    #[test]
    fn a_link_target_or_an_executable_bit_is_work_to_keep() {
        use std::os::unix::fs::PermissionsExt;
        with_home(|home| rt().block_on(async {
            let store = db::Store::new(&home.join("t.db").to_string_lossy()).unwrap();
            let old = config::data_dir().unwrap().join("worktrees");
            let f = folder();
            std::fs::write(f.path().join("run.sh"), "echo hi\n").unwrap();
            std::os::unix::fs::symlink("b.txt", f.path().join("current")).unwrap();
            let make = |id: &str| {
                let copy = old.join("letters").join(id);
                std::fs::create_dir_all(copy.join("src")).unwrap();
                std::fs::write(copy.join("src/a.txt"), "a0\n").unwrap();
                std::fs::write(copy.join("b.txt"), "b0\n").unwrap();
                std::fs::write(copy.join("run.sh"), "echo hi\n").unwrap();
                copy
            };
            let link = make("v-dltarget");
            std::os::unix::fs::symlink("src/a.txt", link.join("current")).unwrap();
            let mode = make("v-dlmode");
            std::os::unix::fs::symlink("b.txt", mode.join("current")).unwrap();
            std::fs::set_permissions(mode.join("run.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
            dl_row(&store, "v-dltarget", f.path(), &link);
            dl_row(&store, "v-dlmode", f.path(), &mode);
            assert_eq!(adopt_old_copies(&store).await, (2, 0));
            assert!(link.exists(), "a copy whose only change is a link's target is kept");
            assert!(mode.exists(), "a copy whose only change is run.sh made executable is kept");
        }))
    }

    /// Review dl1055b D: the owner's `.nebo/hooks.yaml` is seen by git, and
    /// none of Nebo's own folders beside it are.
    #[test]
    fn only_nebos_own_folders_in_nebo_are_hidden_from_git() {
        with_home(|_| rt().block_on(async {
            let r = repo().await;
            nebo_dir(r.path()).unwrap();
            std::fs::write(r.path().join(".nebo/hooks.yaml"), "post_tool: []\n").unwrap();
            for own in NEBO_OWN {
                std::fs::create_dir_all(r.path().join(".nebo").join(own)).unwrap();
                std::fs::write(r.path().join(".nebo").join(own).join("x"), "nebo's\n").unwrap();
            }
            git(r.path(), &["add", "-A"]).await.unwrap();
            let staged = git(r.path(), &["diff", "--cached", "--name-only"]).await.unwrap();
            assert_eq!(staged, ".nebo/hooks.yaml", "only the owner's file is staged");
        }))
    }
}
