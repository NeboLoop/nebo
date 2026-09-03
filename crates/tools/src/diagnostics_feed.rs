//! Passive cross-file diagnostics: what a language server published for
//! files the employee did NOT just touch (an edit in `a.rs` that breaks
//! `b.rs`), delivered once, with the reference's caps, at the top of the
//! next iteration. The touched file's own diagnostics ride on its write
//! result (`file_tool::syntax_note`); this feed is everything else.
//!
//! Caps and the dedup rule are copied from the reference so an agent that
//! has learned its rhythm sees the same one here: 10 per file, 30 in total,
//! a 500-file LRU of what was already delivered, errors first, a 4000-char
//! summary. Editing a file forgets what was delivered for it, so an error
//! the employee reintroduces is shown again.
use std::collections::{HashSet, VecDeque};
use std::sync::Mutex;

use crate::lsp::{Diag, FileDiags, Severity};

pub const MAX_DIAGNOSTICS_PER_FILE: usize = 10;
pub const MAX_TOTAL_DIAGNOSTICS: usize = 30;
pub const MAX_DELIVERED_FILES: usize = 500;
pub const MAX_SUMMARY_CHARS: usize = 4000;

/// Delivered (file → dedup keys), most recently used at the back.
#[derive(Default)]
pub struct DeliveredLedger {
    files: VecDeque<(String, HashSet<String>)>,
}

fn dedup_key(d: &Diag) -> String {
    format!("{:?}|{}|{}|{}", d.severity, d.line, d.col, d.message)
}

impl DeliveredLedger {
    /// Forget what was delivered for `path`: the file changed, so a
    /// diagnostic that comes back is news again.
    pub fn clear(&mut self, path: &str) {
        self.files.retain(|(p, _)| p != path);
    }

    fn entry(&mut self, path: &str) -> &mut HashSet<String> {
        if let Some(i) = self.files.iter().position(|(p, _)| p == path) {
            let e = self.files.remove(i).expect("position just found");
            self.files.push_back(e);
        } else {
            self.files.push_back((path.to_string(), HashSet::new()));
            while self.files.len() > MAX_DELIVERED_FILES {
                self.files.pop_front();
            }
        }
        &mut self.files.back_mut().expect("just pushed").1
    }

    /// Keep only diagnostics not yet delivered, mark them delivered, and
    /// render the summary. `None` when nothing is new.
    pub fn take_new(&mut self, reports: Vec<FileDiags>) -> Option<String> {
        let mut files: Vec<(String, String, Vec<Diag>)> = Vec::new();
        for r in reports {
            let seen = self.entry(&r.path);
            let mut fresh: Vec<Diag> =
                r.diagnostics.into_iter().filter(|d| seen.insert(dedup_key(d))).collect();
            if fresh.is_empty() {
                continue;
            }
            fresh.sort_by_key(|d| (d.severity, d.line, d.col));
            files.push((r.path, r.server, fresh));
        }
        if files.is_empty() {
            return None;
        }
        // Files with errors first, so the cap spends itself on what blocks.
        files.sort_by_key(|(_, _, d)| d.first().map(|x| x.severity).unwrap_or(Severity::Hint));
        Some(render(&files))
    }
}

fn render(files: &[(String, String, Vec<Diag>)]) -> String {
    let mut out = String::new();
    let mut total = 0usize;
    let mut omitted = 0usize;
    for (path, server, diags) in files {
        let room = MAX_TOTAL_DIAGNOSTICS.saturating_sub(total).min(MAX_DIAGNOSTICS_PER_FILE);
        if room == 0 {
            omitted += diags.len();
            continue;
        }
        out.push_str(&format!("{path} ({server}):\n"));
        for d in diags.iter().take(room) {
            let first_line = d.message.lines().next().unwrap_or("");
            out.push_str(&format!(
                "  line {}:{} [{}] {}\n",
                d.line,
                d.col,
                d.severity.label(),
                crate::truncate_str(first_line, 200)
            ));
        }
        total += diags.len().min(room);
        omitted += diags.len().saturating_sub(room);
    }
    if omitted > 0 {
        out.push_str(&format!("{omitted} more not shown.\n"));
    }
    if out.chars().count() > MAX_SUMMARY_CHARS {
        let head: String = out.chars().take(MAX_SUMMARY_CHARS).collect();
        out = format!("{head}…[truncated]");
    }
    format!(
        "<new-diagnostics>The following new diagnostic issues were detected:\n\n{}</new-diagnostics>",
        out.trim_end()
    )
}

static DELIVERED: Mutex<Option<DeliveredLedger>> = Mutex::new(None);

