//! The engine as an OS service: macOS keeps `nebo --engine` running as a
//! per-user LaunchAgent, so it serves with the window closed and the app
//! quit, after login, and comes back after a crash or a stall.
//!
//! macOS (`macos`): the agent's plist ships inside the app
//! (`Contents/Library/LaunchAgents/dev.neboai.nebo.engine.plist`, from
//! `src-tauri/LaunchAgents`) and is registered with `SMAppService` on macOS
//! 13 and later. On macOS 10.15-12 the same plist, with the executable's
//! absolute path, is written to `~/Library/LaunchAgents` and bootstrapped
//! with `launchctl` (kept for one release).
//!
//! launchd starts the engine again whenever it exits non-zero (a crash, a
//! stall, a held port) and leaves it down after exit 0 (Quit, an update, a
//! signal): "Quit Nebo" needs no unregistering. The engine comes back at the
//! next login (`RunAtLoad`) or when the app opens again (`kickstart`).
//!
//! Off by default: the app registers the engine only when the desktop update
//! feed's `engineMode` (or `NEBO_ENGINE_MODE`) says `service`. Otherwise, in
//! development, or when the owner switched Nebo off in Login Items, the
//! engine runs as the app's own child (`engine::supervise`).
//!
//! Windows (`windows`): a per-user Task Scheduler task started at logon runs
//! `nebo.exe --engine-supervisor`, the engine's parent that starts it again
//! after a non-zero exit (Task Scheduler itself only retries a failed start).
//!
//! `nebo --engine-service <install|uninstall|status|kickstart> [--label L]
//! [--port P]` are the same operations for installers, `make` and tests.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// The app's engine job.
pub const LABEL: &str = "dev.neboai.nebo.engine";
/// The app's bundle identifier: the agent runs as part of it.
const BUNDLE_ID: &str = "dev.neboai.nebo";

/// The engine job a call acts on: the app's own, or a test's (its own label,
/// plist and port).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub label: String,
    pub port: u16,
}

impl Target {
    /// The app's own engine, on the port this process serves.
    pub fn app() -> Self {
        Self { label: LABEL.into(), port: crate::engine::port() }
    }

    /// The plist's file name, inside the bundle and in `~/Library/LaunchAgents`.
    pub fn plist_name(&self) -> String {
        format!("{}.plist", self.label)
    }
}

/// What the OS says about the engine's registration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    /// Registered and allowed to run.
    Enabled,
    /// Registered, but the owner switched it off in Login Items.
    RequiresApproval,
    NotRegistered,
    /// The app carries no plist for this label.
    NotFound,
    /// No service on this OS (yet): the engine runs as the app's child.
    Unsupported,
    /// This OS has one, but it can't run here (Linux with no systemd user
    /// manager, or the unit masked): the engine runs as the app's child,
    /// and the window says so.
    Unavailable,
}

/// The engine job as launchd has it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase", tag = "state", content = "pid")]
pub enum Job {
    Running(u32),
    /// Loaded, not running: exited 0, or waiting out its throttle.
    Loaded,
    /// Not loaded at all.
    Absent,
}

// ── Mode and plan ───────────────────────────────────────────────────────

/// How the app runs its engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Registered with the OS, which keeps it running.
    Service,
    /// The app's own child, stopped when the app quits.
    Child,
}

/// The mode the app is asked for: `NEBO_ENGINE_MODE` first, then the desktop
/// update feed's last `engineMode`. Anything but `service` (unset included)
/// is the child: the service is switched on from the feed, never by default.
pub fn wanted_mode(env: Option<&str>, feed: Option<&str>) -> Mode {
    let pick = env.map(str::trim).filter(|s| !s.is_empty()).or(feed.map(str::trim));
    match pick {
        Some("service") => Mode::Service,
        _ => Mode::Child,
    }
}

/// The service is switched on for this app: `NEBO_ENGINE_MODE`, else the
/// feed's last `engineMode`.
pub fn switched_on() -> bool {
    wanted_mode(std::env::var("NEBO_ENGINE_MODE").ok().as_deref(), load().engine_mode.as_deref()) == Mode::Service
}

/// What the app knows when it opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Facts {
    /// `cargo tauri dev`: never registers or unregisters anything.
    pub dev: bool,
    pub mode: Mode,
    /// The owner's Start at login switch.
    pub start_at_login: bool,
    /// The app runs from a disk image or a translocated copy, a path that
    /// goes away.
    pub transient_path: bool,
    pub status: Status,
}

