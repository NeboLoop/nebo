//! Version history for the owner's own apps: the owner can always get an
//! app back the way it was.
//!
//! The app folder (`ui/`, `src/`, its build files) is a git repository,
//! made on the first change to it. Snapshots are automatic, never per
//! keystroke:
//! - before the first change of each turn, when something changed since
//!   the last snapshot ("Before: <what was asked>");
//! - when a turn that changed the folder ends, with the employee's own
//!   description of the change.
//!
//! `app_status(history: true)` lists the versions; `app_reload(restore: ..)`
//! puts the files back to one, as a new version, so a restore is undone by
//! restoring the version before it. The package files (AGENT.md,
//! agent.json, manifest.json) are the employee's settings and stay as they
//! are on a restore.
//!
//! The history lives inside the app folder (`.git`), so it moves with a
//! rename and goes to the trash with a delete (`napp::trash`). A bot
//! without git keeps copies in `.nebo-history/` instead. Both are dot
//! folders, which publishing never bundles.
//!
//! 2026-10-02: an app employee rewrote a working game to fix one loading
//! issue, broke it, and had no way back to the version that worked.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use chrono::{DateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::origin::ToolContext;
use crate::registry::DynTool;

/// Where a bot without git keeps an app's versions.
pub const HISTORY_DIR: &str = ".nebo-history";
/// Copies kept per app on a bot without git; the oldest go first.
pub const COPIES_KEPT: usize = 50;
/// Folders never in the history, at any depth: installed packages, build
/// caches and the history itself.
const SKIPPED_DIRS: &[&str] = &[
    "node_modules",
    "dist",
    ".cache",
    ".vite",
    ".parcel-cache",
    ".turbo",
    ".next",
    "coverage",
    ".git",
    HISTORY_DIR,
];
/// The `.gitignore` a new history starts with.
const GITIGNORE: &str = "# Kept out of the app's history\nnode_modules/\ndist/\n.cache/\n.vite/\n.parcel-cache/\n.turbo/\n.next/\ncoverage/\n.nebo-history/\n.DS_Store\n*.log\n";
/// The employee's settings: kept as they are on a restore.
const PACKAGE_FILES: &[&str] = &["AGENT.md", "agent.json", "manifest.json"];
/// A version's one-line message, at most.
const SUBJECT_CHARS: usize = 72;

/// One saved version of an app.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Version {
    /// What `restore` takes: a commit's short hash, or a copy's number.
    pub id: String,
    pub at: DateTime<Utc>,
    pub message: String,
    /// The files this version changed, relative to the app folder.
    pub files: Vec<String>,
}

/// What a restore did.
#[derive(Debug, Clone)]
pub struct Restored {
    /// The version the files are back to.
    pub to: Version,
    /// The version the restore saved; `None` when the files already
    /// matched.
    pub saved: Option<Version>,
    /// The version just before the restore: restoring it undoes this one.
    pub undo: Option<Version>,
}

// ── Which folder ───────────────────────────────────────────────────

/// The folder an own app's history covers: its package folder (with
/// `ui/` and `src/`), else the folder above its served `ui/`. `None` for
/// an app installed from the marketplace, and for anything that isn't an
/// app.
pub fn app_folder(row: &db::models::Agent) -> Option<PathBuf> {
    if !crate::app_dev::is_own_app(row) {
        return None;
    }
    let served = crate::app_dev::served_dir(row).map(PathBuf::from);
    let package = row
        .napp_path
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_dir());
    let dir = match (package, served) {
        (Some(pkg), served)
            if pkg.join("ui").is_dir()
                || served.as_deref().is_some_and(|s| s.starts_with(&pkg)) =>
        {
            pkg
        }
        (_, Some(served)) if served.file_name().is_some_and(|n| n == "ui") => {
            served.parent()?.to_path_buf()
        }
        (_, Some(served)) => served,
        (Some(pkg), None) => pkg,
        (None, None) => return None,
    };
    dir.is_dir().then_some(dir)
}

// ── Snapshot, list, restore ────────────────────────────────────────

/// Whether git runs on this bot.
fn git_on_bot() -> bool {
    static FOUND: OnceLock<bool> = OnceLock::new();
    *FOUND.get_or_init(|| which::which("git").is_ok())
}

