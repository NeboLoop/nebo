//! Where Nebo's working copies live: the ONE answer, as `data_dir` is for
//! Nebo's own data.
//!
//! A working copy is the project an employee changes in isolation (a git
//! worktree or a scratch copy) and what the verify loop hands its reviewers
//! (a read-only export of the change, the patch, the one build folder
//! they share). None of it is Nebo's own data, so none of it sits in
//! `data_dir`, where every employee is fenced out of everything but a few
//! open parts (`tools::nebo_files`).
//!
//! The root is the system's temporary folder, one per user:
//! `/private/tmp/nebo-<uid>` on macOS, `/tmp/nebo-<uid>` on Linux, and
//! `nebo` in the user's own temporary folder on Windows. The shared `/tmp`
//! is anyone's to write in, so the folder is made private (mode 0700) and
//! refused when someone else made it first (another owner, a looser mode, a
//! link): the copies then go to the user's own temporary folder (`$TMPDIR`,
//! or the user's cache folder where that is the shared one), and the log
//! says so. The choice is made once per start; the folder is checked again
//! every time it is asked for, and made again (private) when the system
//! cleared it.
//!
//! The system may clear it (macOS clears `/private/tmp` at restart and what
//! sits unused for days): a copy's work is saved where it outlives that, and
//! a copy that is gone is made again from it.
//!
//! A relocated Nebo (`NEBO_HOME`: a test, a development copy, a cloud pod)
//! keeps its copies in a folder of its own under the root, so two Nebos on
//! one computer never share one.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use types::NeboError;

/// Nebo's working-copy root, made and checked: private to this user.
pub fn copies_dir() -> Result<PathBuf, NeboError> {
    static CHOSEN: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    let base = CHOSEN.get_or_init(choose).clone().map_err(NeboError::DataDir)?;
    // The system may have cleared it since: made again, and checked again.
    private_dir(&base, current_uid()).map_err(NeboError::DataDir)?;
    if !crate::defaults::data_dir_overridden() {
        return Ok(base);
    }
    let home = crate::defaults::data_dir()?;
    let own = base.join("homes").join(format!("{:016x}", fnv1a(home.to_string_lossy().as_bytes())));
    make_private_all(&own).map_err(NeboError::DataDir)?;
    Ok(own)
}

/// The first of the candidate roots that is (or can be made) private to
/// this user, the refusals logged.
fn choose() -> Result<PathBuf, String> {
    let uid = current_uid();
    let candidates = candidates(uid);
    let mut refused = Vec::new();
    for (n, dir) in candidates.iter().enumerate() {
        match private_dir(dir, uid) {
            Ok(()) => {
                if n > 0 {
                    tracing::warn!(root = %dir.display(), refused = %refused.join("; "), "working copies kept in the user's own temporary folder");
                }
                return Ok(dir.clone());
            }
            Err(e) => {
                tracing::warn!(root = %dir.display(), error = %e, "working-copy root refused");
                refused.push(e);
            }
        }
    }
    Err(format!("no private folder for working copies: {}", refused.join("; ")))
}

/// The roots in the order they are tried: the shared temporary folder's
/// `nebo-<uid>`, then the user's own temporary folder (`$TMPDIR`; the
/// user's cache folder where that is the shared one).
fn candidates(uid: u32) -> Vec<PathBuf> {
    let name = format!("nebo-{uid}");
    if cfg!(windows) {
        return vec![std::env::temp_dir().join("nebo")];
    }
    let shared = PathBuf::from(if cfg!(target_os = "macos") { "/private/tmp" } else { "/tmp" });
    let own_tmp = std::env::temp_dir();
    let same = |p: &Path| p == shared || std::fs::canonicalize(p).ok() == std::fs::canonicalize(&shared).ok();
    let own = if same(&own_tmp) { dirs::cache_dir().map(|c| c.join(&name)) } else { Some(own_tmp.join(&name)) };
    std::iter::once(shared.join(&name)).chain(own).collect()
}

