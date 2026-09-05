//! Per-run read ledger: the cross-method memory of what this run has already
//! observed on disk.
//!
//! Why: the 2026-08 spiral's second act had a model re-reading ONE file
//! twelve different ways — os read, `wc -l`, `sed -n`, grep, spill-file
//! reads — because nothing ever told it "you have seen this file before and
//! it has not changed". Every successful observation landed as fresh,
//! unconnected evidence, so distrust of any one method sent it to the next.
//! The ledger keys every observation by RESOLVED path regardless of method
//! and appends one factual line when a path is observed again.
//!
//! Contract (CODE_AUDITOR §11): notes STATE what happened — observation
//! count, line count, changed/unchanged — and never instruct, never block,
//! never measure anything that did not come back.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

/// One observed file.
#[derive(Debug, Clone)]
struct Entry {
    /// Successful observations of this path so far this run.
    count: usize,
    /// Line count at the most recent full read (None when the observation
    /// method — grep, wc — did not hand us the content).
    lines: Option<usize>,
    /// Content fingerprint at the most recent full read.
    fingerprint: Option<u64>,
}

/// Per-run ledger. Create one per run alongside the identical-call counters;
/// it must NOT outlive the run (a file legitimately changes between turns).
#[derive(Debug, Default)]
pub struct ReadLedger {
    entries: HashMap<PathBuf, Entry>,
}

/// Resolve a path the way the filesystem sees it, falling back to the raw
/// path when it does not exist (the ledger still matches on equal strings).
fn resolve(raw: &str) -> PathBuf {
    let p = Path::new(raw);
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// What the disk says about the file right now: size and modification time.
/// This is the evidence a model cannot argue with when a read comes back the
/// same as before. The 2026-09-03 poll read "unchanged" as "the system is
/// caching my reads" and spent twenty steps trying to get around a cache that
/// did not exist; the same modification time three reads in a row settles it.
/// None when the path cannot be stat'ed; the note then carries no evidence
/// rather than a guess.
fn disk_evidence(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    let modified: chrono::DateTime<chrono::Local> = meta.modified().ok()?.into();
    Some(format!("{} bytes, last modified {}", meta.len(), modified.format("%H:%M:%S")))
}

fn clock_now() -> String {
    chrono::Local::now().format("%H:%M:%S").to_string()
}

fn fingerprint(content: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    content.hash(&mut h);
    h.finish()
}

/// What a run's reads added up to, for the owner's context line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub struct LedgerStats {
    /// Distinct files observed this run.
    pub files: usize,
    /// Files observed more than once.
    pub files_reread: usize,
    /// Observations beyond the first, summed over all files.
    pub redundant_observations: usize,
}

impl ReadLedger {
    pub fn stats(&self) -> LedgerStats {
        let mut s = LedgerStats::default();
        for e in self.entries.values() {
            s.files += 1;
            if e.count > 1 {
                s.files_reread += 1;
                s.redundant_observations += e.count - 1;
            }
        }
        s
    }

    /// Record a successful content-bearing read of `raw_path` and return the
    /// factual note to append to the result — None on first observation
    /// (nothing to say; the content speaks for itself).
    pub fn observe_read(&mut self, raw_path: &str, content: &str) -> Option<String> {
        let path = resolve(raw_path);
        let lines = content.lines().count();
        let fp = fingerprint(content);
        let evidence = disk_evidence(&path);
        let entry = self.entries.entry(path).or_insert(Entry {
            count: 0,
            lines: None,
            fingerprint: None,
        });
        entry.count += 1;
        let note = if entry.count > 1 {
            // No read count in the note. "observation #6 — 1 lines" was read as
            // "6 lines"; "read number 4" as "4 lines"; even "the fourth time you
            // have read it" became "4 lines" in a live run. A model that wants
            // a line count takes any number near one. The evidence that settles
            // a repeat read is the length, the disk's size and modification
            // time, and the time of this read; the count lives in the stats.
            let length = format!("{} {}", lines, if lines == 1 { "line" } else { "lines" });
            let evidence = evidence.map(|e| format!(", {e}")).unwrap_or_default();
            let sentence = match entry.fingerprint {
                Some(prev) if prev == fp => format!("content unchanged since your previous read: still {length}{evidence}"),
                Some(_) => format!("content CHANGED since your previous read: now {length}{evidence}"),
                None => format!("first full read of it this run: {length}{evidence}"),
            };
            Some(format!("\n\n(read ledger: {sentence}; read fresh from disk at {}.)", clock_now()))
        } else {
            None
        };
        entry.lines = Some(lines);
        entry.fingerprint = Some(fp);
        note
    }

