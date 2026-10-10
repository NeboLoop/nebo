//! This process as the desktop app's engine: how it stops, what its exit
//! code tells whoever started it, and what it asks of the OS so it keeps
//! full speed with no window.
//!
//! The desktop app runs the server as its own process (`nebo --engine`); the
//! window process (the shell) attaches to it over this API. Whoever started
//! the engine reads its exit code to decide whether to start it again:
//!
//! | code | meaning | supervisor |
//! |---|---|---|
//! | 0 | stopped on purpose (Quit, an update, a signal) | leaves it down |
//! | [`EXIT_STALL`] | the stall watchdog fired (`liveness`) | starts it again |
//! | [`EXIT_PORT_HELD`] | another process holds the port | starts it again after a pause |
//! | anything else | a crash | starts it again |

use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Json, Response};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};
use tokio::sync::Notify;
use tracing::info;

use types::NeboError;

/// `EX_SOFTWARE`: the stall watchdog stopped a supervised engine.
pub const EXIT_STALL: i32 = 70;
/// `EX_TEMPFAIL`: the port is held by another process (another Nebo engine,
/// or something else); a supervisor tries again once it is free.
pub const EXIT_PORT_HELD: i32 = 75;

/// The exit code for how `server::run` ended.
pub fn exit_code(result: &Result<(), NeboError>) -> i32 {
    match result {
        Ok(()) => 0,
        Err(NeboError::PortInUse(_)) => EXIT_PORT_HELD,
        Err(_) => 1,
    }
}

/// Who starts this engine again when it exits non-zero, from
/// `NEBO_SUPERVISED`: the desktop shell that started it as its child
/// (`shell`), or the OS's service manager. None for a server nothing
/// restarts (`nebo-cli serve`, a cloud pod, where Kubernetes probes `/health`).
pub fn supervisor() -> Option<String> {
    std::env::var("NEBO_SUPERVISED").ok().filter(|s| !s.is_empty())
}

/// This executable where it stays, for anything that starts it later (the
/// browser's native-messaging manifest, the OS service): an AppImage's own
/// file (`$APPIMAGE`), never the `/tmp/.mount_*` copy its runtime runs, which
/// is gone once it exits.
pub fn stable_exe() -> std::io::Result<std::path::PathBuf> {
    stable_exe_from(std::env::var_os("APPIMAGE"), std::env::current_exe)
}

fn stable_exe_from(
    appimage: Option<std::ffi::OsString>,
    current: impl FnOnce() -> std::io::Result<std::path::PathBuf>,
) -> std::io::Result<std::path::PathBuf> {
    match appimage.filter(|p| !p.is_empty()) {
        Some(file) => Ok(file.into()),
        None => current(),
    }
}

/// End this process at once with `code`, from any thread: no exit handlers,
/// no destructors, no flushing. For the stall watchdog: in a stall the
/// other threads are stuck, and an ordinary exit waits on locks they hold
/// (seen on Linux: the watchdog logged its exit, and the process stayed
/// until the frozen threads moved again), so the supervisor never got its
/// exit code. What must reach disk is written before this is called.
pub(crate) fn exit_now(code: i32) -> ! {
    #[cfg(unix)]
    // SAFETY: `_exit` ends the process; it touches no Rust state.
    unsafe {
        libc::_exit(code)
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
        // SAFETY: ends this process (the pseudo-handle) with `code`.
        unsafe { TerminateProcess(GetCurrentProcess(), code as u32) };
        std::process::exit(code)
    }
}

static STOP: LazyLock<Notify> = LazyLock::new(Notify::new);

/// Stop this server the graceful way a shutdown signal does: drain, close,
/// exit 0. Callable from any thread, before or after the server serves.
pub fn stop(reason: &str) {
    info!(reason, "engine asked to stop");
    STOP.notify_one();
}

/// Resolves once [`stop`] is called.
pub(crate) async fn stopped() {
    STOP.notified().await;
}

/// Whether a request is the desktop shell's own call: the install key as its
/// bearer token, and no page behind it. A page in an app window reaches this
/// server through the shell's `neboapp://` proxy, which also carries the key
/// but always names the page's `Origin`; a signed-in browser holds a session,
/// never the key.
pub(crate) fn from_the_shell(headers: &HeaderMap) -> bool {
    let Some(key) = config::read_install_key().filter(|k| !k.is_empty()) else { return false };
    headers.get(header::ORIGIN).is_none()
        && crate::middleware::bearer(headers).is_some_and(|t| crate::handlers::ws::constant_time_eq(t, &key))
}