fn with_ledger<T>(f: impl FnOnce(&mut DeliveredLedger) -> T) -> T {
    let mut guard = DELIVERED.lock().unwrap_or_else(|p| p.into_inner());
    f(guard.get_or_insert_with(DeliveredLedger::default))
}

/// The file changed: forget what was delivered for it.
pub fn clear_delivered(path: &str) {
    with_ledger(|l| l.clear(path));
}

/// New cross-file diagnostics since the last call, rendered, or `None`.
pub fn take_new(reports: Vec<FileDiags>) -> Option<String> {
    if reports.is_empty() {
        return None;
    }
    with_ledger(|l| l.take_new(reports))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(line: u32, sev: Severity, msg: &str) -> Diag {
        Diag { line, col: 1, severity: sev, message: msg.to_string() }
    }
    fn file(path: &str, diags: Vec<Diag>) -> FileDiags {
        FileDiags { path: path.into(), server: "rust-analyzer".into(), diagnostics: diags }
    }

    /// The contract in one test: a diagnostic is delivered once, an edit to
    /// its file makes it news again, other files are untouched by that edit.
    #[test]
    fn delivered_once_until_the_file_is_edited() {
        let mut l = DeliveredLedger::default();
        let err = || d(7, Severity::Error, "mismatched types");
        let first = l.take_new(vec![file("/p/b.rs", vec![err()])]).expect("first delivery");
        assert!(first.starts_with("<new-diagnostics>The following new diagnostic issues were detected:"), "{first}");
        assert!(first.contains("/p/b.rs (rust-analyzer):\n  line 7:1 [error] mismatched types"), "{first}");
        assert!(first.ends_with("</new-diagnostics>"), "{first}");
        assert!(l.take_new(vec![file("/p/b.rs", vec![err()])]).is_none(), "same diagnostic is not re-billed");
        l.clear("/p/c.rs");
        assert!(l.take_new(vec![file("/p/b.rs", vec![err()])]).is_none(), "clearing another file changes nothing");
        l.clear("/p/b.rs");
        assert!(l.take_new(vec![file("/p/b.rs", vec![err()])]).is_some(), "an edit makes it news again");
    }

    /// 10 per file, 30 in total, errors first, and the omission is stated.
    #[test]
    fn caps_are_the_references_and_errors_come_first() {
        let mut l = DeliveredLedger::default();
        let warnings: Vec<Diag> = (1..=12).map(|i| d(i, Severity::Warning, "unused")).collect();
        let errors: Vec<Diag> = (1..=12).map(|i| d(i, Severity::Error, "broken")).collect();
        let more: Vec<Diag> = (1..=25).map(|i| d(i, Severity::Warning, "shadowed")).collect();
        let s = l
            .take_new(vec![file("/p/w.rs", warnings), file("/p/e.rs", errors), file("/p/m.rs", more)])
            .unwrap();
        let e_pos = s.find("/p/e.rs").unwrap();
        let w_pos = s.find("/p/w.rs").unwrap();
        assert!(e_pos < w_pos, "the file with errors leads: {s}");
        assert_eq!(s.matches("[error] broken").count(), MAX_DIAGNOSTICS_PER_FILE, "{s}");
        let shown = s.matches("\n  line ").count();
        assert_eq!(shown, MAX_TOTAL_DIAGNOSTICS, "{s}");
        // 12 + 12 + 25 = 49 offered, 30 shown
        assert!(s.contains("19 more not shown."), "{s}");
    }

    #[test]
    fn summary_is_truncated_at_the_cap_and_the_ledger_is_an_lru() {
        let mut l = DeliveredLedger::default();
        let long = "x".repeat(150);
        let reports: Vec<FileDiags> = (0..40)
            .map(|i| file(&format!("/p/f{i}.rs"), vec![d(1, Severity::Error, &long)]))
            .collect();
        let s = l.take_new(reports).unwrap();
        assert!(s.contains("…[truncated]"), "{s}");
        // the cap is on the summary; the <new-diagnostics> wrapper sits outside it
        assert!(s.chars().count() < MAX_SUMMARY_CHARS + 200, "{}", s.len());
        // LRU: after 500 more files, the first ones are forgotten and deliver again.
        for i in 0..MAX_DELIVERED_FILES {
            l.take_new(vec![file(&format!("/q/{i}.rs"), vec![d(1, Severity::Error, "e")])]);
        }
        assert!(l.take_new(vec![file("/p/f0.rs", vec![d(1, Severity::Error, &long)])]).is_some());
        assert!(l.files.len() <= MAX_DELIVERED_FILES);
    }
}
