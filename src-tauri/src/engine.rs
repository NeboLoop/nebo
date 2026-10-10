//! The engine (`nebo --engine`, the server) and the shell (this window
//! process) as two processes of one executable.
//!
//! The engine serves the local API on its port and runs the agents. The
//! shell is a client of that API like any other: it starts the engine as its
//! child (`supervise`), starts it again when it exits non-zero
//! (`server::process` has the exit codes), opens its window through a
//! sign-in ticket the engine mints (`attach`), and asks it what an app window
//! needs (`desktop_app`). "Quit Nebo" asks the engine to stop (`quit`).
//!
//! The child's stdin is a pipe the shell holds (`hold_lifeline`): when the
//! shell goes, however it goes, the engine stops the graceful way.
//!
//! When the OS runs the engine as a service (`service`: a LaunchAgent on
//! macOS, switched on from the update feed), the shell starts nothing itself:
//! it attaches, asks launchd to start a stopped engine, and falls back to the
//! child when the owner switched Nebo off in Login Items.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use tauri::Manager;

use crate::service;

/// `NEBO_SUPERVISED` for an engine the shell started as its child.
const SHELL: &str = "shell";

/// The engine's port: `NEBO_PORT`, else the default (27895).
static PORT: OnceLock<u16> = OnceLock::new();

/// Set once "Quit Nebo" goes ahead: nothing starts the engine again.
static QUITTING: AtomicBool = AtomicBool::new(false);

/// The last engine the shell started exited because the port was held.
static PORT_HELD: AtomicBool = AtomicBool::new(false);

/// The child engine is being stopped so the OS's service takes over: its
/// exit 0 does not close the app.
static HANDOVER: AtomicBool = AtomicBool::new(false);

/// How the shell runs its engine right now, for Settings, the banner and
/// the tray.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceView {
    /// The OS service is on offer here (asked for by the update feed or
    /// `NEBO_ENGINE_MODE`, on an OS that has one): Settings and the tray
    /// show Start at login.
    pub offered: bool,
    /// The owner's Start at login switch.
    pub start_at_login: bool,
    /// The OS runs the engine; otherwise it is this app's child.
    pub supervised: bool,
    /// Nebo is switched off in Login Items: the engine runs only while the
    /// app is open, and the window says how to change that.
    pub needs_approval: bool,
}

static VIEW: Mutex<ServiceView> =
    Mutex::new(ServiceView { offered: false, start_at_login: true, supervised: false, needs_approval: false });

pub fn service_view() -> ServiceView {
    *VIEW.lock().unwrap_or_else(|e| e.into_inner())
}

fn set_view(f: impl FnOnce(&mut ServiceView)) {
    f(&mut VIEW.lock().unwrap_or_else(|e| e.into_inner()));
}

pub fn port() -> u16 {
    *PORT.get_or_init(|| config::Config::load_embedded().map(|c| c.port).unwrap_or(27895))
}

/// The engine's own address.
pub fn url() -> String {
    format!("http://localhost:{}", port())
}

fn http() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(Duration::from_secs(2)).build()
}

/// A call of the shell's own to the engine, proving itself with the install key.
fn call(method: &str, path: &str) -> ureq::Request {
    call_on(port(), method, path)
}

/// [`call`] to the engine on `port`.
fn call_on(port: u16, method: &str, path: &str) -> ureq::Request {
    let req = http().request(method, &format!("http://127.0.0.1:{port}{path}"));
    match config::read_install_key() {
        Some(key) => req.set("Authorization", &format!("Bearer {key}")),
        None => req,
    }
}

/// A JSON answer's body.
fn json<T: serde::de::DeserializeOwned>(resp: ureq::Response) -> Option<T> {
    serde_json::from_reader(resp.into_reader()).ok()
}

// ── The engine role ─────────────────────────────────────────────────────

