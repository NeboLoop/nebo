//! Windows: a per-user Task Scheduler task starts the engine at logon.
//!
//! Not a Windows service: a service runs in session 0, with no desktop for
//! screenshots, UI automation or banners, and needs admin to install. Not a
//! `Run` key: nothing would start the engine again after a crash or a stall
//! exit. The task runs in the owner's session and needs no admin
//! (`command::task` has the definition: priority 5, no time limit, runs on
//! battery, one instance, never wakes the PC).
//!
//! Task Scheduler does not start a program again when it exits non-zero
//! ("restart on failure" covers a failed *start* only: Phase 0, spec §20.3),
//! so the task runs `nebo.exe --engine-supervisor`, a thin parent with no
//! server that runs `nebo.exe --engine` as its child and applies the
//! engine's exit codes (`engine::after_exit`): 0 stays down and the parent
//! exits, so the task ends; a stall, a crash or a held port starts it again
//! with backoff. The child's stdin is the parent's pipe (its lifeline, as in
//! the shell's child mode): when the parent is ended, however, the engine
//! stops the graceful way.
//!
//! Compiled on every OS so what it decides is tested everywhere; only
//! Windows calls it.
#![cfg_attr(not(windows), allow(dead_code))]

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use super::{Job, Status, Target};
use crate::engine::{self, AfterExit};

/// The app's engine task.
pub const TASK: &str = r"\NeboAI\Nebo Engine";

/// The task's program: this executable as the engine's supervisor.
pub const SUPERVISOR_ARG: &str = "--engine-supervisor";

/// `NEBO_SUPERVISED` for an engine the task's supervisor runs.
pub const TASK_SCHEDULER: &str = "taskscheduler";

/// The engine's port when nothing says otherwise.
const DEFAULT_PORT: u16 = 27895;

/// The task for a label: the app's own is [`TASK`]; a test's label
/// (`dev.neboai.nebo.engine.test`) gets its own task beside it.
pub fn task_name(label: &str) -> String {
    if label == super::LABEL { TASK.to_string() } else { format!(r"\NeboAI\{label}") }
}

/// The supervisor's command line: the port and Nebo folder when they are not
/// the default (a test's). A task has no environment block of its own.
pub fn task_arguments(port: u16, home: Option<&Path>) -> String {
    let mut args = vec![SUPERVISOR_ARG.to_string()];
    if port != DEFAULT_PORT {
        args.push(format!("--port {port}"));
    }
    if let Some(home) = home {
        args.push(format!("--home {}", command::task::quote_arg(&home.to_string_lossy())));
    }
    args.join(" ")
}

/// What a registered task's definition says, for this executable: switched
/// off by the owner, or registered for another copy of the app (moved or
/// reinstalled elsewhere: registering again points it here).
pub fn status_of(xml: &str, exe: &Path) -> Status {
    if command::task::element(xml, "Settings", "Enabled") == Some("false") {
        return Status::RequiresApproval;
    }
    let registered = command::task::element(xml, "Exec", "Command").unwrap_or("").replace("&amp;", "&");
    if registered.eq_ignore_ascii_case(&exe.to_string_lossy()) { Status::Enabled } else { Status::NotRegistered }
}

fn exe() -> Result<PathBuf, String> {
    std::env::current_exe().map_err(|e| format!("this executable: {e}"))
}

pub fn status(target: &Target) -> Status {
    match (command::task::query(&task_name(&target.label)), exe()) {
        (Some(xml), Ok(exe)) => status_of(&xml, &exe),
        _ => Status::NotRegistered,
    }
}

/// Register the task for this executable (replacing one there), with the
/// port and Nebo folder this process was given (a test's).
pub fn install(target: &Target) -> Result<Status, String> {
    let exe = exe()?;
    let home = config::data_dir_overridden().then(|| config::data_dir().ok()).flatten();
    let arguments = task_arguments(target.port, home.as_deref());
    let task = command::task::Task {
        description: "Keeps Nebo working in the background while you are signed in.",
        command: &exe.to_string_lossy(),
        arguments: &arguments,
        at_logon: true,
    };
    command::task::register(&task_name(&target.label), &task)?;
    Ok(status(target))
}

/// Delete the task. An engine it runs keeps running until it stops (Quit);
/// nothing starts it at the next logon.
pub fn unregister(target: &Target) -> Result<(), String> {
    command::task::delete(&task_name(&target.label))
}

