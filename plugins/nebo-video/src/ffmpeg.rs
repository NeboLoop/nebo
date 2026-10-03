//! ffmpeg / ffprobe location + invocation helpers.

use std::path::PathBuf;
use std::process::{Command, Output};

const INSTALL_HINT: &str =
    "ffmpeg not found on PATH. Install: `brew install ffmpeg` (macOS), \
     `apt-get install ffmpeg` (Debian/Ubuntu), `winget install Gyan.FFmpeg` (Windows).";

const FFPROBE_HINT: &str =
    "ffprobe not found on PATH. It ships with ffmpeg — install ffmpeg to get both.";

pub fn ffmpeg_bin() -> Result<PathBuf, String> {
    which::which("ffmpeg").map_err(|_| INSTALL_HINT.to_string())
}

pub fn ffprobe_bin() -> Result<PathBuf, String> {
    which::which("ffprobe").map_err(|_| FFPROBE_HINT.to_string())
}

/// Run a command and return its stdout as a UTF-8 string on success.
/// On non-zero exit, returns stderr prefixed with the command name.
pub fn run(label: &str, mut cmd: Command) -> Result<Output, String> {
    let out = cmd
        .output()
        .map_err(|e| format!("failed to spawn {label}: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(format!("{label} exited {}: {}", out.status, stderr.trim()));
    }
    Ok(out)
}
