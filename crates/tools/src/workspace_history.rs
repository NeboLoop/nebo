//! Nothing in the owner's workspace is lost without history.
//!
//! The workspace is `<data_dir>/files`. A turn changes its files by every
//! means there is (the file tools, plugins, a shell's `>`, a script, an
//! app), and Nebo sees only some of them, so it looks at the workspace
//! instead of at the writes: the turn looks when it starts, before each of
//! its tool rounds and when it ends, however it ends (`agent` harness,
//! `turn::keep_workspace`). A look ([`look`]):
//! - keeps every file's current bytes in the content-addressed blob store
//!   (`files/work/blobs/<sha256>.<ext>`, the store work versions use), so the
//!   bytes are already safe before anything overwrites them;
//! - finds every file changed or gone since the last look (the index in the
//!   database says what each file was: size, mtime, hash) and keeps what it
//!   was as a history entry (`file_history`).
//!
//! Only a file whose size or mtime moved is hashed again, so a look at an
//! unchanged workspace is a walk of `stat`s. Looks run one at a time, and a
//! change is recorded by the first look that sees it, whichever turn's that
//! is: two turns running at once never record one change twice.
//!
//! [`restore`] puts an earlier content back, keeping the current content as
//! history first: a restore never loses anything either.
//!
//! 2026-10-06: an employee overwrote the owner's 36 KB, 9-sheet workbook with
//! a 7.9 KB partial through a command. The workbook had been made by a
//! script, so it was never a work version, and its content was simply gone.

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::UNIX_EPOCH;

use db::{FileHistoryEntry, NewFileHistory, WorkspaceIndexRow};

/// Where the blobs live, under the workspace. Never in the history itself.
pub const BLOBS_DIR: &str = "work/blobs";
/// The workspace's own folder of Nebo's (work versions and their blobs),
/// at its top: never in the history.
const WORK_DIR: &str = "work";
/// Folders never in the history, at any depth: installed packages, build
/// output and caches, rebuilt from their sources. Hidden entries (`.git`,
/// `.shared`, `.previews`, a write's `.part`) are left out too.
const SKIPPED_DIRS: &[&str] = &["node_modules", "target", "__pycache__", "venv"];
// ponytail: a file over 100 MB is left out of the history: a look keeps a
// copy of every file (a clone on APFS, a full copy elsewhere), and the
// workspace's documents are far smaller. Raise it if videos must be kept.
pub const MAX_FILE_BYTES: u64 = 100 * 1024 * 1024;

/// Looks and restores run one at a time: two at once would both see one
/// change.
fn one_at_a_time() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

// ── The blob store ─────────────────────────────────────────────────

/// A blob's path under the workspace: named by its content's hash, with the
/// extension that drives the served content type.
pub fn blob_rel(hash: &str, ext: &str) -> String {
    if ext.is_empty() { format!("{BLOBS_DIR}/{hash}") } else { format!("{BLOBS_DIR}/{hash}.{ext}") }
}

/// The URL a blob is served at.
pub fn blob_url(hash: &str, ext: &str) -> String {
    format!("/api/v1/files/{}", blob_rel(hash, ext))
}

/// Content-addressed blob: store the bytes ONCE keyed by hash, so a revert or
/// the same content across documents reuses one file. The ext keeps
/// serve_file's content-type detection working. Answers the URL a work
/// version points at.
pub fn put_work_blob(
    store: &db::Store,
    files_dir: &Path,
    hash: &str,
    ext: &str,
    bytes: &[u8],
) -> Result<String, types::NeboError> {
    let dest = files_dir.join(blob_rel(hash, ext));
    if !dest.exists() {
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| types::NeboError::Internal(format!("mkdir blobs dir: {e}")))?;
        }
        std::fs::write(&dest, bytes).map_err(|e| types::NeboError::Internal(format!("write blob: {e}")))?;
    }
    let _ = store.register_content_blob(hash, ext, bytes.len() as i64);
    Ok(blob_url(hash, ext))
}

