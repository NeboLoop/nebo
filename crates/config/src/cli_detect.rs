use std::process::Command;
use std::time::Duration;

use serde::Serialize;

/// Whether a CLI command is found in PATH.
#[derive(Debug, Clone, Serialize)]
pub struct CliAvailability {
    pub claude: bool,
    pub codex: bool,
    pub gemini: bool,
}

/// Detailed status of a single CLI tool.
#[derive(Debug, Clone, Serialize)]
pub struct CliStatus {
    pub installed: bool,
    pub authenticated: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub version: String,
}

impl Default for CliStatus {
    fn default() -> Self {
        Self {
            installed: false,
            authenticated: false,
            version: String::new(),
        }
    }
}

/// Statuses for all known CLI tools.
#[derive(Debug, Clone, Serialize)]
pub struct AllCliStatuses {
    pub claude: CliStatus,
    pub codex: CliStatus,
    pub gemini: CliStatus,
}

/// Check if a CLI command is installed via PATH lookup.
#[allow(dead_code)]
pub fn is_cli_installed(command: &str) -> bool {
    which::which(command).is_ok()
}

/// Check detailed status of a CLI command (installed + version via --version).
fn check_cli_status(command: &str) -> CliStatus {
    let path = match which::which(command) {
        Ok(p) => p,
        Err(_) => return CliStatus::default(),
    };

    let mut status = CliStatus {
        installed: true,
        authenticated: false,
        version: String::new(),
    };

    // Run --version with a 3s timeout
    if let Ok(output) = Command::new(&path)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .and_then(|child| {
            // Wait with timeout by polling
            wait_with_timeout(child, Duration::from_secs(3))
        })
    {
        if output.status.success() {
            status.authenticated = true;
            status.version = String::from_utf8_lossy(&output.stdout).trim().to_string();
        }
    }

    status
}

/// Wait for a child process with timeout, killing it if it exceeds the deadline.
fn wait_with_timeout(
    mut child: std::process::Child,
    timeout: Duration,
) -> std::io::Result<std::process::Output> {
    let start = std::time::Instant::now();
    loop {
        match child.try_wait()? {
            Some(status) => {
                let stdout = child.stdout.take().map_or_else(Vec::new, |mut r| {
                    let mut buf = Vec::new();
                    std::io::Read::read_to_end(&mut r, &mut buf).ok();
                    buf
                });
                let stderr = child.stderr.take().map_or_else(Vec::new, |mut r| {
                    let mut buf = Vec::new();
                    std::io::Read::read_to_end(&mut r, &mut buf).ok();
                    buf
                });
                return Ok(std::process::Output {
                    status,
                    stdout,
                    stderr,
                });
            }
            None => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "process timed out",
                    ));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

/// Detect all known CLI tools (claude, codex, gemini). Synchronous.
pub fn detect_all_clis() -> AllCliStatuses {
    AllCliStatuses {
        claude: check_cli_status("claude"),
        codex: check_cli_status("codex"),
        gemini: check_cli_status("gemini"),
    }
}
