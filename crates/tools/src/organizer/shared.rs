//! Cross-platform helpers for the organizer tool.
//!
//! Date parsing, string escaping, and subprocess execution shared across
//! macOS, Linux, and Windows platform modules.

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
use crate::registry::ToolResult;

// ═══════════════════════════════════════════════════════════════════════
// Date parsing
// ═══════════════════════════════════════════════════════════════════════

/// Parse a date/time string into a chrono NaiveDateTime.
///
/// Supports:
/// - ISO format: "2024-01-15 14:00", "2024-01-15"
/// - US format:  "01/15/2024 14:00", "01/15/2024"
/// - Natural language: "today", "tomorrow", "in N minutes|hours|days|weeks"
///
/// `ACCEPTED_DATE_FORMS` is the same list as one line for error messages.
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
pub const ACCEPTED_DATE_FORMS: &str = "'YYYY-MM-DD HH:MM', 'YYYY-MM-DD', 'MM/DD/YYYY HH:MM', 'MM/DD/YYYY', 'today', 'tomorrow', 'in N minutes|hours|days|weeks'";

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
pub fn parse_date(s: &str) -> Result<chrono::NaiveDateTime, String> {
    use chrono::{Duration, Local, NaiveDate};

    let s = s.trim();
    let lower = s.to_lowercase();
    let now = Local::now().naive_local();

    // Natural language
    match lower.as_str() {
        "today" => return Ok(now),
        "tomorrow" => return Ok(now + Duration::days(1)),
        _ => {}
    }

    // "in N {minutes|hours|days|weeks}"
    if lower.starts_with("in ") {
        let parts: Vec<&str> = lower[3..].split_whitespace().collect();
        if parts.len() >= 2 {
            if let Ok(n) = parts[0].parse::<i64>() {
                let unit = parts[1];
                if unit.starts_with("min") {
                    return Ok(now + Duration::minutes(n));
                } else if unit.starts_with("hour") {
                    return Ok(now + Duration::hours(n));
                } else if unit.starts_with("day") {
                    return Ok(now + Duration::days(n));
                } else if unit.starts_with("week") {
                    return Ok(now + Duration::weeks(n));
                }
            }
        }
        return Err(format!(
            "Could not parse '{}'. Relative form is 'in N minutes|hours|days|weeks'.",
            s
        ));
    }

    // Structured formats (with time)
    for fmt in &["%Y-%m-%d %H:%M", "%m/%d/%Y %H:%M"] {
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
            return Ok(dt);
        }
    }

    // Structured formats (date only → midnight)
    for fmt in &["%Y-%m-%d", "%m/%d/%Y"] {
        if let Ok(d) = NaiveDate::parse_from_str(s, fmt) {
            if let Some(dt) = d.and_hms_opt(0, 0, 0) {
                return Ok(dt);
            }
        }
    }

    Err(format!(
        "Could not parse date '{}'. Accepted forms: {}.",
        s, ACCEPTED_DATE_FORMS
    ))
}

// ═══════════════════════════════════════════════════════════════════════
// String escaping
// ═══════════════════════════════════════════════════════════════════════

/// Escape a string for use in AppleScript double-quoted literals.
#[cfg(target_os = "macos")]
pub fn escape_applescript(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\t', "\\t")
}

/// Escape a string for PowerShell double-quoted strings.
/// Order matters: backtick first (it's the escape character itself).
#[cfg(target_os = "windows")]
pub fn escape_powershell(s: &str) -> String {
    s.replace('`', "``")
        .replace('"', "`\"")
        .replace('$', "`$")
        .replace('\n', "`n")
}

// ═══════════════════════════════════════════════════════════════════════
// Subprocess execution
// ═══════════════════════════════════════════════════════════════════════

/// Result text for a subprocess that exited 0 and printed nothing. Callers
/// that know what "nothing" means (an empty listing) compare against this
/// and replace it with a counted statement.
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
pub const NO_OUTPUT: &str = "(exit 0, no output)";

/// Maximum time to wait for a subprocess before killing it.
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
const SUBPROCESS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Error text for a subprocess that hit `SUBPROCESS_TIMEOUT` and was killed.
/// The command may or may not have taken effect before the kill.
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
pub fn timeout_error(cmd: &str) -> String {
    format!(
        "{} did not finish within {} s and was killed; whether it took effect is unknown. Narrow the request (specify an account, mailbox, or calendar name, or lower the limit).",
        cmd,
        SUBPROCESS_TIMEOUT.as_secs()
    )
}

/// Error text for a subprocess that exited non-zero: names the command and
/// the exit code, then whatever the process printed (stderr first, stdout
/// as a fallback, or an explicit marker when it printed nothing).
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
pub fn exit_error(cmd: &str, output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let code = match output.status.code() {
        Some(c) => c.to_string(),
        None => "terminated by signal".to_string(),
    };
    let body = if stdout.is_empty() && stderr.is_empty() {
        "(no output)".to_string()
    } else if stdout.is_empty() {
        stderr
    } else if stderr.is_empty() {
        stdout
    } else {
        format!("{}\n{}", stdout, stderr)
    };
    format!("{} exited {}: {}", cmd, code, body)
}

