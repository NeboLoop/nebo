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
        let entry = self.entries.entry(path).or_insert(Entry {
            count: 0,
            lines: None,
            fingerprint: None,
        });
        entry.count += 1;
        let note = if entry.count > 1 {
            let change = match entry.fingerprint {
                Some(prev) if prev == fp => "content unchanged since the previous read",
                Some(_) => "content CHANGED since the previous read",
                None => "first full read of it this run",
            };
            Some(format!(
                "\n\n(read ledger: observation #{} of this file this run — {} lines, {})",
                entry.count, lines, change
            ))
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
                return Some(format!(
                    "\n\n(read ledger: this command touched {} — observation #{} of that file this run; {})",
                    path.display(),
                    entry.count,
                    lines
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
        assert!(note.contains("observation #2"), "{note}");
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
        assert!(note.contains("observation #2"), "{note}");
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
        assert!(note.contains("observation #2"), "{note}");
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