/// POST /api/v1/engine/quit: the shell's "Quit Nebo". The engine stops the
/// graceful way and exits 0, so no supervisor starts it again. The one stop
/// path on every OS: Windows has no SIGTERM.
pub async fn quit(headers: HeaderMap) -> Response {
    if !from_the_shell(&headers) {
        return (StatusCode::UNAUTHORIZED, "the install key is required").into_response();
    }
    stop("Quit Nebo");
    (StatusCode::ACCEPTED, Json(serde_json::json!({ "stopping": true }))).into_response()
}

/// Keep the OS from slowing this process down while no window of it is in
/// front: it serves the local API and runs agents with no window at all.
/// Each opt-out logs a line saying it took. Neither keeps the machine awake.
pub(crate) fn hold_full_speed() {
    #[cfg(target_os = "macos")]
    hold_app_nap_opt_out();
    #[cfg(windows)]
    opt_out_of_power_throttling();
}

/// Keep macOS from napping Nebo. App Nap moved the whole process to
/// background priority 4 with throttled I/O and coalesced timers; under load
/// it got no CPU for minutes: no answer on :27895, even /health, and the
/// NeboAI write loop exited on "sleep drift" (60-182 s, 2026-10-07..09).
/// Held for the life of the process. `UserInitiatedAllowingIdleSystemSleep`
/// lets the Mac idle-sleep as before: no power assertion.
#[cfg(target_os = "macos")]
fn hold_app_nap_opt_out() {
    use objc2_foundation::{NSActivityOptions, NSProcessInfo, NSString};
    let activity = NSProcessInfo::processInfo().beginActivityWithOptions_reason(
        NSActivityOptions::UserInitiatedAllowingIdleSystemSleep,
        &NSString::from_str("Nebo serves the local API and runs agents in the background"),
    );
    // Never ended: the activity lasts as long as the process.
    std::mem::forget(activity);
    FULL_SPEED_HELD.store(true, Ordering::Relaxed);
    info!("App Nap opt-out held (idle system sleep still allowed)");
}

/// Keep Windows 11 from putting Nebo in EcoQoS (efficiency mode) or
/// coarsening its timers while it has no window in front: the same
/// starvation App Nap caused on macOS. A state mask of 0 over both controls
/// turns both throttles off for this process. No power request is taken: the
/// PC still sleeps.
#[cfg(windows)]
fn opt_out_of_power_throttling() {
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION, PROCESS_POWER_THROTTLING_STATE, ProcessPowerThrottling,
        SetProcessInformation,
    };
    let state = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED | PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION,
        StateMask: 0,
    };
    // SAFETY: the pseudo-handle of this process and a correctly sized,
    // initialized struct that outlives the call.
    let ok = unsafe {
        SetProcessInformation(
            GetCurrentProcess(),
            ProcessPowerThrottling,
            (&state as *const PROCESS_POWER_THROTTLING_STATE).cast(),
            std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        )
    };
    if ok != 0 {
        FULL_SPEED_HELD.store(true, Ordering::Relaxed);
        info!("power throttling opt-out held (EcoQoS and timer coarsening off; the PC still sleeps)");
    } else {
        tracing::warn!(error = %std::io::Error::last_os_error(), "power throttling opt-out not taken");
    }
}

/// The App Nap (macOS) or power-throttling (Windows) opt-out is held.
static FULL_SPEED_HELD: AtomicBool = AtomicBool::new(false);

// ── What a stall report says about this machine ─────────────────────────

/// The desktop window as the shell last told it (`POST /api/v1/client/events`,
/// event `window`): focused, background, hidden. "none" until the shell says.
static WINDOW_STATE: Mutex<String> = Mutex::new(String::new());

/// The shell's word on its window, kept for stall reports.
pub(crate) fn set_window_state(state: &str) {
    let state: String = state.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').take(16).collect();
    *WINDOW_STATE.lock().unwrap_or_else(|e| e.into_inner()) = state;
}

static OS_VERSION: LazyLock<String> =
    LazyLock::new(|| sysinfo::System::os_version().unwrap_or_default());

/// The OS version: macOS's product version (kern.osproductversion), Linux's
/// os-release VERSION_ID, Windows's version with its build.
pub(crate) fn os_version() -> String {
    OS_VERSION.clone()
}

