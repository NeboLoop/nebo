//! The review lenses a change gets, and whether it is small.
//!
//! Chosen by the same judgment for every change, from what it touches: its
//! paths and the lines it adds or removes. Correctness and regressions is
//! always one; security when input, auth, the network or secrets are
//! touched; billing when money is; data when the schema or stored data is;
//! performance when hot or concurrent paths are; the owner's experience when
//! what people see is. Code decides, never the model, so the same change
//! always gets the same reviewers.

use serde::{Deserialize, Serialize};

use super::shadow::ChangeSet;

/// One reviewer's single concern.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lens {
    Correctness,
    Security,
    Billing,
    Data,
    Performance,
    Experience,
}

impl Lens {
    pub fn as_str(self) -> &'static str {
        match self {
            Lens::Correctness => "correctness",
            Lens::Security => "security",
            Lens::Billing => "billing",
            Lens::Data => "data",
            Lens::Performance => "performance",
            Lens::Experience => "experience",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "correctness" => Lens::Correctness,
            "security" => Lens::Security,
            "billing" => Lens::Billing,
            "data" => Lens::Data,
            "performance" => Lens::Performance,
            "experience" => Lens::Experience,
            _ => return None,
        })
    }

    /// What the reviewer of this lens hunts for: its charter in the brief.
    pub fn charter(self) -> &'static str {
        match self {
            Lens::Correctness => {
                "Correctness and regressions. Does the change do what the request asks, completely? Find logic \
                 errors, wrong edge cases (empty, zero, one, many, None, errors, concurrency, restarts), broken \
                 callers of anything whose signature or behaviour changed, existing behaviour removed or altered \
                 without being asked, and tests that pass without testing the change."
            }
            Lens::Security => {
                "Security. Find untrusted input reaching a shell, a query, a path, a template or a redirect \
                 without checks; authentication or permission checks missing, bypassable or weakened; secrets \
                 logged, returned, stored in plain text or sent where they shouldn't go; network calls to places \
                 a caller controls; anything that widens what a stranger can do."
            }
            Lens::Billing => {
                "Money. Find any path that charges the wrong amount, charges twice, charges nothing, loses a \
                 charge on an error or retry, rounds wrongly, mixes units (cents and microcents, dollars and \
                 cents), or lets usage run past what was paid for. Work the numbers through with concrete values."
            }
            Lens::Data => {
                "Data and migrations. Find migrations that lose or corrupt existing rows, can't run twice or on a \
                 database that already has the data, break older code still running, or have no way down; \
                 queries that miss an index on a large table; writes that can leave stored data half-changed."
            }
            Lens::Performance => {
                "Performance. Find work on a hot path that grows with data (a query or file read in a loop, a \
                 whole-table scan, an unbounded list), blocking calls in async code, locks held across awaits, \
                 missing limits on fan-out, and memory that grows without bound."
            }
            Lens::Experience => {
                "What people see. Find screens or messages that are wrong, confusing, untranslated, unreadable at \
                 a phone's width, inaccessible, or that show a state that isn't true (done when it failed, a \
                 spinner forever, an error that names internals)."
            }
        }
    }
}

/// How big and risky a change is: whether it gets the understand-and-plan
/// pass before building, and the lenses it is reviewed through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assessment {
    pub lenses: Vec<Lens>,
    /// Few files, few lines, and nothing risky touched.
    pub small: bool,
}

/// The most files a small change touches.
pub const SMALL_FILES: usize = 3;
/// The most lines a small change adds and removes.
pub const SMALL_LINES: u32 = 150;

const SECURITY_WORDS: &[&str] = &[
    "auth",
    "token",
    "secret",
    "password",
    "passwd",
    "credential",
    "apikey",
    "api_key",
    "jwt",
    "oauth",
    "session",
    "cookie",
    "cors",
    "csrf",
    "permission",
    "grant",
    "encrypt",
    "decrypt",
    "signature",
    "verify_sig",
    "hmac",
    "keychain",
    "keyring",
    "sanitize",
    "escape",
    "shell",
    "exec(",
    "command::new",
    "spawn(",
    "subprocess",
    "eval(",
    "http://",
    "https://",
    "reqwest",
    "fetch(",
    "axios",
    "websocket",
    "socket",
    "listen(",
    "bind(",
    "redirect",
    "upload",
    "deserialize",
    "from_str",
    "sql",
    "query(",
];

const BILLING_WORDS: &[&str] = &[
    "billing",
    "price",
    "pricing",
    "charge",
    "invoice",
    "payment",
    "stripe",
    "credit",
    "balance",
    "ledger",
    "cents",
    "microcents",
    "microdollars",
    "subscription",
    "refund",
    "wallet",
    "spend",
    "checkout",
    "plan_",
];

