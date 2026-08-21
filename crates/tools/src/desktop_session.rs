//! The bot's computer: an on-demand X11 desktop inside the cloud pod.
//!
//! Nothing here runs at boot. When a viewer opens the desktop panel (or a
//! desktop-tool call needs a display in server mode), the session manager
//! spawns Xvfb + a curated xfce (xfwm4 + xfce4-panel) + x11vnc, all shipped
//! in the server image. The X display is the ONE canonical frame source:
//! x11vnc serves the live view, and recording (teach-a-task) consumes the
//! same display. When nothing has touched the desktop for [`IDLE_STOP`] the
//! tree is torn down; the workspace on /data is untouched.
//!
//! The desktop tree runs at `nice 10` so nebo-server always wins the CPU —
//! a busy Chromium must never starve the /health probe (5s timeout).

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use tokio::process::{Child, Command};
use tracing::{info, warn};

/// The session's X display. Fixed: one desktop per pod.
pub const DISPLAY: &str = ":99";
/// x11vnc listens loopback-only; the WS bridge in nebo-server is the only way in.
pub const VNC_PORT: u16 = 5900;
const IDLE_STOP: Duration = Duration::from_secs(15 * 60);
const REAPER_TICK: Duration = Duration::from_secs(60);

struct Inner {
    xvfb: Child,
    /// dbus-run-session wrapping xfwm4 + xfce4-panel; killing it takes the WM down.
    session: Child,
    x11vnc: Child,
}

static SLOT: OnceLock<tokio::sync::Mutex<Option<Inner>>> = OnceLock::new();
static ACTIVE: AtomicBool = AtomicBool::new(false);
static VIEWERS: AtomicUsize = AtomicUsize::new(0);
static RECORDING: AtomicBool = AtomicBool::new(false);
static LAST_TOUCH_MS: AtomicU64 = AtomicU64::new(0);
static REAPER_STARTED: AtomicBool = AtomicBool::new(false);

fn slot() -> &'static tokio::sync::Mutex<Option<Inner>> {
    SLOT.get_or_init(|| tokio::sync::Mutex::new(None))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// True while the desktop tree is up. This is what un-gates the desktop
/// resources in `os_tool` on an otherwise headless server.
pub fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// The DISPLAY to inject into X11 subprocesses, when a session is live.
pub fn display() -> Option<&'static str> {
    active().then_some(DISPLAY)
}

/// Record activity (tool call, viewer input) so the idle reaper holds off.
pub fn touch() {
    LAST_TOUCH_MS.store(now_ms(), Ordering::Relaxed);
}

/// Viewer accounting for the idle reaper. The bridge holds one guard per
/// connected VNC client; a live viewer inhibits idle-stop.
pub struct ViewerGuard(());

pub fn viewer_connected() -> ViewerGuard {
    VIEWERS.fetch_add(1, Ordering::Relaxed);
    touch();
    ViewerGuard(())
}

impl Drop for ViewerGuard {
    fn drop(&mut self) {
        VIEWERS.fetch_sub(1, Ordering::Relaxed);
        touch();
    }
}

/// Recording (teach-a-task) inhibits idle-stop for its whole duration.
pub fn set_recording(on: bool) {
    RECORDING.store(on, Ordering::Relaxed);
    touch();
}

fn nice(cmd: &str) -> Command {
    let mut c = Command::new("nice");
    c.arg("-n").arg("10").arg(cmd);
    c
}

