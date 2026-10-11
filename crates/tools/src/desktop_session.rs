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
/// Keeps each paste from the viewer fresh in the desktop's apps.
const CLIPBOARD_KEEPER: &str = include_str!("desktop_clipboard_keeper.py");
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

/// How many viewers are watching the desktop now.
pub fn viewers() -> usize {
    VIEWERS.load(Ordering::Relaxed)
}

/// Recording (teach-a-task) inhibits idle-stop for its whole duration,
/// which [`TEACH_CAP`] bounds.
pub fn set_recording(on: bool) {
    RECORDING.store(on, Ordering::Relaxed);
    touch();
}

fn nice(cmd: &str) -> Command {
    let mut c = command::new::<Command>("nice", command::Console::Hidden);
    c.arg("-n").arg("10").arg(cmd);
    c
}

/// Start the desktop if it isn't running; idempotent. Returns the VNC port once
/// the desktop is all the way up (x11vnc accepting, the browser on screen).
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
        .map_err(|e| format!("Xvfb failed to start: {e}. The desktop needs the server image with Xvfb installed."))?;

    // Wait for the display to accept connections before starting clients.
    let mut ready = false;
    for _ in 0..50 {
        if command::new::<Command>("xdpyinfo", command::Console::Hidden)
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
        return Err(format!("X display {DISPLAY} did not answer within 5 seconds"));
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

    // Clipboard (copy and paste with the owner's computer, verified against
    // x11vnc 0.9.16 + Chromium under Xvfb):
    // - X11VNC_AVOID_WINDOWS=never: x11vnc otherwise waits 8-45 seconds
    //   after a viewer connects (in case a login manager is still up) before
    //   it creates the window that owns the selections, and drops every
    //   paste until then.
    // - -noprimary: only an explicit copy (CLIPBOARD, Ctrl+C) goes to the
    //   viewer. A mere highlight (PRIMARY) would otherwise overwrite the
    //   owner's own clipboard. A paste from the viewer still sets both, so
    //   middle-click pastes it too.
    let x11vnc = nice("x11vnc")
        .args([
            "-display", DISPLAY,
            "-localhost",
            "-rfbport", "5900",
            "-shared",
            "-forever",
            "-nopw",
            "-noprimary",
            "-quiet",
        ])
        .env("X11VNC_AVOID_WINDOWS", "never")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("x11vnc failed to start: {e}"))?;

    // A second paste from the viewer must reach Chromium too: see
    // desktop_clipboard_keeper.py. It exits with the display, so it needs no
    // handle; without python3 a paste still works, only later ones may
    // repeat the first in Chromium.
    if let Err(e) = nice("python3")
        .args(["-c", CLIPBOARD_KEEPER])
        .env("DISPLAY", DISPLAY)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        warn!(error = %e, "desktop clipboard keeper did not start");
    }

    // Return only once the desktop is all the way up, so a viewer's first
    // frame is the finished desktop and never a half-drawn one: x11vnc
    // accepts connections, then the browser window is on screen.
    let inner = Inner { xvfb, session, x11vnc };
    if !wait_until(VNC_READY_WAIT, READY_POLL, vnc_accepts).await {
        stop_inner(inner).await;
        return Err(format!(
            "the desktop's screen did not come up within {} seconds",
            VNC_READY_WAIT.as_secs()
        ));
    }
    if wait_until(BROWSER_READY_WAIT, READY_POLL, browser_window_shown).await {
        // The window maps a beat before Chromium paints into it.
        tokio::time::sleep(BROWSER_PAINT_SETTLE).await;
    } else {
        // The desktop itself is up and usable from the dock; serve it.
        warn!(
            wait_secs = BROWSER_READY_WAIT.as_secs(),
            "desktop: the browser window did not appear; serving the desktop without it"
        );
    }

    *guard = Some(inner);
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

/// How long x11vnc gets to accept connections after it is spawned.
const VNC_READY_WAIT: Duration = Duration::from_secs(10);
/// How long the session gets to put the browser window on screen. Chromium
/// at `nice 10` on a cold pod is the slowest part of the boot.
const BROWSER_READY_WAIT: Duration = Duration::from_secs(30);
const BROWSER_PAINT_SETTLE: Duration = Duration::from_millis(750);
const READY_POLL: Duration = Duration::from_millis(150);

/// Probe until it answers true, for at most `limit`. True when it did.
async fn wait_until<F, Fut>(limit: Duration, every: Duration, mut probe: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + limit;
    loop {
        if probe().await {
            return true;
        }
        if tokio::time::Instant::now() + every > deadline {
            return false;
        }
        tokio::time::sleep(every).await;
    }
}

async fn vnc_accepts() -> bool {
    tokio::net::TcpStream::connect(("127.0.0.1", VNC_PORT)).await.is_ok()
}

async fn browser_window_shown() -> bool {
    command::new::<Command>("xdotool", command::Console::Hidden)
        .args(["search", "--onlyvisible", "--class", "chromium"])
        .env("DISPLAY", DISPLAY)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

// --- Teach-a-task recording -------------------------------------------------
//
// Two places a demonstration can be recorded (`TeachWhere`):
//
// - Local: the owner's own screen, on the computer this Nebo runs on. The
//   owner simply does the task where they always do it; a sampler takes a
//   full-screen still about once a second and keeps the ones that changed.
//   No window, no virtual machine, no ffmpeg.
// - Computer: the bot's on-demand virtual desktop. ffmpeg captures the SAME
//   X display x11vnc serves (one canonical frame source) as a deduped ~1fps
//   keyframe series and a small 5fps replay for the owner, and xinput logs
//   the raw input events, each stamped with its time into the recording.
//   The learner reads the keyframes and the timeline, never the replay.
//
// The owner's replay (session.mp4, the narration muxed in; for a recording
// of their own screen, made from its keyframes) is kept at most
// [`TEACH_VIDEO_KEEP`] on this disk.
//
// A recording runs for at most [`TEACH_CAP`]; the teach handler stops it
// there even when no app is left to.
//
// Artifacts land in the bot's own home: ~/teach-sessions/<id>/{frames/, and
// for the computer events.log; timeline.md where there are events or
// element records (elements.jsonl, from the browser extension);
// narration.md when the owner narrated a recording that has no timeline;
// session.mp4}.

/// The longest a demonstration records. At the cap it stops and is learned
/// from like any other; the app warns the owner before it.
pub const TEACH_CAP: Duration = Duration::from_secs(10 * 60);

/// The longest the owner's replay of a demonstration stays on this disk.
pub const TEACH_VIDEO_KEEP: Duration = Duration::from_secs(7 * 24 * 3600);

/// When the recording under way started, in ms since the epoch; 0 when none
/// is running. Element records stamped on this machine's clock are placed
/// on the recording's timeline by it.
static REC_STARTED_MS: AtomicU64 = AtomicU64::new(0);

/// When the recording under way started, in ms since the epoch.
pub fn recording_started_ms() -> Option<u64> {
    Some(REC_STARTED_MS.load(Ordering::Relaxed)).filter(|ms| *ms > 0)
}

/// Where a teach-a-task demonstration is recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeachWhere {
    /// This computer's own screen: the owner's desktop.
    Local,
    /// The bot's virtual computer (the on-demand desktop session).
    Computer,
}

impl TeachWhere {
    /// The caller's choice, or the default. A bot with a screen of its own
    /// (a Mac, a Windows or Linux desktop) records there; a cloud bot or a
    /// headless server has none and records on its virtual computer, and
    /// refuses `local`.
    pub fn resolve(requested: Option<&str>, own_screen: bool) -> Result<Self, String> {
        match requested.map(str::trim).filter(|r| !r.is_empty()) {
            None => Ok(if own_screen { Self::Local } else { Self::Computer }),
            Some("local") if own_screen => Ok(Self::Local),
            Some("local") => Err("this bot has no screen of its own to record; record on its computer instead".into()),
            Some("computer") => Ok(Self::Computer),
            Some(other) => Err(format!("where must be \"local\" or \"computer\", not {other:?}")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Computer => "computer",
        }
    }
}

/// The error a local recording returns when this computer will not let Nebo
/// record its screen (macOS Screen Recording permission). The teach handler
/// recognises it and tells the app, which offers the settings pane.
pub const SCREEN_RECORDING_PERMISSION: &str = "Nebo needs Screen Recording permission to watch you do the task. \
     Turn on Nebo in System Settings > Privacy & Security > Screen Recording, then try again.";

/// Whether this Nebo runs on a computer with a screen of its own, the one
/// the owner sits at. Not a cloud bot, not a headless server.
pub fn own_screen() -> bool {
    !crate::server_mode()
}

// One recording at a time: the variants' size difference costs nothing.
#[allow(clippy::large_enum_variant)]
enum Recorder {
    Computer { video: Child, frames: Child, events: Child, focus: Option<Child>, log: tokio::task::JoinHandle<()> },
    Local { stop: tokio_util::sync::CancellationToken, sampler: tokio::task::JoinHandle<Vec<FrameAt>> },
}

struct RecInner {
    id: String,
    dir: std::path::PathBuf,
    recorder: Recorder,
    started: std::time::Instant,
}

/// A keyframe and when it was taken: its number in frames/ and the time
/// into the recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameAt {
    pub frame: usize,
    pub at: Duration,
}

