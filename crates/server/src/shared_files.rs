//! The files handed to the owner, kept under `<data_dir>/files/.shared/
//! <content hash>/<name>` (`chat_dispatch::keep_shared_file`), are removed
//! once nothing shows them any more.
//!
//! A folder goes only when all of these hold: no stored message, outbound
//! message or work version names it (`Store::referenced_shared_folders`; a
//! deleted message names nothing), nothing in it was written or reused for
//! [`KEEP_UNUSED`], and its name is a content hash this code made. Reusing a
//! kept file marks it as just used ([`touch`]), so a run sharing it again
//! before its message is stored keeps it. When the references can't be read,
//! nothing is removed.
//!
//! Copies, not hard links: a hard link shares the source's bytes, so a file
//! changed in place later (a shell `>`, an editor) would change what an
//! earlier card shows. A copy is already a clone where the disk can make one
//! (APFS, btrfs), so it costs no space until the source changes.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use tracing::{info, warn};

use crate::chat_dispatch::SHARED_DIR;

/// How long a kept file nothing names stays: longer than any run takes to
/// store the message that shows it.
const KEEP_UNUSED: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// How often the sweep runs after the first, a minute after start.
const EVERY: Duration = Duration::from_secs(24 * 60 * 60);

/// Mark a kept file as just used.
pub(crate) fn touch(path: &Path) {
    if let Err(e) = std::fs::File::options().write(true).open(path).and_then(|f| f.set_modified(SystemTime::now())) {
        warn!(error = %e, path = %path.display(), "could not mark a shared file as used");
    }
}

/// Sweep `.shared` a minute after start and once a day after, in the
/// background.
pub(crate) fn spawn_sweep(store: Arc<db::Store>) {
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(60)).await;
        // Only the running copy that holds the bot changes anything.
        comm::lease::process().granted_or_unleased().await;
        loop {
            let Ok(files_dir) = config::data_dir().map(|d| d.join("files")) else {
                return;
            };
            let store = store.clone();
            match tokio::task::spawn_blocking(move || sweep(&store, &files_dir, KEEP_UNUSED)).await {
                Ok(0) => {}
                Ok(removed) => info!(removed, "removed shared files nothing shows any more"),
                Err(e) => warn!(error = %e, "the shared-files sweep stopped"),
            }
            tokio::time::sleep(EVERY).await;
        }
    });
}

/// Remove every `.shared/<folder>/` under `files_dir` that nothing stored
/// names and that went unused for `keep`. Returns how many went.
pub(crate) fn sweep(store: &db::Store, files_dir: &Path, keep: Duration) -> usize {
    let root = files_dir.join(SHARED_DIR);
    let Ok(entries) = std::fs::read_dir(&root) else {
        return 0;
    };
    let referenced = match store.referenced_shared_folders() {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "shared files kept: what the messages show could not be read");
            return 0;
        }
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let dir = entry.path();
        let ours = name.len() == 16 && name.chars().all(|c| c.is_ascii_hexdigit());
        if !ours || !dir.is_dir() || referenced.contains(&name) {
            continue;
        }
        // Read again right before removing: a run reusing it now keeps it.
        if !unused_for(&dir, keep) {
            continue;
        }
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => removed += 1,
            Err(e) => warn!(error = %e, folder = %name, "could not remove a shared folder"),
        }
    }
    removed
}

/// Nothing in `dir` (the folder or a file in it) was changed within `keep`.
/// Anything unreadable counts as just used.
fn unused_for(dir: &Path, keep: Duration) -> bool {
    let old = |path: &Path| {
        std::fs::metadata(path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age >= keep)
    };
    let Ok(files) = std::fs::read_dir(dir) else {
        return false;
    };
    old(dir) && files.into_iter().all(|f| f.is_ok_and(|f| old(&f.path())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(dir: &Path) -> db::Store {
        db::Store::new(&dir.join("nebo.db").to_string_lossy()).expect("store")
    }

    /// A folder at `.shared/<name>/` holding one file, as old as `age`.
    fn kept(files: &Path, name: &str, age: Duration) -> std::path::PathBuf {
        let dir = files.join(SHARED_DIR).join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.png");
        std::fs::write(&file, b"\x89PNG").unwrap();
        let then = SystemTime::now() - age;
        for p in [&file, &dir] {
            std::fs::File::open(p).unwrap().set_modified(then).unwrap();
        }
        dir
    }

    const DAY: Duration = Duration::from_secs(24 * 60 * 60);

    /// A file a stored message shows is never removed, however old; one no
    /// message shows any more goes once unused long enough; one used lately
    /// (just shared, or reused) stays.
    #[test]
    fn the_sweep_never_removes_a_file_a_message_shows() {
        let tmp = tempfile::tempdir().unwrap();
        let s = store(tmp.path());
        let files = tmp.path().join("files");
        let shown = kept(&files, "0123456789abcdef", 30 * DAY);
        let deleted = kept(&files, "fedcba9876543210", 30 * DAY);
        let fresh = kept(&files, "aaaaaaaaaaaaaaaa", Duration::ZERO);
        let not_ours = kept(&files, "notahash", 30 * DAY);

        s.create_chat("c1", "Pictures").unwrap();
        s.create_chat_message("m1", "c1", "assistant", "Here they are.", None).unwrap();
        s.attach_artifacts_to_latest_assistant_message(
            "c1",
            &[
                serde_json::json!("/api/v1/files/.shared/0123456789abcdef/a.png"),
                serde_json::json!("/api/v1/files/.shared/fedcba9876543210/a.png"),
            ],
        )
        .unwrap();
        s.create_chat_message("m2", "c1", "assistant", "And one more.", None).unwrap();
        s.attach_artifacts_to_latest_assistant_message("c1", &[serde_json::json!("/api/v1/files/.shared/0123456789abcdef/a.png")])
            .unwrap();

        assert_eq!(sweep(&s, &files, KEEP_UNUSED), 0, "every old folder is still shown");
        assert!(shown.exists() && deleted.exists());

        // The message showing the second folder is deleted.
        s.delete_chat_message("m1").unwrap();
        assert_eq!(sweep(&s, &files, KEEP_UNUSED), 1);
        assert!(!deleted.exists(), "nothing shows it any more");
        assert!(shown.join("a.png").exists(), "m2 still shows it");
        assert!(fresh.exists(), "just shared: its message may not be stored yet");
        assert!(not_ours.exists(), "a folder this code didn't name is left alone");

        // Reusing an old, unshown file marks it as used: the sweep leaves it.
        let reused = kept(&files, "bbbbbbbbbbbbbbbb", 30 * DAY);
        touch(&reused.join("a.png"));
        assert_eq!(sweep(&s, &files, KEEP_UNUSED), 0);
        assert!(reused.exists());
    }
}