/// Run an AppleScript via `osascript -e` and return a ToolResult.
#[cfg(target_os = "macos")]
pub async fn run_osascript(script: &str) -> ToolResult {
    let child = match tokio::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return ToolResult::error(format!("Failed to run osascript: {}", e)),
    };

    match tokio::time::timeout(SUBPROCESS_TIMEOUT, child.wait_with_output()).await {
        Ok(Ok(output)) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            ToolResult::ok(if text.is_empty() {
                NO_OUTPUT.to_string()
            } else {
                text
            })
        }
        Ok(Ok(output)) => ToolResult::error(exit_error("osascript", &output)),
        Ok(Err(e)) => ToolResult::error(format!("Failed to run osascript: {}", e)),
        Err(_) => {
            // child is killed on drop via kill_on_drop(true)
            ToolResult::error(timeout_error("osascript"))
        }
    }
}

/// Run a command with arguments and return a ToolResult.
/// Uses direct exec (no shell) — safe from shell injection.
#[cfg(target_os = "linux")]
pub async fn run_command(cmd: &str, args: &[&str]) -> ToolResult {
    let child = match tokio::process::Command::new(cmd)
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return ToolResult::error(format!("Failed to run {}: {}", cmd, e)),
    };

    match tokio::time::timeout(SUBPROCESS_TIMEOUT, child.wait_with_output()).await {
        Ok(Ok(output)) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            ToolResult::ok(if text.is_empty() {
                NO_OUTPUT.to_string()
            } else {
                text
            })
        }
        Ok(Ok(output)) => ToolResult::error(exit_error(cmd, &output)),
        Ok(Err(e)) => ToolResult::error(format!("Failed to run {}: {}", cmd, e)),
        Err(_) => ToolResult::error(timeout_error(cmd)),
    }
}

/// Run a command piping data to stdin. Safer than shell interpolation for
/// user-supplied content (email bodies, vCard data, calcurse appointments).
#[cfg(target_os = "linux")]
pub async fn run_command_with_stdin(cmd: &str, args: &[&str], stdin_data: &str) -> ToolResult {
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt;

    let mut child = match tokio::process::Command::new(cmd)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return ToolResult::error(format!("Failed to spawn {}: {}", cmd, e)),
    };

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(stdin_data.as_bytes()).await;
    }

    match tokio::time::timeout(SUBPROCESS_TIMEOUT, child.wait_with_output()).await {
        Ok(Ok(output)) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            ToolResult::ok(if text.is_empty() {
                NO_OUTPUT.to_string()
            } else {
                text
            })
        }
        Ok(Ok(output)) => ToolResult::error(exit_error(cmd, &output)),
        Ok(Err(e)) => ToolResult::error(format!("{} failed: {}", cmd, e)),
        Err(_) => {
            // child already consumed by wait_with_output; process is cleaned up on drop
            ToolResult::error(timeout_error(cmd))
        }
    }
}

/// Run a PowerShell script with -NoProfile for fast startup.
#[cfg(target_os = "windows")]
pub async fn run_powershell(script: &str) -> ToolResult {
    let mut cmd = tokio::process::Command::new("powershell");
    cmd.args(["-NoProfile", "-Command", script]);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    cmd.kill_on_drop(true);
    crate::process::hide_window(&mut cmd);

    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return ToolResult::error(format!("Failed to run PowerShell: {}", e)),
    };

    match tokio::time::timeout(SUBPROCESS_TIMEOUT, child.wait_with_output()).await {
        Ok(Ok(output)) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            ToolResult::ok(if text.is_empty() {
                NO_OUTPUT.to_string()
            } else {
                text
            })
        }
        Ok(Ok(output)) => ToolResult::error(exit_error("powershell", &output)),
        Ok(Err(e)) => ToolResult::error(format!("Failed to run PowerShell: {}", e)),
        Err(_) => ToolResult::error(timeout_error("powershell")),
    }
}

/// Check if a binary is available on PATH.
#[allow(dead_code)] // Used on Linux
pub fn which_exists(cmd: &str) -> bool {
    which::which(cmd).is_ok()
}