/// A finished recording, as [`stop_recording`] leaves it.
#[derive(Debug, Clone)]
pub struct Finished {
    pub id: String,
    pub dir: std::path::PathBuf,
    /// How many keyframes are in frames/.
    pub keyframes: usize,
    /// How long it recorded.
    pub length: Duration,
    /// When each keyframe was taken, where that is known (the owner's own
    /// screen); empty for the computer, whose timeline carries the times.
    pub frames: Vec<FrameAt>,
}

/// Something the owner said while recording: the time into the recording
/// and the words.
#[derive(Debug, Clone, PartialEq)]
pub struct Said {
    pub at: Duration,
    pub text: String,
}

/// How often the local recording looks at the screen.
const LOCAL_FRAME_EVERY: Duration = Duration::from_secs(1);
/// Local keyframes are scaled to this width: enough to read a screen,
/// small enough that a few minutes of them stay cheap to study.
const LOCAL_FRAME_MAX_WIDTH: u32 = 1280;
const LOCAL_FRAME_QUALITY: u8 = 70;

/// A captured screen (any format the image crate reads) as a keyframe JPEG.
/// Deterministic, so an unchanged screen yields the same bytes and is
/// skipped by the sampler.
fn to_keyframe(raw: &[u8]) -> Result<Vec<u8>, String> {
    let img = image::ImageReader::new(std::io::Cursor::new(raw))
        .with_guessed_format()
        .map_err(|e| e.to_string())?
        .decode()
        .map_err(|e| e.to_string())?;
    let img = if img.width() > LOCAL_FRAME_MAX_WIDTH {
        img.resize(LOCAL_FRAME_MAX_WIDTH, u32::MAX, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let mut out = std::io::Cursor::new(Vec::new());
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, LOCAL_FRAME_QUALITY);
    img.to_rgb8().write_with_encoder(encoder).map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}

/// One still of the owner's screen as a keyframe.
async fn local_keyframe(scratch: &std::path::Path) -> Result<Vec<u8>, String> {
    let path = scratch.to_string_lossy().into_owned();
    crate::desktop_tool::capture_full_screen(&path).await?;
    let raw = tokio::fs::read(scratch).await.map_err(|e| format!("screen capture: {e}"))?;
    let _ = tokio::fs::remove_file(scratch).await;
    tokio::task::spawn_blocking(move || to_keyframe(&raw))
        .await
        .map_err(|e| e.to_string())?
}

/// Keeps sampling the owner's screen until stopped, writing each frame that
/// differs from the last as frames/NNNN.jpg after `first`.
async fn sample_local(
    frames_dir: std::path::PathBuf,
    scratch: std::path::PathBuf,
    first: Vec<u8>,
    stop: tokio_util::sync::CancellationToken,
    started: std::time::Instant,
) -> Vec<FrameAt> {
    let mut written = 1usize;
    let mut taken = vec![FrameAt { frame: 1, at: Duration::ZERO }];
    let mut last = first;
    let mut tick = tokio::time::interval(LOCAL_FRAME_EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    tick.tick().await; // the first tick is immediate; `first` covers it
    loop {
        tokio::select! {
            _ = stop.cancelled() => break,
            _ = tick.tick() => {}
        }
        let frame = match local_keyframe(&scratch).await {
            Ok(f) => f,
            Err(e) => {
                warn!(error = %e, "teach: a local frame was not captured");
                continue;
            }
        };
        if frame == last {
            continue;
        }
        written += 1;
        if let Err(e) = tokio::fs::write(frames_dir.join(format!("{written:04}.jpg")), &frame).await {
            warn!(error = %e, "teach: a local frame was not saved");
        }
        taken.push(FrameAt { frame: written, at: started.elapsed() });
        last = frame;
    }
    let _ = tokio::fs::remove_file(&scratch).await;
    taken
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

/// A teach session's folder, by its id; `None` for anything that is not a
/// session id (it is a path segment).
pub fn session_dir(id: &str) -> Option<std::path::PathBuf> {
    uuid::Uuid::parse_str(id).ok().map(|u| teach_root().join(u.to_string()))
}

/// Start recording a demonstration `where_`. Returns (session_id,
/// session_dir). One recording at a time. On the computer the desktop must
/// be up (start it first); locally the first frame is taken before this
/// returns, so a screen that cannot be recorded (on macOS, Screen Recording
/// permission) is refused here rather than recorded as nothing.
pub async fn start_recording(where_: TeachWhere) -> Result<(String, std::path::PathBuf), String> {
    if where_ == TeachWhere::Computer && !active() {
        return Err("the desktop is not running. Start it first: open the Computer panel (it connects /ws/desktop, which starts the desktop) or POST /api/v1/desktop/teach/start, which starts it before recording".into());
    }
    let mut rec = rec_slot().lock().await;
    if rec.is_some() {
        return Err("a recording is already in progress".into());
    }

    let id = uuid::Uuid::new_v4().to_string();
    let dir = teach_root().join(&id);
    let frames_dir = dir.join("frames");
    std::fs::create_dir_all(&frames_dir).map_err(|e| format!("teach dir: {e}"))?;

    if where_ == TeachWhere::Local {
        if !crate::desktop_tool::screen_recording_allowed() {
            let _ = std::fs::remove_dir_all(&dir);
            return Err(SCREEN_RECORDING_PERMISSION.into());
        }
        // The capture command picks its format; the decoder sniffs it. Not a
        // dot-file: macOS screencapture refuses to write one (and exits 0).
        let scratch = dir.join(if cfg!(target_os = "macos") { "capture.jpg" } else { "capture.png" });
        let first = match local_keyframe(&scratch).await {
            Ok(f) => f,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&dir);
                return Err(e);
            }
        };
        std::fs::write(frames_dir.join("0001.jpg"), &first).map_err(|e| format!("teach frame: {e}"))?;
        let started = std::time::Instant::now();
        REC_STARTED_MS.store(now_ms(), Ordering::Relaxed);
        let stop = tokio_util::sync::CancellationToken::new();
        let sampler = tokio::spawn(sample_local(frames_dir, scratch, first, stop.clone(), started));
        info!(session = %id, "teach recording started on this computer's screen");
        *rec = Some(RecInner { id: id.clone(), dir: dir.clone(), recorder: Recorder::Local { stop, sampler }, started });
        return Ok((id, dir));
    }

    let started = std::time::Instant::now();
    REC_STARTED_MS.store(now_ms(), Ordering::Relaxed);
    // The owner's replay: 5fps is enough to follow a desktop, and a high CRF
    // keeps ten minutes of it small.
    let video = command::new::<Command>("ffmpeg", command::Console::Hidden)
        .args([
            "-loglevel", "error",
            "-f", "x11grab", "-framerate", "5", "-i", DISPLAY,
            "-c:v", "libx264", "-preset", "veryfast", "-crf", "32", "-pix_fmt", "yuv420p",
        ])
        .arg(dir.join(VIDEO))
        .stdin(std::process::Stdio::piped()) // 'q' for a clean mp4 finalize
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("recorder failed to start: {e}"))?;

    // Deduped ~1fps keyframes — the distillation pass reads these, not video.
    let frames = command::new::<Command>("ffmpeg", command::Console::Hidden)
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
    // Each event is stamped with its time into the recording on the way to
    // the file, so the timeline can say when, and narration can be put in
    // order beside it.
    let events_file = tokio::fs::File::create(dir.join("events.log"))
        .await
        .map_err(|e| format!("events log: {e}"))?;
    let mut events = command::new::<Command>("xinput", command::Console::Hidden)
        .args(["test-xi2", "--root"])
        .env("DISPLAY", DISPLAY)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("input logger failed to start: {e}"))?;
    // Which kind of field has the keyboard (AT-SPI focus): what is typed
    // into a password field never reaches the timeline in clear. Without
    // it the recording goes on; the timeline then redacts by shape alone.
    let mut focus = command::new::<Command>("python3", command::Console::Hidden)
        .args(["-c", FOCUS_WATCHER])
        .env("DISPLAY", DISPLAY)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| warn!(error = %e, "teach: the focus watcher did not start; typed secrets are redacted by shape alone"))
        .ok();
    let focus_out = focus.as_mut().and_then(|f| f.stdout.take());
    let focus_in = focus.as_mut().and_then(|f| f.stdin.take());
    let log = match events.stdout.take() {
        Some(out) => tokio::spawn(stamp_events(out, focus_out, focus_in, events_file, dir.join(DESKTOP_ELEMENTS), started)),
        None => tokio::spawn(async {}),
    };

    set_recording(true);
    info!(session = %id, "teach recording started");
    *rec = Some(RecInner { id: id.clone(), dir: dir.clone(), recorder: Recorder::Computer { video, frames, events, focus, log }, started });
    Ok((id, dir))
}

