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
//! - **Windows console windows.** ~160 `Command::new` sites; a handful set
//!   `CREATE_NO_WINDOW`. Every other child the desktop app started — git,
//!   powershell, `where`, MCP servers, sidecars — opened a console window, so
//!   opening Nebo on Windows opened a bunch of terminals.
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

/// Is this module declared `#[cfg(test)]` by the module above it?
///
/// A `tests/` directory is not the only test code in the tree. A module can be
/// declared `#[cfg(test)] mod name;` and live under `src/` beside the thing it
/// proves — `crates/server/src/staffed_proof/` is exactly that, a suite of
/// scenarios that boots the real server and is compiled only by `cargo test`.
/// These gates police what SHIPS: a `Client::builder()` that reaches no
/// released binary is in no outage's path and must not spend the ratchet.
fn declared_cfg_test(module: &Path) -> bool {
    let Some(name) = module.file_stem().and_then(|n| n.to_str()) else {
        return false;
    };
    let Some(parent) = module.parent() else {
        return false;
    };
    let decl = format!("mod {name};");
    for owner in ["mod.rs", "lib.rs", "main.rs"] {
        let Ok(text) = std::fs::read_to_string(parent.join(owner)) else {
            continue;
        };
        let mut cfg_test_pending = false;
        for line in text.lines() {
            let t = line.trim();
            if t.is_empty() || t.starts_with("//") {
                continue;
            }
            if t.ends_with(&decl) && (cfg_test_pending || t.contains("#[cfg(test)]")) {
                return true;
            }
            cfg_test_pending = t.contains("#[cfg(test)]");
        }
    }
    false
}

/// Every `.rs` file under `crates/` and `src-tauri/src/` that ships — no
/// `tests/` directory, and no module the crate declares `#[cfg(test)]`.
fn source_files() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name == "target" || name == "tests" || declared_cfg_test(&path) {
                    continue;
                }
                walk(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs")
                && !declared_cfg_test(&path)
            {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(&repo_root().join("crates"), &mut out);
    walk(&repo_root().join("src-tauri").join("src"), &mut out);
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
    const PLUGIN_LAUNCH_FILES: &[&str] = &[
        "tools/src/plugin_tool.rs",
        // Channel/watch bridges — migrated to spawn_streaming; a raw Command
        // here would silently drop the shared env assembly and guard wiring.
        "agent/src/agent_worker.rs",
    ];

    let in_launch_files = |p: &Path| {
        let s = p.to_string_lossy().replace('\\', "/");
        PLUGIN_LAUNCH_FILES.iter().any(|f| s.ends_with(f))
    };
    let mut offenders = count_matches("Command::new", in_launch_files);
    offenders.extend(count_matches("command::new", in_launch_files));

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
/// Shipped code only: `source_files` skips `#[cfg(test)]` modules, so a client
/// a proof harness builds against 127.0.0.1 does not spend the ratchet. The
/// gate is about certificates a released binary has to verify.
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

/// A `tokio::time::timeout` around `cmd.output()` without `kill_on_drop` leaks
/// the child process FOREVER when the timeout fires — the dropped future
/// abandons the child, it reparents to launchd/init, and never exits.
///
/// This exact pattern accumulated 330 orphaned plugin processes on a customer
/// box (plugin_tool), leaked a `dns-sd -B` on every voice printer query
/// (shell_tool), and was found a THIRD time in execute_tool during release
/// review. Three independent authors wrote the same leak; a fourth will too.
#[test]
fn timed_process_waits_always_kill_on_drop() {
    let offenders: Vec<(PathBuf, usize)> = source_files()
        .into_iter()
        .filter_map(|file| {
            let text = std::fs::read_to_string(&file).ok()?;
            let has_timeout = text.contains("tokio::time::timeout");
            let output_waits = text
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .filter(|l| l.contains(".output()).await") || l.contains("cmd.output()"))
                .count();
            let has_kill = text.contains("kill_on_drop");
            (has_timeout && output_waits > 0 && !has_kill).then_some((file, output_waits))
        })
        .collect();

    assert!(
        offenders.is_empty(),
        "timeout(_, cmd.output()) without kill_on_drop — the dropped future \
         abandons the child and it runs forever. Set cmd.kill_on_drop(true) \
         before the wait.\nOffending files: {offenders:#?}"
    );
}

/// Every child process is made by `command::new` (`crates/command`), which
/// sets the Windows creation flags from the `Console` its caller names.
///
/// A raw `Command::new` starts a console program with no flags, and from the
/// desktop app — which has no console — Windows gives it a window of its own.
/// That is how opening Nebo on Windows opened a bunch of terminals: a handful of
/// ~160 sites remembered `CREATE_NO_WINDOW`. `creation_flags` outside the
/// constructor is a second place deciding the same thing. Test code (`tests/`
/// and `#[cfg(test)]` module files) may start what it likes.
#[test]
fn child_processes_are_made_by_the_one_constructor() {
    const CONSTRUCTOR: &str = "crates/command/src/lib.rs";

    /// `needle` where it starts a path segment: `Command::new(` but not
    /// `CommandBuilder::new(` or a `FooCommand::new(`.
    fn starts_segment(line: &str, needle: &str) -> bool {
        line.match_indices(needle).any(|(i, _)| {
            !line[..i].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_')
        })
    }

    let mut offenders = Vec::new();
    for file in source_files() {
        let path = file.to_string_lossy().replace('\\', "/");
        if path.ends_with(CONSTRUCTOR) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        for (n, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            if starts_segment(line, "Command::new(") || line.contains("creation_flags(") {
                offenders.push(format!("{path}:{}: {}", n + 1, line.trim()));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "Start a child with `command::new(program, command::Console::Hidden)` (or \
         `Console::Detached` / `Console::Inherit`, see crates/command), never \
         `Command::new` or `creation_flags` — on Windows a child made any other way \
         opens a console window.\n{}",
        offenders.join("\n")
    );
}