/// Start the desktop if it isn't running; idempotent. Returns the VNC port.
pub async fn ensure_started() -> Result<u16, String> {
    // In the cloud, the provisioner stamps NEBO_DESKTOP=1 only on pods whose
    // owner enabled the computer — those get the bigger envelope. Without it
    // the pod is sized 1Gi and Chromium would OOM the bot mid-run: refuse.
    if std::env::var_os("NEBO_SERVER_MODE").is_some()
        && std::env::var_os("NEBO_DESKTOP").is_none()
    {
        return Err(
            "this bot's computer isn't enabled — turn it on in the bot's cloud settings \
             (the bot restarts once to get a bigger machine)"
                .into(),
        );
    }
    touch();
    let mut guard = slot().lock().await;
    if let Some(inner) = guard.as_mut() {
        // A crashed child means the session is broken — tear down and respawn.
        let crashed = matches!(inner.xvfb.try_wait(), Ok(Some(_)))
            || matches!(inner.session.try_wait(), Ok(Some(_)))
            || matches!(inner.x11vnc.try_wait(), Ok(Some(_)));
        if !crashed {
            return Ok(VNC_PORT);
        }
        warn!("desktop session child died; restarting the tree");
        stop_inner(guard.take().unwrap()).await;
        ACTIVE.store(false, Ordering::Relaxed);
    }

    let xvfb = nice("Xvfb")
        .args([DISPLAY, "-screen", "0", "1280x800x24", "-nolisten", "tcp"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("Xvfb failed to start (is the desktop image installed?): {e}"))?;

    // Wait for the display to accept connections before starting clients.
    let mut ready = false;
    for _ in 0..50 {
        if Command::new("xdpyinfo")
            .arg("-display")
            .arg(DISPLAY)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await
            .map(|s| s.success())
            .unwrap_or(false)
        {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    if !ready {
        let mut xvfb = xvfb;
        let _ = xvfb.kill().await;
        return Err("X display did not become ready".into());
    }

    // xfwm4 stays FOREGROUND (&) — `--daemon` double-forks it out of
    // supervision and it died unnoticed in the pod, leaving unmanaged
    // windows. Compositor off: no GPU under Xvfb, and x11vnc reads the
    // plain framebuffer anyway.
    let session = nice("dbus-run-session")
        .args(["--", "sh", "-c", "xsetroot -solid '#101726' 2>/dev/null; xfwm4 --compositor=off & exec xfce4-panel"])
        .env("DISPLAY", DISPLAY)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("desktop session failed to start: {e}"))?;

    let x11vnc = nice("x11vnc")
        .args([
            "-display", DISPLAY,
            "-localhost",
            "-rfbport", "5900",
            "-shared",
            "-forever",
            "-nopw",
            "-quiet",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("x11vnc failed to start: {e}"))?;

    *guard = Some(Inner { xvfb, session, x11vnc });
    ACTIVE.store(true, Ordering::Relaxed);
    info!(display = DISPLAY, "desktop session started");

    if !REAPER_STARTED.swap(true, Ordering::Relaxed) {
        tokio::spawn(async {
            loop {
                tokio::time::sleep(REAPER_TICK).await;
                let idle_ms = now_ms().saturating_sub(LAST_TOUCH_MS.load(Ordering::Relaxed));
                if active()
                    && VIEWERS.load(Ordering::Relaxed) == 0
                    && !RECORDING.load(Ordering::Relaxed)
                    && idle_ms > IDLE_STOP.as_millis() as u64
                {
                    info!("desktop session idle; stopping");
                    stop().await;
                }
            }
        });
    }

    Ok(VNC_PORT)
}

// --- Teach-a-task recording -------------------------------------------------
//
// A recording is a mode of the live session: ffmpeg captures the SAME X
// display x11vnc serves (one canonical frame source) into a human-viewable
// mp4 plus a deduped ~1fps keyframe series, and xinput logs the raw input
// events. Artifacts land in the bot's own home so they're visible in its
// file manager: ~/teach-sessions/<id>/{session.mp4, frames/, events.log}.

struct RecInner {
    id: String,
    dir: std::path::PathBuf,
    video: Child,
    frames: Child,
    events: Child,
}

static REC: OnceLock<tokio::sync::Mutex<Option<RecInner>>> = OnceLock::new();

fn rec_slot() -> &'static tokio::sync::Mutex<Option<RecInner>> {
    REC.get_or_init(|| tokio::sync::Mutex::new(None))
}

/// Where teach sessions live: the bot's home, browsable in Thunar.
fn teach_root() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    std::path::Path::new(&home).join("teach-sessions")
}

/// Start recording the live desktop. Returns (session_id, session_dir).
/// One recording at a time; the desktop must be up (start it first).
pub async fn start_recording() -> Result<(String, std::path::PathBuf), String> {
    if !active() {
        return Err("the desktop isn't running — open the computer first".into());
    }
    let mut rec = rec_slot().lock().await;
    if rec.is_some() {
        return Err("a recording is already in progress".into());
    }

    let id = uuid::Uuid::new_v4().to_string();
    let dir = teach_root().join(&id);
    let frames_dir = dir.join("frames");
    std::fs::create_dir_all(&frames_dir).map_err(|e| format!("teach dir: {e}"))?;

    // Human-viewable replay of the whole demonstration.
    let video = Command::new("ffmpeg")
        .args([
            "-loglevel", "error",
            "-f", "x11grab", "-framerate", "10", "-i", DISPLAY,
            "-vcodec", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p",
        ])
        .arg(dir.join("session.mp4"))
        .stdin(std::process::Stdio::piped()) // 'q' for a clean mp4 finalize
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("recorder failed to start: {e}"))?;

    // Deduped ~1fps keyframes — the distillation pass reads these, not video.
    let frames = Command::new("ffmpeg")
        .args([
            "-loglevel", "error",
            "-f", "x11grab", "-framerate", "2", "-i", DISPLAY,
            "-vf", "mpdecimate,fps=1", "-q:v", "4",
        ])
        .arg(frames_dir.join("%04d.jpg"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("keyframe sampler failed to start: {e}"))?;

    // Raw input event log (clicks, keys, focus) — parsed at distill time.
    let events_file = std::fs::File::create(dir.join("events.log"))
        .map_err(|e| format!("events log: {e}"))?;
    let events = Command::new("xinput")
        .args(["test-xi2", "--root"])
        .env("DISPLAY", DISPLAY)
        .stdin(std::process::Stdio::null())
        .stdout(events_file)
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("input logger failed to start: {e}"))?;

    set_recording(true);
    info!(session = %id, "teach recording started");
    *rec = Some(RecInner { id: id.clone(), dir: dir.clone(), video, frames, events });
    Ok((id, dir))
}