/// The line before each event in events.log: its time into the recording,
/// in milliseconds.
const EVENT_TIME: &str = "TIME ";

/// The line in events.log saying which kind of field the keyboard moved
/// to: `FOCUS password` or `FOCUS other`.
const FOCUS_LINE: &str = "FOCUS ";

/// Where the watcher's desktop element records go (`click <json>` lines),
/// each with its `t_ms` into the recording.
const DESKTOP_ELEMENTS: &str = "desktop-elements.jsonl";

/// The owner's replay of a demonstration.
pub const VIDEO: &str = "session.mp4";

/// Watches the desktop through AT-SPI (which the cloud image ships for the
/// desktop tool). Prints `focus password` or `focus other` each time a
/// field takes the keyboard (never what is in the field). For each
/// `at X Y T` line on its stdin (a press the input log saw, T ms into the
/// recording) it prints `click <json>`: the element under that screen point
/// (role, name, application, window, frame), so a click in a desktop app is
/// recorded as what was clicked, not only where. AT-SPI's own mouse events
/// carry no position on Xvfb and miss short presses, hence the input log
/// asks.
const FOCUS_WATCHER: &str = r#"
import json, os, sys, warnings, gi
warnings.simplefilter("ignore", DeprecationWarning)
gi.require_version("Atspi", "2.0")
from gi.repository import Atspi, GLib
def on(ev):
    if not ev.detail1:
        return
    try:
        role = ev.source.get_role_name()
    except Exception:
        return
    sys.stdout.write("focus password\n" if role == "password text" else "focus other\n")
    sys.stdout.flush()
Atspi.EventListener.new(on).register("object:state-changed:focused")
def name_of(node):
    name = node.get_name() or ""
    if name:
        return name
    try:
        ph = (node.get_attributes() or {}).get("placeholder-text")
        if ph:
            return ph
        for rel in node.get_relation_set() or []:
            if rel.get_relation_type() == Atspi.RelationType.LABELLED_BY and rel.get_n_targets():
                return rel.get_target(0).get_name() or ""
    except Exception:
        pass
    return node.get_description() or ""
def at_point(x, y):
    desk = Atspi.get_desktop(0)
    best = None
    for i in range(desk.get_child_count()):
        app = desk.get_child_at_index(i)
        for j in range(app.get_child_count() if app else 0):
            win = app.get_child_at_index(j)
            try:
                st = win.get_state_set()
                comp = win.get_component_iface()
                if not st.contains(Atspi.StateType.SHOWING) or not comp or not comp.contains(x, y, Atspi.CoordType.SCREEN):
                    continue
            except Exception:
                continue
            active = st.contains(Atspi.StateType.ACTIVE)
            if best is None or (active and not best[0]):
                best = (active, app, win)
    if not best:
        return None
    _, app, win = best
    node = win
    for _ in range(40):
        c = node.get_component_iface()
        nxt = c.get_accessible_at_point(x, y, Atspi.CoordType.SCREEN) if c else None
        if nxt is None or nxt == node:
            break
        node = nxt
    c = node.get_component_iface()
    e = c.get_extents(Atspi.CoordType.SCREEN) if c else None
    return {"role": node.get_role_name() or "", "name": name_of(node)[:100], "app": app.get_name() or "",
            "window": win.get_name() or "", "frame": [e.x, e.y, e.width, e.height] if e else None}
pending = b""
def asked(fd, cond):
    global pending
    chunk = os.read(fd, 4096)
    if not chunk:
        return False
    pending += chunk
    while b"\n" in pending:
        line, pending = pending.split(b"\n", 1)
        p = line.split()
        if len(p) != 4 or p[0] != b"at":
            continue
        try:
            x, y, t = int(float(p[1])), int(float(p[2])), int(p[3])
            hit = at_point(x, y)
        except Exception:
            hit = None
        if hit:
            hit.update({"x": x, "y": y, "t_ms": t})
            sys.stdout.write("click " + json.dumps(hit) + "\n")
            sys.stdout.flush()
    return True
GLib.io_add_watch(sys.stdin.fileno(), GLib.IO_IN | GLib.IO_HUP, asked)
Atspi.event_main()
"#;

/// Copies xinput's event dump into events.log, a [`EVENT_TIME`] line before
/// each event, and the watcher's focus moves as [`FOCUS_LINE`]s at their
/// time; its element records go to `elements` with their `t_ms`. Each
/// left or right press is handed to the watcher (`ask`) to name what is
/// under it.
async fn stamp_events(
    out: tokio::process::ChildStdout,
    focus: Option<tokio::process::ChildStdout>,
    mut ask: Option<tokio::process::ChildStdin>,
    file: tokio::fs::File,
    elements: std::path::PathBuf,
    started: std::time::Instant,
) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    let mut lines = tokio::io::BufReader::new(out).lines();
    let mut focus = focus.map(|f| tokio::io::BufReader::new(f).lines());
    let mut file = tokio::io::BufWriter::new(file);
    let time = |started: std::time::Instant| format!("{EVENT_TIME}{}\n", started.elapsed().as_millis());
    let mut press = PressReader::default();
    loop {
        tokio::select! {
            line = lines.next_line() => {
                let Ok(Some(line)) = line else { break };
                if line.trim_start().starts_with("EVENT type ") {
                    let _ = file.write_all(time(started).as_bytes()).await;
                }
                if let Some((x, y)) = press.feed(&line, started.elapsed())
                    && let Some(watcher) = ask.as_mut()
                    && watcher.write_all(format!("at {x} {y} {}\n", press.at.as_millis()).as_bytes()).await.is_err()
                {
                    ask = None;
                }
                let _ = file.write_all(line.as_bytes()).await;
                let _ = file.write_all(b"\n").await;
            }
            moved = async { focus.as_mut().unwrap().next_line().await }, if focus.is_some() => {
                match moved {
                    Ok(Some(said)) => {
                        let said = said.trim();
                        if let Some(record) = said.strip_prefix("click ") {
                            if let Ok(mut v) = serde_json::from_str::<serde_json::Value>(record) {
                                // The press's own time when the watcher echoes it.
                                if v.get("t_ms").and_then(|t| t.as_u64()).is_none() {
                                    v["t_ms"] = (started.elapsed().as_millis() as u64).into();
                                }
                                let mut f = tokio::fs::OpenOptions::new().create(true).append(true).open(&elements).await;
                                if let Ok(f) = f.as_mut() {
                                    let _ = f.write_all(format!("{v}\n").as_bytes()).await;
                                }
                            }
                        } else if let Some(kind) = said.strip_prefix("focus ") {
                            let kind = if kind == "password" { "password" } else { "other" };
                            let _ = file.write_all(format!("{}{FOCUS_LINE}{kind}\n", time(started)).as_bytes()).await;
                        }
                    }
                    _ => focus = None,
                }
            }
        }
    }
    let _ = file.flush().await;
}

/// Picks the left and right presses out of xinput's event dump, as it
/// streams: `feed` each line, and the press's root point comes back on its
/// `root:` line. `at` is when its event began.
#[derive(Default)]
struct PressReader {
    in_press: bool,
    button: u32,
    at: Duration,
}

impl PressReader {
    fn feed(&mut self, line: &str, now: Duration) -> Option<(i64, i64)> {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("EVENT type ") {
            self.in_press = rest.starts_with("4 ");
            self.button = 0;
            self.at = now;
        } else if let Some(d) = t.strip_prefix("detail: ") {
            self.button = d.trim().parse().unwrap_or(0);
        } else if let Some(xy) = t.strip_prefix("root: ")
            && self.in_press
        {
            self.in_press = false;
            if matches!(self.button, 1 | 3) {
                let (x, y) = xy.split_once('/')?;
                return Some((x.trim().parse::<f64>().ok()? as i64, y.trim().parse::<f64>().ok()? as i64));
            }
        }
        None
    }
}

