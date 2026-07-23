//! Global child-process registry — guarantees plugin/sidecar processes die
//! when Nebo dies, even on SIGTERM/SIGKILL/`pkill` where Rust destructors
//! don't run.
//!
//! ## Why this exists
//!
//! Tokio's `Command::kill_on_drop(true)` only fires when the `Child` handle
//! is *dropped* — that requires:
//! - Normal program exit (Drop on statics runs)
//! - Panic unwind (Drop runs while stack unwinds)
//! - Task cancellation (Drop on the owning future runs)
//!
//! It does NOT run when:
//! - Process receives `SIGTERM` (`pkill nebo`, hot-reload restart, `kill <pid>`)
//! - Process receives `SIGKILL` (`pkill -9 nebo`, OS OOM kill)
//! - Process aborts on stack overflow / abort()
//!
//! Without a signal handler, every Nebo restart leaks the plugin processes it
//! spawned — they get reparented to PID 1 (init) and keep running with no
//! parent to deliver replies to. On a developer's machine after a few
//! hot-reloads the workstation accumulates orphans. On a customer's machine,
//! crash + restart silently doubles the number of slack bridges holding the
//! same WebSocket, each posting placeholders to the same channel.
//!
//! ## What this provides
//!
//! [`register_child`] / [`unregister_child`] — every long-lived child spawner
//! calls these around the child's lifetime.
//!
//! [`install_signal_handler`] — call once at startup. Installs SIGTERM, SIGINT
//! and SIGHUP handlers that:
//!   1. SIGTERM all registered children (graceful shutdown signal)
//!   2. Wait briefly for them to exit cleanly
//!   3. SIGKILL any that didn't exit
//!   4. Exit Nebo cleanly so the runtime can run Drop on remaining state
//!
//! [`kill_all_now`] — synchronous best-effort cleanup. Call before any code
//! path that intends to exit the process.
//!
//! SIGKILL of the Nebo process itself can't be intercepted — that's an OS
//! constraint, no signal handler can run. For that pathological case we'd
//! need OS-level facilities (`PR_SET_PDEATHSIG` on Linux, Job Objects on
//! Windows, `kqueue NOTE_EXIT` on macOS), which require per-platform code
//! and ideally plugin-side cooperation. That's intentionally out of scope
//! here — `SIGTERM` covers `pkill`, hot reload, and graceful shutdown,
//! which is the orphan source we actually keep hitting.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
#[cfg(unix)]
use std::time::Duration;

#[cfg(unix)]
use tracing::{info, warn};

static REGISTRY: OnceLock<Mutex<HashSet<u32>>> = OnceLock::new();

fn registry() -> &'static Mutex<HashSet<u32>> {
    REGISTRY.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Register a child process PID for shutdown-time cleanup.
/// Safe to call from any thread. Idempotent.
pub fn register_child(pid: u32) {
    if pid == 0 {
        return;
    }
    if let Ok(mut g) = registry().lock() {
        g.insert(pid);
    }
}

/// Remove a child from the registry — call when the child exits naturally
/// so we don't try to kill a PID that's been recycled.
pub fn unregister_child(pid: u32) {
    if pid == 0 {
        return;
    }
    if let Ok(mut g) = registry().lock() {
        g.remove(&pid);
    }
}

/// Snapshot the current set of tracked PIDs (for diagnostics).
pub fn tracked_pids() -> Vec<u32> {
    match registry().lock() {
        Ok(g) => g.iter().copied().collect(),
        Err(_) => Vec::new(),
    }
}

/// Reap any process currently running the given binary that ISN'T tracked
/// by us. Call this BEFORE spawning a plugin child to guarantee Nebo owns
/// the lifecycle of every running instance of that binary.
///
/// Why this exists: plugins like the Slack bridge open a single WebSocket
/// to a remote service. If a prior Nebo crashed or was SIGKILL'd, its
/// plugin children were reparented to init and kept holding that socket.
/// The next Nebo spawns its OWN bridge, and now two bridges race for the
/// same inbound messages — each posting "_Thinking..._" placeholders,
/// only one of which can be answered. Reaping pre-spawn eliminates the race
/// at the source, regardless of how the previous Nebo died (SIGTERM,
/// SIGKILL, panic, crashed without unwinding).
///
/// Safe to call repeatedly. Won't kill children we currently track
/// (children of the running Nebo). Returns the count of processes killed.
/// Parse one `ps -o pid=,command=` line into `(pid, command)`. The command is
/// the FULL remainder after the pid — it must NOT be tokenized, because argv[0]
/// (the binary path) can contain spaces, e.g. the macOS path
/// `~/Library/Application Support/Nebo/...`. Tokenizing here is what made the
/// reaper silently match nothing on macOS and leak every watch process.
#[cfg(unix)]
fn parse_ps_line(line: &str) -> Option<(u32, &str)> {
    let line = line.trim_start();
    let (pid_str, rest) = line.split_once(char::is_whitespace)?;
    Some((pid_str.parse().ok()?, rest.trim_start()))
}