    /// Record a successful non-content observation (shell `wc`, `stat`, a grep
    /// hit) of any ledgered path referenced by `command`, and return the note.
    /// Commands that mention no ledgered path return None — the ledger never
    /// speaks about files it has not seen.
    pub fn observe_command(&mut self, command: &str) -> Option<String> {
        // Tokens split on whitespace and common shell punctuation; a token
        // counts only if it resolves to a path already in the ledger.
        for token in command.split(|c: char| c.is_whitespace() || "\"'();|&<>".contains(c)) {
            if token.len() < 2 || !token.contains('/') {
                continue;
            }
            let path = resolve(token);
            if let Some(entry) = self.entries.get_mut(&path) {
                entry.count += 1;
                let lines = entry
                    .lines
                    .map(|l| format!("{} lines at the last full read", l))
                    .unwrap_or_else(|| "no full read yet this run".to_string());
                let evidence = disk_evidence(&path).map(|e| format!("; {e}")).unwrap_or_default();
                return Some(format!(
                    "\n\n(read ledger: this command touched {}, a file you have already looked at this run; {}{}; checked at {}.)",
                    path.display(),
                    lines,
                    evidence,
                    clock_now()
                ));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// First sight of a file says nothing — the content speaks for itself.
    /// The ledger only ever ADDS context, starting at observation #2.
    #[test]
    fn first_observation_is_silent() {
        let mut l = ReadLedger::default();
        assert!(l.observe_read("/tmp/nonexistent-ledger-test.py", "a\nb\n").is_none());
    }

    /// A repeat read of unchanged content states the count and "unchanged" —
    /// the fact that dissolves method-hopping distrust.
    #[test]
    fn repeat_read_states_count_and_unchanged() {
        let mut l = ReadLedger::default();
        l.observe_read("/tmp/ledger-a.py", "a\nb\nc\n");
        let note = l.observe_read("/tmp/ledger-a.py", "a\nb\nc\n").unwrap();
        assert!(note.contains("read ledger"), "{note}");
        assert!(note.contains("3 lines"), "{note}");
        assert!(note.contains("content unchanged"), "{note}");
    }

    /// Changed content must be stated as CHANGED — the ledger is an honest
    /// witness, not a reassurance machine (this doubles as external-edit
    /// detection for files the run already saw).
    #[test]
    fn changed_content_is_reported_as_changed() {
        let mut l = ReadLedger::default();
        l.observe_read("/tmp/ledger-b.py", "old\n");
        let note = l.observe_read("/tmp/ledger-b.py", "new\n").unwrap();
        assert!(note.contains("CHANGED"), "{note}");
    }

    /// A shell command that touches a ledgered path gets the cross-method
    /// note — `wc -l` on a file the run already read is the exact incident
    /// shape. Commands touching unknown paths stay silent.
    #[test]
    fn shell_commands_cross_reference_the_ledger() {
        let mut l = ReadLedger::default();
        l.observe_read("/tmp/ledger-c.py", "x\ny\n");
        let note = l.observe_command("wc -l /tmp/ledger-c.py").unwrap();
        assert!(note.contains("read ledger"), "{note}");
        assert!(note.contains("2 lines at the last full read"), "{note}");
        assert!(l.observe_command("wc -l /tmp/never-seen.py").is_none());
        assert!(l.observe_command("echo hello").is_none());
    }

    /// Different spellings of one real file (relative vs absolute, symlink)
    /// must land on ONE entry — that is the whole point of keying on the
    /// resolved path. Exercised with a real file + relative component.
    #[test]
    fn path_spellings_resolve_to_one_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("real.py");
        std::fs::write(&file, "a\n").unwrap();
        let dotted = tmp.path().join(".").join("real.py");
        let mut l = ReadLedger::default();
        assert!(l.observe_read(file.to_str().unwrap(), "a\n").is_none());
        let note = l.observe_read(dotted.to_str().unwrap(), "a\n").unwrap();
        assert!(note.contains("read ledger"), "{note}");
    }

    /// A repeat read carries the disk's own evidence, size and modification
    /// time, plus the time of this read: the facts that settle "is this a
    /// stale copy" without the note ever arguing about it.
    #[test]
    fn a_repeat_read_carries_disk_evidence_when_the_file_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("evidence.txt");
        std::fs::write(&file, "line one\n").unwrap();
        let path = file.to_str().unwrap();
        let mut l = ReadLedger::default();
        l.observe_read(path, "line one\n");
        let note = l.observe_read(path, "line one\n").unwrap();
        assert!(note.contains("read fresh from disk at"), "{note}");
        assert!(note.contains("9 bytes, last modified"), "{note}");
        assert!(note.contains("still 1 line, 9 bytes"), "length with its evidence: {note}");
        // No count anywhere in the note: every digit in it is the length, the
        // size, or a clock reading.
        assert!(!note.contains("second") && !note.contains("number 2") && !note.contains("#2"), "{note}");
        assert!(!note.to_lowercase().contains("cach"), "never names the wrong theory: {note}");
        let cmd = l.observe_command(&format!("wc -l {path}")).unwrap();
        assert!(cmd.contains("9 bytes, last modified"), "{cmd}");
        assert!(cmd.contains("checked at"), "{cmd}");

        // A path that is not on disk gets no evidence, not a guess.
        let mut m = ReadLedger::default();
        m.observe_read("/tmp/ledger-not-there.py", "a\n");
        let ghost = m.observe_read("/tmp/ledger-not-there.py", "a\n").unwrap();
        assert!(ghost.contains("read fresh from disk at"), "{ghost}");
        assert!(!ghost.contains("bytes"), "{ghost}");
    }
}

#[cfg(test)]
mod stats_tests {
    use super::*;

    /// The owner's context line counts files re-read and the reads beyond the first.
    #[test]
    fn context_stats_event_counts_re_reads_and_compactions() {
        let mut l = ReadLedger::default();
        assert_eq!(l.stats(), LedgerStats::default());
        l.observe_read("/tmp/a.rs", "one");
        l.observe_read("/tmp/b.rs", "two");
        assert_eq!(l.stats(), LedgerStats { files: 2, files_reread: 0, redundant_observations: 0 });
        l.observe_read("/tmp/a.rs", "one");
        l.observe_read("/tmp/a.rs", "one");
        l.observe_read("/tmp/b.rs", "two");
        assert_eq!(l.stats(), LedgerStats { files: 2, files_reread: 2, redundant_observations: 3 });
    }
}