/// Make `dir` (mode 0700) when it is missing, and refuse it unless it is a
/// folder (never a link) owned by `uid` that only its owner can reach.
fn private_dir(dir: &Path, uid: u32) -> Result<(), String> {
    match std::fs::symlink_metadata(dir) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = dir.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
            }
            match make_private(dir) {
                Ok(()) => {}
                // Made by someone else between the look and the make: it is
                // checked like any folder found there.
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(format!("{}: {e}", dir.display())),
            }
        }
        Err(e) => return Err(format!("{}: {e}", dir.display())),
    }
    check_private(dir, uid)
}

/// Whether `dir` is a folder of `uid`'s own, closed to everyone else.
#[cfg(unix)]
fn check_private(dir: &Path, uid: u32) -> Result<(), String> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let meta = std::fs::symlink_metadata(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    if !meta.file_type().is_dir() {
        return Err(format!("{} is not a folder", dir.display()));
    }
    if meta.uid() != uid {
        return Err(format!("{} belongs to another user (uid {})", dir.display(), meta.uid()));
    }
    let mode = meta.permissions().mode() & 0o777;
    if mode != 0o700 {
        return Err(format!("{} is mode {mode:o}, not 700", dir.display()));
    }
    Ok(())
}

/// The user's own temporary folder is closed to other users already.
#[cfg(not(unix))]
fn check_private(dir: &Path, _uid: u32) -> Result<(), String> {
    let meta = std::fs::symlink_metadata(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    if meta.file_type().is_dir() { Ok(()) } else { Err(format!("{} is not a folder", dir.display())) }
}

fn make_private(dir: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)
}

/// `dir` and every folder above it, inside a private root, made private.
fn make_private_all(dir: &Path) -> Result<(), String> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir).map_err(|e| format!("{}: {e}", dir.display()))
}

#[cfg(unix)]
fn current_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

#[cfg(not(unix))]
fn current_uid() -> u32 {
    0
}

/// FNV-1a: a folder name that stays the same from one build to the next.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn a_missing_root_is_made_private() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("nebo-1");
        private_dir(&dir, current_uid()).unwrap();
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        // Asked again (as on every use): still accepted.
        private_dir(&dir, current_uid()).unwrap();
        // Cleared by the system: made again.
        std::fs::remove_dir(&dir).unwrap();
        private_dir(&dir, current_uid()).unwrap();
        assert!(dir.is_dir());
    }

    /// Anyone can make `/tmp/nebo-<uid>` first: a root that is not this
    /// user's, not 0700, or not a folder is refused, never used or fixed.
    #[test]
    fn a_root_someone_else_could_reach_is_refused() {
        let t = tempfile::tempdir().unwrap();
        let loose = t.path().join("loose");
        std::fs::create_dir(&loose).unwrap();
        std::fs::set_permissions(&loose, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(private_dir(&loose, current_uid()).unwrap_err().contains("mode 755"));
        assert_eq!(std::fs::metadata(&loose).unwrap().permissions().mode() & 0o777, 0o755, "never fixed in place");

        let theirs = t.path().join("theirs");
        make_private(&theirs).unwrap();
        assert!(private_dir(&theirs, current_uid() + 1).unwrap_err().contains("another user"));

        let target = t.path().join("target");
        make_private(&target).unwrap();
        let link = t.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(private_dir(&link, current_uid()).unwrap_err().contains("not a folder"), "a link is refused");

        let file = t.path().join("file");
        std::fs::write(&file, "x").unwrap();
        assert!(private_dir(&file, current_uid()).is_err());
    }

    #[test]
    fn the_shared_temporary_folder_comes_first_then_the_users_own() {
        let c = candidates(501);
        let shared = if cfg!(target_os = "macos") { "/private/tmp/nebo-501" } else { "/tmp/nebo-501" };
        assert_eq!(c[0], PathBuf::from(shared));
        assert_eq!(c.len(), 2, "{c:?}");
        assert_ne!(c[1], c[0]);
        assert!(c[1].ends_with("nebo-501"));
    }

    #[test]
    fn the_folder_name_is_stable() {
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
    }
}
