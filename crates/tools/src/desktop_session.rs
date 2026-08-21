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

    // A pod recreate leaves Chromium's profile lock pointing at the old
    // hostname; Chromium then refuses to start from the dock ("profile in
    // use on another computer"). Nothing else can hold the profile in a
    // fresh session — clear the stale locks before the desktop comes up.
    if let Ok(home) = std::env::var("HOME") {
        for f in ["SingletonLock", "SingletonSocket", "SingletonCookie"] {
            let _ = std::fs::remove_file(
                std::path::Path::new(&home).join(".config/chromium").join(f),
            );
        }
        // A pod roll is always an "unclean" Chromium exit; without this every
        // session opens on a "Restore pages?" nag (the CLI flag no longer
        // suppresses it). Rewriting the recorded exit state is the fix.
        let prefs = std::path::Path::new(&home).join(".config/chromium/Default/Preferences");
        if let Ok(txt) = std::fs::read_to_string(&prefs) {
            let fixed = txt
                .replace("\"exit_type\":\"Crashed\"", "\"exit_type\":\"Normal\"")
                .replace("\"exited_cleanly\":false", "\"exited_cleanly\":true");
            let _ = std::fs::write(&prefs, fixed);
        }
    }

    // Seed the panel layout ONCE per home: xfconf system defaults don't carry
    // the launcher item arrays, so the dock rendered placeholder gears. The
    // panel's native form is per-launcher .desktop dirs in user config — seed
    // them only when the user has no panel config, so customization survives.
    if let Ok(home) = std::env::var("HOME") {
        let user_panel = std::path::Path::new(&home)
            .join(".config/xfce4/xfconf/xfce-perchannel-xml/xfce4-panel.xml");
        if !user_panel.exists() {
            let _ = std::fs::create_dir_all(user_panel.parent().unwrap());
            let _ = std::fs::copy("/etc/nebo/desktop-skel/xfce4-panel.xml", &user_panel);
            for (n, app) in [(1, "chromium"), (2, "xfce4-terminal"), (3, "thunar")] {
                let dir = std::path::Path::new(&home)
                    .join(format!(".config/xfce4/panel/launcher-{n}"));
                let _ = std::fs::create_dir_all(&dir);
                let _ = std::fs::copy(
                    format!("/usr/share/applications/{app}.desktop"),
                    dir.join(format!("{app}.desktop")),
                );
            }
        }
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
        .args(["--", "sh", "-c", "xsetroot -solid '#101726' 2>/dev/null; xfwm4 --compositor=off & (sleep 1; chromium --no-first-run --hide-crash-restore-bubble --start-maximized --disable-dev-shm-usage --disable-gpu >/dev/null 2>&1) & exec xfce4-panel"])
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

    // Events-first distillation: the raw X11 dump carries most of the task's
    // truth (the typed keys literally spell it out — proven in the field when
    // a blind study still recovered the task from keystrokes alone). Parse it
    // into a readable timeline so the study pass reads THIS first and uses
    // vision to confirm, instead of grepping 400KB of protocol noise.
    if let Ok(log) = std::fs::read_to_string(inner.dir.join("events.log")) {
        let timeline = summarize_x11_events(&log);
        let _ = std::fs::write(inner.dir.join("timeline.md"), timeline);
    }

    info!(session = %inner.id, keyframes = count, "teach recording stopped");
    Ok((inner.id, inner.dir, count))
}

