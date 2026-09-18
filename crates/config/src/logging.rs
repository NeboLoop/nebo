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
    let dir = crate::data_dir().ok()?;
    RotatingFile::open(dir.join("logs").join(name)).ok()
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
}