// ═══════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_date_iso_with_time() {
        let dt = parse_date("2024-06-15 14:30").unwrap();
        assert_eq!(dt.format("%Y-%m-%d %H:%M").to_string(), "2024-06-15 14:30");
    }

    #[test]
    fn test_parse_date_iso_date_only() {
        let dt = parse_date("2024-06-15").unwrap();
        assert_eq!(dt.format("%Y-%m-%d %H:%M").to_string(), "2024-06-15 00:00");
    }

    #[test]
    fn test_parse_date_us_with_time() {
        let dt = parse_date("06/15/2024 14:30").unwrap();
        assert_eq!(dt.format("%Y-%m-%d %H:%M").to_string(), "2024-06-15 14:30");
    }

    #[test]
    fn test_parse_date_us_date_only() {
        let dt = parse_date("06/15/2024").unwrap();
        assert_eq!(dt.format("%Y-%m-%d %H:%M").to_string(), "2024-06-15 00:00");
    }

    #[test]
    fn test_parse_date_today() {
        let dt = parse_date("today").unwrap();
        let now = chrono::Local::now().naive_local();
        // Same day
        assert_eq!(dt.date(), now.date());
    }

    #[test]
    fn test_parse_date_tomorrow() {
        let dt = parse_date("tomorrow").unwrap();
        let tomorrow = chrono::Local::now().naive_local() + chrono::Duration::days(1);
        assert_eq!(dt.date(), tomorrow.date());
    }

    #[test]
    fn test_parse_date_in_n_days() {
        let dt = parse_date("in 3 days").unwrap();
        let expected = chrono::Local::now().naive_local() + chrono::Duration::days(3);
        assert_eq!(dt.date(), expected.date());
    }

    #[test]
    fn test_parse_date_in_n_weeks() {
        let dt = parse_date("in 2 weeks").unwrap();
        let expected = chrono::Local::now().naive_local() + chrono::Duration::weeks(2);
        assert_eq!(dt.date(), expected.date());
    }

    #[test]
    fn test_parse_date_in_n_hours() {
        let dt = parse_date("in 5 hours").unwrap();
        let now = chrono::Local::now().naive_local();
        let diff = dt - now;
        // Should be approximately 5 hours (within 1 second tolerance)
        assert!((diff.num_seconds() - 5 * 3600).abs() < 2);
    }

    #[test]
    fn test_parse_date_in_n_minutes() {
        let dt = parse_date("in 30 minutes").unwrap();
        let now = chrono::Local::now().naive_local();
        let diff = dt - now;
        assert!((diff.num_seconds() - 30 * 60).abs() < 2);
    }

    #[test]
    fn test_parse_date_invalid() {
        assert!(parse_date("not-a-date").is_err());
        assert!(parse_date("").is_err());
        assert!(parse_date("in banana days").is_err());
    }

    #[test]
    fn test_parse_date_errors_name_accepted_forms() {
        let err = parse_date("in banana days").unwrap_err();
        assert!(err.contains("in banana days"), "{err}");
        assert!(err.contains("in N minutes|hours|days|weeks"), "{err}");

        let err = parse_date("not-a-date").unwrap_err();
        assert!(err.contains("not-a-date"), "{err}");
        for form in ["YYYY-MM-DD HH:MM", "MM/DD/YYYY", "today", "tomorrow", "in N minutes"] {
            assert!(err.contains(form), "missing {form} in {err}");
        }
    }

    #[test]
    fn test_timeout_error_names_command_and_seconds() {
        let msg = timeout_error("notmuch");
        assert!(msg.starts_with("notmuch did not finish within 30 s"), "{msg}");
        assert!(msg.contains("whether it took effect is unknown"), "{msg}");
    }

    #[test]
    fn test_exit_error_names_command_code_and_output() {
        #[cfg(unix)]
        {
            let output = std::process::Command::new("sh")
                .args(["-c", "echo out; echo err 1>&2; exit 3"])
                .output()
                .unwrap();
            let msg = exit_error("sh", &output);
            assert!(msg.starts_with("sh exited 3: "), "{msg}");
            assert!(msg.contains("out") && msg.contains("err"), "{msg}");

            let silent = std::process::Command::new("sh")
                .args(["-c", "exit 2"])
                .output()
                .unwrap();
            assert_eq!(exit_error("sh", &silent), "sh exited 2: (no output)");
        }
    }

    #[test]
    fn test_parse_date_whitespace_trimmed() {
        let dt = parse_date("  2024-06-15 14:30  ").unwrap();
        assert_eq!(dt.format("%Y-%m-%d %H:%M").to_string(), "2024-06-15 14:30");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_escape_applescript() {
        assert_eq!(escape_applescript("hello"), "hello");
        assert_eq!(escape_applescript(r#"say "hi""#), r#"say \"hi\""#);
        assert_eq!(escape_applescript("path\\to"), "path\\\\to");
        assert_eq!(escape_applescript("line1\nline2"), "line1\\nline2");
        assert_eq!(escape_applescript("col1\tcol2"), "col1\\tcol2");
    }

    #[test]
    fn test_which_exists() {
        // "ls" or "cmd" should exist on any platform
        #[cfg(unix)]
        assert!(which_exists("ls"));
        #[cfg(windows)]
        assert!(which_exists("cmd"));

        assert!(!which_exists("definitely_not_a_real_binary_xyz123"));
    }
}
