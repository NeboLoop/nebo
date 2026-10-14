//! Give back the page cache a bulk write leaves behind.
//!
//! Extracting a plugin or unpacking a restored bot writes hundreds of
//! megabytes that nothing reads again soon, and Linux keeps every page of it
//! cached. On a Nebo Cloud bot that cache is charged to the pod's memory
//! limit and, inside its VM, to the host: an idle bot measured 336 MB of page
//! cache beside a 237 MB server (2026-10-13). [`release`] writes a tree's
//! dirty pages out and tells the kernel the clean ones are not needed; what is
//! read later is read from disk again.
//!
//! Linux only. Elsewhere it does nothing: macOS has no `posix_fadvise`, and a
//! desktop's page cache is the owner's machine doing its job.

use std::path::Path;

/// Write `root` (a file, or a directory and everything under it) to disk and
/// drop its pages from the page cache. Best effort and blocking: a file that
/// cannot be opened keeps its pages, and nothing is reported. Symlinks are
/// not followed.
pub fn release(root: &Path) {
    #[cfg(target_os = "linux")]
    linux::release(root);
    #[cfg(not(target_os = "linux"))]
    let _ = root;
}

#[cfg(target_os = "linux")]
mod linux {
    use std::fs::File;
    use std::os::unix::io::AsRawFd;
    use std::path::Path;

    pub(super) fn release(root: &Path) {
        let Ok(meta) = std::fs::symlink_metadata(root) else {
            return;
        };
        // One syncfs writes every dirty page on the filesystem out: a page
        // still dirty is not dropped, and an fsync per file costs a journal
        // commit per file on a restore of thousands.
        if let Ok(f) = File::open(root) {
            // SAFETY: a valid descriptor, owned by `f` for the call.
            unsafe { libc::syncfs(f.as_raw_fd()) };
        }
        if meta.is_dir() {
            walk(root);
        } else if meta.is_file() {
            drop_file(root);
        }
    }

    fn walk(dir: &Path) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in rd.flatten() {
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if ft.is_dir() {
                walk(&entry.path());
            } else if ft.is_file() {
                drop_file(&entry.path());
            }
        }
    }

    fn drop_file(path: &Path) {
        if let Ok(f) = File::open(path) {
            // SAFETY: a valid descriptor, owned by `f` for the call.
            unsafe { libc::posix_fadvise(f.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tree, a single file, and a path that is not there: none fails, and
    /// every byte is still on disk afterwards.
    #[test]
    fn release_leaves_every_byte_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("plugin/1.0.0/skills");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), b"# a skill").unwrap();
        std::fs::write(tmp.path().join("plugin/1.0.0/bin"), vec![7u8; 1 << 20]).unwrap();
        release(&tmp.path().join("plugin"));
        release(&tmp.path().join("plugin/1.0.0/bin"));
        release(&tmp.path().join("absent"));
        assert_eq!(std::fs::read(dir.join("SKILL.md")).unwrap(), b"# a skill");
        assert_eq!(std::fs::read(tmp.path().join("plugin/1.0.0/bin")).unwrap(), vec![7u8; 1 << 20]);
    }
}