/// `nebo --engine`: the server, with no window, no Tauri and no AppKit.
/// Exits with `server::process::exit_code` (or `EXIT_STALL` from the
/// watchdog): its supervisor reads it.
pub fn run_engine() -> ! {
    // A GUI or service launch inherits a minimal PATH; the CLIs the agents
    // run live elsewhere. Before any thread starts: it sets PATH.
    config::ensure_full_path();
    tracing::info!(entries = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).count()).unwrap_or(0), "PATH augmented for the CLIs agents run");

    let mut cfg = config::Config::load_embedded().expect("failed to load config");
    let settings = config::load_settings().expect("failed to load settings");
    cfg.auth.access_secret = settings.access_secret;
    cfg.auth.access_expire = settings.access_expire;
    cfg.auth.refresh_token_expire = settings.refresh_token_expire;
    config::ensure_data_dir().expect("failed to create data directory");

    let supervisor = server::process::supervisor();
    tracing::info!(pid = std::process::id(), port = cfg.port, supervisor = ?supervisor, exe = ?std::env::current_exe().ok(), "starting the Nebo engine");
    if supervisor.as_deref() == Some(SHELL) {
        hold_lifeline();
    }
    #[cfg(unix)]
    if supervisor.as_deref() == Some("launchd") {
        redirect_stdio();
    }

    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    let result = rt.block_on(server::run(cfg, true));
    let code = server::process::exit_code(&result);
    match &result {
        Ok(()) => tracing::info!("engine stopped"),
        Err(e) => tracing::error!(code, "engine exited with error: {e}"),
    }
    std::process::exit(code);
}

/// launchd's agent has nowhere for stdout and stderr (its plist can't name
/// a path under the home folder): they go to `logs/engine-stdio.log`.
#[cfg(unix)]
fn redirect_stdio() {
    use std::os::fd::AsRawFd;
    let Ok(dir) = config::data_dir() else { return };
    let path = dir.join("logs").join("engine-stdio.log");
    match std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        Ok(file) => {
            // SAFETY: duplicates an open descriptor onto stdout and stderr.
            unsafe {
                libc::dup2(file.as_raw_fd(), 1);
                libc::dup2(file.as_raw_fd(), 2);
            }
        }
        Err(e) => tracing::warn!(error = %e, path = %path.display(), "engine: stdout and stderr stay unredirected"),
    }
}

/// Stop the engine when the shell that started it is gone: its stdin is
/// the shell's pipe, which reads to its end only once the shell exits.
fn hold_lifeline() {
    let spawned = std::thread::Builder::new().name("nebo-lifeline".into()).spawn(|| {
        use std::io::Read;
        let mut sink = [0u8; 64];
        let mut stdin = std::io::stdin();
        while matches!(stdin.read(&mut sink), Ok(n) if n > 0) {}
        server::process::stop("the app that started the engine is gone");
    });
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "engine: no lifeline to the app that started it");
    }
}

// ── Supervising the engine ──────────────────────────────────────────────

/// What the shell does after its engine exits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AfterExit {
    /// Stopped on purpose (Quit, an update): the shell goes too.
    StayDown,
    /// Start it again after this long.
    Restart(Duration),
}

/// How long the port wait is when another process holds it.
const PORT_HELD_RETRY: Duration = Duration::from_secs(2);
/// A run this long was healthy: the crash backoff starts over.
const HEALTHY_RUN: Duration = Duration::from_secs(60);
const FIRST_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// The restart policy for an engine that exited with `code` (None: killed by
/// a signal) after running `ran`, with `crashes` crashes in a row before it.
/// Returns what to do and the crash count to carry on.
pub(crate) fn after_exit(code: Option<i32>, ran: Duration, crashes: u32) -> (AfterExit, u32) {
    match code {
        Some(0) => (AfterExit::StayDown, 0),
        Some(server::process::EXIT_PORT_HELD) => (AfterExit::Restart(PORT_HELD_RETRY), crashes),
        _ => {
            let crashes = if ran >= HEALTHY_RUN { 1 } else { crashes + 1 };
            let backoff = FIRST_BACKOFF.saturating_mul(1 << (crashes - 1).min(5)).min(MAX_BACKOFF);
            (AfterExit::Restart(backoff), crashes)
        }
    }
}

