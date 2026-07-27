//! Drift gates for CODE_AUDITOR §8.1 — "no competing pathways".
//!
//! These are source-tree assertions, not behavior tests. They exist because the
//! two worst outages this codebase has had both came from the SAME failure: a
//! capability that had one canonical implementation, and callers that quietly
//! grew their own instead.
//!
//! - **Plugin launching.** Eight independent `Command::new` sites. Only one had
//!   all three of kill_on_drop, a timeout, and child-guard registration. The one
//!   that lacked kill_on_drop leaked a process on every timed-out call — 330
//!   orphans on a customer box in 30 hours, until it ran out of file descriptors
//!   and every outbound request began failing.
//! - **HTTP clients.** ~30 `Client::builder()` sites, so TLS configuration could
//!   not be fixed in one place. When macOS securityd wedged on that same box, the
//!   OS returned an EMPTY trust store, every certificate was rejected, and there
//!   was no single place to add a fallback.
//!
//! A code review cannot catch the 31st caller. A failing build can.

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop(); // crates/
    p.pop(); // repo root
    p
}

/// Every `.rs` file under `crates/`, excluding test files.
fn source_files() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name == "target" || name == "tests" {
                    continue;
                }
                walk(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(&repo_root().join("crates"), &mut out);
    out
}

fn count_matches(needle: &str, filter: impl Fn(&Path) -> bool) -> Vec<(PathBuf, usize)> {
    let mut hits = Vec::new();
    for file in source_files() {
        if !filter(&file) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let n = text
            .lines()
            .filter(|l| {
                let t = l.trim_start();
                !t.starts_with("//") && l.contains(needle)
            })
            .count();
        if n > 0 {
            hits.push((file, n));
        }
    }
    hits
}

/// Plugin binaries are launched through `PluginRuntime` and nowhere else.
///
/// `PluginRuntime` guarantees kill_on_drop, a mandatory timeout, child-guard
/// registration and one env assembly. A raw `Command::new` in these files means
/// a caller has opted out of all four, silently.
#[test]
fn plugin_launches_go_through_plugin_runtime() {
    // Files whose job is launching PLUGIN binaries. Launching other programs
    // (osascript, powershell, git) is not what this gate is about.
    const PLUGIN_LAUNCH_FILES: &[&str] = &["tools/src/plugin_tool.rs"];

    let offenders = count_matches("Command::new", |p| {
        let s = p.to_string_lossy().replace('\\', "/");
        PLUGIN_LAUNCH_FILES.iter().any(|f| s.ends_with(f))
    });

    assert!(
        offenders.is_empty(),
        "Plugin launch must go through PluginRuntime (run_capture / run_capture_args / \
         spawn_streaming), never a hand-rolled Command. A raw Command silently drops \
         kill_on_drop — every timed-out call then leaks a process forever.\n\
         Offending files: {offenders:#?}"
    );
}

/// HTTP client construction is a ratchet: it may shrink, never grow.
///
/// The target is ONE factory that owns the TLS root store (native certs with a
/// bundled fallback, and a loud warning when the OS returns nothing). Until that
/// migration lands, this gate stops the count from climbing — a new
/// `Client::builder()` is a new place the trust-store fix will not reach.
///
/// See docs/plans/nebo-tls-rustls-migration.md. Lower this number as sites are
/// migrated; it must never be raised.
#[test]
fn http_client_construction_does_not_spread() {
    const MAX_CLIENT_BUILDERS: usize = 17;

    let hits = count_matches("Client::builder()", |_| true);
    let total: usize = hits.iter().map(|(_, n)| n).sum();

    assert!(
        total <= MAX_CLIENT_BUILDERS,
        "HTTP client construction grew to {total} sites (ceiling {MAX_CLIENT_BUILDERS}, the count when this gate was written).\n\
         Every site is a place the TLS trust-store fallback will not reach — that is \
         how a wedged securityd took a customer offline for two days.\n\
         Use the shared factory instead of building a client here.\n\
         Sites: {hits:#?}"
    );
}
