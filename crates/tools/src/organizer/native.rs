//! Native PIM helper: a Swift EventKit/Contacts binary for fast calendar,
//! contacts, and reminders access on macOS, built by `crate::swift_helper`.
//! `None` means the native path is unavailable (no `swiftc`); callers fall
//! back to AppleScript.

use crate::registry::ToolResult;
use std::path::PathBuf;

/// Swift source embedded at compile time.
const PIM_HELPER_SOURCE: &str = include_str!("pim_helper.swift");

async fn ensure_helper() -> Option<PathBuf> {
    crate::swift_helper::ensure_compiled("pim-helper", PIM_HELPER_SOURCE, &["EventKit", "Contacts"]).await
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