/// What answers on the engine's port.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Health {
    /// An engine of this version, healthy.
    Ours,
    /// An engine of another version: an older app's, still running.
    Stale,
    /// Nothing, or not an engine.
    None,
}

fn health_on(port: u16) -> Health {
    let Ok(resp) = http().get(&format!("http://127.0.0.1:{port}/health")).call() else { return Health::None };
    let Some(health) = json::<serde_json::Value>(resp) else { return Health::None };
    classify(&health)
}

fn classify(health: &serde_json::Value) -> Health {
    if health["role"] != "engine" {
        Health::None
    } else if health["version"] != env!("CARGO_PKG_VERSION") {
        Health::Stale
    } else if health["status"] == "ok" {
        Health::Ours
    } else {
        Health::None
    }
}

/// An engine of this version answers on the port, healthy.
fn engine_answers() -> bool {
    health_on(port()) == Health::Ours
}

/// Keep an engine serving for as long as the shell runs: as the OS's
/// service when the app runs it as one ([`service::plan`]), otherwise as
/// this process's child, started again as [`after_exit`] says. When a child
/// engine stops on purpose, the shell exits.
/// Decides how the engine runs before it returns (Settings and the tray read
/// it), then supervises on its own thread.
pub fn supervise(app: tauri::AppHandle) {
    let first = start();
    let spawned = std::thread::Builder::new().name("nebo-engine-supervisor".into()).spawn(move || {
        let mut run = first;
        let mut crashes = 0;
        let mut watch = ServiceWatch::default();
        loop {
            if QUITTING.load(Ordering::SeqCst) {
                return;
            }
            run = match run {
                Run::Child => match run_child(&mut crashes) {
                    ChildEnd::Stopped if HANDOVER.swap(false, Ordering::SeqCst) => {
                        tracing::info!("the app's engine stopped; the OS runs it from here");
                        watch = ServiceWatch::default();
                        set_view(|v| {
                            v.supervised = true;
                            v.needs_approval = false;
                        });
                        Run::Service
                    }
                    ChildEnd::Stopped => {
                        tracing::info!("engine stopped; closing the app");
                        app.exit(0);
                        return;
                    }
                    ChildEnd::Again => Run::Child,
                },
                Run::Service => match service_tick(&mut watch) {
                    Tick::Serving => Run::Service,
                    Tick::Fallback { banner } => {
                        fall_back(banner);
                        Run::Child
                    }
                    Tick::Updating => {
                        tracing::info!("Nebo is updating; closing the app (the update opens it again)");
                        QUITTING.store(true, Ordering::SeqCst);
                        app.exit(0);
                        return;
                    }
                },
            };
        }
    });
    if let Err(e) = spawned {
        tracing::error!(error = %e, "could not start the engine supervisor");
    }
}

/// How the shell runs the engine at this moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Run {
    Child,
    Service,
}