/// This process and machine now, for a stall report (`types::stall`). No
/// user content.
pub(crate) fn stall_context() -> types::stall::Context {
    let window = WINDOW_STATE.lock().unwrap_or_else(|e| e.into_inner()).clone();
    types::stall::Context {
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        os: std::env::consts::OS.to_string(),
        os_version: os_version(),
        cpus: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0),
        load: sysinfo::System::load_average().one,
        window_state: if window.is_empty() { "none".into() } else { window },
        app_nap_opt_out: FULL_SPEED_HELD.load(Ordering::Relaxed),
        supervisor: supervisor().unwrap_or_default(),
    }
}

// ── engine-run.json: how the last run ended ─────────────────────────────

/// The file each run keeps in the data directory, so the next one knows
/// whether it ended on purpose. The same on every OS and every supervisor.
const RUN_FILE: &str = "engine-run.json";
const DAY_SECS: i64 = 24 * 3600;

/// One run of this server, as `engine-run.json` holds it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EngineRun {
    pid: u32,
    started_at: i64,
    /// Last seen alive (the watchdog stamps it): when a crash happened, near enough.
    #[serde(default)]
    alive_at: i64,
    /// Set by a run that stopped on purpose. Missing: it crashed, was
    /// killed, or the watchdog exited it.
    #[serde(default)]
    clean_exit: bool,
    /// When each restart in the last 24 hours happened, unix seconds.
    #[serde(default)]
    restarts: Vec<i64>,
}

/// How the previous run ended, when it did not stop on purpose.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LastExit {
    /// The exit code when known: [`EXIT_STALL`] for the watchdog.
    code: Option<i32>,
    /// `stall` (the watchdog exited it) or `crash` (a crash, a kill, a power loss).
    reason: &'static str,
    /// When, unix seconds (for a crash: when it was last seen alive).
    at: i64,
}

/// This run, from the previous one: a run that did not end cleanly makes
/// this one a restart.
fn next_run(prev: Option<EngineRun>, stalled: Option<i64>, pid: u32, now: i64) -> (EngineRun, Option<LastExit>) {
    let mut run = EngineRun { pid, started_at: now, alive_at: now, ..Default::default() };
    let Some(prev) = prev else { return (run, None) };
    run.restarts = prev.restarts.into_iter().filter(|t| now - t < DAY_SECS).collect();
    if prev.clean_exit {
        return (run, None);
    }
    run.restarts.push(now);
    let last = match stalled {
        Some(at) => LastExit { code: Some(EXIT_STALL), reason: "stall", at },
        None => LastExit { code: None, reason: "crash", at: prev.alive_at.max(prev.started_at) },
    };
    (run, Some(last))
}

struct RunState {
    dir: std::path::PathBuf,
    run: EngineRun,
    last_exit: Option<LastExit>,
}

static RUN: Mutex<Option<RunState>> = Mutex::new(None);

fn unix_now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn write_run(dir: &Path, run: &EngineRun) {
    let tmp = dir.join(format!("{RUN_FILE}.tmp"));
    let written = serde_json::to_vec(run)
        .map_err(std::io::Error::other)
        .and_then(|body| std::fs::write(&tmp, body))
        .and_then(|()| std::fs::rename(&tmp, dir.join(RUN_FILE)));
    if let Err(e) = written {
        tracing::warn!(error = %e, "engine-run.json not written");
    }
}