/// The recording under way: its session id and how long it has run.
pub async fn recording() -> Option<(String, Duration)> {
    rec_slot().lock().await.as_ref().map(|r| (r.id.clone(), r.started.elapsed()))
}

/// No recording to stop: none is running, or (with `only`) the one
/// running is another.
pub const NOT_RECORDING: &str = "no recording in progress";

/// Stop the recording and finalize its artifacts. `only` stops it only when
/// it is that session (a cap that comes due must never stop a newer one).
/// No such recording in progress is [`NOT_RECORDING`].
pub async fn stop_recording(only: Option<&str>) -> Result<Finished, String> {
    let mut rec = rec_slot().lock().await;
    if only.is_some_and(|id| rec.as_ref().is_some_and(|r| r.id != id)) {
        return Err(NOT_RECORDING.into());
    }
    let Some(inner) = rec.take() else {
        return Err(NOT_RECORDING.into());
    };
    let mut frames_at = Vec::new();
    match inner.recorder {
        Recorder::Local { stop, sampler } => {
            stop.cancel();
            if let Ok(Ok(taken)) = tokio::time::timeout(Duration::from_secs(10), sampler).await {
                frames_at = taken;
            }
        }
        Recorder::Computer { mut video, mut frames, mut events, focus, log } => {
            set_recording(false);
            // Ask ffmpeg to finalize (the mp4's index, the last frame)
            // instead of killing it.
            for child in [&mut video, &mut frames] {
                if let Some(mut stdin) = child.stdin.take() {
                    use tokio::io::AsyncWriteExt;
                    let _ = stdin.write_all(b"q").await;
                }
            }
            let _ = tokio::time::timeout(Duration::from_secs(10), video.wait()).await;
            let _ = tokio::time::timeout(Duration::from_secs(10), frames.wait()).await;
            let _ = video.kill().await;
            let _ = frames.kill().await;
            let _ = events.kill().await;
            if let Some(mut f) = focus {
                let _ = f.kill().await;
            }
            // The stamper ends with xinput's output; the log is whole after it.
            let _ = tokio::time::timeout(Duration::from_secs(5), log).await;
        }
    }

    let count = std::fs::read_dir(inner.dir.join("frames"))
        .map(|d| d.count())
        .unwrap_or(0);

    // Events-first distillation: the raw X11 dump carries most of the task's
    // truth (the typed keys literally spell it out — proven in the field when
    // a blind study still recovered the task from keystrokes alone). Parse it
    // into a readable timeline so the study pass reads THIS first and uses
    // vision to confirm, instead of grepping 400KB of protocol noise.
    REC_STARTED_MS.store(0, Ordering::Relaxed);
    let _ = write_timeline(&inner.dir, &[]);

    let length = inner.started.elapsed();
    info!(session = %inner.id, keyframes = count, secs = length.as_secs(), "teach recording stopped");
    Ok(Finished { id: inner.id, dir: inner.dir, keyframes: count, length, frames: frames_at })
}

/// timeline.md from what the recording has: the input events (the
/// computer's events.log), the element records (elements.jsonl, the browser
/// extension's), or both, with `said` folded in. `None` when it has
/// neither.
fn write_timeline(dir: &std::path::Path, said: &[Said]) -> Option<Result<(), String>> {
    let log = std::fs::read_to_string(dir.join("events.log")).ok();
    let elements: Option<Vec<serde_json::Value>> = std::fs::read_to_string(dir.join("elements.jsonl"))
        .ok()
        .map(|raw| raw.lines().filter_map(|l| serde_json::from_str(l.trim()).ok()).collect());
    if log.is_none() && elements.is_none() {
        return None;
    }
    let timeline = summarize_x11_events(log.as_deref().unwrap_or(""), elements.as_deref().unwrap_or(&[]), said);
    Some(std::fs::write(dir.join("timeline.md"), timeline).map_err(|e| format!("timeline: {e}")))
}

/// Make the owner's replay, `session.mp4`, with the narration (`audio`,
/// starting `offset` into the recording) muxed in: the computer's own
/// capture, or, for a recording of the owner's screen, a video made from
/// its keyframes at the times they were taken. Errors when this machine
/// has no ffmpeg or nothing to make it from.
pub async fn finish_video(rec: &Finished, audio: Option<(&std::path::Path, Duration)>) -> Result<std::path::PathBuf, String> {
    let out = rec.dir.join(VIDEO);
    let offset = |o: Duration| format!("{:.3}", o.as_secs_f64());
    let mut cmd = command::new::<Command>("ffmpeg", command::Console::Hidden);
    cmd.args(["-loglevel", "error", "-y"]).current_dir(&rec.dir);
    let target = if out.exists() {
        // The computer's capture: the narration joins it, the video is copied.
        let Some((audio, at)) = audio else { return Ok(out) };
        cmd.args(["-i", VIDEO, "-itsoffset", &offset(at), "-i"])
            .arg(audio)
            .args(["-map", "0:v", "-map", "1:a", "-c:v", "copy", "-c:a", "aac", "-b:a", "64k", "-movflags", "+faststart"]);
        rec.dir.join("session.muxed.mp4")
    } else {
        if rec.frames.is_empty() {
            return Err("there are no keyframes to make a video from".into());
        }
        std::fs::write(rec.dir.join("video.txt"), slideshow(&rec.frames, rec.length)).map_err(|e| format!("video: {e}"))?;
        cmd.args(["-f", "concat", "-safe", "0", "-i", "video.txt"]);
        if let Some((audio, at)) = audio {
            cmd.args(["-itsoffset", &offset(at), "-i"]).arg(audio).args(["-map", "0:v", "-map", "1:a", "-c:a", "aac", "-b:a", "64k"]);
        }
        cmd.args([
            "-vf", "scale=trunc(iw/2)*2:trunc(ih/2)*2,fps=5",
            "-c:v", "libx264", "-preset", "veryfast", "-crf", "32", "-pix_fmt", "yuv420p", "-movflags", "+faststart",
        ]);
        out.clone()
    };
    cmd.arg(&target).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::piped()).kill_on_drop(true);
    let made = tokio::time::timeout(Duration::from_secs(300), cmd.output()).await;
    let _ = std::fs::remove_file(rec.dir.join("video.txt"));
    let made = made.map_err(|_| "making the video took longer than 5 minutes".to_string())?.map_err(|e| format!("ffmpeg is not available: {e}"))?;
    if !made.status.success() {
        let _ = std::fs::remove_file(&target);
        return Err(format!("ffmpeg could not make the video: {}", String::from_utf8_lossy(&made.stderr).trim()));
    }
    if target != out {
        std::fs::rename(&target, &out).map_err(|e| format!("video: {e}"))?;
    }
    Ok(out)
}

/// An ffmpeg concat list showing each keyframe from when it was taken
/// until the next, the last until the recording's end.
fn slideshow(frames: &[FrameAt], length: Duration) -> String {
    let mut out = String::new();
    for (i, f) in frames.iter().enumerate() {
        let until = frames.get(i + 1).map_or(length.max(f.at + Duration::from_secs(1)), |n| n.at);
        out.push_str(&format!("file 'frames/{:04}.jpg'\nduration {:.3}\n", f.frame, until.saturating_sub(f.at).as_secs_f64().max(0.04)));
    }
    // The concat demuxer holds the last entry's duration only when it is
    // listed again.
    if let Some(last) = frames.last() {
        out.push_str(&format!("file 'frames/{:04}.jpg'\n", last.frame));
    }
    out
}

/// Remove what a demonstration leaves that is the owner's own voice and
/// face, once it is older than `keep`: the replay and any narration audio
/// still on disk. The keyframes and timeline stay; they are what was
/// learned from.
pub fn sweep_teach_media(keep: Duration) {
    sweep_media_in(&teach_root(), keep);
}

fn sweep_media_in(root: &std::path::Path, keep: Duration) {
    let Ok(sessions) = std::fs::read_dir(root) else { return };
    let cutoff = std::time::SystemTime::now().checked_sub(keep);
    for session in sessions.flatten() {
        let Ok(files) = std::fs::read_dir(session.path()) else { continue };
        for f in files.flatten() {
            let name = f.file_name().to_string_lossy().into_owned();
            if !(name == VIDEO || name.starts_with("narration.") && name != "narration.md") {
                continue;
            }
            let old = f.metadata().and_then(|m| m.modified()).ok().zip(cutoff).is_some_and(|(at, cut)| at < cut);
            if old {
                let _ = std::fs::remove_file(f.path());
            }
        }
    }
}