/// The app opens: decide how its engine runs ([`service::plan`]) and do the
/// registration part of it.
fn start() -> Run {
    let target = service::Target::app();
    let saved = service::load();
    let dev = tauri::is_dev();
    let mode = service::wanted_mode(std::env::var("NEBO_ENGINE_MODE").ok().as_deref(), saved.engine_mode.as_deref());
    // A child-only app (the default, and dev) asks nothing of the OS beyond
    // whether a registration is left to remove.
    let status = if dev { service::Status::Unsupported } else { service::status(&target) };
    let exe = std::env::current_exe().unwrap_or_default();
    let facts = service::Facts {
        dev,
        mode,
        start_at_login: saved.start_at_login,
        transient_path: service::is_transient(&exe),
        status,
    };
    let plan = service::plan(&facts);
    tracing::info!(?mode, ?status, ?plan, start_at_login = saved.start_at_login, "how the engine runs");
    set_view(|v| {
        v.offered = !dev && mode == service::Mode::Service && status != service::Status::Unsupported;
        v.start_at_login = saved.start_at_login;
    });
    if !dev {
        service::refresh_feed_mode();
    }
    match plan {
        service::Plan::Child { banner, unregister } => {
            if unregister && let Err(e) = service::unregister(&target) {
                tracing::warn!(error = %e, "could not remove the engine's registration");
            }
            if banner {
                fall_back(true);
            }
            Run::Child
        }
        service::Plan::MoveToApplications => {
            std::thread::spawn(say_move_to_applications);
            Run::Child
        }
        service::Plan::Register => match service::install(&target) {
            Ok(service::Status::Enabled) => {
                tracing::info!("engine registered with the OS");
                set_view(|v| v.supervised = true);
                Run::Service
            }
            Ok(status) => {
                tracing::warn!(?status, "engine registered but not enabled; it runs while the app is open");
                fall_back(status == service::Status::RequiresApproval);
                Run::Child
            }
            Err(e) => {
                tracing::error!(error = %e, "could not register the engine; it runs while the app is open");
                Run::Child
            }
        },
        service::Plan::Service => {
            set_view(|v| v.supervised = true);
            Run::Service
        }
    }
}

/// The engine runs as this app's child from here. With `banner`, Nebo is
/// switched off in Login Items: the window says so, and once it is switched
/// on again the OS takes the engine over.
fn fall_back(banner: bool) {
    set_view(|v| {
        v.supervised = false;
        v.needs_approval = banner;
    });
    if banner {
        tracing::warn!("Nebo is switched off in Login Items; the engine runs only while the app is open");
        std::thread::spawn(|| watch_for_approval(service::Target::app()));
    }
}

/// While Nebo is switched off in Login Items: check every few seconds, and
/// once it is on, stop the child so the OS runs the engine.
fn watch_for_approval(target: service::Target) {
    loop {
        std::thread::sleep(Duration::from_secs(10));
        if QUITTING.load(Ordering::SeqCst) || !service_view().needs_approval {
            return;
        }
        if service::status(&target) == service::Status::Enabled && service::job(&target) != service::Job::Absent {
            hand_over(&target);
            return;
        }
    }
}

/// Stop the child engine the graceful way so the OS's runs instead.
fn hand_over(target: &service::Target) {
    tracing::info!("Nebo is on in Login Items; handing the engine to the OS");
    set_view(|v| v.needs_approval = false);
    HANDOVER.store(true, Ordering::SeqCst);
    quit_engine(target.port);
}

/// How one run of the child engine ended.
enum ChildEnd {
    /// It exited 0: stopped on purpose.
    Stopped,
    /// Look again (it exited and is due a restart, or another engine answers).
    Again,
}

/// One turn of the child loop: attach to an engine that answers, otherwise
/// start one and wait for it to exit.
fn run_child(crashes: &mut u32) -> ChildEnd {
    if engine_answers() {
        std::thread::sleep(Duration::from_secs(2));
        return ChildEnd::Again;
    }
    let started = Instant::now();
    let mut child = match spawn_engine() {
        Ok(child) => child,
        Err(e) => {
            tracing::error!(error = %e, "could not start the engine");
            std::thread::sleep(MAX_BACKOFF);
            return ChildEnd::Again;
        }
    };
    tracing::info!(pid = child.id(), "engine started");
    // Held until the engine exits: `wait` would close it first.
    let lifeline = child.stdin.take();
    let status = child.wait();
    drop(lifeline);
    let code = status.as_ref().ok().and_then(|s| s.code());
    PORT_HELD.store(code == Some(server::process::EXIT_PORT_HELD), Ordering::SeqCst);
    let (next, now) = after_exit(code, started.elapsed(), *crashes);
    *crashes = now;
    match next {
        AfterExit::StayDown => ChildEnd::Stopped,
        AfterExit::Restart(after) => {
            if code == Some(server::process::EXIT_PORT_HELD) {
                tracing::warn!(port = port(), "the engine's port is held by another process; trying again");
            } else {
                tracing::error!(status = ?status, after_secs = after.as_secs(), "engine exited; starting it again");
            }
            std::thread::sleep(after);
            ChildEnd::Again
        }
    }
}