/// What the app does with its engine when it opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plan {
    /// Run the engine as the app's child. `banner`: the owner switched Nebo
    /// off in Login Items, so say how to keep it working after Quit.
    /// `unregister`: a registration from before is removed first.
    Child { banner: bool, unregister: bool },
    /// Say "Move Nebo to Applications", then run it as a child.
    MoveToApplications,
    /// Register with the OS, then run as a service.
    Register,
    /// Registered and enabled: attach, starting it if it is down.
    Service,
}

pub fn plan(f: &Facts) -> Plan {
    let registered = matches!(f.status, Status::Enabled | Status::RequiresApproval);
    if f.dev {
        return Plan::Child { banner: false, unregister: false };
    }
    if f.mode == Mode::Child || !f.start_at_login {
        return Plan::Child { banner: false, unregister: registered };
    }
    if f.transient_path {
        return Plan::MoveToApplications;
    }
    match f.status {
        Status::Enabled => Plan::Service,
        Status::NotRegistered => Plan::Register,
        Status::RequiresApproval => Plan::Child { banner: true, unregister: false },
        Status::NotFound | Status::Unsupported | Status::Unavailable => Plan::Child { banner: false, unregister: false },
    }
}

/// A path macOS takes away: a mounted disk image, or Gatekeeper's
/// translocated copy of an app opened where it was downloaded.
pub fn is_transient(exe: &Path) -> bool {
    let s = exe.to_string_lossy();
    s.starts_with("/Volumes/") || s.contains("/AppTranslocation/")
}

// ── The owner's choices, kept by the app ────────────────────────────────

/// `<data_dir>/engine-service.json`: Start at login (on unless the owner
/// turned it off) and the feed's last `engineMode` (read at the next open).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Saved {
    pub start_at_login: bool,
    pub engine_mode: Option<String>,
}

impl Default for Saved {
    fn default() -> Self {
        Self { start_at_login: true, engine_mode: None }
    }
}

fn saved_path() -> Option<PathBuf> {
    config::data_dir().ok().map(|d| d.join("engine-service.json"))
}

pub fn load() -> Saved {
    saved_path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn save(saved: &Saved) {
    let Some(path) = saved_path() else { return };
    if let Ok(body) = serde_json::to_vec_pretty(saved)
        && let Err(e) = std::fs::write(&path, body)
    {
        tracing::warn!(error = %e, path = %path.display(), "could not save the engine service settings");
    }
}

/// Read the desktop update feed's `engineMode` and keep it for the next open.
/// Off the main thread; a feed that does not answer changes nothing.
pub fn refresh_feed_mode() {
    std::thread::spawn(|| {
        let url = format!("{}/version.json", updater::NEBO.base_url);
        let agent = ureq::AgentBuilder::new().timeout(std::time::Duration::from_secs(10)).build();
        let Ok(resp) = agent.get(&url).set("User-Agent", concat!("nebo/", env!("CARGO_PKG_VERSION"))).call() else {
            return;
        };
        let Ok(body) = serde_json::from_reader::<_, serde_json::Value>(resp.into_reader()) else { return };
        let mode = updater::engine_mode(&body);
        let mut saved = load();
        if saved.engine_mode != mode {
            tracing::info!(engine_mode = ?mode, "desktop feed: engine mode changed; takes effect when Nebo opens next");
            saved.engine_mode = mode;
            save(&saved);
        }
    });
}

// ── The plist ───────────────────────────────────────────────────────────

/// How the plist names the program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Program {
    /// Inside the app (`SMAppService`, macOS 13+): relative to the bundle.
    Bundle,
    /// An absolute path (`~/Library/LaunchAgents`, macOS 10.15-12).
    Path(PathBuf),
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The agent's plist. `ProcessType Interactive`: launchd applies no resource
/// limits (unset means throttled CPU and I/O, App Nap's starvation again)
/// and takes no power assertion, so the Mac still sleeps. `KeepAlive
/// SuccessfulExit=false`: started again after a non-zero exit, left down
/// after exit 0. `env` adds to `NEBO_SUPERVISED=launchd`.
pub fn render_plist(label: &str, program: &Program, env: &[(&str, &str)]) -> String {
    let exe = match program {
        Program::Bundle => "Contents/MacOS/nebo".to_string(),
        Program::Path(p) => xml_escape(&p.to_string_lossy()),
    };
    let mut out = String::from(concat!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
        "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n",
        "<plist version=\"1.0\">\n<dict>\n",
    ));
    out += &format!("\t<key>Label</key>\n\t<string>{}</string>\n", xml_escape(label));
    if *program == Program::Bundle {
        out += &format!("\t<key>BundleProgram</key>\n\t<string>{exe}</string>\n");
    }
    out += &format!("\t<key>ProgramArguments</key>\n\t<array>\n\t\t<string>{exe}</string>\n\t\t<string>--engine</string>\n\t</array>\n");
    out += &format!("\t<key>AssociatedBundleIdentifiers</key>\n\t<array>\n\t\t<string>{BUNDLE_ID}</string>\n\t</array>\n");
    out += concat!(
        "\t<key>RunAtLoad</key>\n\t<true/>\n",
        "\t<key>KeepAlive</key>\n\t<dict>\n\t\t<key>SuccessfulExit</key>\n\t\t<false/>\n\t</dict>\n",
        "\t<key>ThrottleInterval</key>\n\t<integer>10</integer>\n",
        "\t<key>ProcessType</key>\n\t<string>Interactive</string>\n",
        "\t<key>LimitLoadToSessionType</key>\n\t<string>Aqua</string>\n",
        "\t<key>EnvironmentVariables</key>\n\t<dict>\n\t\t<key>NEBO_SUPERVISED</key>\n\t\t<string>launchd</string>\n",
    );
    for (k, v) in env {
        out += &format!("\t\t<key>{}</key>\n\t\t<string>{}</string>\n", xml_escape(k), xml_escape(v));
    }
    out += "\t</dict>\n</dict>\n</plist>\n";
    out
}

