//! The embedded ffmpeg, ffprobe and font: written to disk once, then reused.
//!
//! They live under `$NEBO_DATA_DIR/ffmpeg/<content hash>/` (Nebo sets
//! `NEBO_DATA_DIR` for every plugin run). A process writes each file under a
//! temporary name and renames it into place, so a concurrent run never sees a
//! half-written executable.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

static FFMPEG: &[u8] = include_bytes!(env!("NEBO_VIDEO_FFMPEG"));
static FFPROBE: &[u8] = include_bytes!(env!("NEBO_VIDEO_FFPROBE"));
static FONT: &[u8] = include_bytes!(env!("NEBO_VIDEO_FONT"));
const ASSET_HASH: &str = env!("NEBO_VIDEO_ASSET_HASH");

const EXE: &str = if cfg!(windows) { ".exe" } else { "" };

pub fn ffmpeg_bin() -> Result<PathBuf, String> {
    extract(&format!("ffmpeg{EXE}"), FFMPEG, true)
}

pub fn ffprobe_bin() -> Result<PathBuf, String> {
    extract(&format!("ffprobe{EXE}"), FFPROBE, true)
}

pub fn font_path() -> Result<PathBuf, String> {
    extract("Inter-Regular.ttf", FONT, false)
}

fn asset_dir() -> PathBuf {
    let base = std::env::var_os("NEBO_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("nebo-video"));
    base.join("ffmpeg").join(ASSET_HASH)
}

fn extract(name: &str, bytes: &[u8], executable: bool) -> Result<PathBuf, String> {
    let dir = asset_dir();
    let path = dir.join(name);
    if fs::metadata(&path).is_ok_and(|m| m.len() == bytes.len() as u64) {
        return Ok(path);
    }
    fs::create_dir_all(&dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;

    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    write_file(&tmp, bytes, executable)
        .map_err(|e| format!("could not write {}: {e}", tmp.display()))?;
    if let Err(e) = fs::rename(&tmp, &path) {
        let _ = fs::remove_file(&tmp);
        // Windows refuses to replace a file another run already put in place
        // (and may be executing). That copy is identical, so use it.
        if !fs::metadata(&path).is_ok_and(|m| m.len() == bytes.len() as u64) {
            return Err(format!("could not install {}: {e}", path.display()));
        }
    }
    Ok(path)
}

fn write_file(path: &Path, bytes: &[u8], executable: bool) -> std::io::Result<()> {
    let mut f = fs::File::create(path)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    #[cfg(unix)]
    if executable {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(not(unix))]
    let _ = executable;
    Ok(())
}

/// Run a command; on non-zero exit return its stderr prefixed with `label`.
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