/// What the service loop remembers between looks.
#[derive(Debug, Default)]
struct ServiceWatch {
    /// Since when no engine of this version has answered.
    down_since: Option<Instant>,
    /// When the shell last asked launchd to start the engine.
    kicked: Option<Instant>,
}

/// One look at a service-run engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tick {
    Serving,
    /// The OS will not run it: the app's child runs it instead.
    Fallback { banner: bool },
    /// An update is swapping the app: the shell goes too.
    Updating,
}

/// How long launchd gets to start an engine it was asked for before the
/// shell runs the engine itself (`launchctl disable` leaves the
/// registration "enabled" while nothing starts).
const KICK_GRACE: Duration = Duration::from_secs(15);
/// How long a started engine may take to answer before it is started again.
const START_LIMIT: Duration = Duration::from_secs(120);
/// How often a stale engine (an older version's) is asked to restart.
const STALE_KICK_EVERY: Duration = Duration::from_secs(30);

fn service_tick(watch: &mut ServiceWatch) -> Tick {
    let target = service::Target::app();
    match health_on(target.port) {
        Health::Ours => {
            watch.down_since = None;
            watch.kicked = None;
            std::thread::sleep(Duration::from_secs(2));
            return Tick::Serving;
        }
        Health::Stale => {
            // The bundle holds this version; the engine still runs the old
            // one. A restart runs the new.
            if watch.kicked.is_none_or(|at| at.elapsed() >= STALE_KICK_EVERY) {
                tracing::info!("the engine runs an older version; restarting it");
                if let Err(e) = service::kickstart(&target, true) {
                    tracing::warn!(error = %e, "could not restart the engine");
                }
                watch.kicked = Some(Instant::now());
            }
            std::thread::sleep(Duration::from_secs(2));
            return Tick::Serving;
        }
        Health::None => {}
    }
    if config::data_dir().is_ok_and(|d| updater::updating(&d)) {
        return Tick::Updating;
    }
    match service::status(&target) {
        service::Status::Enabled => {}
        status => return Tick::Fallback { banner: status == service::Status::RequiresApproval },
    }
    let down = *watch.down_since.get_or_insert_with(Instant::now);
    let kicked_ago = watch.kicked.map(|at| at.elapsed());
    match service::job(&target) {
        service::Job::Running(_) => {
            // Starting (migrations, a first index load). Started again only
            // when it has had far longer than any start takes.
            if down.elapsed() >= START_LIMIT && kicked_ago.is_none_or(|ago| ago >= START_LIMIT) {
                tracing::warn!("the engine has not answered in {}s; restarting it", START_LIMIT.as_secs());
                let _ = service::kickstart(&target, true);
                watch.kicked = Some(Instant::now());
            }
        }
        job => {
            if kicked_ago.is_some_and(|ago| ago >= KICK_GRACE) {
                tracing::warn!(?job, "launchd did not start the engine; it runs while the app is open");
                return Tick::Fallback { banner: true };
            }
            if kicked_ago.is_none() {
                if job == service::Job::Absent
                    && let Err(e) = service::install(&target)
                {
                    tracing::warn!(error = %e, "could not register the engine again");
                }
                tracing::info!("starting the engine");
                if let Err(e) = service::kickstart(&target, false) {
                    tracing::warn!(error = %e, "launchd would not start the engine; it runs while the app is open");
                    return Tick::Fallback { banner: true };
                }
                watch.kicked = Some(Instant::now());
            }
        }
    }
    std::thread::sleep(Duration::from_secs(1));
    Tick::Serving
}