const DATA_WORDS: &[&str] = &[
    "create table",
    "alter table",
    "drop table",
    "drop column",
    "add column",
    "create index",
    "migration",
    "schema",
];

const PERFORMANCE_WORDS: &[&str] = &[
    "mutex",
    "rwlock",
    "lock()",
    "spawn_blocking",
    "tokio::spawn",
    "thread::spawn",
    "semaphore",
    "cache",
    "pool",
    "concurrency",
    "parallel",
    "rayon",
    "for_each",
    "collect::<vec",
    "clone()",
    "loop {",
    "while ",
];

const EXPERIENCE_EXTENSIONS: &[&str] = &[
    ".svelte",
    ".tsx",
    ".jsx",
    ".vue",
    ".css",
    ".scss",
    ".html",
    ".dart",
    ".swift",
    ".kt",
    ".xib",
    ".storyboard",
];

/// The lenses for `change`, correctness first, and whether it is small.
pub fn assess(change: &ChangeSet) -> Assessment {
    let paths: Vec<String> = change
        .files
        .iter()
        .map(|f| f.path.to_ascii_lowercase())
        .collect();
    let touched: String = changed_lines(&change.patch).to_ascii_lowercase();
    let any_path = |words: &[&str]| paths.iter().any(|p| words.iter().any(|w| p.contains(w)));
    let any_line = |words: &[&str]| words.iter().any(|w| touched.contains(w));

    let mut lenses = vec![Lens::Correctness];
    if any_path(SECURITY_WORDS)
        || any_line(SECURITY_WORDS)
        || paths
            .iter()
            .any(|p| p.contains("middleware") || p.contains("handler"))
    {
        lenses.push(Lens::Security);
    }
    if any_path(BILLING_WORDS) || any_line(BILLING_WORDS) {
        lenses.push(Lens::Billing);
    }
    if paths
        .iter()
        .any(|p| p.contains("migration") || p.ends_with(".sql"))
        || any_line(DATA_WORDS)
    {
        lenses.push(Lens::Data);
    }
    if any_line(PERFORMANCE_WORDS) {
        lenses.push(Lens::Performance);
    }
    if paths.iter().any(|p| {
        EXPERIENCE_EXTENSIONS.iter().any(|e| p.ends_with(e))
            || p.contains("/i18n/")
            || p.contains("locales")
    }) {
        lenses.push(Lens::Experience);
    }
    let risky = lenses
        .iter()
        .any(|l| matches!(l, Lens::Security | Lens::Billing | Lens::Data));
    let small =
        !risky && change.files.len() <= SMALL_FILES && change.lines_changed() <= SMALL_LINES;
    Assessment { lenses, small }
}

/// The lines a patch adds or removes, without its headers.
fn changed_lines(patch: &str) -> String {
    patch
        .lines()
        .filter(|l| {
            (l.starts_with('+') || l.starts_with('-'))
                && !l.starts_with("+++")
                && !l.starts_with("---")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::verify::shadow::FileChange;

    fn change(paths: &[&str], patch: &str) -> ChangeSet {
        ChangeSet {
            files: paths
                .iter()
                .map(|p| FileChange {
                    status: "modified".into(),
                    path: p.to_string(),
                    from: None,
                    added_lines: 2,
                    removed_lines: 1,
                })
                .collect(),
            patch: patch.to_string(),
            patch_cut: false,
        }
    }

    #[test]
    fn correctness_is_always_reviewed_and_a_small_plain_change_is_small() {
        let a = assess(&change(&["src/format.rs"], "+let n = a + b;\n-let n = a;"));
        assert_eq!(a.lenses, vec![Lens::Correctness]);
        assert!(a.small);
    }

    #[test]
    fn what_a_change_touches_picks_its_lenses() {
        let a = assess(&change(
            &[
                "crates/db/migrations/0300_x.sql",
                "src/billing.rs",
                "app/src/Page.svelte",
            ],
            "+ALTER TABLE runs ADD COLUMN cents INTEGER;\n+let token = req.header(\"Authorization\");",
        ));
        assert_eq!(
            a.lenses,
            vec![
                Lens::Correctness,
                Lens::Security,
                Lens::Billing,
                Lens::Data,
                Lens::Experience
            ]
        );
        assert!(!a.small, "money, auth and schema are never small");
    }

    #[test]
    fn many_files_are_not_small() {
        let a = assess(&change(&["a.rs", "b.rs", "c.rs", "d.rs"], "+x"));
        assert!(!a.small);
    }

    #[test]
    fn diff_headers_are_not_read_as_changed_lines() {
        assert_eq!(
            changed_lines("--- a/auth.rs\n+++ b/auth.rs\n+x\n context"),
            "+x"
        );
    }
}