/// History operations run one at a time: two git commands on one index
/// collide, and they take milliseconds.
fn one_at_a_time() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Keeper {
    Git,
    Copies,
}

/// How a folder's history is kept: the one it already has, else git when
/// the bot has it.
fn keeper(dir: &Path, git: bool) -> Keeper {
    if dir.join(".git").exists() {
        Keeper::Git
    } else if dir.join(HISTORY_DIR).is_dir() || !git {
        Keeper::Copies
    } else {
        Keeper::Git
    }
}

/// Save the folder as a new version when anything changed since the last
/// one (the first call starts the history). `None` when nothing changed.
pub fn snapshot(dir: &Path, message: &str) -> Result<Option<Version>, String> {
    let _one = one_at_a_time();
    snapshot_with(dir, message, git_on_bot())
}

/// The newest `limit` versions, newest first.
pub fn list(dir: &Path, limit: usize) -> Result<Vec<Version>, String> {
    let _one = one_at_a_time();
    match keeper(dir, git_on_bot()) {
        Keeper::Git if dir.join(".git").exists() => git_list(dir, limit),
        Keeper::Git => Ok(Vec::new()),
        Keeper::Copies => copies_list(dir, limit),
    }
}

/// Put the files back the way they were at version `id`, saving the
/// result as a new version. What changed since the last version is saved
/// first, so nothing is lost and the restore can be undone.
pub fn restore(dir: &Path, id: &str) -> Result<Restored, String> {
    let _one = one_at_a_time();
    restore_with(dir, id, git_on_bot())
}

fn snapshot_with(dir: &Path, message: &str, git: bool) -> Result<Option<Version>, String> {
    if !dir.is_dir() {
        return Err(format!("{} is not a folder", dir.display()));
    }
    // git keeps the whole message (its first line is the subject); a
    // copy keeps the first line.
    match keeper(dir, git) {
        Keeper::Git => git_snapshot(dir, message.trim()),
        Keeper::Copies => copies_snapshot(dir, &one_line(message)),
    }
}

fn restore_with(dir: &Path, id: &str, git: bool) -> Result<Restored, String> {
    let id = id.trim().trim_start_matches('#');
    if id.is_empty() {
        return Err("Name the version to restore: an id from app_status(history: true).".into());
    }
    let keeper = keeper(dir, git);
    let to = match keeper {
        Keeper::Git => git_version(dir, id)?,
        Keeper::Copies => copies_version(dir, id)?,
    };
    let when = local_time(&to.at);
    snapshot_with(dir, &format!("Before restoring to {} ({when})", to.id), git)?;
    let undo = match keeper {
        Keeper::Git => git_list(dir, 1)?.into_iter().next(),
        Keeper::Copies => copies_list(dir, 1)?.into_iter().next(),
    };
    let message = format!("Restored to {} ({when}): {}", to.id, to.message);
    let saved = match keeper {
        Keeper::Git => {
            let mut args = vec![
                "restore",
                "--source",
                &to.id,
                "--staged",
                "--worktree",
                "--",
                ".",
            ];
            let excluded: Vec<String> = PACKAGE_FILES
                .iter()
                .map(|f| format!(":(exclude){f}"))
                .collect();
            args.extend(excluded.iter().map(String::as_str));
            git_run(dir, &args)?;
            git_snapshot(dir, &message)?
        }
        Keeper::Copies => {
            copies_put_back(dir, &to.id)?;
            copies_snapshot(dir, &one_line(&message))?
        }
    };
    Ok(Restored { to, saved, undo })
}

// ── git ────────────────────────────────────────────────────────────