/// Start at login, from Settings or the tray: on registers the engine with
/// the OS (which then runs it), off removes the registration (the engine
/// runs while the app is open). Kept for the next open either way.
pub fn set_start_at_login(on: bool) -> Result<ServiceView, String> {
    let target = service::Target::app();
    let mut saved = service::load();
    saved.start_at_login = on;
    service::save(&saved);
    set_view(|v| v.start_at_login = on);
    if !service_view().offered {
        return Ok(service_view());
    }
    if on {
        match service::install(&target)? {
            service::Status::Enabled if !service_view().supervised => hand_over(&target),
            service::Status::RequiresApproval if !service_view().needs_approval => fall_back(true),
            _ => {}
        }
    } else {
        // launchd stops the engine (its graceful path); the supervisor
        // starts the app's child.
        service::unregister(&target)?;
        set_view(|v| v.needs_approval = false);
    }
    Ok(service_view())
}

/// "Nebo is running" / "Nebo is starting…" / "Nebo is stopped", for the tray.
pub fn status_line() -> &'static str {
    if QUITTING.load(Ordering::SeqCst) {
        "Nebo is stopped"
    } else if engine_answers() {
        "Nebo is running"
    } else {
        "Nebo is starting…"
    }
}

/// Opened from a disk image or a translocated copy: registering would point
/// the OS at a path that goes away. Nebo still runs, as the app's child.
fn say_move_to_applications() {
    rfd::MessageDialog::new()
        .set_title("Move Nebo to Applications")
        .set_description(
            "Nebo is open from a disk image or a temporary copy. Move Nebo to your Applications folder, then open it from there.",
        )
        .set_buttons(rfd::MessageButtons::Ok)
        .show();
}

/// This executable as the engine, a child of the shell: its own process
/// group (a Ctrl-C in the terminal reaches the shell, and the engine stops
/// by its lifeline, the graceful way), stdin the lifeline.
fn spawn_engine() -> std::io::Result<std::process::Child> {
    let exe = std::env::current_exe()?;
    let mut cmd: std::process::Command = command::new(exe, command::Console::Hidden);
    cmd.arg("--engine").env("NEBO_SUPERVISED", SHELL).stdin(std::process::Stdio::piped());
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    cmd.spawn()
}

// ── The window ──────────────────────────────────────────────────────────

/// The page the main window shows while no engine answers: a static page
/// from the app's own files, never the engine's (`app/static/starting.html`).
pub const STARTING_PAGE: &str = "starting.html";

/// The starting page, told whether the port is held by another process
/// (`#held`: it says so instead of "taking longer than usual").
fn starting_url(page: &tauri::Url, held: bool) -> tauri::Url {
    let mut url = page.clone();
    url.set_fragment(held.then_some("held"));
    url
}

/// A sign-in path from the engine (`POST /api/v1/local-session/ticket`).
fn sign_in_path() -> Option<String> {
    let resp = call("POST", "/api/v1/local-session/ticket").call().ok()?;
    let body: serde_json::Value = json(resp)?;
    body["path"].as_str().map(str::to_string)
}

/// How long the engine may not answer before the window shows the starting
/// page again: short gaps stay silent.
const SILENT_GAP: u32 = 3;