/// Keep a file's current bytes as a blob and answer their hash and size.
/// The file is copied first (a clone where the disk can make one: APFS,
/// btrfs) and the copy hashed, so the hash is of exactly the bytes kept,
/// however the file changes meanwhile.
fn keep_file(store: &db::Store, files_dir: &Path, src: &Path, ext: &str) -> std::io::Result<(String, u64)> {
    use sha2::{Digest, Sha256};
    let blobs = files_dir.join(BLOBS_DIR);
    std::fs::create_dir_all(&blobs)?;
    let part = blobs.join(format!(".in-{}", uuid::Uuid::new_v4()));
    let kept = (|| -> std::io::Result<(String, u64)> {
        let size = std::fs::copy(src, &part)?;
        let mut hasher = Sha256::new();
        std::io::copy(&mut std::fs::File::open(&part)?, &mut hasher)?;
        let hash = hex::encode(hasher.finalize());
        let dest = files_dir.join(blob_rel(&hash, ext));
        if dest.exists() {
            std::fs::remove_file(&part)?;
        } else {
            std::fs::rename(&part, &dest)?;
        }
        Ok((hash, size))
    })();
    if kept.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    let (hash, size) = kept?;
    let _ = store.register_content_blob(&hash, ext, size as i64);
    Ok((hash, size))
}

// ── Looking at the workspace ───────────────────────────────────────

/// What one look did.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Look {
    /// Files in the history's reach.
    pub files: usize,
    /// Files hashed and kept again because their size or mtime moved.
    pub hashed: usize,
    /// Earlier contents kept as history.
    pub kept: usize,
}

fn ext_of(path: &Path) -> String {
    path.extension().and_then(|e| e.to_str()).map(str::to_lowercase).unwrap_or_default()
}

fn mtime_ns(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as i64)
        .unwrap_or(0)
}

/// Whether a walk leaves this entry (and everything under it) out.
fn left_out(entry: &walkdir::DirEntry) -> bool {
    if entry.depth() == 0 {
        return false;
    }
    let name = entry.file_name().to_string_lossy();
    if name.starts_with('.') {
        return true;
    }
    entry.file_type().is_dir()
        && (SKIPPED_DIRS.contains(&name.as_ref()) || (entry.depth() == 1 && name == WORK_DIR))
}

/// A file's place in the workspace, '/'-separated; None for a name that
/// isn't UTF-8 (it couldn't be named back to restore it).
fn rel_of(files_dir: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(files_dir).ok()?;
    let parts: Option<Vec<&str>> = rel.components().map(|c| c.as_os_str().to_str()).collect();
    Some(parts?.join("/"))
}

/// Look at the workspace (see the module doc). `chat_id` is the
/// conversation whose turn is looking: what it finds changed is recorded
/// under it.
pub fn look(store: &db::Store, files_dir: &Path, chat_id: Option<&str>) -> Result<Look, String> {
    let _one = one_at_a_time();
    if !files_dir.is_dir() {
        return Ok(Look::default());
    }
    let index: HashMap<String, WorkspaceIndexRow> = store
        .workspace_index()
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|r| (r.path.clone(), r))
        .collect();
    let mut seen: HashSet<String> = HashSet::new();
    let mut upserts = Vec::new();
    let mut removed = Vec::new();
    let mut history = Vec::new();
    let mut walk_failed = false;
    let mut look = Look::default();

    let walk = walkdir::WalkDir::new(files_dir).follow_links(false).into_iter().filter_entry(|e| !left_out(e));
    for entry in walk {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => {
                // A folder that couldn't be read is not a folder emptied:
                // nothing is taken as deleted on this look.
                walk_failed = true;
                continue;
            }
        };
        // Symlinks are not files (`follow_links(false)`): left out.
        if !entry.file_type().is_file() {
            continue;
        }
        let Some(rel) = rel_of(files_dir, entry.path()) else { continue };
        let Ok(meta) = entry.metadata() else {
            walk_failed = true;
            continue;
        };
        seen.insert(rel.clone());
        let before = index.get(&rel);
        if meta.len() > MAX_FILE_BYTES {
            // Grown past the ceiling: what it was is kept, and it leaves
            // the history's reach.
            if let Some(b) = before {
                history.push(earlier(b, "modified"));
                removed.push(rel);
            }
            continue;
        }
        look.files += 1;
        let (size, mtime) = (meta.len() as i64, mtime_ns(&meta));
        if before.is_some_and(|b| b.size_bytes == size && b.mtime_ns == mtime) {
            continue;
        }
        let ext = ext_of(entry.path());
        let (hash, _) = match keep_file(store, files_dir, entry.path(), &ext) {
            Ok(kept) => kept,
            Err(e) => {
                // Gone or unreadable mid-look: the next look sees it.
                tracing::debug!(path = %rel, error = %e, "workspace history: file not kept on this look");
                continue;
            }
        };
        look.hashed += 1;
        if let Some(b) = before.filter(|b| b.hash != hash) {
            history.push(earlier(b, "modified"));
        }
        upserts.push(WorkspaceIndexRow { path: rel, size_bytes: size, mtime_ns: mtime, hash, ext });
    }
    if !walk_failed {
        for (path, b) in &index {
            if !seen.contains(path) {
                history.push(earlier(b, "deleted"));
                removed.push(path.clone());
            }
        }
    }
    look.kept = history.len();
    store.record_workspace_look(&upserts, &removed, &history, chat_id).map_err(|e| e.to_string())?;
    Ok(look)
}