/// One git command on the app folder's own repository, never a repository
/// above it: `GIT_DIR` and `GIT_WORK_TREE` name it. Identity, signing,
/// hooks and line endings are fixed here, whatever the bot's git config
/// says.
fn git_run(dir: &Path, args: &[&str]) -> Result<String, String> {
    let mut cmd: std::process::Command = command::new("git", command::Console::Hidden);
    cmd.current_dir(dir)
        .env("GIT_DIR", dir.join(".git"))
        .env("GIT_WORK_TREE", dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GIT_INDEX_FILE")
        .args([
            "-c",
            "user.name=Nebo",
            "-c",
            "user.email=history@nebo.local",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.autocrlf=false",
            "-c",
            "core.quotepath=false",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.hooksPath=.git/hooks",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .stdin(std::process::Stdio::null());
    let out = cmd
        .output()
        .map_err(|e| format!("git could not run: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        Err(format!(
            "git {}: {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// Make the folder a repository when it isn't one, with the `.gitignore`
/// that keeps installed packages and build caches out.
fn git_ensure(dir: &Path) -> Result<(), String> {
    if !dir.join(".git").exists() {
        git_run(dir, &["init", "-q"])?;
    }
    let ignore = dir.join(".gitignore");
    let current = std::fs::read_to_string(&ignore).unwrap_or_default();
    if !current
        .lines()
        .any(|l| l.trim().trim_matches('/') == "node_modules")
    {
        let joined = if current.is_empty() || current.ends_with('\n') {
            format!("{current}{GITIGNORE}")
        } else {
            format!("{current}\n{GITIGNORE}")
        };
        std::fs::write(&ignore, joined).map_err(|e| format!("writing .gitignore: {e}"))?;
    }
    Ok(())
}

fn git_snapshot(dir: &Path, message: &str) -> Result<Option<Version>, String> {
    git_ensure(dir)?;
    git_run(dir, &["add", "-A"])?;
    if git_run(dir, &["diff", "--cached", "--name-only"])?
        .trim()
        .is_empty()
    {
        return Ok(None);
    }
    git_run(dir, &["commit", "-q", "--no-verify", "-m", message])?;
    Ok(git_list(dir, 1)?.into_iter().next())
}

const RECORD: char = '\u{1e}';
const FIELD: char = '\u{1f}';

fn git_log(dir: &Path, extra: &[&str]) -> Result<Vec<Version>, String> {
    let mut args = vec!["log", "--format=%x1e%h%x1f%ct%x1f%s", "--name-only"];
    args.extend_from_slice(extra);
    let out = match git_run(dir, &args) {
        Ok(out) => out,
        // A repository with no version yet.
        Err(e) if e.contains("does not have any commits") || e.contains("bad default revision") => {
            return Ok(Vec::new());
        }
        Err(e) => return Err(e),
    };
    Ok(out
        .split(RECORD)
        .filter(|r| !r.trim().is_empty())
        .filter_map(|record| {
            let mut lines = record.lines();
            let mut head = lines.next()?.split(FIELD);
            let id = head.next()?.to_string();
            let at = Utc.timestamp_opt(head.next()?.parse().ok()?, 0).single()?;
            let message = head.next().unwrap_or_default().to_string();
            let files = lines
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .collect();
            Some(Version {
                id,
                at,
                message,
                files,
            })
        })
        .collect())
}

fn git_list(dir: &Path, limit: usize) -> Result<Vec<Version>, String> {
    git_log(dir, &["-n", &limit.max(1).to_string()])
}

fn git_version(dir: &Path, id: &str) -> Result<Version, String> {
    let hex = id.len() >= 4 && id.chars().all(|c| c.is_ascii_hexdigit());
    if !hex
        || git_run(
            dir,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{id}^{{commit}}"),
            ],
        )
        .is_err()
    {
        return Err(format!(
            "There is no version '{id}' in this app's history. app_status(history: true) lists them."
        ));
    }
    git_log(dir, &["-n", "1", id])?
        .into_iter()
        .next()
        .ok_or_else(|| format!("Version '{id}' could not be read."))
}

// ── Copies (a bot without git) ─────────────────────────────────────

/// The files the history covers, by their path from the folder (always
/// with `/`): everything but the skipped folders, symlinks and, at the
/// top, the package files.
fn tree_files(dir: &Path) -> BTreeMap<String, PathBuf> {
    walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_entry(|e| {
            e.depth() == 0
                || !(e.file_type().is_dir()
                    && SKIPPED_DIRS.contains(&e.file_name().to_string_lossy().as_ref()))
        })
        .flatten()
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| {
            let rel = e
                .path()
                .strip_prefix(dir)
                .ok()?
                .to_string_lossy()
                .replace('\\', "/");
            (!PACKAGE_FILES.contains(&rel.as_str())).then(|| (rel, e.path().to_path_buf()))
        })
        .collect()
}

#[derive(Serialize, Deserialize)]
struct CopyRecord {
    at: DateTime<Utc>,
    message: String,
    files: Vec<String>,
}

/// The saved copies, oldest first, by number.
fn copies(dir: &Path) -> Vec<(u64, PathBuf)> {
    let mut found: Vec<(u64, PathBuf)> = std::fs::read_dir(dir.join(HISTORY_DIR))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| Some((e.file_name().to_str()?.parse::<u64>().ok()?, e.path())))
        .filter(|(_, p)| p.join("version.json").is_file())
        .collect();
    found.sort_by_key(|(n, _)| *n);
    found
}

fn copy_version(n: u64, slot: &Path) -> Option<Version> {
    let record: CopyRecord =
        serde_json::from_slice(&std::fs::read(slot.join("version.json")).ok()?).ok()?;
    Some(Version {
        id: n.to_string(),
        at: record.at,
        message: record.message,
        files: record.files,
    })
}

fn same_bytes(a: &Path, b: &Path) -> bool {
    let (Ok(ma), Ok(mb)) = (std::fs::metadata(a), std::fs::metadata(b)) else {
        return false;
    };
    ma.len() == mb.len() && std::fs::read(a).ok() == std::fs::read(b).ok()
}

fn copies_snapshot(dir: &Path, message: &str) -> Result<Option<Version>, String> {
    let tree = tree_files(dir);
    let all = copies(dir);
    let last = all.last().map(|(_, p)| p.join("files"));
    let before = last.as_deref().map(tree_files).unwrap_or_default();
    let mut changed: BTreeSet<String> = before
        .keys()
        .filter(|k| !tree.contains_key(*k))
        .cloned()
        .collect();
    for (rel, path) in &tree {
        if before.get(rel).is_none_or(|old| !same_bytes(old, path)) {
            changed.insert(rel.clone());
        }
    }
    if last.is_some() && changed.is_empty() {
        return Ok(None);
    }
    let n = all.last().map(|(n, _)| n + 1).unwrap_or(1);
    let slot = dir.join(HISTORY_DIR).join(n.to_string());
    let files = slot.join("files");
    let io = |e: std::io::Error| format!("saving a copy of the app: {e}");
    for (rel, path) in &tree {
        let to = files.join(rel);
        std::fs::create_dir_all(to.parent().unwrap_or(&files)).map_err(io)?;
        // An unchanged file shares the last copy's bytes (a hard link
        // between copies, never into the app's own files).
        let linked = !changed.contains(rel)
            && before
                .get(rel)
                .is_some_and(|old| std::fs::hard_link(old, &to).is_ok());
        if !linked {
            std::fs::copy(path, &to).map_err(io)?;
        }
    }
    std::fs::create_dir_all(&slot).map_err(io)?;
    let record = CopyRecord {
        at: Utc::now(),
        message: message.to_string(),
        files: changed.into_iter().collect(),
    };
    std::fs::write(
        slot.join("version.json"),
        serde_json::to_vec_pretty(&record).unwrap_or_default(),
    )
    .map_err(io)?;
    let all = copies(dir);
    for (_, old) in all.iter().take(all.len().saturating_sub(COPIES_KEPT)) {
        let _ = std::fs::remove_dir_all(old);
    }
    Ok(copy_version(n, &slot))
}

fn copies_list(dir: &Path, limit: usize) -> Result<Vec<Version>, String> {
    Ok(copies(dir)
        .iter()
        .rev()
        .take(limit.max(1))
        .filter_map(|(n, p)| copy_version(*n, p))
        .collect())
}

fn copies_version(dir: &Path, id: &str) -> Result<Version, String> {
    copies(dir)
        .iter()
        .find(|(n, _)| n.to_string() == id)
        .and_then(|(n, p)| copy_version(*n, p))
        .ok_or_else(|| format!("There is no version '{id}' in this app's history. app_status(history: true) lists them."))
}

/// Make the folder's files match copy `id`: changed files copied back,
/// files it did not have removed.
fn copies_put_back(dir: &Path, id: &str) -> Result<(), String> {
    let source = dir.join(HISTORY_DIR).join(id).join("files");
    let saved = tree_files(&source);
    let io = |e: std::io::Error| format!("restoring the app's files: {e}");
    for (rel, path) in tree_files(dir) {
        if !saved.contains_key(&rel) {
            std::fs::remove_file(&path).map_err(io)?;
        }
    }
    for (rel, from) in &saved {
        let to = dir.join(rel);
        if to.is_file() && same_bytes(from, &to) {
            continue;
        }
        std::fs::create_dir_all(to.parent().unwrap_or(dir)).map_err(io)?;
        // Never write through a link into the copies: a fresh file.
        let _ = std::fs::remove_file(&to);
        std::fs::copy(from, &to).map_err(io)?;
    }
    Ok(())
}

// ── Messages ───────────────────────────────────────────────────────

/// The first line of `text`, without markdown marks, cut at a sentence end
/// or [`SUBJECT_CHARS`].
pub fn one_line(text: &str) -> String {
    let line = text
        .lines()
        .map(|l| {
            l.trim()
                .trim_start_matches(['#', '>', '-', '*', ' '])
                .replace(['*', '`'], "")
        })
        .find(|l| !l.trim().is_empty())
        .unwrap_or_default();
    let line = line.trim();
    let line = match line.find(". ") {
        Some(end) if end >= 12 => &line[..end + 1],
        _ => line,
    };
    if line.chars().count() <= SUBJECT_CHARS {
        return line.to_string();
    }
    let cut: String = line.chars().take(SUBJECT_CHARS - 1).collect();
    format!("{}…", cut.trim_end())
}

/// A version's time the way the owner reads it.
pub fn local_time(at: &DateTime<Utc>) -> String {
    at.with_timezone(&chrono::Local)
        .format("%a %b %-d, %-I:%M %p")
        .to_string()
}

/// The history as the model and the owner read it.
pub fn describe(name: &str, versions: &[Version]) -> String {
    if versions.is_empty() {
        return format!(
            "{name} has no saved versions yet: the first change to its files starts its history."
        );
    }
    let mut out = format!(
        "{name}'s saved versions, newest first (restore one with app_reload(restore: \"<id>\")):\n"
    );
    for v in versions {
        let files = match v.files.len() {
            0 => String::new(),
            n if n <= 4 => format!(" [{}]", v.files.join(", ")),
            n => format!(" [{}, and {} more]", v.files[..3].join(", "), n - 3),
        };
        out.push_str(&format!(
            "- {} · {} · {}{files}\n",
            v.id,
            local_time(&v.at),
            v.message
        ));
    }
    out
}

/// The one line `app_status` adds about the history.
pub fn status_line(dir: &Path) -> String {
    match list(dir, 1) {
        Ok(v) if !v.is_empty() => format!(
            "History: saved automatically; latest version {} ({}, \"{}\"). app_status(history: true) lists them; \
             app_reload(restore: \"<id>\") goes back to one.",
            v[0].id,
            local_time(&v[0].at),
            v[0].message
        ),
        _ => "History: saved automatically from the next change to the app's files.".to_string(),
    }
}

// ── The turn ───────────────────────────────────────────────────────

#[derive(Default)]
struct Turn {
    /// What the turn was asked, in one line.
    request: String,
    /// App folders the turn changed, snapshotted before the first change.
    apps: BTreeSet<PathBuf>,
    /// Opened by a change outside a harness turn (no end will come).
    loose: bool,
}

fn turns() -> std::sync::MutexGuard<'static, HashMap<String, Turn>> {
    static TURNS: OnceLock<Mutex<HashMap<String, Turn>>> = OnceLock::new();
    TURNS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

/// Loose turns kept before the oldest are forgotten (their changes are
/// saved by the next turn's before-snapshot).
const LOOSE_KEPT: usize = 200;

/// A turn starts: what it was asked names its before-snapshots.
pub fn begin_turn(session_id: &str, request: &str) {
    turns().insert(
        session_id.to_string(),
        Turn {
            request: one_line(request),
            ..Turn::default()
        },
    );
}

/// The turn ends: every app folder it changed is saved, with the
/// employee's own description of the change (`description`, asked only
/// when something is saved), else what the turn was asked.
pub async fn end_turn(session_id: &str, description: impl FnOnce() -> String) {
    let Some(turn) = turns().remove(session_id) else {
        return;
    };
    if turn.apps.is_empty() {
        return;
    }
    let said = one_line(&description());
    let message = match (said.is_empty(), turn.request.is_empty()) {
        (false, true) => said,
        (false, false) => format!("{said}\n\nAsked: {}", turn.request),
        (true, false) => format!("Changed: {}", turn.request),
        (true, true) => "Changed the app".to_string(),
    };
    let dirs: Vec<PathBuf> = turn.apps.into_iter().filter(|d| d.is_dir()).collect();
    let _ = tokio::task::spawn_blocking(move || {
        for dir in dirs {
            if let Err(e) = snapshot(&dir, &message) {
                tracing::warn!(dir = %dir.display(), error = %e, "app history: the turn's version was not saved");
            }
        }
    })
    .await;
}

/// The ONE hook before a call that may change files runs
/// (`Registry::execute`): each app folder the call reaches is saved the
/// first time this turn reaches it, when it changed since its last
/// version (the first time ever, its history starts).
pub async fn before_change(
    store: &db::Store,
    ctx: &ToolContext,
    tool: &dyn DynTool,
    input: &Value,
) {
    let caller = types::keyparser::extract_agent_id(&ctx.session_key);
    let mut texts = Vec::new();
    strings_in(input, &mut texts);
    let writes_files = matches!(tool.capability(input), Some("file" | "shell"));
    let reached: Vec<PathBuf> = store
        .list_agents(1000, 0)
        .unwrap_or_default()
        .iter()
        .filter_map(|row| {
            let dir = app_folder(row)?;
            let its_own = writes_files && !caller.is_empty() && row.id == caller;
            let named = texts.iter().any(|t| {
                mentions(t, &dir) || t.trim() == row.id || t.trim().eq_ignore_ascii_case(&row.name)
            });
            let works_in = ctx
                .cwd
                .as_deref()
                .is_some_and(|cwd| Path::new(cwd).starts_with(&dir));
            (its_own || named || works_in).then_some(dir)
        })
        .collect();
    if reached.is_empty() {
        return;
    }
    let (fresh, request) = {
        let mut turns = turns();
        if !turns.contains_key(&ctx.session_id)
            && turns.values().filter(|t| t.loose).count() >= LOOSE_KEPT
        {
            turns.retain(|_, t| !t.loose);
        }
        let turn = turns.entry(ctx.session_id.clone()).or_insert_with(|| Turn {
            loose: true,
            ..Turn::default()
        });
        let fresh: Vec<PathBuf> = reached
            .into_iter()
            .filter(|d| turn.apps.insert(d.clone()))
            .collect();
        (fresh, turn.request.clone())
    };
    if fresh.is_empty() {
        return;
    }
    let message = if request.is_empty() {
        "Before changes".to_string()
    } else {
        format!("Before: {request}")
    };
    let _ = tokio::task::spawn_blocking(move || {
        for dir in fresh {
            if let Err(e) = snapshot(&dir, &message) {
                tracing::warn!(dir = %dir.display(), error = %e, "app history: the version before this change was not saved");
            }
        }
    })
    .await;
}

/// Every string in a call's input (a path, a command, a name), up to a
/// bound.
fn strings_in<'a>(value: &'a Value, out: &mut Vec<&'a str>) {
    if out.len() >= 256 {
        return;
    }
    match value {
        Value::String(s) => out.push(s),
        Value::Array(items) => items.iter().for_each(|v| strings_in(v, out)),
        Value::Object(map) => map.values().for_each(|v| strings_in(v, out)),
        _ => {}
    }
}

/// Whether `text` names `dir` or a path inside it (not a sibling that
/// merely starts the same: `Flip` is not `Flip-Flap`).
fn mentions(text: &str, dir: &Path) -> bool {
    let dir = dir.to_string_lossy();
    let dir = dir.trim_end_matches(['/', '\\']);
    if dir.is_empty() {
        return false;
    }
    text.match_indices(dir).any(|(at, _)| {
        text[at + dir.len()..].chars().next().is_none_or(|c| {
            matches!(
                c,
                '/' | '\\' | '"' | '\'' | ' ' | '\n' | '\t' | ';' | '&' | ')' | '|'
            )
        })
    })
}

#[cfg(test)]
mod tests;
