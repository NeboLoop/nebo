//! The embedded ffmpeg, ffprobe and font: written to disk once, then reused.
//!
//! They live under `$NEBO_DATA_DIR/ffmpeg/<content hash>/` (Nebo sets
//! `NEBO_DATA_DIR` for every plugin run). A process writes each file under a
//! temporary name and renames it into place, so a concurrent run never sees a
//! half-written executable.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use flate2::read::GzDecoder;

// ffmpeg and ffprobe are embedded gzipped; the font as-is.
static FFMPEG_GZ: &[u8] = include_bytes!(env!("NEBO_VIDEO_FFMPEG"));
static FFPROBE_GZ: &[u8] = include_bytes!(env!("NEBO_VIDEO_FFPROBE"));
static FONT: &[u8] = include_bytes!(env!("NEBO_VIDEO_FONT"));
const ASSET_HASH: &str = env!("NEBO_VIDEO_ASSET_HASH");

fn size(s: &str) -> u64 {
    s.parse().expect("build.rs writes a number")
}

const EXE: &str = if cfg!(windows) { ".exe" } else { "" };

pub fn ffmpeg_bin() -> Result<PathBuf, String> {
    let len = size(env!("NEBO_VIDEO_FFMPEG_SIZE"));
    extract(&format!("ffmpeg{EXE}"), len, true, || GzDecoder::new(FFMPEG_GZ))
}

pub fn ffprobe_bin() -> Result<PathBuf, String> {
    let len = size(env!("NEBO_VIDEO_FFPROBE_SIZE"));
    extract(&format!("ffprobe{EXE}"), len, true, || GzDecoder::new(FFPROBE_GZ))
}

pub fn font_path() -> Result<PathBuf, String> {
    extract("Inter-Regular.ttf", FONT.len() as u64, false, || FONT)
}

fn asset_dir() -> PathBuf {
    let base = std::env::var_os("NEBO_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("nebo-video"));
    base.join("ffmpeg").join(ASSET_HASH)
}

fn extract<R: Read>(
    name: &str,
    len: u64,
    executable: bool,
    content: impl FnOnce() -> R,
) -> Result<PathBuf, String> {
    let dir = asset_dir();
    let path = dir.join(name);
    if fs::metadata(&path).is_ok_and(|m| m.len() == len) {
        return Ok(path);
    }
    fs::create_dir_all(&dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;

    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    write_file(&tmp, content(), executable)
        .map_err(|e| format!("could not write {}: {e}", tmp.display()))?;
    if let Err(e) = fs::rename(&tmp, &path) {
        let _ = fs::remove_file(&tmp);
        // Windows refuses to replace a file another run already put in place
        // (and may be executing). That copy is identical, so use it.
        if !fs::metadata(&path).is_ok_and(|m| m.len() == len) {
            return Err(format!("could not install {}: {e}", path.display()));
        }
    }
    Ok(path)
}

fn write_file(path: &Path, mut content: impl Read, executable: bool) -> io::Result<()> {
    let mut f = fs::File::create(path)?;
    io::copy(&mut content, &mut f)?;
    f.flush()?;
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