/// `launchctl print`'s answer as a [`Job`].
pub fn parse_print(out: &str) -> Job {
    let field = |name: &str| {
        out.lines().find_map(|l| {
            let (k, v) = l.trim().split_once(" = ")?;
            (k == name).then(|| v.trim().to_string())
        })
    };
    match (field("state").as_deref(), field("pid").and_then(|p| p.parse().ok())) {
        (Some("running"), Some(pid)) => Job::Running(pid),
        _ => Job::Loaded,
    }
}

// ── The operations ──────────────────────────────────────────────────────

// One implementation per OS, each behind its own `cfg`, with the same six
// functions: `status`, `install`, `unregister`, `kickstart`, `job`,
// `open_login_items`. An OS with none here runs the engine as the app's child.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::{install, job, kickstart, open_login_items, status, unregister};
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::{install, job, keep_after_logout, kickstart, open_login_items, set_keep_after_logout, status, unregister};

// Windows: a per-user logon task runs the engine's supervisor. Compiled on
// every OS so what it decides is tested everywhere.
mod windows;
pub use windows::{SUPERVISOR_ARG, TASK_SCHEDULER};
#[cfg(windows)]
pub use windows::{install, job, kickstart, open_login_items, run_supervisor, status, unregister};

/// Whether the engine keeps running after the owner logs out: Linux only
/// (`loginctl enable-linger`), None where the OS has no such choice.
#[cfg(not(target_os = "linux"))]
pub fn keep_after_logout() -> Option<bool> {
    None
}
#[cfg(not(target_os = "linux"))]
pub fn set_keep_after_logout(_on: bool) -> Result<(), String> {
    Err("no such setting on this OS".into())
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
pub fn status(_: &Target) -> Status {
    Status::Unsupported
}
#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
pub fn install(_: &Target) -> Result<Status, String> {
    Ok(Status::Unsupported)
}
#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
pub fn unregister(_: &Target) -> Result<(), String> {
    Ok(())
}
#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
pub fn kickstart(_: &Target, _restart: bool) -> Result<(), String> {
    Err("no engine service on this OS".into())
}
#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
pub fn job(_: &Target) -> Job {
    Job::Absent
}
#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
pub fn open_login_items() {}

/// `--engine-service uninstall`: stop the engine the graceful way (the
/// install key's Quit), then remove the registration. With `app_removed`
/// (the Windows uninstaller's `--app-removed`), also remove the browser's
/// native-messaging host, which names the executable being removed.
pub fn uninstall(target: &Target, app_removed: bool) -> Result<(), String> {
    // The uninstaller asks on every machine; until the service was switched
    // on here (or a task is left from when it was), it changes nothing.
    if app_removed && !switched_on() && status(target) == Status::NotRegistered {
        return Ok(());
    }
    crate::engine::quit_engine(target.port);
    if app_removed {
        browser::native_host::uninstall_manifest();
    }
    unregister(target)
}

// ── The verbs ───────────────────────────────────────────────────────────

/// `nebo --engine-service <verb> [--label L] [--port P]`: prints the result
/// as JSON and exits 0, or prints the error and exits 1.
pub fn run_verb(args: &[String]) -> ! {
    let mut verb = None;
    let mut target = Target::app();
    let mut within = 90;
    let mut app_removed = false;
    let mut it = args.iter().skip_while(|a| *a != "--engine-service").skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--label" => target.label = it.next().cloned().unwrap_or_default(),
            "--port" => target.port = it.next().and_then(|p| p.parse().ok()).unwrap_or(target.port),
            "--within" => within = it.next().and_then(|s| s.parse().ok()).unwrap_or(within),
            "--app-removed" => app_removed = true,
            v if verb.is_none() && !v.starts_with("--") => verb = Some(v.to_string()),
            other => fail(&format!("unknown argument {other}")),
        }
    }
    if target.label.is_empty() {
        fail("--label needs a value");
    }
    let result = match verb.as_deref() {
        Some("status") => Ok(serde_json::json!({ "status": status(&target), "job": job(&target) })),
        Some("install") => install(&target).map(|s| serde_json::json!({ "status": s, "job": job(&target) })),
        Some("uninstall") => uninstall(&target, app_removed).map(|()| serde_json::json!({ "status": status(&target) })),
        Some("kickstart") => kickstart(&target, false).map(|()| serde_json::json!({ "job": job(&target) })),
        // The session's login entry (Linux): the kill switch holds there too,
        // with no window opened; otherwise the engine gets the session's
        // display.
        Some("login") => {
            let mode = wanted_mode(std::env::var("NEBO_ENGINE_MODE").ok().as_deref(), load().engine_mode.as_deref());
            if mode == Mode::Child {
                uninstall(&target, false).map(|()| serde_json::json!({ "status": status(&target) }))
            } else {
                kickstart(&target, false).map(|()| serde_json::json!({ "job": job(&target) }))
            }
        }
        // The update helper's gate, asked of the new executable: an engine
        // of this executable's own version answers healthy within `--within`
        // seconds.
        Some("health") => crate::engine::wait_for_engine_version(target.port, env!("CARGO_PKG_VERSION"), Duration::from_secs(within))
            .map(|()| serde_json::json!({ "version": env!("CARGO_PKG_VERSION") })),
        _ => Err("usage: nebo --engine-service <install|uninstall|status|kickstart|login|health> [--label L] [--port P] [--app-removed]".into()),
    };
    match result {
        Ok(v) => {
            println!("{v}");
            std::process::exit(0)
        }
        Err(e) => fail(&e),
    }
}