/// Keep the main window on the engine: open it through a fresh sign-in once
/// the engine answers, and back on the starting page (where it opened) when
/// the engine stops answering for longer than a short gap.
pub fn attach(app: tauri::AppHandle, frontend: String) {
    let Some(starting) = app.get_webview_window("main").and_then(|w| w.url().ok()) else { return };
    let spawned = std::thread::Builder::new().name("nebo-engine-attach".into()).spawn(move || {
        let mut attached = false;
        let mut misses = 0;
        let mut held_shown = false;
        loop {
            if QUITTING.load(Ordering::SeqCst) {
                return;
            }
            let Some(window) = app.get_webview_window("main") else { return };
            if engine_answers() {
                misses = 0;
                if !attached
                    && let Some(path) = sign_in_path()
                    && let Ok(url) = format!("{frontend}{path}").parse()
                {
                    tracing::info!("engine answering; opening the window on it");
                    attached = window.navigate(url).is_ok();
                }
            } else if attached {
                misses += 1;
                if misses >= SILENT_GAP {
                    tracing::warn!("engine not answering; the window shows the starting page");
                    held_shown = PORT_HELD.load(Ordering::SeqCst);
                    let _ = window.navigate(starting_url(&starting, held_shown));
                    attached = false;
                }
            } else if PORT_HELD.load(Ordering::SeqCst) != held_shown {
                held_shown = !held_shown;
                let _ = window.navigate(starting_url(&starting, held_shown));
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    });
    if let Err(e) = spawned {
        tracing::error!(error = %e, "could not watch the engine for the window");
    }
}

// ── An app window's facts ───────────────────────────────────────────────

/// What the engine says about one app for its window (`GET
/// /api/v1/apps/{id}/desktop`).
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DesktopApp {
    pub offers_publish: bool,
    pub developer_script: String,
    pub ui_dir: Option<PathBuf>,
}

/// How long one answer serves the window: every asset of an app page asks.
const DESKTOP_APP_FOR: Duration = Duration::from_secs(5);

/// [`DesktopApp`] for `agent_id`, empty when the engine does not answer.
pub fn desktop_app(agent_id: &str) -> DesktopApp {
    static CACHE: Mutex<Option<HashMap<String, (Instant, DesktopApp)>>> = Mutex::new(None);
    if let Some((at, app)) = CACHE.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_default().get(agent_id)
        && at.elapsed() < DESKTOP_APP_FOR
    {
        return app.clone();
    }
    let path = format!("/api/v1/apps/{}/desktop", urlencoding_component(agent_id));
    let app = call("GET", &path)
        .call()
        .ok()
        .and_then(json::<DesktopApp>)
        .unwrap_or_default();
    CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_default()
        .insert(agent_id.to_string(), (Instant::now(), app.clone()));
    app
}

/// `s` as one path segment.
fn urlencoding_component(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

// ── Quit ────────────────────────────────────────────────────────────────

/// How long the shell waits for the engine to stop before it exits anyway
/// (the engine finishes stopping on its own).
const QUIT_WAIT: Duration = Duration::from_secs(15);

/// Tell the engine what the main window is doing (`focused`, `background`,
/// `hidden`): its stall reports carry it (`POST /api/v1/client/events`, the
/// shell's own `window` event). Only a change is sent; off the main thread.
pub fn window_state(state: &'static str) {
    static LAST: std::sync::Mutex<&str> = std::sync::Mutex::new("");
    {
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        if *last == state {
            return;
        }
        *last = state;
    }
    std::thread::spawn(move || {
        let body = serde_json::json!({ "event": "window", "detail": state }).to_string();
        let _ = call("POST", "/api/v1/client/events").set("Content-Type", "application/json").send_string(&body);
    });
}

/// "Quit Nebo": when work is in flight, ask first; then the engine stops
/// the graceful way, and the shell exits once it has (or after
/// [`QUIT_WAIT`]). Runs off the main thread.
pub fn quit(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        let working = call("GET", "/api/v1/runs/active")
            .call()
            .ok()
            .and_then(json::<serde_json::Value>)
            .and_then(|v| v["runs"].as_array().map(Vec::len))
            .unwrap_or(0);
        if working > 0 && !confirm_quit(working) {
            return;
        }
        QUITTING.store(true, Ordering::SeqCst);
        quit_engine(port());
        app.exit(0);
    });
}

/// Ask the engine on `port` to stop the graceful way (it exits 0, so
/// launchd leaves it down), and wait up to [`QUIT_WAIT`] for it to go.
pub fn quit_engine(port: u16) {
    if let Err(e) = call_on(port, "POST", "/api/v1/engine/quit").call() {
        tracing::warn!(error = %e, "the engine did not take Quit");
        return;
    }
    let deadline = Instant::now() + QUIT_WAIT;
    while Instant::now() < deadline && health_on(port) != Health::None {
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn confirm_quit(working: usize) -> bool {
    let things = if working == 1 { "1 thing".to_string() } else { format!("{working} things") };
    rfd::MessageDialog::new()
        .set_title("Quit Nebo?")
        .set_description(format!(
            "Nebo is working on {things}. Quit anyway? They'll pick up when you open Nebo again."
        ))
        .set_buttons(rfd::MessageButtons::OkCancelCustom("Quit".into(), "Cancel".into()))
        .show()
        == rfd::MessageDialogResult::Custom("Quit".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use server::process::{EXIT_PORT_HELD, EXIT_STALL};

    #[test]
    fn a_clean_exit_stays_down() {
        assert_eq!(after_exit(Some(0), Duration::from_secs(5), 3), (AfterExit::StayDown, 0));
    }

    #[test]
    fn a_held_port_is_retried_without_backoff() {
        assert_eq!(after_exit(Some(EXIT_PORT_HELD), Duration::ZERO, 2), (AfterExit::Restart(PORT_HELD_RETRY), 2));
    }

    #[test]
    fn crashes_and_stalls_restart_with_backoff() {
        let quick = Duration::from_secs(1);
        let (next, crashes) = after_exit(Some(EXIT_STALL), quick, 0);
        assert_eq!((next, crashes), (AfterExit::Restart(FIRST_BACKOFF), 1));
        let (next, crashes) = after_exit(None, quick, crashes);
        assert_eq!((next, crashes), (AfterExit::Restart(FIRST_BACKOFF * 2), 2));
        let (next, _) = after_exit(Some(101), quick, 20);
        assert_eq!(next, AfterExit::Restart(MAX_BACKOFF));
    }

    #[test]
    fn a_long_healthy_run_resets_the_backoff() {
        assert_eq!(after_exit(Some(1), HEALTHY_RUN, 9), (AfterExit::Restart(FIRST_BACKOFF), 1));
    }

    #[test]
    fn the_starting_page_says_when_the_port_is_held() {
        let page: tauri::Url = "tauri://localhost/starting.html".parse().unwrap();
        assert_eq!(starting_url(&page, true).as_str(), "tauri://localhost/starting.html#held");
        assert_eq!(starting_url(&starting_url(&page, true), false).as_str(), "tauri://localhost/starting.html");
    }

    #[test]
    fn health_tells_ours_from_an_older_engine() {
        let me = env!("CARGO_PKG_VERSION");
        let h = |v: serde_json::Value| classify(&v);
        assert_eq!(h(serde_json::json!({"role": "engine", "version": me, "status": "ok"})), Health::Ours);
        assert_eq!(h(serde_json::json!({"role": "engine", "version": "0.0.1", "status": "ok"})), Health::Stale);
        assert_eq!(h(serde_json::json!({"role": "engine", "version": me, "status": "stalled"})), Health::None);
        // An in-process Nebo (0.16.x) says no role: not an engine to restart.
        assert_eq!(h(serde_json::json!({"version": "0.16.14", "status": "ok"})), Health::None);
    }

    #[test]
    fn app_ids_are_one_path_segment() {
        assert_eq!(urlencoding_component("Design Studio/x"), "Design%20Studio%2Fx");
        assert_eq!(urlencoding_component("app-1_a.b~"), "app-1_a.b~");
    }
}