/// True if a `ps` command line was launched from `target` — argv[0] is exactly
/// the binary path, with or without trailing args (`<target>` or `<target> …`).
#[cfg(unix)]
fn command_is_for(command: &str, target: &str) -> bool {
    command == target
        || command
            .strip_prefix(target)
            .is_some_and(|r| r.starts_with(char::is_whitespace))
}

#[cfg(unix)]
pub fn reap_existing_for(binary_path: &std::path::Path) -> usize {
    use std::process::Command;

    let target = binary_path.to_string_lossy();
    if target.is_empty() {
        return 0;
    }

    let out = match Command::new("ps")
        .args(["-A", "-o", "pid=,command="])
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            warn!(error = %e, "reap_existing_for: ps invocation failed");
            return 0;
        }
    };
    let text = String::from_utf8_lossy(&out.stdout);

    let tracked: HashSet<u32> = match registry().lock() {
        Ok(g) => g.iter().copied().collect(),
        Err(_) => HashSet::new(),
    };

    let mut killed = 0usize;
    for line in text.lines() {
        let Some((pid, command)) = parse_ps_line(line) else {
            continue;
        };
        // Don't kill our own currently-tracked children.
        if tracked.contains(&pid) {
            continue;
        }
        if command_is_for(command, target.as_ref()) {
            info!(
                pid,
                command, "reap_existing_for: killing pre-existing process for binary"
            );
            unsafe {
                libc::kill(pid as i32, libc::SIGTERM);
            }
            killed += 1;
        }
    }

    if killed > 0 {
        // Brief grace, then SIGKILL holdouts.
        std::thread::sleep(Duration::from_millis(200));
        let out2 = Command::new("ps")
            .args(["-A", "-o", "pid=,command="])
            .output();
        if let Ok(o2) = out2 {
            let text2 = String::from_utf8_lossy(&o2.stdout);
            for line in text2.lines() {
                let Some((pid, command)) = parse_ps_line(line) else {
                    continue;
                };
                if tracked.contains(&pid) {
                    continue;
                }
                if command_is_for(command, target.as_ref()) {
                    unsafe {
                        libc::kill(pid as i32, libc::SIGKILL);
                    }
                }
            }
        }
    }

    killed
}

#[cfg(not(unix))]
pub fn reap_existing_for(_binary_path: &std::path::Path) -> usize {
    // Windows: relies on tokio's `kill_on_drop` + future Job Object work.
    0
}

/// SIGTERM every tracked child, wait briefly, then SIGKILL anything still alive.
/// Safe to call from a signal handler (uses only async-signal-safe syscalls).
///
/// Returns the number of children that received signals.
pub fn kill_all_now() -> usize {
    let pids: Vec<u32> = match registry().lock() {
        Ok(g) => g.iter().copied().collect(),
        Err(_) => return 0,
    };
    if pids.is_empty() {
        return 0;
    }

    // Phase 1: polite SIGTERM
    #[cfg(unix)]
    {
        for pid in &pids {
            unsafe {
                libc::kill(*pid as i32, libc::SIGTERM);
            }
        }
        // Brief grace period so children can flush + exit cleanly.
        std::thread::sleep(Duration::from_millis(300));
        // Phase 2: SIGKILL any holdouts
        for pid in &pids {
            unsafe {
                libc::kill(*pid as i32, libc::SIGKILL);
            }
        }
    }

    #[cfg(windows)]
    {
        // Windows doesn't have SIGTERM/SIGKILL the same way; on hot-reload
        // tauri kills nebo, and we have no clean signal to react to.
        // Job Objects are the right fix on Windows but require more setup.
        let _ = pids;
    }

    pids.len()
}

/// Install signal handlers (Unix only). Call once at server startup.
///
/// On SIGTERM/SIGINT/SIGHUP: kills all tracked children, then exits the
/// process with code 0.
#[cfg(unix)]
pub fn install_signal_handler() {
    use tokio::signal::unix::{SignalKind, signal};

    tokio::spawn(async {
        let mut term = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, "failed to install SIGTERM handler");
                return;
            }
        };
        let mut int = match signal(SignalKind::interrupt()) {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, "failed to install SIGINT handler");
                return;
            }
        };
        let mut hup = match signal(SignalKind::hangup()) {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, "failed to install SIGHUP handler");
                return;
            }
        };

        let sig = tokio::select! {
            _ = term.recv() => "SIGTERM",
            _ = int.recv() => "SIGINT",
            _ = hup.recv() => "SIGHUP",
        };
        let count = tracked_pids().len();
        info!(signal = sig, child_count = count, "shutdown signal received, killing children");
        kill_all_now();
        // Give logs a moment to flush before exit.
        std::thread::sleep(Duration::from_millis(50));
        std::process::exit(0);
    });
}