/// Start the task now (a no-op while it runs: `IgnoreNew`). With `restart`,
/// the running engine stops the graceful way first, its supervisor exits
/// with it, and the task starts both again.
pub fn kickstart(target: &Target, restart: bool) -> Result<(), String> {
    if restart {
        engine::quit_engine(target.port);
        let deadline = Instant::now() + Duration::from_secs(10);
        while supervisor_pid().is_some() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    command::task::run(&task_name(&target.label))
}

pub fn job(target: &Target) -> Job {
    match supervisor_pid() {
        Some(pid) => Job::Running(pid),
        None if command::task::query(&task_name(&target.label)).is_some() => Job::Loaded,
        None => Job::Absent,
    }
}

/// Task Scheduler, where the owner switches the task back on.
pub fn open_login_items() {
    let _ = command::new::<std::process::Command>("mmc", command::Console::Hidden).arg("taskschd.msc").spawn();
}

// ── The supervisor ──────────────────────────────────────────────────────

/// `<data_dir>/engine-supervisor.pid`: the running supervisor's pid.
fn pid_file() -> Option<PathBuf> {
    config::data_dir().ok().map(|d| d.join("engine-supervisor.pid"))
}

/// Whether `tasklist /FO CSV` output lists `pid` as a `nebo` process.
fn listed(tasklist: &str, pid: u32) -> bool {
    tasklist.lines().any(|l| {
        let l = l.to_ascii_lowercase();
        l.contains(&format!("\"{pid}\"")) && l.starts_with("\"nebo")
    })
}

/// The task's supervisor, while it runs: the pid it wrote, still a Nebo.
fn supervisor_pid() -> Option<u32> {
    let pid: u32 = std::fs::read_to_string(pid_file()?).ok()?.trim().parse().ok()?;
    let out = command::new::<std::process::Command>("tasklist", command::Console::Hidden)
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output()
        .ok()?;
    listed(&String::from_utf8_lossy(&out.stdout), pid).then_some(pid)
}

/// `nebo.exe --engine-supervisor [--port P] [--home DIR]`, the task's
/// program: run the engine as a child and start it again as
/// `engine::after_exit` says. Exits 0 once the engine stopped on purpose
/// (Quit, an update, uninstall). `main` puts the port and Nebo folder in
/// this process's environment first; the engine inherits them.
pub fn run_supervisor() -> ! {
    tracing::info!(pid = std::process::id(), "engine supervisor started");
    let pid_file = pid_file();
    if let Some(path) = &pid_file {
        let _ = std::fs::write(path, std::process::id().to_string());
    }
    let mut crashes = 0;
    loop {
        let started = Instant::now();
        let mut child = match spawn_engine() {
            Ok(child) => child,
            Err(e) => {
                tracing::error!(error = %e, "could not start the engine");
                std::thread::sleep(engine::MAX_BACKOFF);
                continue;
            }
        };
        tracing::info!(pid = child.id(), "engine started");
        // Held until the engine exits: `wait` would close it first.
        let lifeline = child.stdin.take();
        let status = child.wait();
        drop(lifeline);
        let code = status.as_ref().ok().and_then(|s| s.code());
        let (next, now) = engine::after_exit(code, started.elapsed(), crashes);
        crashes = now;
        match next {
            AfterExit::StayDown => {
                tracing::info!("engine stopped on purpose; the supervisor exits");
                if let Some(path) = &pid_file {
                    let _ = std::fs::remove_file(path);
                }
                std::process::exit(0);
            }
            AfterExit::Restart(after) => {
                tracing::warn!(?code, after_secs = after.as_secs(), "engine exited; starting it again");
                std::thread::sleep(after);
            }
        }
    }
}

fn spawn_engine() -> std::io::Result<std::process::Child> {
    command::new::<std::process::Command>(std::env::current_exe()?, command::Console::Hidden)
        .arg("--engine")
        .env("NEBO_SUPERVISED", TASK_SCHEDULER)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exe() -> PathBuf {
        PathBuf::from(r"C:\Users\Owner\AppData\Local\Nebo\nebo.exe")
    }

    fn registered(exe: &Path) -> String {
        let task = command::task::Task {
            description: "",
            command: &exe.to_string_lossy(),
            arguments: SUPERVISOR_ARG,
            at_logon: true,
        };
        command::task::xml(&task, &command::task::Account::from_sid("S-1-5-21-1-2-3-1001"))
    }

    #[test]
    fn the_app_task_runs_the_supervisor_and_a_test_task_carries_its_port_and_home() {
        assert_eq!(task_arguments(DEFAULT_PORT, None), "--engine-supervisor");
        assert_eq!(
            task_arguments(37895, Some(Path::new(r"C:\Run Temp\nebo home\"))),
            r#"--engine-supervisor --port 37895 --home "C:\Run Temp\nebo home""#
        );
        assert_eq!(task_name(super::super::LABEL), TASK);
        assert_eq!(task_name("dev.neboai.nebo.engine.test"), r"\NeboAI\dev.neboai.nebo.engine.test");
    }

    #[test]
    fn status_reads_the_registered_definition() {
        let xml = registered(&exe());
        assert_eq!(status_of(&xml, &exe()), Status::Enabled);
        // Windows paths are case-insensitive.
        assert_eq!(status_of(&xml, Path::new(r"c:\users\owner\appdata\local\nebo\NEBO.EXE")), Status::Enabled);
        // Registered for another copy of the app: registering again points it here.
        assert_eq!(status_of(&xml, Path::new(r"D:\Nebo\nebo.exe")), Status::NotRegistered);
        // Switched off by the owner (Task Scheduler writes the Settings' Enabled).
        let off = xml.replace("<Enabled>true</Enabled>\n    <Hidden>", "<Enabled>false</Enabled>\n    <Hidden>");
        assert_ne!(off, xml);
        assert_eq!(status_of(&off, &exe()), Status::RequiresApproval);
        // Task Scheduler leaves `Enabled` out when it is on.
        let read_back = xml.replace("<Enabled>true</Enabled>\n    <Hidden>", "<Hidden>");
        assert_eq!(status_of(&read_back, &exe()), Status::Enabled);
    }

    #[test]
    fn the_supervisor_is_found_by_its_pid_as_a_nebo() {
        let list = "\"nebo.exe\",\"4242\",\"Console\",\"1\",\"21,000 K\"\r\n";
        assert!(listed(list, 4242));
        assert!(!listed(list, 424));
        assert!(!listed("\"notepad.exe\",\"4242\",\"Console\",\"1\",\"9,000 K\"", 4242));
        assert!(!listed("INFO: No tasks are running which match the specified criteria.", 4242));
    }
}
