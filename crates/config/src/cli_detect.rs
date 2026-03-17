use std::process::Command;
use std::sync::Once;
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

static PATH_INIT: Once = Once::new();

/// Augment PATH with common CLI install locations.
/// GUI apps (Tauri, Finder, Start Menu) inherit a minimal PATH that
/// typically excludes npm/cargo/homebrew bin directories. This must
/// be called before any `which::which` lookups.
pub fn ensure_full_path() {
    PATH_INIT.call_once(|| {
        let current = std::env::var("PATH").unwrap_or_default();
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_default();

        let mut extra_dirs: Vec<String> = Vec::new();

        if cfg!(target_os = "macos") {
            // Common macOS CLI install locations
            extra_dirs.extend([
                format!("{home}/.npm-global/bin"),
                format!("{home}/.nvm/versions/node/default/bin"),
                format!("{home}/.local/bin"),
                format!("{home}/.cargo/bin"),
                "/usr/local/bin".into(),
                "/opt/homebrew/bin".into(),
                "/opt/homebrew/sbin".into(),
            ]);
            // Try to get the real shell PATH via login shell
            if let Ok(output) = Command::new("/bin/zsh")
                .args(["-l", "-c", "echo $PATH"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .output()
            {
                if output.status.success() {
                    let shell_path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    for dir in shell_path.split(':') {
                        if !dir.is_empty() && !current.contains(dir) {
                            extra_dirs.push(dir.to_string());
                        }
                    }
                }
            }
        } else if cfg!(target_os = "linux") {
            extra_dirs.extend([
                format!("{home}/.npm-global/bin"),
                format!("{home}/.nvm/versions/node/default/bin"),
                format!("{home}/.local/bin"),
                format!("{home}/.cargo/bin"),
                "/usr/local/bin".into(),
                "/snap/bin".into(),
            ]);
        } else if cfg!(target_os = "windows") {
            // Windows: npm global, cargo, AppData\Local programs
            let appdata = std::env::var("APPDATA").unwrap_or_default();
            let local_appdata = std::env::var("LOCALAPPDATA").unwrap_or_default();
            extra_dirs.extend([
                format!("{appdata}\\npm"),
                format!("{home}\\.cargo\\bin"),
                format!("{local_appdata}\\Programs\\claude-code\\bin"),
                format!("{local_appdata}\\Programs\\codex\\bin"),
            ]);
        }

        // Append only dirs not already in PATH
        let mut new_path = current.clone();
        let sep = if cfg!(target_os = "windows") { ";" } else { ":" };
        for dir in extra_dirs {
            if !dir.is_empty() && !current.contains(&dir) {
                new_path.push_str(sep);
                new_path.push_str(&dir);
            }
        }

        if new_path != current {
            // SAFETY: called once during init via sync::Once before any threads
            // spawn, so no concurrent reads of PATH can race.
            unsafe { std::env::set_var("PATH", &new_path) };
        }
    });
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
    let mut cmd = Command::new(&path);
    cmd.arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());

    // Windows: suppress console window flash
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    if let Ok(output) = cmd.spawn()
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
/// Ensures PATH is augmented for GUI-launched apps before detection.
pub fn detect_all_clis() -> AllCliStatuses {
    ensure_full_path();
    AllCliStatuses {
        claude: check_cli_status("claude"),
        codex: check_cli_status("codex"),
        gemini: check_cli_status("gemini"),
    }
}
