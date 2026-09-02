//! Checkpoints without destructive git (coding harness P6.2).
//!
//! Before a risky multi-file change the employee snapshots the files it is
//! about to touch; "restore" copies them back. Nothing here runs git, so the
//! house rules (never stash, never reset) are kept by construction, and a
//! checkpoint works in a directory that is not a repository at all.
//!
//! Storage: `<data_dir>/sessions/<session>/checkpoints/<id>/` holds a
//! `manifest.json` and the file bytes as `files/<n>`. Restore is itself
//! reversible: it first checkpoints the current state of the same paths, so
//! the result of a restore always names the checkpoint that undoes it.
//! Files that did not exist at checkpoint time are removed on restore —
//! "restore" means "make the tree look like it did", not "overwrite what's
//! there".

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Checkpoints kept per session. Creating the 101st evicts the oldest, except
/// the undo checkpoint of the most recent restore (that one is the only way
/// back from the last restore, so it outlives the cap).
pub const MAX_CHECKPOINTS: usize = 100;

/// One file inside a checkpoint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileSnap {
    /// The path as the caller gave it, made absolute.
    pub path: String,
    /// Whether the file existed when the checkpoint was taken. A file that
    /// did not exist is restored by removing it.
    pub existed: bool,
    /// Content fingerprint (0 when the file did not exist).
    pub fingerprint: u64,
    /// Size in bytes at checkpoint time.
    pub bytes: u64,
}

/// A checkpoint's manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub id: String,
    pub label: String,
    /// RFC3339 UTC.
    pub created_at: String,
    pub files: Vec<FileSnap>,
}

/// What a restore did to one file.
#[derive(Debug, Clone, PartialEq)]
pub enum RestoreAction {
    /// Bytes copied back (file differed or was missing).
    Restored,
    /// Removed because it did not exist at checkpoint time.
    Removed,
    /// Already identical to the checkpoint.
    Unchanged,
    /// The caller asked for a subset and this file was not in it.
    Skipped,
}

#[derive(Debug, Clone)]
pub struct RestoreReport {
    pub checkpoint: Checkpoint,
    /// Checkpoint taken of the state just before this restore — the undo.
    pub undo: Checkpoint,
    pub actions: Vec<(String, RestoreAction)>,
}

/// Resolve Nebo's data directory the way `config::defaults::data_dir` does,
/// without a dependency on the config crate (tools sits below it).
fn data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("NEBO_HOME") {
        return PathBuf::from(dir);
    }
    if let Ok(dir) = std::env::var("NEBO_DATA_DIR") {
        return PathBuf::from(dir);
    }
    let name = if cfg!(target_os = "linux") { "nebo" } else { "Nebo" };
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(name)
}

/// A session's private directory under Nebo's data dir (checkpoints,
/// spilled tool results). Mode 0700: it holds file contents the employee read.
pub fn session_dir(session_id: &str) -> PathBuf {
    let session = if session_id.trim().is_empty() {
        "default"
    } else {
        session_id
    };
    // A session key can carry ':' (subagent:parent:task) — keep the tree flat.
    let safe: String = session
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    data_dir().join("sessions").join(safe)
}

/// Where a session keeps its checkpoints.
pub fn root(session_id: &str) -> PathBuf {
    session_dir(session_id).join("checkpoints")
}

/// Restrict a directory (0700) or file (0600) to the owner. No-op elsewhere.
#[cfg(unix)]
pub fn restrict_private(path: &Path, is_dir: bool) {
    use std::os::unix::fs::PermissionsExt;
    let mode = if is_dir { 0o700 } else { 0o600 };
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}
#[cfg(not(unix))]
pub fn restrict_private(_path: &Path, _is_dir: bool) {}

/// Content fingerprint. xxh3 is stable across Rust releases, which matters
/// because the value is persisted in `manifest.json` and compared after an
/// app upgrade (`DefaultHasher` is documented as unstable across versions).
pub fn fingerprint(bytes: &[u8]) -> u64 {
    xxhash_rust::xxh3::xxh3_64(bytes)
}

/// Every absolute path a checkpoint covers (for the allowed-paths fence).
pub fn paths(cp: &Checkpoint) -> Vec<String> {
    cp.files.iter().map(|f| f.path.clone()).collect()
}