/// Put what the owner said while recording beside what they did. A
/// recording with a timeline (the computer's) gets the words folded into
/// timeline.md in time order; one without (the owner's own screen) gets
/// narration.md, each line naming the keyframe on screen as it was said.
/// Returns the file written.
pub fn write_narration(rec: &Finished, said: &[Said]) -> Result<&'static str, String> {
    if let Some(written) = write_timeline(&rec.dir, said) {
        return written.map(|()| "timeline.md");
    }
    std::fs::write(rec.dir.join("narration.md"), narration_md(said, &rec.frames)).map_err(|e| format!("narration: {e}"))?;
    Ok("narration.md")
}

/// `mm:ss` into the recording.
fn clock(at: Duration) -> String {
    let s = at.as_secs();
    format!("{:02}:{:02}", s / 60, s % 60)
}

/// One narration line: `owner said: "…"`.
fn said_line(text: &str) -> String {
    format!("owner said: \"{}\"", text.trim())
}

/// narration.md: what the owner said, in order, each with the keyframe on
/// screen at that moment when the frames' times are known.
fn narration_md(said: &[Said], frames: &[FrameAt]) -> String {
    let mut out = String::from(
        "# The owner's narration

What the owner said while recording, in order. Each line starts with the time into the recording (mm:ss)",
    );
    out.push_str(if frames.is_empty() {
        ".\n\n"
    } else {
        " and the keyframe on screen as it was said (frames/NNNN.jpg).\n\n"
    });
    let mut said: Vec<&Said> = said.iter().collect();
    said.sort_by_key(|s| s.at);
    for s in said {
        let frame = frames.iter().rev().find(|f| f.at <= s.at).or(frames.first());
        match frame {
            Some(f) => out.push_str(&format!("{}  frame {:04}  {}\n", clock(s.at), f.frame, said_line(&s.text))),
            None => out.push_str(&format!("{}  {}\n", clock(s.at), said_line(&s.text))),
        }
    }
    out
}

/// What a typed run that may be a secret reads as in the timeline.
const TYPED_SECRET: &str = "Typed: [typed secret]";
/// A paste: what was pasted never reaches the timeline.
const PASTED: &str = "Pasted [pasted text]";

/// How near in time an element record and an input event must be to be
/// the same action.
const ELEMENT_MATCH_MS: u64 = 400;
/// And how near on screen, when both say where.
const ELEMENT_MATCH_PX: i64 = 40;

/// Put the browser extension's element records into the steps: each one
/// replaces the input event that was the same action (a click at the same
/// moment and place; every keystroke run typed while it was the field), so
/// a field it saw as sensitive never comes through from keycodes. Records
/// with no input event (a page loaded) are steps of their own.
fn merge_elements(steps: &mut Vec<(Duration, String)>, elements: &[serde_json::Value]) {
    let near = |a: Duration, b: u64| (a.as_millis() as i64 - b as i64).unsigned_abs() <= ELEMENT_MATCH_MS;
    let at_of = |step: &str| -> Option<(i64, i64)> {
        let inner = step.split_once(" at (")?.1.strip_suffix(')')?;
        let (x, y) = inner.split_once(", ")?;
        Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
    };
    for el in elements {
        let Some(t) = el["t_ms"].as_u64() else { continue };
        let action = el["action"].as_str().unwrap_or("");
        match action {
            "type" => {
                let end = el["t_end_ms"].as_u64().unwrap_or(t);
                let from = t.saturating_sub(ELEMENT_MATCH_MS);
                let to = end + ELEMENT_MATCH_MS;
                steps.retain(|(at, s)| {
                    let ms = at.as_millis() as u64;
                    !(s.starts_with("Typed: ") && ms >= from && ms <= to)
                });
            }
            "press" => {
                // The same key, pressed at the same moment, is one step.
                let key = el["key"].as_str().unwrap_or("");
                let token = match key {
                    "Escape" => "Esc",
                    "ArrowUp" => "Up",
                    "ArrowDown" => "Down",
                    "ArrowLeft" => "Left",
                    "ArrowRight" => "Right",
                    other => other,
                };
                let pressed = format!("Pressed [{token}]");
                if let Some(i) = steps.iter().position(|(at, s)| *s == pressed && near(*at, t)) {
                    steps.remove(i);
                }
            }
            "click" | "right_click" | "double_click" | "check" | "submit" | "select" => {
                let clicks = if action == "double_click" { 2 } else { 1 };
                let xy = el["x"].as_f64().zip(el["y"].as_f64()).map(|(x, y)| (x as i64, y as i64));
                for _ in 0..clicks {
                    let same = steps.iter().position(|(at, s)| {
                        (s.starts_with("Click at (") || s.starts_with("Right-click at ("))
                            && near(*at, t)
                            && match (xy, at_of(s)) {
                                (Some((ex, ey)), Some((sx, sy))) => (ex - sx).abs() <= ELEMENT_MATCH_PX && (ey - sy).abs() <= ELEMENT_MATCH_PX,
                                _ => true,
                            }
                    });
                    if let Some(i) = same {
                        steps.remove(i);
                    }
                }
            }
            _ => {}
        }
        if let Some(line) = element_line(el) {
            steps.push((Duration::from_millis(t), line));
        }
    }
}

/// One element record as a timeline step: `Click button "Save order" at
/// (386, 300)  locators: … | …  url: …  after: …  crop: …`.
fn element_line(el: &serde_json::Value) -> Option<String> {
    let text = |k: &str| el[k].as_str().map(str::trim).filter(|v| !v.is_empty());
    let action = text("action")?;
    let verb = match action {
        "click" => "Click",
        "right_click" => "Right-click",
        "double_click" => "Double-click",
        "type" => "Type into",
        "select" => "Select in",
        "check" => "Check",
        "submit" => "Submit",
        "press" => "Press",
        "navigate" => "Go to",
        other => other,
    };
    let mut line = verb.to_string();
    match action {
        "navigate" => line.push_str(&format!(" {}", text("url").unwrap_or(""))),
        "press" => line.push_str(&format!(" {}", text("key").unwrap_or(""))),
        _ => {
            if let Some(role) = text("role") {
                line.push_str(&format!(" {role}"));
            }
            if let Some(name) = text("name") {
                line.push_str(&format!(" \"{name}\""));
            }
        }
    }
    if let Some(app) = text("app") {
        line.push_str(&format!(" in {app}"));
        if let Some(window) = text("window") {
            line.push_str(&format!(" \"{window}\""));
        }
    }
    if let Some(form) = text("submits") {
        line.push_str(&format!(", submitting \"{form}\""));
    }
    if matches!(action, "type" | "select") {
        let value = match text("value") {
            Some("[hidden]") => "[typed secret]",
            Some(v) => v,
            None => "",
        };
        line.push_str(&format!(": {value}"));
    }
    if let (Some(x), Some(y)) = (el["x"].as_f64(), el["y"].as_f64()) {
        line.push_str(&format!(" at ({}, {})", x as i64, y as i64));
    }
    let locators: Vec<&str> = el["locators"].as_array().map(|a| a.iter().filter_map(|l| l.as_str()).collect()).unwrap_or_default();
    if !locators.is_empty() {
        line.push_str(&format!("  locators: {}", locators.join(" | ")));
    }
    if action != "navigate"
        && let Some(url) = text("url")
    {
        line.push_str(&format!("  url: {url}"));
    }
    let after = &el["after"];
    let seen = |k: &str| after[k].as_str().map(str::trim).filter(|v| !v.is_empty());
    let mut state: Vec<String> = Vec::new();
    if let Some(u) = seen("url") {
        state.push(format!("url={u}"));
    }
    if let Some(t) = seen("title") {
        state.push(format!("title \"{t}\""));
    }
    if let Some(h) = seen("heading") {
        state.push(format!("heading \"{h}\""));
    }
    if let Some(a) = seen("appeared") {
        state.push(format!("text \"{a}\" visible"));
    }
    if let (Some(r), Some(n)) = (after["visible"]["role"].as_str(), after["visible"]["name"].as_str()) {
        state.push(format!("{r} \"{n}\" visible"));
    }
    if !state.is_empty() {
        line.push_str(&format!("  after: {}", state.join("; ")));
    }
    if let Some(crop) = text("crop") {
        line.push_str(&format!("  crop: {crop}"));
    }
    Some(line)
}