fn earlier(row: &WorkspaceIndexRow, reason: &'static str) -> NewFileHistory {
    NewFileHistory {
        path: row.path.clone(),
        hash: row.hash.clone(),
        ext: row.ext.clone(),
        size_bytes: row.size_bytes,
        reason,
    }
}

// ── History and restore ────────────────────────────────────────────

/// A path's place in the workspace: a path inside `files_dir`, or one
/// relative to it. None for anything outside it, or that climbs out of it.
pub fn workspace_path(files_dir: &Path, path: &str) -> Option<String> {
    let path = path.trim();
    let given = Path::new(path);
    let rel: PathBuf = if given.is_absolute() {
        match given.strip_prefix(files_dir) {
            Ok(rel) => rel.to_path_buf(),
            // `/tmp` for `/private/tmp`: compare the real folders.
            Err(_) => {
                let root = files_dir.canonicalize().ok()?;
                let parent = given.parent()?.canonicalize().ok()?;
                parent.strip_prefix(&root).ok()?.join(given.file_name()?)
            }
        }
    } else {
        given.to_path_buf()
    };
    let mut parts = Vec::new();
    for c in rel.components() {
        match c {
            Component::Normal(p) => parts.push(p.to_str()?),
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// A file's earlier contents, newest first.
pub fn history(store: &db::Store, rel: &str) -> Result<Vec<FileHistoryEntry>, String> {
    store.list_file_history(rel, 200).map_err(|e| e.to_string())
}

/// What a restore did.
#[derive(Debug)]
pub struct Restored {
    /// The content the file is back to.
    pub to: FileHistoryEntry,
    /// The content it had just before, kept as history; None when it was
    /// gone or already that content.
    pub saved: Option<FileHistoryEntry>,
    /// The work document behind the file, with the version the restore
    /// added to it (same conversation, same name), so its version list
    /// shows the restore.
    pub work: Option<(db::WorkDocument, db::WorkDocumentVersion)>,
}

/// Put entry `id`'s content back at its path, keeping the file's current
/// content as history first.
pub fn restore(store: &db::Store, files_dir: &Path, id: i64, chat_id: Option<&str>) -> Result<Restored, String> {
    let _one = one_at_a_time();
    let to = store
        .get_file_history(id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("No earlier version {id} is kept."))?;
    let rel = workspace_path(files_dir, &to.path).ok_or_else(|| format!("{} is not in the workspace.", to.path))?;
    let target = files_dir.join(&rel);
    let blob = files_dir.join(blob_rel(&to.hash, &to.ext));
    if !blob.is_file() {
        return Err(format!("The kept content of {rel} is missing."));
    }
    let index_row = store
        .workspace_index()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|r| r.path == rel);

    // What the file is now, kept first.
    let mut current = Vec::new();
    match std::fs::symlink_metadata(&target) {
        Ok(meta) if meta.is_file() => {
            let ext = ext_of(&target);
            let (hash, size) = match index_row.as_ref() {
                Some(r) if r.size_bytes == meta.len() as i64 && r.mtime_ns == mtime_ns(&meta) => {
                    (r.hash.clone(), meta.len())
                }
                _ => keep_file(store, files_dir, &target, &ext).map_err(|e| format!("could not keep {rel} before restoring: {e}"))?,
            };
            if hash != to.hash {
                current.push(NewFileHistory { path: rel.clone(), hash, ext, size_bytes: size as i64, reason: "modified" });
            }
        }
        Ok(_) => return Err(format!("{rel} is not a file.")),
        Err(_) => {}
    }
    let saved_hash = current.first().map(|h| h.hash.clone());

    // Copy beside it, then rename: a reader never meets half a file.
    let dir = target.parent().ok_or_else(|| format!("{rel} has no folder"))?;
    std::fs::create_dir_all(dir).map_err(|e| format!("could not make {}: {e}", dir.display()))?;
    let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let part = dir.join(format!(".{name}.restore-{}", uuid::Uuid::new_v4()));
    if let Err(e) = std::fs::copy(&blob, &part).and_then(|_| std::fs::rename(&part, &target)) {
        let _ = std::fs::remove_file(&part);
        return Err(format!("could not restore {rel}: {e}"));
    }
    let meta = std::fs::metadata(&target).map_err(|e| e.to_string())?;
    let row = WorkspaceIndexRow {
        path: rel.clone(),
        size_bytes: meta.len() as i64,
        mtime_ns: mtime_ns(&meta),
        hash: to.hash.clone(),
        ext: to.ext.clone(),
    };
    store.record_workspace_look(&[row], &[], &current, chat_id).map_err(|e| e.to_string())?;
    let saved = match saved_hash {
        Some(hash) => history(store, &rel)?.into_iter().find(|e| e.hash == hash),
        None => None,
    };
    let work = to.chat_id.as_deref().and_then(|chat| work_version(store, chat, &name, &to));
    Ok(Restored { to, saved, work })
}

/// The restored content as a new version of the work document of that name
/// in `chat`, when there is one and its latest version isn't that content.
fn work_version(
    store: &db::Store,
    chat: &str,
    filename: &str,
    to: &FileHistoryEntry,
) -> Option<(db::WorkDocument, db::WorkDocumentVersion)> {
    let doc = store.work_document_for(chat, filename).ok()??;
    let latest = store.latest_work_version(&doc.id).ok()?;
    if latest.as_ref().is_some_and(|v| v.content_hash.as_deref() == Some(to.hash.as_str())) {
        return None;
    }
    let version = store
        .add_work_version(
            &doc.id,
            latest.as_ref().map(|v| v.id.as_str()),
            &blob_url(&to.hash, &to.ext),
            Some(&to.hash),
            None,
            None,
        )
        .ok()?;
    Some((doc, version))
}

// ── As the employee reads it ───────────────────────────────────────

/// The prefix of an earlier content's id as the checkpoint tools take it
/// (`fh-12`), beside a checkpoint's `cp-…`.
pub const ID_PREFIX: &str = "fh-";

/// The entry id an `fh-<n>` names.
pub fn parse_id(id: &str) -> Option<i64> {
    id.trim().strip_prefix(ID_PREFIX)?.parse().ok()
}

fn when(e: &FileHistoryEntry) -> String {
    chrono::DateTime::from_timestamp(e.captured_at, 0)
        .map(|t| crate::app_history::local_time(&t))
        .unwrap_or_default()
}

fn size(bytes: i64) -> String {
    match bytes {
        b if b >= 1024 * 1024 => format!("{:.1} MB", b as f64 / (1024.0 * 1024.0)),
        b if b >= 1024 => format!("{:.1} KB", b as f64 / 1024.0),
        b => format!("{b} bytes"),
    }
}

/// A file's earlier contents as the employee reads them.
pub fn describe(rel: &str, entries: &[FileHistoryEntry]) -> String {
    if entries.is_empty() {
        return format!("{rel} has no earlier versions: it hasn't changed since Nebo started keeping them.");
    }
    let mut out = format!(
        "Earlier versions of {rel}, newest first. Put one back with restore_checkpoint(checkpoint: \"<id>\"); \
         what the file is now is kept first, so a restore can be undone.\n"
    );
    for e in entries {
        let what = if e.reason == "deleted" { "before it was deleted" } else { "before it changed" };
        out.push_str(&format!("- {ID_PREFIX}{}  {}  {}  ({what})\n", e.id, when(e), size(e.size_bytes)));
    }
    out
}

/// What a restore did, as the employee reads it.
pub fn describe_restore(r: &Restored) -> String {
    let mut out = format!("Restored {} to its version from {} ({}).", r.to.path, when(&r.to), size(r.to.size_bytes));
    if let Some(saved) = &r.saved {
        out.push_str(&format!(" What it was just before is kept as {ID_PREFIX}{}: restore that to undo this.", saved.id));
    }
    out
}

#[cfg(test)]
mod tests;