/// This run begins: read how the last one ended (and the stall it ended
/// inside, `stalled_at`), then record this one. Called once the port is held.
pub(crate) fn begin_run(dir: &Path, stalled_at: Option<i64>) {
    let prev = std::fs::read(dir.join(RUN_FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok());
    let (run, last_exit) = next_run(prev, stalled_at, std::process::id(), unix_now());
    if let Some(last) = &last_exit {
        tracing::warn!(reason = last.reason, code = ?last.code, restarts_24h = run.restarts.len(), "the last run did not stop on purpose");
    }
    write_run(dir, &run);
    *RUN.lock().unwrap_or_else(|e| e.into_inner()) = Some(RunState { dir: dir.to_path_buf(), run, last_exit });
}

/// The watchdog's stamp: this run is alive now.
pub(crate) fn alive() {
    let mut guard = RUN.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(state) = guard.as_mut() {
        state.run.alive_at = unix_now();
        write_run(&state.dir, &state.run);
    }
}

/// This run stops on purpose: the next one is not a restart.
pub(crate) fn end_run() {
    let mut guard = RUN.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(state) = guard.as_mut() {
        state.run.clean_exit = true;
        state.run.alive_at = unix_now();
        write_run(&state.dir, &state.run);
    }
}

/// The `engine` object of the workforce report: who supervises this
/// engine, the OS version, restarts in the last 24 hours, and how the last
/// run ended until the platform has acked it once ([`engine_reported`]).
pub(crate) fn engine_report() -> serde_json::Value {
    let guard = RUN.lock().unwrap_or_else(|e| e.into_inner());
    let now = unix_now();
    let (restarts, last_exit) = match guard.as_ref() {
        Some(s) => (s.run.restarts.iter().filter(|t| now - **t < DAY_SECS).count(), s.last_exit.clone()),
        None => (0, None),
    };
    let mut engine = serde_json::json!({
        "supervisor": supervisor().unwrap_or_default(),
        "osVersion": os_version(),
        "appVersion": env!("CARGO_PKG_VERSION"),
        "restarts24h": restarts,
    });
    if let Some(last) = last_exit {
        engine["lastExit"] = serde_json::json!(last);
    }
    engine
}

/// The platform has this run's `lastExit`: it is not sent again.
pub(crate) fn engine_reported() {
    if let Some(state) = RUN.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        state.last_exit = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_appimage_is_its_own_file_not_its_mount() {
        let mount = || Ok(std::path::PathBuf::from("/tmp/.mount_NeboAbc/usr/bin/nebo"));
        let file = stable_exe_from(Some("/home/a/Applications/Nebo.AppImage".into()), mount).unwrap();
        assert_eq!(file, std::path::PathBuf::from("/home/a/Applications/Nebo.AppImage"));
        assert_eq!(stable_exe_from(Some("".into()), mount).unwrap(), mount().unwrap());
        assert_eq!(stable_exe_from(None, mount).unwrap(), mount().unwrap());
    }

    #[test]
    fn exit_codes_tell_the_supervisor_what_happened() {
        assert_eq!(exit_code(&Ok(())), 0);
        assert_eq!(exit_code(&Err(NeboError::PortInUse(27895))), EXIT_PORT_HELD);
        assert_eq!(exit_code(&Err(NeboError::Server("boom".into()))), 1);
        assert_ne!(EXIT_STALL, EXIT_PORT_HELD);
    }

    #[test]
    fn a_first_run_and_a_clean_stop_are_not_restarts() {
        let (run, last) = next_run(None, None, 7, 1_000);
        assert_eq!((run.pid, run.started_at, run.clean_exit, last), (7, 1_000, false, None));
        let prev = EngineRun { clean_exit: true, restarts: vec![900], ..run };
        let (run, last) = next_run(Some(prev), None, 8, 2_000);
        assert_eq!(last, None);
        assert_eq!(run.restarts, vec![900]);
    }

    #[test]
    fn a_run_that_did_not_stop_cleanly_counts_as_a_restart() {
        let prev = EngineRun { pid: 1, started_at: 1_000, alive_at: 1_500, clean_exit: false, restarts: vec![] };
        let (run, last) = next_run(Some(prev.clone()), None, 2, 1_600);
        assert_eq!(last, Some(LastExit { code: None, reason: "crash", at: 1_500 }));
        assert_eq!(run.restarts, vec![1_600]);
        // The watchdog exited it inside a stall.
        let (_, last) = next_run(Some(prev), Some(1_400), 2, 1_600);
        assert_eq!(last, Some(LastExit { code: Some(EXIT_STALL), reason: "stall", at: 1_400 }));
    }

    #[test]
    fn restarts_older_than_a_day_drop_off() {
        let now = 10 * DAY_SECS;
        let prev = EngineRun { restarts: vec![now - DAY_SECS - 1, now - 60], ..Default::default() };
        let (run, _) = next_run(Some(prev), None, 3, now);
        assert_eq!(run.restarts, vec![now - 60, now]);
    }

    #[test]
    fn window_state_keeps_only_a_short_word() {
        set_window_state("hidden<script>alert(1)</script>");
        assert_eq!(stall_context().window_state, "hiddenscriptaler");
        set_window_state("focused");
        assert_eq!(stall_context().window_state, "focused");
    }
}