/// The login shape (something typed, Tab, something typed, Enter): the
/// second is a password whether or not the focus watcher saw the field.
fn redact_logins(steps: &mut [(Duration, String)]) {
    for i in 0..steps.len().saturating_sub(3) {
        if steps[i].1.starts_with("Typed: ")
            && steps[i + 1].1 == "Pressed [Tab]"
            && steps[i + 2].1.starts_with("Typed: ")
            && steps[i + 3].1 == "Pressed [Enter]"
        {
            steps[i + 2].1 = TYPED_SECRET.to_string();
        }
    }
}

/// Turn an `xinput test-xi2` dump into a human/model-readable action timeline:
/// clicks with coordinates, typed text reassembled from keycodes (US layout),
/// special keys as tokens, each at its time into the recording. What the
/// owner said (`said`) is folded in, in time order. Best-effort — unknown
/// keycodes render as [#n].
fn summarize_x11_events(log: &str, elements: &[serde_json::Value], said: &[Said]) -> String {
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

Reconstructed from the input event log. Each line starts with the time into the recording (mm:ss). Typed text is reassembled from keycodes (US layout); clicks carry screen coordinates on the 1280x800 display. `[typed secret]` is something typed into a password field: never write it down; use a saved credential.

",
    );
    if !elements.is_empty() {
        out.push_str(
            "Where the page element is known, a step names it by role and name, then `locators:` (the ways to find it again, strongest first), the page's `url:`, and `after:` (what the screen showed once the step was done). Coordinates are the last resort.\n\n",
        );
    }
    if !said.is_empty() {
        out.push_str("Lines reading `owner said: \"…\"` are what the owner said aloud at that moment.\n\n");
    }
    let mut pending = Pending::None;
    let mut detail: u32 = 0;
    let mut now = Duration::ZERO;
    // The typed run in progress, when its first key was pressed, and
    // whether any of it went into a password field.
    let mut typed = String::new();
    let mut typed_at = Duration::ZERO;
    let mut secret = false;
    let mut in_password = false;
    let mut steps: Vec<(Duration, String)> = Vec::new();
    let flush_typed = |typed: &mut String, secret: &mut bool, at: Duration, steps: &mut Vec<(Duration, String)>| {
        if !typed.is_empty() {
            steps.push((at, if *secret { TYPED_SECRET.to_string() } else { format!("Typed: {typed}") }));
            typed.clear();
        }
        *secret = false;
    };
    for line in log.lines() {
        let t = line.trim();
        if let Some(ms) = t.strip_prefix(EVENT_TIME) {
            now = Duration::from_millis(ms.trim().parse().unwrap_or(0));
        } else if let Some(kind) = t.strip_prefix(FOCUS_LINE) {
            // A new field: what was typed in the last one is a run of its own.
            flush_typed(&mut typed, &mut secret, typed_at, &mut steps);
            in_password = kind.trim() == "password";
        } else if let Some(rest) = t.strip_prefix("EVENT type ") {
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
                    flush_typed(&mut typed, &mut secret, typed_at, &mut steps);
                    let btn = match detail {
                        1 => "Click",
                        3 => "Right-click",
                        4 | 5 => "Scroll",
                        _ => "Click",
                    };
                    if btn != "Scroll" {
                        let px = xy[0].trim().parse::<f64>().unwrap_or(0.0) as i64;
                        let py = xy[1].trim().parse::<f64>().unwrap_or(0.0) as i64;
                        steps.push((now, format!("{btn} at ({px}, {py})")));
                    }
                }
                pending = Pending::None;
            }
        } else if t.starts_with("modifiers:") && pending == Pending::Key {
            // The effective mask is hex ("0x5"), or "0": bit 0 Shift, bit 2
            // Control. Locks (NumLock 0x10) set other bits.
            let mask = t
                .split("effective:")
                .nth(1)
                .map(str::trim)
                .and_then(|m| match m.strip_prefix("0x") {
                    Some(hex) => u32::from_str_radix(hex, 16).ok(),
                    None => m.parse().ok(),
                })
                .unwrap_or(0);
            let shift = mask & 0x1 != 0;
            let ctrl = mask & 0x4 != 0;
            // A paste (Ctrl+V, Shift+Insert) never spells its text out: the
            // owner's clipboard reaches the computer over RFB, not as keys,
            // and the timeline says only that something was pasted.
            if (ctrl && detail == 55) || (shift && detail == 118) {
                flush_typed(&mut typed, &mut secret, typed_at, &mut steps);
                steps.push((now, PASTED.to_string()));
            } else if let Some(ch) =
                keychar(detail, shift).filter(|c| ctrl && c.chars().count() == 1)
            {
                flush_typed(&mut typed, &mut secret, typed_at, &mut steps);
                steps.push((now, format!("Pressed [Ctrl+{}]", ch.to_uppercase())));
            } else if let Some(ch) = keychar(detail, shift) {
                if ch.starts_with('[') {
                    flush_typed(&mut typed, &mut secret, typed_at, &mut steps);
                    steps.push((now, format!("Pressed {ch}")));
                } else {
                    if typed.is_empty() {
                        typed_at = now;
                    }
                    secret |= in_password;
                    typed.push_str(&ch);
                }
            }
            pending = Pending::None;
        }
    }
    flush_typed(&mut typed, &mut secret, typed_at, &mut steps);
    redact_logins(&mut steps);
    merge_elements(&mut steps, elements);
    steps.extend(said.iter().map(|s| (s.at, said_line(&s.text))));
    // Stable: at the same moment, what was done comes before what was said.
    steps.sort_by_key(|(at, _)| *at);
    for (at, s) in &steps {
        out.push_str(&format!("{}  {s}\n", clock(*at)));
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

    #[tokio::test]
    async fn the_ready_gate_waits_for_its_probe() {
        let mut calls = 0;
        let ready = wait_until(Duration::from_secs(5), Duration::from_millis(5), || {
            calls += 1;
            let up = calls >= 4;
            async move { up }
        })
        .await;
        assert!(ready);
        assert_eq!(calls, 4, "the gate opens on the first passing probe, not before");
    }

    #[tokio::test]
    async fn the_ready_gate_gives_up_at_its_limit() {
        let started = tokio::time::Instant::now();
        let ready = wait_until(Duration::from_millis(200), Duration::from_millis(30), || async { false }).await;
        assert!(!ready);
        let waited = started.elapsed();
        assert!(waited >= Duration::from_millis(150), "keeps probing until the limit: {waited:?}");
        assert!(waited < Duration::from_secs(2), "never waits far past the limit: {waited:?}");
    }

    #[test]
    fn opening_the_computer_never_starts_a_recording() {
        // The live view connects through ensure_started alone; recording is
        // only ever begun by the teach handler.
        let src = include_str!("desktop_session.rs");
        let start = src.find("pub async fn ensure_started").unwrap();
        let end = start + src[start..].find("\n}\n").unwrap();
        let body = &src[start..end];
        assert!(!body.contains("set_recording"), "ensure_started must not record");
        assert!(!body.contains("start_recording"), "ensure_started must not record");
    }

    #[test]
    fn teach_records_locally_by_default_on_a_bot_with_its_own_screen() {
        assert_eq!(TeachWhere::resolve(None, true), Ok(TeachWhere::Local));
        assert_eq!(TeachWhere::resolve(Some(""), true), Ok(TeachWhere::Local));
        assert_eq!(TeachWhere::resolve(Some("computer"), true), Ok(TeachWhere::Computer));
    }

    #[test]
    fn teach_records_on_the_computer_when_the_bot_has_no_screen() {
        assert_eq!(TeachWhere::resolve(None, false), Ok(TeachWhere::Computer));
        assert_eq!(TeachWhere::resolve(Some("computer"), false), Ok(TeachWhere::Computer));
        assert!(TeachWhere::resolve(Some("local"), false).is_err());
        assert!(TeachWhere::resolve(Some("elsewhere"), true).is_err());
    }

    #[test]
    fn keyframes_are_scaled_and_identical_screens_encode_identically() {
        let img = image::RgbImage::from_pixel(2560, 1600, image::Rgb([40, 80, 120]));
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img).write_to(&mut png, image::ImageFormat::Png).unwrap();
        let a = to_keyframe(png.get_ref()).unwrap();
        let b = to_keyframe(png.get_ref()).unwrap();
        assert_eq!(a, b, "an unchanged screen must be skipped");
        let decoded = image::load_from_memory(&a).unwrap();
        assert_eq!(decoded.width(), LOCAL_FRAME_MAX_WIDTH);
        assert_eq!(decoded.height(), 800);
    }

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
        let t = summarize_x11_events(log, &[], &[]);
        assert!(t.contains("Click at (386, 300)"), "click missing: {t}");
        assert!(t.contains("Typed: re"), "typed text missing: {t}");
        assert!(t.contains("Pressed [Enter]"), "enter missing: {t}");
    }

    /// Each step carries its time into the recording, and what the owner
    /// said is folded in beside it, in time order.
    #[test]
    fn narration_is_folded_into_the_timeline_in_time_order() {
        let log = "\
TIME 3000\n\
EVENT type 4 (ButtonPress)\n\
    detail: 1\n\
    root: 386.00/300.00\n\
TIME 14500\n\
EVENT type 2 (KeyPress)\n\
    detail: 27\n\
    modifiers: locked 0 latched 0 base 0 effective: 0\n\
TIME 14700\n\
EVENT type 2 (KeyPress)\n\
    detail: 26\n\
    modifiers: locked 0 latched 0 base 0 effective: 0\n\
TIME 75000\n\
EVENT type 2 (KeyPress)\n\
    detail: 36\n\
    modifiers: locked 0 latched 0 base 0 effective: 0\n";
        let said = [
            Said { at: Duration::from_millis(12_400), text: " filter last week ".into() },
            Said { at: Duration::from_millis(1_000), text: "go to the orders page".into() },
        ];
        let t = summarize_x11_events(log, &[], &said);
        let body: Vec<&str> = t.lines().filter(|l| l.as_bytes().first().is_some_and(u8::is_ascii_digit)).collect();
        assert_eq!(
            body,
            [
                "00:01  owner said: \"go to the orders page\"",
                "00:03  Click at (386, 300)",
                "00:12  owner said: \"filter last week\"",
                "00:14  Typed: re",
                "01:15  Pressed [Enter]",
            ]
        );
        assert!(t.contains("owner said"), "the header explains the narration lines: {t}");
        assert!(!summarize_x11_events(log, &[], &[]).contains("owner said"));
    }

    /// A recording of the owner's own screen has no timeline: its narration
    /// is written on its own, each line naming the keyframe on screen.
    #[test]
    fn narration_without_a_timeline_names_the_keyframe_on_screen() {
        let frames = [
            FrameAt { frame: 1, at: Duration::ZERO },
            FrameAt { frame: 2, at: Duration::from_secs(4) },
            FrameAt { frame: 3, at: Duration::from_secs(9) },
        ];
        let said = [
            Said { at: Duration::from_secs(5), text: "open the first one".into() },
            Said { at: Duration::from_millis(500), text: "this is the orders page".into() },
            Said { at: Duration::from_secs(61), text: "copy the PO number".into() },
        ];
        let md = narration_md(&said, &frames);
        let lines: Vec<&str> = md.lines().filter(|l| l.starts_with('0')).collect();
        assert_eq!(
            lines,
            [
                "00:00  frame 0001  owner said: \"this is the orders page\"",
                "00:05  frame 0002  owner said: \"open the first one\"",
                "01:01  frame 0003  owner said: \"copy the PO number\"",
            ]
        );
        assert!(narration_md(&said, &[]).lines().any(|l| l == "00:05  owner said: \"open the first one\""));
    }

    /// The learner reads keyframes and events; the owner gets a small
    /// replay.
    #[test]
    fn a_demonstration_records_keyframes_events_and_a_small_replay() {
        let src = include_str!("desktop_session.rs");
        let start = src.find("pub async fn start_recording").unwrap();
        let end = start + src[start..].find("\n}\n").unwrap();
        let body = &src[start..end];
        assert!(body.contains("\"-framerate\", \"5\"") && body.contains("\"-crf\", \"32\""), "a small 5fps replay is recorded");
        assert!(body.contains("mpdecimate"), "keyframes are");
        assert!(body.contains("test-xi2"), "input events are");
    }

    #[test]
    fn a_recording_is_capped_at_ten_minutes() {
        assert_eq!(TEACH_CAP, Duration::from_secs(600));
    }

    /// What is typed into a password field never reaches the timeline in
    /// clear: by the field's role when the focus watcher saw it, and by the
    /// login shape when it did not.
    // Proven against x11vnc 0.9.16 + Chromium under Xvfb: without the env
    // the first pastes are dropped; without -noprimary every highlight
    // overwrites the owner's clipboard; without the keeper Chromium keeps
    // pasting the first text.
    #[test]
    fn the_desktop_shares_its_clipboard_with_the_viewer() {
        let src = include_str!("desktop_session.rs");
        let start = src.find("pub async fn ensure_started").unwrap();
        let end = start + src[start..].find("\n}\n").unwrap();
        let body = &src[start..end];
        assert!(body.contains("\"-noprimary\""));
        assert!(body.contains(".env(\"X11VNC_AVOID_WINDOWS\", \"never\")"));
        assert!(body.contains("CLIPBOARD_KEEPER"));
        assert!(CLIPBOARD_KEEPER.contains("xclip -o -selection clipboard | xclip -selection clipboard"));
        assert!(!CLIPBOARD_KEEPER.contains("print("), "the keeper never prints clipboard text");
    }

    // A login: "ab", Ctrl+V of a password, Ctrl+C, Shift+Insert, then "t"
    // with NumLock on. The pasted text is never in events.log (it arrives
    // over RFB), and each paste reads as one step.
    #[test]
    fn timeline_redacts_a_paste_and_names_shortcuts() {
        let key = |ms: u32, detail: u32, effective: &str| {
            format!(
                "TIME {ms}\nEVENT type 2 (KeyPress)\n    detail: {detail}\n    root: 1.00/1.00\n    modifiers: locked 0 latched 0 base {effective} effective: {effective}\n"
            )
        };
        let log = [
            key(0, 38, "0"),
            key(100, 56, "0"),
            key(1000, 37, "0"),
            key(1100, 55, "0x4"),
            key(2000, 54, "0x4"),
            key(3000, 118, "0x1"),
            key(4000, 28, "0x10"),
        ]
        .concat();
        let t = summarize_x11_events(&log, &[], &[]);
        assert!(t.contains("00:00  Typed: ab\n"), "typed text missing: {t}");
        assert!(t.contains("00:01  Pasted [pasted text]\n"), "ctrl+v not redacted: {t}");
        assert!(t.contains("00:02  Pressed [Ctrl+C]\n"), "ctrl+c not named: {t}");
        assert!(t.contains("00:03  Pasted [pasted text]\n"), "shift+insert not redacted: {t}");
        assert!(t.contains("00:04  Typed: t\n"), "numlock must not read as shift: {t}");
        assert!(!t.contains("Typed: v") && !t.contains("Typed: c"), "shortcut keys leaked as text: {t}");
    }

    #[test]
    fn typed_passwords_are_redacted() {
        let key = |ms: u32, code: u32| {
            format!("TIME {ms}\nEVENT type 2 (KeyPress)\n    detail: {code}\n    modifiers: locked 0 latched 0 base 0 effective: 0\n")
        };
        // "re" in a text field, then "qwe" in a password field, then a click.
        let watched = format!(
            "TIME 0\nFOCUS other\n{}{}TIME 900\nFOCUS password\n{}{}{}TIME 2000\nFOCUS other\nTIME 2100\nEVENT type 4 (ButtonPress)\n    detail: 1\n    root: 10.00/20.00\n",
            key(100, 27),
            key(200, 26),
            key(1000, 24),
            key(1100, 25),
            key(1200, 26)
        );
        let t = summarize_x11_events(&watched, &[], &[]);
        assert!(t.contains("00:00  Typed: re"), "{t}");
        assert!(t.contains("00:01  Typed: [typed secret]"), "{t}");
        assert!(!t.contains("qwe"), "the password is never in the timeline: {t}");

        // No focus seen: user, Tab, password, Enter.
        let shaped = format!("{}{}{}{}", key(0, 27), key(100, 23), key(200, 24), key(300, 36));
        let t = summarize_x11_events(&shaped, &[], &[]);
        assert!(t.contains("Typed: r") && t.contains("Typed: [typed secret]"), "{t}");
        assert!(!t.contains("Typed: q"), "{t}");
        // A search typed and entered stays as typed.
        let search = format!("{}{}", key(0, 27), key(100, 36));
        assert!(summarize_x11_events(&search, &[], &[]).contains("Typed: r"));
    }

    /// An element the browser saw replaces the input event that was the
    /// same action: its role, name, locator chain, page and after-state,
    /// coordinates last. What it saw typed into a sensitive field never
    /// comes through from keycodes.
    #[test]
    fn element_records_replace_the_clicks_and_keystrokes_they_were() {
        let key = |ms: u32, code: u32| {
            format!("TIME {ms}\nEVENT type 2 (KeyPress)\n    detail: {code}\n    modifiers: locked 0 latched 0 base 0 effective: 0\n")
        };
        let log = format!(
            "TIME 3000\nEVENT type 4 (ButtonPress)\n    detail: 1\n    root: 386.00/300.00\n{}{}TIME 9000\nEVENT type 4 (ButtonPress)\n    detail: 1\n    root: 900.00/90.00\n",
            key(5000, 24),
            key(5100, 25)
        );
        let elements: Vec<serde_json::Value> = vec![
            serde_json::json!({"t_ms": 3050, "action": "click", "role": "button", "name": "Save order", "x": 388, "y": 302,
                "locators": ["role=button[name=\"Save order\"]", "css=#save-order", "coords=(386,300)"],
                "url": "https://app.example.com/orders/4471",
                "after": {"url": "/orders/4471/saved", "heading": "Order 4471", "appeared": "Order saved"},
                "crop": "elements/0001.jpg"}),
            serde_json::json!({"t_ms": 4980, "t_end_ms": 5150, "action": "type", "role": "textbox", "name": "Password", "value": "[hidden]",
                "locators": ["label=\"Password\""]}),
            serde_json::json!({"t_ms": 7000, "action": "navigate", "url": "https://app.example.com/reports"}),
        ];
        let t = summarize_x11_events(&log, &elements, &[]);
        let body: Vec<&str> = t.lines().filter(|l| l.as_bytes().first().is_some_and(u8::is_ascii_digit)).collect();
        assert_eq!(
            body,
            [
                "00:03  Click button \"Save order\" at (388, 302)  locators: role=button[name=\"Save order\"] | css=#save-order | coords=(386,300)  url: https://app.example.com/orders/4471  after: url=/orders/4471/saved; heading \"Order 4471\"; text \"Order saved\" visible  crop: elements/0001.jpg",
                "00:04  Type into textbox \"Password\": [typed secret]  locators: label=\"Password\"",
                "00:07  Go to https://app.example.com/reports",
                "00:09  Click at (900, 90)",
            ]
        );
        assert!(!t.contains("Typed: qw"), "{t}");
    }

    /// Left and right presses are picked out of the streaming dump with
    /// their root point; key presses, releases and wheel clicks are not.
    #[test]
    fn presses_are_read_from_the_input_dump() {
        let dump = "EVENT type 2 (KeyPress)\n    detail: 36\n    root: 10.00/10.00\n\
                    EVENT type 4 (ButtonPress)\n    device: 4 (4)\n    detail: 1\n    root: 386.00/300.00\n\
                    EVENT type 5 (ButtonRelease)\n    detail: 1\n    root: 386.00/300.00\n\
                    EVENT type 4 (ButtonPress)\n    detail: 4\n    root: 50.00/60.00\n\
                    EVENT type 4 (ButtonPress)\n    detail: 3\n    root: 12.50/700.25\n";
        let mut r = PressReader::default();
        let mut got = Vec::new();
        for (i, line) in dump.lines().enumerate() {
            if let Some(p) = r.feed(line, Duration::from_millis(i as u64 * 10)) {
                got.push((p, r.at.as_millis()));
            }
        }
        assert_eq!(got, [((386, 300), 30), ((12, 700), 130)], "each press at the time its event began");
    }

    /// The element recorder's own output (browser crate `teach::build_actions`)
    /// renders with its after-state, the next step that must show, and a
    /// desktop app click's application and window.
    #[test]
    fn element_lines_carry_the_next_step_and_the_desktop_app() {
        let browser = serde_json::json!({
            "t_ms": 14000, "action": "click", "source": "browser", "role": "link", "name": "Orders",
            "locators": ["role=link[name=\"Orders\"]", "css=#nav > a:nth-of-type(2)", "coords=(100,69)@1280x800"],
            "url": "https://shop.test/dashboard", "submits": "Search",
            "after": {"url": "https://shop.test/orders", "heading": "Orders", "visible": {"role": "button", "name": "New order"}}
        });
        assert_eq!(
            element_line(&browser).unwrap(),
            "Click link \"Orders\", submitting \"Search\"  locators: role=link[name=\"Orders\"] | css=#nav > a:nth-of-type(2) | coords=(100,69)@1280x800  \
             url: https://shop.test/dashboard  after: url=https://shop.test/orders; heading \"Orders\"; button \"New order\" visible"
        );
        let desktop = serde_json::json!({
            "t_ms": 9000, "action": "click", "source": "desktop", "role": "push button", "name": "New Folder",
            "app": "Thunar", "window": "Home", "x": 90, "y": 60,
            "locators": ["app=\"Thunar\" window=\"Home\" role=push button[name=\"New Folder\"]", "coords=(90,60)"]
        });
        assert_eq!(
            element_line(&desktop).unwrap(),
            "Click push button \"New Folder\" in Thunar \"Home\" at (90, 60)  locators: app=\"Thunar\" window=\"Home\" role=push button[name=\"New Folder\"] | coords=(90,60)"
        );
    }

    /// A recording of the owner's own screen with element records gets a
    /// timeline from them alone, and its narration goes there.
    #[test]
    fn element_records_alone_make_a_timeline() {
        let dir = std::env::temp_dir().join(format!("teach-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("elements.jsonl"), "{\"t_ms\": 2000, \"action\": \"click\", \"role\": \"link\", \"name\": \"Orders\"}\n").unwrap();
        let rec = Finished { id: "s".into(), dir: dir.clone(), keyframes: 1, length: Duration::from_secs(5), frames: vec![] };
        let said = [Said { at: Duration::from_secs(1), text: "open the orders".into() }];
        assert_eq!(write_narration(&rec, &said), Ok("timeline.md"));
        let t = std::fs::read_to_string(dir.join("timeline.md")).unwrap();
        assert!(t.contains("00:01  owner said: \"open the orders\"\n00:02  Click link \"Orders\""), "{t}");
        assert!(!dir.join("narration.md").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The replay of the owner's own screen shows each keyframe from when it
    /// was taken until the next, the last until the end.
    #[test]
    fn the_keyframe_replay_holds_each_frame_until_the_next() {
        let frames = [FrameAt { frame: 1, at: Duration::ZERO }, FrameAt { frame: 3, at: Duration::from_millis(2500) }];
        assert_eq!(
            slideshow(&frames, Duration::from_secs(10)),
            "file 'frames/0001.jpg'\nduration 2.500\nfile 'frames/0003.jpg'\nduration 7.500\nfile 'frames/0003.jpg'\n"
        );
    }

    /// The replay and any narration audio leave the disk after their week;
    /// the keyframes and timeline stay.
    #[test]
    fn replays_and_narration_audio_are_swept_after_their_week() {
        let home = std::env::temp_dir().join(format!("teach-home-{}", uuid::Uuid::new_v4()));
        let session = home.join("teach-sessions").join("s1");
        std::fs::create_dir_all(session.join("frames")).unwrap();
        for f in [VIDEO, "narration.m4a", "narration.md", "timeline.md", "frames/0001.jpg"] {
            std::fs::write(session.join(f), b"x").unwrap();
        }
        let root = home.join("teach-sessions");
        sweep_media_in(&root, TEACH_VIDEO_KEEP);
        assert!(session.join(VIDEO).exists(), "a fresh replay stays");
        sweep_media_in(&root, Duration::ZERO);
        assert!(!session.join(VIDEO).exists() && !session.join("narration.m4a").exists());
        for kept in ["narration.md", "timeline.md", "frames/0001.jpg"] {
            assert!(session.join(kept).exists(), "{kept} stays");
        }
        assert_eq!(TEACH_VIDEO_KEEP, Duration::from_secs(7 * 24 * 3600));
        let _ = std::fs::remove_dir_all(&home);
    }

    /// A key the browser saw pressed replaces the same keystroke from the
    /// input log; one pressed elsewhere stays.
    #[test]
    fn a_pressed_key_the_browser_saw_is_one_step() {
        let key = |ms: u32, code: u32| {
            format!("TIME {ms}\nEVENT type 2 (KeyPress)\n    detail: {code}\n    modifiers: locked 0 latched 0 base 0 effective: 0\n")
        };
        let log = format!("{}{}", key(1000, 23), key(5000, 9));
        let elements = vec![serde_json::json!({"t_ms": 1100, "action": "press", "key": "Tab"})];
        let t = summarize_x11_events(&log, &elements, &[]);
        assert!(t.contains("00:01  Press Tab") && !t.contains("Pressed [Tab]"), "{t}");
        assert!(t.contains("00:05  Pressed [Esc]"), "{t}");
    }
}
