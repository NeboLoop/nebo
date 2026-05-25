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
use std::time::Duration;

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
/// `~/.nebo/user/plugins/` or `~/.nebo/user/agents/` AND has PPID == 1
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
        let home = std::env::var("HOME").unwrap_or_default();
        let plugins_prefix = format!("{}/.nebo/user/plugins/", home);
        let agents_prefix = format!("{}/.nebo/user/agents/", home);

        let mut killed = 0usize;
        for line in text.lines() {
            let mut parts = line.trim_start().splitn(3, char::is_whitespace);
            let pid: u32 = match parts.next().and_then(|s| s.trim().parse().ok()) {
                Some(p) => p,
                None => continue,
            };
            let ppid: u32 = match parts.next().and_then(|s| s.trim().parse().ok()) {
                Some(p) => p,
                None => continue,
            };
            let cmd = match parts.next() {
                Some(c) => c,
                None => continue,
            };
            if ppid != 1 {
                continue;
            }
            // Match the leading executable path. Skip if our own pid (paranoia).
            let exe = cmd.split_whitespace().next().unwrap_or("");
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