/// Enforce `MAX_CHECKPOINTS`: drop the oldest until the session is under the
/// cap. `keep` is never evicted (the undo of the last restore).
fn evict(session_id: &str, keep: Option<&str>) {
    let Ok(list) = list(session_id) else { return };
    if list.len() < MAX_CHECKPOINTS {
        return;
    }
    let root = root(session_id);
    let mut excess = list.len() + 1 - MAX_CHECKPOINTS; // room for the one being created
    for cp in list {
        if excess == 0 {
            break;
        }
        if keep == Some(cp.id.as_str()) {
            continue;
        }
        if let Err(e) = std::fs::remove_dir_all(root.join(&cp.id)) {
            // Not fatal: the cap is a disk bound, not a correctness one.
            tracing::warn!(id = %cp.id, error = %e, "checkpoint not evicted");
        }
        excess -= 1;
    }
}

/// The undo checkpoint of the most recent restore, if any (by label).
fn last_undo(session_id: &str) -> Option<String> {
    list(session_id)
        .ok()?
        .into_iter()
        .rev()
        .find(|c| c.label.starts_with("before restore of "))
        .map(|c| c.id)
}

fn absolute(p: &str) -> String {
    std::path::absolute(Path::new(p))
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| p.to_string())
}

fn manifest_path(dir: &Path) -> PathBuf {
    dir.join("manifest.json")
}

fn write_manifest(dir: &Path, cp: &Checkpoint) -> Result<(), String> {
    let json = serde_json::to_vec_pretty(cp).map_err(|e| e.to_string())?;
    std::fs::write(manifest_path(dir), json).map_err(|e| format!("write manifest: {e}"))
}

fn read_manifest(dir: &Path) -> Result<Checkpoint, String> {
    let bytes = std::fs::read(manifest_path(dir)).map_err(|e| format!("read manifest: {e}"))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("parse manifest: {e}"))
}

#[cfg(unix)]
fn restrict(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
}
#[cfg(not(unix))]
fn restrict(_dir: &Path) {}