fn fail(msg: &str) -> ! {
    eprintln!("nebo --engine-service: {msg}");
    std::process::exit(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(status: Status) -> Facts {
        Facts { dev: false, mode: Mode::Service, start_at_login: true, transient_path: false, status }
    }

    #[test]
    fn the_service_is_off_unless_asked_for() {
        assert_eq!(wanted_mode(None, None), Mode::Child);
        assert_eq!(wanted_mode(None, Some("in_process")), Mode::Child);
        assert_eq!(wanted_mode(None, Some("service")), Mode::Service);
        assert_eq!(wanted_mode(Some("service"), None), Mode::Service);
        // The local override wins over the feed, either way.
        assert_eq!(wanted_mode(Some("in_process"), Some("service")), Mode::Child);
        assert_eq!(wanted_mode(Some("service"), Some("in_process")), Mode::Service);
        assert_eq!(wanted_mode(Some(""), Some("service")), Mode::Service);
    }

    #[test]
    fn dev_never_registers_or_unregisters() {
        for status in [Status::Enabled, Status::NotRegistered, Status::RequiresApproval] {
            let f = Facts { dev: true, ..facts(status) };
            assert_eq!(plan(&f), Plan::Child { banner: false, unregister: false });
        }
    }

    #[test]
    fn the_kill_switch_and_start_at_login_off_run_a_child_and_unregister() {
        let f = Facts { mode: Mode::Child, ..facts(Status::Enabled) };
        assert_eq!(plan(&f), Plan::Child { banner: false, unregister: true });
        let f = Facts { start_at_login: false, ..facts(Status::RequiresApproval) };
        assert_eq!(plan(&f), Plan::Child { banner: false, unregister: true });
        let f = Facts { mode: Mode::Child, ..facts(Status::NotRegistered) };
        assert_eq!(plan(&f), Plan::Child { banner: false, unregister: false });
    }

    #[test]
    fn first_run_registers_and_enabled_attaches() {
        assert_eq!(plan(&facts(Status::NotRegistered)), Plan::Register);
        assert_eq!(plan(&facts(Status::Enabled)), Plan::Service);
    }

    #[test]
    fn switched_off_in_login_items_runs_a_child_with_the_banner() {
        assert_eq!(plan(&facts(Status::RequiresApproval)), Plan::Child { banner: true, unregister: false });
    }

    #[test]
    fn no_plist_or_no_service_runs_a_child() {
        assert_eq!(plan(&facts(Status::NotFound)), Plan::Child { banner: false, unregister: false });
        assert_eq!(plan(&facts(Status::Unsupported)), Plan::Child { banner: false, unregister: false });
    }

    #[test]
    fn a_transient_copy_never_registers() {
        let f = Facts { transient_path: true, ..facts(Status::NotRegistered) };
        assert_eq!(plan(&f), Plan::MoveToApplications);
        assert!(is_transient(Path::new("/Volumes/Nebo/Nebo.app/Contents/MacOS/nebo")));
        assert!(is_transient(Path::new(
            "/private/var/folders/x/T/AppTranslocation/ABC/d/Nebo.app/Contents/MacOS/nebo"
        )));
        assert!(!is_transient(Path::new("/Applications/Nebo.app/Contents/MacOS/nebo")));
    }

    /// The update helper's health gate compares the new app's Info.plist
    /// version (tauri.conf's) with the one its engine reports (Cargo's): a
    /// difference would roll back every update.
    #[test]
    fn the_app_and_the_engine_carry_one_version() {
        let conf: serde_json::Value = serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
        assert_eq!(conf["version"], env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn the_bundled_plist_is_the_rendered_one() {
        // A Windows checkout may carry the file with CRLF line ends.
        let bundled = include_str!("../../LaunchAgents/dev.neboai.nebo.engine.plist").replace("\r\n", "\n");
        assert_eq!(render_plist(LABEL, &Program::Bundle, &[]), bundled);
    }

    #[test]
    fn the_legacy_plist_names_the_executable_and_the_test_env() {
        let body = render_plist(
            "dev.neboai.nebo.engine.test",
            &Program::Path("/tmp/A & B/Nebo.app/Contents/MacOS/nebo".into()),
            &[("NEBO_PORT", "37895")],
        );
        assert!(!body.contains("BundleProgram"));
        assert!(body.contains("<string>/tmp/A &amp; B/Nebo.app/Contents/MacOS/nebo</string>\n\t\t<string>--engine</string>"));
        assert!(body.contains("<key>NEBO_PORT</key>\n\t\t<string>37895</string>"));
        assert!(body.contains("<string>dev.neboai.nebo.engine.test</string>"));
    }

    #[test]
    fn launchctl_print_reads_running_and_loaded() {
        let running = "gui/501/dev.neboai.nebo.engine = {\n\tactive count = 1\n\tstate = running\n\tpid = 4242\n\tlast exit code = 70: EX_SOFTWARE\n}";
        assert_eq!(parse_print(running), Job::Running(4242));
        let exited = "gui/501/dev.neboai.nebo.engine = {\n\tstate = not running\n\tlast exit code = 0\n}";
        assert_eq!(parse_print(exited), Job::Loaded);
    }

    #[test]
    fn saved_settings_default_to_start_at_login() {
        let s: Saved = serde_json::from_str("{}").unwrap();
        assert!(s.start_at_login);
        assert_eq!(s.engine_mode, None);
        let s: Saved = serde_json::from_str(r#"{"startAtLogin":false,"engineMode":"service"}"#).unwrap();
        assert!(!s.start_at_login);
        assert_eq!(s.engine_mode.as_deref(), Some("service"));
    }
}