#[cfg(not(unix))]
pub fn install_signal_handler() {
    // No-op on non-Unix. See module docs for the Windows path (Job Objects).
}

/// Best-effort startup cleanup: scan for orphan plugin processes left over
/// from a prior crashed Nebo and kill them.
///
/// Heuristic: any process whose argv starts with a path under
/// `<data_dir>/user/plugins/` or `<data_dir>/user/agents/` AND has PPID == 1
/// (reparented to init) is an orphan from a previous Nebo run. Kill it.
///
/// Returns the number of orphans cleaned up.
pub fn cleanup_orphans_at_startup() -> usize {
    #[cfg(unix)]
    {
        use std::process::Command;

        // ps -A -o pid=,ppid=,command=  (portable across BSD/macOS/Linux)
        let out = match Command::new("ps")
            .args(["-A", "-o", "pid=,ppid=,command="])
            .output()
        {
            Ok(o) => o,
            Err(e) => {
                warn!(error = %e, "startup orphan scan: ps invocation failed");
                return 0;
            }
        };
        let text = String::from_utf8_lossy(&out.stdout);
        let data_dir = config::data_dir()
            .map(|d| d.to_string_lossy().into_owned())
            .unwrap_or_default();
        let plugins_prefix = format!("{}/user/plugins/", data_dir);
        let agents_prefix = format!("{}/user/agents/", data_dir);

        let mut killed = 0usize;
        for line in text.lines() {
            // `ps` columns are separated by runs of whitespace, so
            // `split_whitespace()` (which collapses runs) is correct here.
            // An earlier version used `splitn(3, char::is_whitespace)` and
            // silently failed because adjacent spaces produced empty parts
            // that `.parse::<u32>()` rejected — every line continued, every
            // orphan survived. Don't repeat that mistake.
            let mut parts = line.split_whitespace();
            let pid: u32 = match parts.next().and_then(|s| s.parse().ok()) {
                Some(p) => p,
                None => continue,
            };
            let ppid: u32 = match parts.next().and_then(|s| s.parse().ok()) {
                Some(p) => p,
                None => continue,
            };
            if ppid != 1 {
                continue;
            }
            // Next whitespace-delimited token is the executable path.
            // (The rest of argv is dropped — we only need to match the path.)
            let exe = match parts.next() {
                Some(e) => e,
                None => continue,
            };
            if exe.starts_with(&plugins_prefix) || exe.starts_with(&agents_prefix) {
                info!(orphan_pid = pid, exe = exe, "startup: killing orphan plugin process");
                unsafe {
                    libc::kill(pid as i32, libc::SIGKILL);
                }
                killed += 1;
            }
        }
        if killed > 0 {
            info!(orphans_killed = killed, "startup orphan cleanup complete");
        }
        killed
    }

    #[cfg(not(unix))]
    {
        0
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::{command_is_for, parse_ps_line};

    #[test]
    fn parses_pid_and_full_command_with_spaces() {
        // macOS path contains a space ("Application Support") — the command must
        // come back whole, not tokenized.
        let line = "  4242 /Users/x/Library/Application Support/Nebo/nebo/plugins/example-ops/0.1.0/example-ops watch --events";
        let (pid, cmd) = parse_ps_line(line).expect("parse");
        assert_eq!(pid, 4242);
        assert_eq!(
            cmd,
            "/Users/x/Library/Application Support/Nebo/nebo/plugins/example-ops/0.1.0/example-ops watch --events"
        );
    }

    #[test]
    fn matches_binary_with_space_in_path_and_args() {
        let target = "/Users/x/Library/Application Support/Nebo/nebo/plugins/example-ops/0.1.0/example-ops";
        // argv0 == target, with trailing args → match (the leak case)
        assert!(command_is_for(&format!("{target} watch --events"), target));
        // exact, no args → match
        assert!(command_is_for(target, target));
        // a different binary whose path starts with the same prefix → no match
        assert!(!command_is_for(&format!("{target}-helper run"), target));
        // unrelated → no match
        assert!(!command_is_for("/usr/bin/ssh -N", target));
    }
}