/// Snapshot `paths` under `session_id`. Paths may be files that do not exist
/// yet (recorded as such, so a restore removes them). Directories are refused:
/// a checkpoint names files on purpose, so the restore's blast radius is the
/// list the model asked for.
pub fn create(session_id: &str, label: &str, paths: &[String]) -> Result<Checkpoint, String> {
    if paths.is_empty() {
        return Err("checkpoint needs `paths`: the files you are about to change".into());
    }
    let root = root(session_id);
    std::fs::create_dir_all(&root).map_err(|e| format!("create {}: {e}", root.display()))?;
    restrict(&root);
    evict(session_id, last_undo(session_id).as_deref());
    // Second-precision stamp for humans, a process counter so two checkpoints
    // in the same second still list in creation order, and a random tail.
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    const SEQ_DIGITS_MOD: u64 = 10_000; // four digits in the id
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % SEQ_DIGITS_MOD;
    let now = chrono::Utc::now();
    let id = format!(
        "cp-{}-{seq:04}-{}",
        now.format("%Y%m%d-%H%M%S"),
        &uuid::Uuid::new_v4().to_string()[..6]
    );
    let dir = root.join(&id);
    let files_dir = dir.join("files");
    std::fs::create_dir_all(&files_dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    restrict(&dir);

    let mut files = Vec::with_capacity(paths.len());
    for (n, raw) in paths.iter().enumerate() {
        let path = absolute(raw);
        let p = Path::new(&path);
        if p.is_dir() {
            // A refused checkpoint leaves nothing behind; the dir is ours and empty.
            let _ = std::fs::remove_dir_all(&dir);
            return Err(format!(
                "{path} is a directory: checkpoint names the files you will change, not a tree"
            ));
        }
        match std::fs::read(p) {
            Ok(bytes) => {
                std::fs::write(files_dir.join(n.to_string()), &bytes)
                    .map_err(|e| format!("snapshot {path}: {e}"))?;
                files.push(FileSnap {
                    path,
                    existed: true,
                    fingerprint: fingerprint(&bytes),
                    bytes: bytes.len() as u64,
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => files.push(FileSnap {
                path,
                existed: false,
                fingerprint: 0,
                bytes: 0,
            }),
            Err(e) => {
                // Same: a half-written checkpoint must not be listed.
                let _ = std::fs::remove_dir_all(&dir);
                return Err(format!("read {path}: {e}"));
            }
        }
    }
    let cp = Checkpoint {
        id,
        label: if label.trim().is_empty() { "checkpoint".into() } else { label.trim().into() },
        created_at: now.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true),
        files,
    };
    write_manifest(&dir, &cp)?;
    Ok(cp)
}

/// Every checkpoint of the session, oldest first.
pub fn list(session_id: &str) -> Result<Vec<Checkpoint>, String> {
    let root = root(session_id);
    let entries = match std::fs::read_dir(&root) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("list {}: {e}", root.display())),
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        if let Ok(cp) = read_manifest(&entry.path()) {
            out.push(cp);
        }
    }
    // Creation order: the nanosecond stamp across processes, the counter
    // (inside the id) within one.
    out.sort_by(|a, b| a.created_at.cmp(&b.created_at).then_with(|| a.id.cmp(&b.id)));
    Ok(out)
}

pub fn get(session_id: &str, id: &str) -> Result<Checkpoint, String> {
    if id.trim().is_empty() {
        return Err("restore needs `checkpoint`: the id from checkpoint/checkpoints".into());
    }
    let dir = root(session_id).join(id);
    if !manifest_path(&dir).exists() {
        let known: Vec<String> = list(session_id)?.into_iter().map(|c| c.id).collect();
        return Err(if known.is_empty() {
            format!("no checkpoint {id}: this session has no checkpoints yet")
        } else {
            format!("no checkpoint {id}. Known: {}", known.join(", "))
        });
    }
    read_manifest(&dir)
}

/// Put the files of checkpoint `id` back. `only` limits the restore to a
/// subset of the checkpoint's paths (empty = all). The current state of the
/// affected paths is checkpointed first, so the report's `undo` reverses it.
pub fn restore(session_id: &str, id: &str, only: &[String]) -> Result<RestoreReport, String> {
    let cp = get(session_id, id)?;
    let dir = root(session_id).join(&cp.id);
    let wanted: Vec<String> = only.iter().map(|p| absolute(p)).collect();
    let selected: Vec<(usize, &FileSnap)> = cp
        .files
        .iter()
        .enumerate()
        .filter(|(_, f)| wanted.is_empty() || wanted.contains(&f.path))
        .collect();
    if selected.is_empty() {
        return Err(format!(
            "none of the requested paths are in checkpoint {}. It holds: {}",
            cp.id,
            cp.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>().join(", ")
        ));
    }

    // The undo point: what these paths look like right now.
    let undo_paths: Vec<String> = selected.iter().map(|(_, f)| f.path.clone()).collect();
    let undo = create(session_id, &format!("before restore of {}", cp.id), &undo_paths)?;

    let mut actions = Vec::with_capacity(cp.files.len());
    for (n, snap) in cp.files.iter().enumerate() {
        if !selected.iter().any(|(i, _)| *i == n) {
            actions.push((snap.path.clone(), RestoreAction::Skipped));
            continue;
        }
        let target = Path::new(&snap.path);
        if !snap.existed {
            match std::fs::remove_file(target) {
                Ok(()) => actions.push((snap.path.clone(), RestoreAction::Removed)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    actions.push((snap.path.clone(), RestoreAction::Unchanged))
                }
                Err(e) => return Err(format!("remove {}: {e}", snap.path)),
            }
            continue;
        }
        let bytes = std::fs::read(dir.join("files").join(n.to_string()))
            .map_err(|e| format!("checkpoint {} is missing its copy of {}: {e}", cp.id, snap.path))?;
        let same = std::fs::read(target)
            .map(|cur| fingerprint(&cur) == snap.fingerprint)
            .unwrap_or(false);
        if same {
            actions.push((snap.path.clone(), RestoreAction::Unchanged));
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
        }
        std::fs::write(target, &bytes).map_err(|e| format!("restore {}: {e}", snap.path))?;
        actions.push((snap.path.clone(), RestoreAction::Restored));
    }
    Ok(RestoreReport { checkpoint: cp, undo, actions })
}

/// One line per checkpoint, for the `checkpoints` action.
pub fn render_list(list: &[Checkpoint]) -> String {
    if list.is_empty() {
        return "(no checkpoints in this session; take one with action: \"checkpoint\", paths: [...])"
            .to_string();
    }
    let mut out = String::new();
    for cp in list {
        out.push_str(&format!(
            "{}  {}  \"{}\"  {} file{}\n",
            cp.id,
            // Seconds are enough for a human; the manifest keeps the nanos.
            cp.created_at.get(..19).map(|s| format!("{s}Z")).unwrap_or_else(|| cp.created_at.clone()),
            cp.label,
            cp.files.len(),
            if cp.files.len() == 1 { "" } else { "s" }
        ));
        for f in &cp.files {
            out.push_str(&format!(
                "  - {}{}\n",
                f.path,
                if f.existed { format!(" ({} bytes)", f.bytes) } else { " (did not exist)".into() }
            ));
        }
    }
    out.trim_end().to_string()
}

pub fn render_created(cp: &Checkpoint) -> String {
    let mut out = format!(
        "checkpoint {} taken (\"{}\"): {} file{}\n",
        cp.id,
        cp.label,
        cp.files.len(),
        if cp.files.len() == 1 { "" } else { "s" }
    );
    for f in &cp.files {
        out.push_str(&format!(
            "  - {}{}\n",
            f.path,
            if f.existed { format!(" ({} bytes)", f.bytes) } else { " (did not exist)".into() }
        ));
    }
    out.push_str(&format!(
        "Restore with os(resource: \"file\", action: \"restore\", checkpoint: \"{}\")",
        cp.id
    ));
    out
}

pub fn render_restore(r: &RestoreReport) -> String {
    let mut out = format!("restored checkpoint {} (\"{}\")\n", r.checkpoint.id, r.checkpoint.label);
    for (path, action) in &r.actions {
        let word = match action {
            RestoreAction::Restored => "restored",
            RestoreAction::Removed => "removed (did not exist at checkpoint)",
            RestoreAction::Unchanged => "unchanged",
            RestoreAction::Skipped => continue,
        };
        out.push_str(&format!("  - {path}: {word}\n"));
    }
    out.push_str(&format!(
        "The state before this restore is checkpoint {}. Restore it to undo.",
        r.undo.id
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> String {
        // Each test gets its own session dir under a temp NEBO_HOME.
        format!("test-{}", uuid::Uuid::new_v4())
    }

    fn with_home<T>(f: impl FnOnce() -> T) -> T {
        // Tests share the process env; serialize on a lock so parallel tests
        // don't race NEBO_HOME.
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        // SAFETY: the lock above serializes env mutation across these tests.
        unsafe { std::env::set_var("NEBO_HOME", home.path()) };
        let out = f();
        unsafe { std::env::remove_var("NEBO_HOME") };
        out
    }

    #[test]
    fn create_then_restore_round_trips_bytes_and_removes_new_files() {
        with_home(|| {
            let work = tempfile::tempdir().unwrap();
            let a = work.path().join("a.txt");
            let b = work.path().join("new.txt"); // does not exist yet
            std::fs::write(&a, "one").unwrap();
            let s = session();
            let cp = create(&s, "before refactor", &[a.display().to_string(), b.display().to_string()]).unwrap();
            assert_eq!(cp.files.len(), 2);
            assert!(cp.files[0].existed);
            assert!(!cp.files[1].existed);

            std::fs::write(&a, "two").unwrap();
            std::fs::write(&b, "created later").unwrap();

            let r = restore(&s, &cp.id, &[]).unwrap();
            assert_eq!(std::fs::read_to_string(&a).unwrap(), "one");
            assert!(!b.exists(), "a file that did not exist at checkpoint time is removed");
            assert_eq!(r.actions[0].1, RestoreAction::Restored);
            assert_eq!(r.actions[1].1, RestoreAction::Removed);

            // The restore is reversible: its undo checkpoint holds "two" and "created later".
            let u = restore(&s, &r.undo.id, &[]).unwrap();
            assert_eq!(std::fs::read_to_string(&a).unwrap(), "two");
            assert_eq!(std::fs::read_to_string(&b).unwrap(), "created later");
            assert_eq!(u.actions.len(), 2);
        });
    }

    #[test]
    fn subset_restore_touches_only_the_named_paths() {
        with_home(|| {
            let work = tempfile::tempdir().unwrap();
            let a = work.path().join("a.txt");
            let b = work.path().join("b.txt");
            std::fs::write(&a, "a1").unwrap();
            std::fs::write(&b, "b1").unwrap();
            let s = session();
            let cp = create(&s, "", &[a.display().to_string(), b.display().to_string()]).unwrap();
            assert_eq!(cp.label, "checkpoint");
            std::fs::write(&a, "a2").unwrap();
            std::fs::write(&b, "b2").unwrap();
            let r = restore(&s, &cp.id, &[b.display().to_string()]).unwrap();
            assert_eq!(std::fs::read_to_string(&a).unwrap(), "a2", "a was not requested");
            assert_eq!(std::fs::read_to_string(&b).unwrap(), "b1");
            assert_eq!(r.actions[0].1, RestoreAction::Skipped);
            assert_eq!(r.actions[1].1, RestoreAction::Restored);
            assert_eq!(r.undo.files.len(), 1, "undo covers only the restored subset");
        });
    }

    #[test]
    fn list_is_oldest_first_and_unknown_ids_name_the_known_ones() {
        with_home(|| {
            let work = tempfile::tempdir().unwrap();
            let a = work.path().join("a.txt");
            std::fs::write(&a, "x").unwrap();
            let s = session();
            assert!(list(&s).unwrap().is_empty());
            let c1 = create(&s, "first", &[a.display().to_string()]).unwrap();
            let c2 = create(&s, "second", &[a.display().to_string()]).unwrap();
            let l = list(&s).unwrap();
            assert_eq!(l.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), vec![c1.id.as_str(), c2.id.as_str()]);
            let err = restore(&s, "cp-nope", &[]).unwrap_err();
            assert!(err.contains(&c1.id) && err.contains(&c2.id), "{err}");
        });
    }

    #[test]
    fn fingerprint_is_stable_across_processes() {
        // A literal, not a computed value: if the hash function changes,
        // persisted manifests silently misreport "unchanged".
        assert_eq!(fingerprint(b"one"), 10_457_265_594_366_419_360_u64);
        assert_ne!(fingerprint(b"one"), fingerprint(b"two"));
    }

    #[test]
    fn cap_evicts_oldest_but_never_the_undo_of_the_last_restore() {
        with_home(|| {
            let work = tempfile::tempdir().unwrap();
            let a = work.path().join("a.txt");
            std::fs::write(&a, "v0").unwrap();
            let s = session();
            let first = create(&s, "first", &[a.display().to_string()]).unwrap();
            std::fs::write(&a, "v1").unwrap();
            // A restore whose undo must survive the cap.
            let r = restore(&s, &first.id, &[]).unwrap();
            let undo = r.undo.id.clone();
            for i in 0..MAX_CHECKPOINTS {
                create(&s, &format!("cp{i}"), &[a.display().to_string()]).unwrap();
            }
            let ids: Vec<String> = list(&s).unwrap().into_iter().map(|c| c.id).collect();
            assert_eq!(ids.len(), MAX_CHECKPOINTS, "capped");
            assert!(!ids.contains(&first.id), "the oldest was evicted");
            assert!(ids.contains(&undo), "the undo of the last restore survives");
        });
    }

    #[test]
    fn restore_recreates_a_missing_parent_directory() {
        with_home(|| {
            let work = tempfile::tempdir().unwrap();
            let dir = work.path().join("sub");
            std::fs::create_dir_all(&dir).unwrap();
            let a = dir.join("a.txt");
            std::fs::write(&a, "keep").unwrap();
            let s = session();
            let cp = create(&s, "", &[a.display().to_string()]).unwrap();
            std::fs::remove_dir_all(&dir).unwrap();
            let r = restore(&s, &cp.id, &[]).unwrap();
            assert_eq!(std::fs::read_to_string(&a).unwrap(), "keep");
            assert_eq!(r.actions[0].1, RestoreAction::Restored);
        });
    }

    #[test]
    fn restore_of_a_missing_snapshot_file_fails_before_writing_anything() {
        with_home(|| {
            let work = tempfile::tempdir().unwrap();
            let a = work.path().join("a.txt");
            std::fs::write(&a, "v0").unwrap();
            let s = session();
            let cp = create(&s, "", &[a.display().to_string()]).unwrap();
            std::fs::remove_file(root(&s).join(&cp.id).join("files").join("0")).unwrap();
            std::fs::write(&a, "v1").unwrap();
            let err = restore(&s, &cp.id, &[]).unwrap_err();
            assert!(err.contains("missing its copy"), "{err}");
            assert_eq!(std::fs::read_to_string(&a).unwrap(), "v1", "target untouched");
        });
    }

    #[test]
    fn directories_and_empty_path_lists_are_refused() {
        with_home(|| {
            let work = tempfile::tempdir().unwrap();
            let s = session();
            assert!(create(&s, "", &[]).unwrap_err().contains("paths"));
            let err = create(&s, "", &[work.path().display().to_string()]).unwrap_err();
            assert!(err.contains("directory"), "{err}");
            assert!(list(&s).unwrap().is_empty(), "a refused checkpoint leaves nothing behind");
        });
    }
}
