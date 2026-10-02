//! The log file, rotated by size.
//!
//! `nebo.log` used to be opened in append mode and never touched again; a
//! cloud bot's grew to 343 MB in eighteen days (a third of its disk). A log
//! that never rotates is a disk that eventually fills, so every process that
//! writes one goes through this ONE writer: 20 MB per file, the last five
//! kept, `nebo.log` → `nebo.log.1` → … → `nebo.log.5` → gone. Size, not time,
//! because a chatty loop can write a day's worth in a minute.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

const MAX_BYTES: u64 = 20 * 1024 * 1024;
const KEEP: usize = 5;

pub struct RotatingFile {
    path: PathBuf,
    file: File,
    written: u64,
    max_bytes: u64,
    keep: usize,
}

impl RotatingFile {
    /// Open `path` for appending, creating its directory. A file already over
    /// the limit (the 343 MB case) is rotated out before the first write.
    pub fn open(path: impl Into<PathBuf>) -> io::Result<Self> {
        Self::with_limits(path, MAX_BYTES, KEEP)
    }

    fn with_limits(path: impl Into<PathBuf>, max_bytes: u64, keep: usize) -> io::Result<Self> {
        let path = path.into();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = Self::append(&path)?;
        let written = file.metadata()?.len();
        let mut this = Self { path, file, written, max_bytes, keep };
        if this.written >= this.max_bytes {
            this.rotate()?;
        }
        Ok(this)
    }

    fn append(path: &Path) -> io::Result<File> {
        OpenOptions::new().create(true).append(true).open(path)
    }

    fn numbered(&self, n: usize) -> PathBuf {
        let mut p = self.path.clone().into_os_string();
        p.push(format!(".{n}"));
        PathBuf::from(p)
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.file.flush()?;
        for n in (1..self.keep).rev() {
            let (from, to) = (self.numbered(n), self.numbered(n + 1));
            if from.exists() {
                std::fs::rename(from, to)?;
            }
        }
        std::fs::rename(&self.path, self.numbered(1))?;
        self.file = Self::append(&self.path)?;
        self.written = 0;
        Ok(())
    }
}

impl Write for RotatingFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.written > 0 && self.written + buf.len() as u64 > self.max_bytes {
            self.rotate()?;
        }
        let n = self.file.write(buf)?;
        self.written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

/// `<data_dir>/logs/<name>`, rotated. None when there is no data dir to log
/// into (the terminal layer still runs).
pub fn log_file(name: &str) -> Option<RotatingFile> {
    let dir = crate::data_dir().ok()?.join("logs");
    purge_pre_redaction_logs(&dir);
    RotatingFile::open(dir.join(name)).ok()
}

/// Written once the logs from before redaction are gone.
const PURGED_MARKER: &str = ".pre-redaction-logs-purged";

/// Before 0.16.7 the server logged every WebSocket client frame raw, and the
/// `auth` frame carries the owner's session token, so `nebo.log` (and its
/// rotations, and the dev-build `neboai.log` traffic log) can hold it. On the
/// first start of a version that redacts, those files are DELETED — not
/// redacted in place: deleting is certain, a pattern pass over 120 MB is not.
/// Whoever creates the marker does the purge, so it runs exactly once even
/// when two processes start together.
fn purge_pre_redaction_logs(dir: &Path) {
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let first = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dir.join(PURGED_MARKER))
        .is_ok();
    if !first {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == "nebo.log" || name.starts_with("nebo.log.") || name == "neboai.log" {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotates_by_size_and_keeps_the_last_few() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("logs").join("nebo.log");
        let mut f = RotatingFile::with_limits(&path, 100, 3).unwrap();
        for i in 0..40 {
            writeln!(f, "line {i:03} is exactly twenty-eight bytes").unwrap();
        }
        // 40 lines × ~40 B = ~1.6 kB over a 100 B limit: many rotations, only
        // the live file and .1 ..= .3 survive, nothing beyond.
        assert!(path.exists());
        for n in 1..=3 {
            assert!(f.numbered(n).exists(), "nebo.log.{n} kept");
        }
        assert!(!f.numbered(4).exists(), "nebo.log.4 must be gone");
        assert!(f.written <= 100);

        // A file already over the limit is rotated out on open.
        drop(f);
        std::fs::write(&path, vec![b'x'; 500]).unwrap();
        let f = RotatingFile::with_limits(&path, 100, 3).unwrap();
        assert_eq!(f.written, 0);
        assert_eq!(std::fs::metadata(f.numbered(1)).unwrap().len(), 500);
    }

    #[test]
    fn logs_from_before_redaction_are_deleted_once() {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        let old = ["nebo.log", "nebo.log.1", "nebo.log.5", "neboai.log"];
        for f in old {
            std::fs::write(logs.join(f), "ws client message: fake-token-not-real").unwrap();
        }
        let kept = ["update.log", "chromium.log", "nebo-crash.log"];
        for f in kept {
            std::fs::write(logs.join(f), "kept").unwrap();
        }

        purge_pre_redaction_logs(&logs);
        for f in old {
            assert!(!logs.join(f).exists(), "{f} must be deleted");
        }
        for f in kept {
            assert!(logs.join(f).exists(), "{f} is not ours to delete");
        }
        assert!(logs.join(PURGED_MARKER).exists());

        // The next start leaves the new, redacted log alone.
        std::fs::write(logs.join("nebo.log"), "redacted").unwrap();
        purge_pre_redaction_logs(&logs);
        assert!(logs.join("nebo.log").exists(), "purge runs once");
    }
}