/// Turn an `xinput test-xi2` dump into a human/model-readable action timeline:
/// clicks with coordinates, typed text reassembled from keycodes (US layout),
/// special keys as tokens. Best-effort — unknown keycodes render as [#n].
fn summarize_x11_events(log: &str) -> String {
    #[derive(PartialEq)]
    enum Pending {
        None,
        Click,
        Key,
    }
    let keychar = |code: u32, shift: bool| -> Option<String> {
        let unshifted = |c: char, s: char| if shift { s } else { c };
        Some(match code {
            10..=18 => {
                let digits = ['1', '2', '3', '4', '5', '6', '7', '8', '9'];
                let shifted = ['!', '@', '#', '$', '%', '^', '&', '*', '('];
                let i = (code - 10) as usize;
                unshifted(digits[i], shifted[i]).to_string()
            }
            19 => unshifted('0', ')').to_string(),
            20 => unshifted('-', '_').to_string(),
            21 => unshifted('=', '+').to_string(),
            22 => "[Backspace]".into(),
            23 => "[Tab]".into(),
            24..=33 => {
                let row = ['q', 'w', 'e', 'r', 't', 'y', 'u', 'i', 'o', 'p'];
                let c = row[(code - 24) as usize];
                if shift { c.to_ascii_uppercase().to_string() } else { c.to_string() }
            }
            36 => "[Enter]".into(),
            38..=46 => {
                let row = ['a', 's', 'd', 'f', 'g', 'h', 'j', 'k', 'l'];
                let c = row[(code - 38) as usize];
                if shift { c.to_ascii_uppercase().to_string() } else { c.to_string() }
            }
            47 => unshifted(';', ':').to_string(),
            48 => unshifted('\'', '"').to_string(),
            52..=58 => {
                let row = ['z', 'x', 'c', 'v', 'b', 'n', 'm'];
                let c = row[(code - 52) as usize];
                if shift { c.to_ascii_uppercase().to_string() } else { c.to_string() }
            }
            59 => unshifted(',', '<').to_string(),
            60 => unshifted('.', '>').to_string(),
            61 => unshifted('/', '?').to_string(),
            65 => " ".into(),
            50 | 62 | 37 | 105 | 64 | 108 | 133 => return None, // bare modifiers
            111 => "[Up]".into(),
            113 => "[Left]".into(),
            114 => "[Right]".into(),
            116 => "[Down]".into(),
            9 => "[Esc]".into(),
            _ => format!("[#{code}]"),
        })
    };

    let mut out = String::from(
        "# Recorded action timeline

Reconstructed from the input event log.          Typed text is reassembled from keycodes (US layout); clicks carry          screen coordinates on the 1280x800 display.

",
    );
    let mut pending = Pending::None;
    let mut detail: u32 = 0;
    let mut typed = String::new();
    let mut steps: Vec<String> = Vec::new();
    let mut flush_typed = |typed: &mut String, steps: &mut Vec<String>| {
        if !typed.is_empty() {
            steps.push(format!("Typed: {typed}"));
            typed.clear();
        }
    };
    for line in log.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("EVENT type ") {
            pending = if rest.starts_with("4 ") {
                Pending::Click
            } else if rest.starts_with("2 ") {
                Pending::Key
            } else {
                Pending::None
            };
            detail = 0;
        } else if let Some(d) = t.strip_prefix("detail: ") {
            detail = d.trim().parse().unwrap_or(0);
        } else if let Some(coords) = t.strip_prefix("root: ") {
            if pending == Pending::Click {
                let xy: Vec<&str> = coords.split('/').collect();
                if xy.len() == 2 {
                    flush_typed(&mut typed, &mut steps);
                    let btn = match detail {
                        1 => "Click",
                        3 => "Right-click",
                        4 | 5 => "Scroll",
                        _ => "Click",
                    };
                    if btn != "Scroll" {
                        let px = xy[0].trim().parse::<f64>().unwrap_or(0.0) as i64;
                        let py = xy[1].trim().parse::<f64>().unwrap_or(0.0) as i64;
                        steps.push(format!("{btn} at ({px}, {py})"));
                    }
                }
                pending = Pending::None;
            }
        } else if t.starts_with("modifiers:") && pending == Pending::Key {
            let shift = t.contains("effective: 1") || t.contains("effective: 0x1");
            if let Some(ch) = keychar(detail, shift) {
                if ch.starts_with('[') {
                    flush_typed(&mut typed, &mut steps);
                    steps.push(format!("Pressed {ch}"));
                } else {
                    typed.push_str(&ch);
                }
            }
            pending = Pending::None;
        }
    }
    flush_typed(&mut typed, &mut steps);
    for (i, s) in steps.iter().enumerate() {
        out.push_str(&format!("{}. {s}
", i + 1));
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    // Real xinput test-xi2 shapes: a click at 386,300 then "re" typed —
    // the opening of the field session that proved keystrokes carry the task.
    #[test]
    fn timeline_reassembles_clicks_and_text() {
        let log = "\
EVENT type 4 (ButtonPress)\n\
    device: 4 (4)\n\
    detail: 1\n\
    root: 386.00/300.00\n\
    event: 386.00/300.00\n\
    modifiers: locked 0 latched 0 base 0 effective: 0\n\
EVENT type 2 (KeyPress)\n\
    detail: 27\n\
    root: 390.00/328.00\n\
    modifiers: locked 0 latched 0 base 0 effective: 0\n\
EVENT type 2 (KeyPress)\n\
    detail: 26\n\
    root: 390.00/328.00\n\
    modifiers: locked 0 latched 0 base 0 effective: 0\n\
EVENT type 2 (KeyPress)\n\
    detail: 36\n\
    root: 390.00/328.00\n\
    modifiers: locked 0 latched 0 base 0 effective: 0\n";
        let t = summarize_x11_events(log);
        assert!(t.contains("Click at (386, 300)"), "click missing: {t}");
        assert!(t.contains("Typed: re"), "typed text missing: {t}");
        assert!(t.contains("Pressed [Enter]"), "enter missing: {t}");
    }
}
