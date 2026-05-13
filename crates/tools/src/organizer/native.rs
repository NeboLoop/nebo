//! Native PIM helper: compiles and caches a Swift EventKit/Contacts binary
//! for fast calendar, contacts, and reminders access on macOS.
//!
//! Falls back to `None` if `swiftc` is unavailable or compilation fails,
//! signalling the caller to use AppleScript instead.
//!
//! The binary auto-recompiles when the embedded source changes (e.g. after
//! a Nebo update) using a source hash file alongside the binary.

use crate::registry::ToolResult;
use std::path::PathBuf;
use std::sync::OnceLock;
use tokio::sync::Mutex;

/// Cached path to the compiled helper binary (or `None` if unavailable).
static HELPER_PATH: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();

/// Swift source embedded at compile time.
const PIM_HELPER_SOURCE: &str = include_str!("pim_helper.swift");

/// FNV-1a hash of the embedded source, computed at compile time.
/// Used to detect when the source changes and the binary needs recompiling.
const SOURCE_HASH: u64 = const_fnv1a(PIM_HELPER_SOURCE.as_bytes());

const fn const_fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u64;
        hash = hash.wrapping_mul(0x100000001b3);
        i += 1;
    }
    hash
}

/// Return the path to the compiled pim-helper binary, compiling it if needed.
/// Automatically recompiles when the embedded Swift source changes.
async fn ensure_helper() -> Option<PathBuf> {
    let mutex = HELPER_PATH.get_or_init(|| Mutex::new(None));
    let mut guard = mutex.lock().await;

    if let Some(ref path) = *guard {
        if path.exists() {
            return Some(path.clone());
        }
    }

    // Determine target directory
    let bin_dir = match config::data_dir() {
        Ok(d) => d.join("bin"),
        Err(_) => return None,
    };
    if std::fs::create_dir_all(&bin_dir).is_err() {
        return None;
    }

    let binary_path = bin_dir.join("pim-helper");
    let hash_path = bin_dir.join("pim-helper.hash");

    // Check if existing binary matches current source
    if binary_path.exists() {
        if let Ok(stored) = std::fs::read_to_string(&hash_path) {
            if stored.trim() == SOURCE_HASH.to_string() {
                *guard = Some(binary_path.clone());
                return Some(binary_path);
            }
            tracing::info!("PIM helper source changed, recompiling…");
        }
        // Hash missing or mismatched — recompile
        let _ = std::fs::remove_file(&binary_path);
    }

    // Write source to temp file and compile
    let source_path = bin_dir.join("pim_helper.swift");
    if std::fs::write(&source_path, PIM_HELPER_SOURCE).is_err() {
        return None;
    }

    tracing::info!("compiling native PIM helper…");
    let output = tokio::process::Command::new("swiftc")
        .args(["-O", "-framework", "EventKit", "-framework", "Contacts"])
        .arg("-o")
        .arg(&binary_path)
        .arg(&source_path)
        .output()
        .await;

    match output {
        Ok(o) if o.status.success() => {
            tracing::info!(path = %binary_path.display(), "PIM helper compiled");
            let _ = std::fs::remove_file(&source_path);
            // Write source hash so we know when to recompile
            let _ = std::fs::write(&hash_path, SOURCE_HASH.to_string());
            *guard = Some(binary_path.clone());
            Some(binary_path)
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            tracing::warn!(%stderr, "PIM helper compilation failed, falling back to AppleScript");
            let _ = std::fs::remove_file(&source_path);
            None
        }
        Err(e) => {
            tracing::warn!(%e, "swiftc not found, falling back to AppleScript");
            let _ = std::fs::remove_file(&source_path);
            None
        }
    }
}

/// Run the native PIM helper. Returns `None` if native path is unavailable
/// (caller should fall back to AppleScript).
pub async fn run_pim(domain: &str, action: &str, args: &[(&str, &str)]) -> Option<ToolResult> {
    let helper = ensure_helper().await?;

    let mut cmd = tokio::process::Command::new(&helper);
    cmd.arg(domain).arg(action);
    for (key, value) in args {
        cmd.arg(format!("--{}", key)).arg(value);
    }
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    cmd.kill_on_drop(true);

    let timeout = std::time::Duration::from_secs(30);
    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(%e, "failed to spawn pim-helper");
            return None;
        }
    };

    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(o)) => {
            let stdout = String::from_utf8_lossy(&o.stdout).trim().to_string();

            if stdout.starts_with("ERROR: ") {
                Some(ToolResult::error(
                    stdout.trim_start_matches("ERROR: ").to_string(),
                ))
            } else if o.status.success() {
                Some(ToolResult::ok(stdout))
            } else {
                let stderr = String::from_utf8_lossy(&o.stderr).trim().to_string();
                if !stderr.is_empty() {
                    Some(ToolResult::error(stderr))
                } else {
                    Some(ToolResult::error(stdout))
                }
            }
        }
        Ok(Err(e)) => Some(ToolResult::error(format!("PIM helper error: {e}"))),
        Err(_) => Some(ToolResult::error("PIM helper timed out".to_string())),
    }
}
