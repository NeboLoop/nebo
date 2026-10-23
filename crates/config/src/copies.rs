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
//! The root is `nebo-copies` in the user's own local data folder, beside
//! Nebo's own and outside it: `~/Library/Application Support/nebo-copies`
//! on macOS, `~/.local/share/nebo-copies` on Linux (`$XDG_DATA_HOME`), and
//! `%LOCALAPPDATA%\nebo-copies` on Windows. No system cleaner prunes those
//! folders. Every temporary or cache folder is pruned behind the app's back,
//! file by file, so a copy kept there loses files and reads as the
//! employee's deletions:
//! - macOS's `tmp_cleaner` deletes files in `/private/tmp` untouched for
//!   three days, and `dirhelper` prunes `$TMPDIR` the same way;
//! - the system may purge `~/Library/Caches` when the disk runs low;
//! - `systemd-tmpfiles` ages out `/tmp` and `/var/tmp` on Linux, where
//!   `/tmp` is often memory that a restart empties;
//! - Storage Sense empties `%TEMP%` on Windows.
//!
//! The folder is made private (mode 0700) and refused unless it is a folder
//! of this user's own that nobody else can reach. It is checked again every
//! time it is asked for, and made again (private) when it is gone.
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

/// The root, made private to this user ([`root`]).
fn choose() -> Result<PathBuf, String> {
    let dir = root().ok_or("no local data folder for working copies (no home folder)")?;
    private_dir(&dir, current_uid()).map_err(|e| format!("the working-copy root is refused: {e}"))?;
    Ok(dir)
}

/// `nebo-copies` in the user's own local data folder, which no system
/// cleaner prunes (see the module docs).
fn root() -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| d.join("nebo-copies"))
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

    /// The data-loss review (10-23): macOS's `tmp_cleaner` deleted files of
    /// copies kept in `/private/tmp`, and a landing read them as the
    /// employee's deletions. The root is in the user's local data folder,
    /// outside every temporary and cache folder the system prunes, and
    /// outside Nebo's own folder.
    #[test]
    fn the_root_is_where_no_system_cleaner_prunes() {
        let r = root().unwrap();
        assert_eq!(r, dirs::data_local_dir().unwrap().join("nebo-copies"));
        let pruned = [Some(std::env::temp_dir()), Some(PathBuf::from("/tmp")), Some(PathBuf::from("/private/tmp")), Some(PathBuf::from("/var/tmp")), dirs::cache_dir()];
        for p in pruned.into_iter().flatten() {
            let canon = |x: &Path| std::fs::canonicalize(x).unwrap_or_else(|_| x.to_path_buf());
            assert!(!r.starts_with(&p) && !canon(&r).starts_with(canon(&p)), "{} is under {}", r.display(), p.display());
        }
        assert!(!r.starts_with(crate::defaults::default_data_dir().unwrap()), "never inside Nebo's own folder");
    }

    #[test]
    fn the_folder_name_is_stable() {
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
    }
}
