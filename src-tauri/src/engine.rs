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

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use tauri::Manager;

/// `NEBO_SUPERVISED` for an engine the shell started as its child.
const SHELL: &str = "shell";

/// The engine's port: `NEBO_PORT`, else the default (27895).
static PORT: OnceLock<u16> = OnceLock::new();

/// Set once "Quit Nebo" goes ahead: nothing starts the engine again.
static QUITTING: AtomicBool = AtomicBool::new(false);

/// The last engine the shell started exited because the port was held.
static PORT_HELD: AtomicBool = AtomicBool::new(false);

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
    let req = http().request(method, &format!("http://127.0.0.1:{}{path}", port()));
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
    tracing::info!(pid = std::process::id(), port = cfg.port, supervisor = ?supervisor, "starting the Nebo engine");
    if supervisor.as_deref() == Some(SHELL) {
        hold_lifeline();
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

/// An engine of this version answers on the port, healthy.
fn engine_answers() -> bool {
    let Ok(resp) = http().get(&format!("http://127.0.0.1:{}/health", port())).call() else { return false };
    let Some(health) = json::<serde_json::Value>(resp) else { return false };
    health["role"] == "engine" && health["version"] == env!("CARGO_PKG_VERSION") && health["status"] == "ok"
}

/// Keep an engine serving for as long as the shell runs: attach to one that
/// already answers, otherwise start one as a child and start it again as
/// [`after_exit`] says. When the engine stops on purpose, the shell exits.
pub fn supervise(app: tauri::AppHandle) {
    let spawned = std::thread::Builder::new().name("nebo-engine-supervisor".into()).spawn(move || {
        let mut crashes = 0;
        loop {
            if QUITTING.load(Ordering::SeqCst) {
                return;
            }
            if engine_answers() {
                std::thread::sleep(Duration::from_secs(2));
                continue;
            }
            let started = Instant::now();
            let mut child = match spawn_engine() {
                Ok(child) => child,
                Err(e) => {
                    tracing::error!(error = %e, "could not start the engine");
                    std::thread::sleep(MAX_BACKOFF);
                    continue;
                }
            };
            tracing::info!(pid = child.id(), "engine started");
            // Held until the engine exits: `wait` would close it first.
            let lifeline = child.stdin.take();
            let status = child.wait();
            drop(lifeline);
            let code = status.as_ref().ok().and_then(|s| s.code());
            PORT_HELD.store(code == Some(server::process::EXIT_PORT_HELD), Ordering::SeqCst);
            let (next, now) = after_exit(code, started.elapsed(), crashes);
            crashes = now;
            match next {
                AfterExit::StayDown => {
                    tracing::info!("engine stopped; closing the app");
                    app.exit(0);
                    return;
                }
                AfterExit::Restart(after) => {
                    if code == Some(server::process::EXIT_PORT_HELD) {
                        tracing::warn!(port = port(), "the engine's port is held by another process; trying again");
                    } else {
                        tracing::error!(status = ?status, after_secs = after.as_secs(), "engine exited; starting it again");
                    }
                    std::thread::sleep(after);
                }
            }
        }
    });
    if let Err(e) = spawned {
        tracing::error!(error = %e, "could not start the engine supervisor");
    }
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
        if let Err(e) = call("POST", "/api/v1/engine/quit").call() {
            tracing::warn!(error = %e, "the engine did not take Quit; it stops when the app closes");
        }
        let deadline = Instant::now() + QUIT_WAIT;
        while Instant::now() < deadline && engine_answers() {
            std::thread::sleep(Duration::from_millis(250));
        }
        app.exit(0);
    });
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
    fn app_ids_are_one_path_segment() {
        assert_eq!(urlencoding_component("Design Studio/x"), "Design%20Studio%2Fx");
        assert_eq!(urlencoding_component("app-1_a.b~"), "app-1_a.b~");
    }
}