/// Stop the recording and finalize artifacts. Returns (session_id, dir,
/// keyframe_count). No recording in progress is an error.
pub async fn stop_recording() -> Result<(String, std::path::PathBuf, usize), String> {
    let mut rec = rec_slot().lock().await;
    let Some(mut inner) = rec.take() else {
        return Err("no recording in progress".into());
    };
    set_recording(false);

    // Ask ffmpeg to finalize (write the moov atom) instead of killing it.
    for child in [&mut inner.video, &mut inner.frames] {
        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            let _ = stdin.write_all(b"q").await;
        }
    }
    let _ = tokio::time::timeout(Duration::from_secs(10), inner.video.wait()).await;
    let _ = tokio::time::timeout(Duration::from_secs(10), inner.frames.wait()).await;
    let _ = inner.video.kill().await;
    let _ = inner.frames.kill().await;
    let _ = inner.events.kill().await;

    let count = std::fs::read_dir(inner.dir.join("frames"))
        .map(|d| d.count())
        .unwrap_or(0);
    info!(session = %inner.id, keyframes = count, "teach recording stopped");
    Ok((inner.id, inner.dir, count))
}

/// Tear the desktop down. The workspace is untouched; a later
/// [`ensure_started`] brings a fresh desktop back in ~2-3s.
pub async fn stop() {
    let mut guard = slot().lock().await;
    if let Some(inner) = guard.take() {
        ACTIVE.store(false, Ordering::Relaxed);
        stop_inner(inner).await;
        info!("desktop session stopped");
    }
}

async fn stop_inner(mut inner: Inner) {
    // Clients first, X server last.
    let _ = inner.x11vnc.kill().await;
    let _ = inner.session.kill().await;
    let _ = inner.xvfb.kill().await;
}
