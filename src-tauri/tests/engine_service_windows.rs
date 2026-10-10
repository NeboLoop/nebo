//! The engine as a Windows logon task, for real (spec §15, Windows column):
//! register, the engine serves under its supervisor at normal priority; a
//! crash, a stall (exit 70) and a held port (exit 75) start it again; the
//! task ended takes the supervisor and the engine with it; Quit leaves both down; an
//! update through the task is kept when it answers and rolled back when it
//! doesn't; unregister leaves nothing.
//!
//! Everything is the test's own: the task `\NeboAI\dev.neboai.nebo.engine.test`,
//! a free port, a scratch Nebo folder and a copy of this build as the
//! "installed" nebo.exe. Never the owner's task, port 27895 or Nebo folder.
//!
//! The house runner (stadium-win-1) runs as NETWORK SERVICE, which never
//! logs on: the task registers with no logon trigger and is started on
//! demand. The logon trigger and `InteractiveToken` are the same definition
//! with a trigger (`command::task`, unit-tested on every OS); everything else
//! here is the production path.
//!
//! Run (Windows):
//!   cargo test -p nebo --test engine_service_windows -- --nocapture
#![cfg(windows)]

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const LABEL: &str = "dev.neboai.nebo.engine.test";
const TASK: &str = r"\NeboAI\dev.neboai.nebo.engine.test";

struct Rig {
    root: PathBuf,
    port: u16,
}

impl Rig {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("nebo-engine-service-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("install")).unwrap();
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::copy(env!("CARGO_BIN_EXE_nebo"), root.join("install").join("nebo.exe")).unwrap();
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        Self { root, port }
    }

    fn exe(&self) -> PathBuf {
        self.root.join("install").join("nebo.exe")
    }

    fn data(&self) -> PathBuf {
        self.root.join("data")
    }

    /// `nebo.exe --engine-service <verb>` for the test's task, port and folder.
    fn verb(&self, verb: &str, extra: &[&str]) -> serde_json::Value {
        let out = Command::new(self.exe())
            .args(["--engine-service", verb, "--label", LABEL, "--port", &self.port.to_string()])
            .args(extra)
            .env("NEBO_HOME", self.data())
            .stdin(Stdio::null())
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "--engine-service {verb}: {text} {}", String::from_utf8_lossy(&out.stderr));
        serde_json::from_str(text.trim()).unwrap_or_else(|e| panic!("--engine-service {verb} said {text:?}: {e}"))
    }

    fn health(&self) -> Option<serde_json::Value> {
        let http = ureq::AgentBuilder::new().timeout(Duration::from_secs(2)).build();
        let resp = http.get(&format!("http://127.0.0.1:{}/health", self.port)).call().ok()?;
        serde_json::from_reader(resp.into_reader()).ok()
    }

    /// The engine answering healthy, other than `not` (a pid), within `within`.
    fn serving(&self, not: Option<u32>, within: Duration) -> serde_json::Value {
        let deadline = Instant::now() + within;
        loop {
            if let Some(h) = self.health()
                && h["status"] == "ok"
                && not.is_none_or(|pid| h["pid"] != pid)
            {
                return h;
            }
            assert!(Instant::now() < deadline, "no engine served within {within:?}\n{}", self.logs());
            std::thread::sleep(Duration::from_millis(500));
        }
    }

    fn down(&self, within: Duration) {
        let deadline = Instant::now() + within;
        while self.health().is_some() {
            assert!(Instant::now() < deadline, "the engine still answers after {within:?}");
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    fn quit(&self) {
        let key = std::fs::read_to_string(self.data().join(".install-key")).expect("the engine's install key");
        let http = ureq::AgentBuilder::new().timeout(Duration::from_secs(5)).build();
        let resp = http
            .post(&format!("http://127.0.0.1:{}/api/v1/engine/quit", self.port))
            .set("Authorization", &format!("Bearer {}", key.trim()))
            .call()
            .expect("Quit");
        assert_eq!(resp.status(), 202);
    }

    fn supervisor(&self) -> Option<u32> {
        let pid: u32 = std::fs::read_to_string(self.data().join("engine-supervisor.pid")).ok()?.trim().parse().ok()?;
        alive(pid).then_some(pid)
    }

    fn supervisor_gone(&self, within: Duration) {
        let deadline = Instant::now() + within;
        while self.supervisor().is_some() {
            assert!(Instant::now() < deadline, "the supervisor still runs after {within:?}");
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    fn supervisor_log(&self) -> String {
        std::fs::read_to_string(self.data().join("logs").join("engine-supervisor.log")).unwrap_or_default()
    }

    fn logs(&self) -> String {
        let tail = |name: &str| {
            let text = std::fs::read_to_string(self.data().join("logs").join(name)).unwrap_or_default();
            let lines: Vec<&str> = text.lines().collect();
            lines[lines.len().saturating_sub(30)..].join("\n")
        };
        format!(
            "--- engine-supervisor.log\n{}\n--- nebo.log\n{}\n--- update.log\n{}",
            tail("engine-supervisor.log"),
            tail("nebo.log"),
            tail("update.log")
        )
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        // The engine stops the graceful way; Task Scheduler ends what is left.
        if self.health().is_some() && self.data().join(".install-key").exists() {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.quit()));
            std::thread::sleep(Duration::from_secs(5));
        }
        let _ = Command::new("schtasks").args(["/End", "/TN", TASK]).output();
        let _ = Command::new("schtasks").args(["/Delete", "/TN", TASK, "/F"]).output();
        let _ = Command::new("schtasks").args(["/Delete", "/TN", r"\NeboAI\Nebo Update Engine Test", "/F"]).output();
        std::thread::sleep(Duration::from_secs(2));
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn alive(pid: u32) -> bool {
    let out = Command::new("tasklist").args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).contains(&format!("\"{pid}\""))
}

/// The process's base priority as Windows reports it (`Win32_Process`):
/// 8 is the normal class; Task Scheduler's default priority 7 gives 6
/// (below normal).
fn base_priority(pid: u32) -> String {
    let out = Command::new("powershell")
        .args(["-NoProfile", "-Command", &format!("(Get-CimInstance Win32_Process -Filter 'ProcessId={pid}').Priority")])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn pid(health: &serde_json::Value) -> u32 {
    health["pid"].as_u64().expect("health names the pid") as u32
}

/// A fake installer: a script that copies `from` over the installed exe.
fn installer(rig: &Rig, name: &str, from: &Path) -> PathBuf {
    let path = rig.root.join(format!("{name}.cmd"));
    std::fs::write(
        &path,
        format!("@echo off\r\ncopy /y \"{}\" \"{}\" >NUL\r\nexit /b %ERRORLEVEL%\r\n", from.display(), rig.exe().display()),
    )
    .unwrap();
    path
}

fn wait_for(what: &str, within: Duration, mut done: impl FnMut() -> bool, rig: &Rig) {
    let deadline = Instant::now() + within;
    while !done() {
        assert!(Instant::now() < deadline, "{what}: not within {within:?}\n{}", rig.logs());
        std::thread::sleep(Duration::from_millis(500));
    }
}

#[test]
fn the_engine_runs_as_a_windows_logon_task() {
    let rig = Rig::new();

    // ── Register: the definition as Task Scheduler keeps it ──────────────
    let installed = rig.verb("install", &[]);
    assert_eq!(installed["status"], "enabled", "{installed}");
    let xml = String::from_utf8_lossy(&Command::new("schtasks").args(["/Query", "/TN", TASK, "/XML"]).output().unwrap().stdout)
        .into_owned();
    for setting in [
        "<Priority>5</Priority>",
        "<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>",
        "<DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>",
        "<StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>",
        "<MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>",
        "--engine-supervisor",
        &format!("--port {}", rig.port),
    ] {
        assert!(xml.contains(setting), "the registered task lacks {setting}:\n{xml}");
    }
    assert!(!xml.contains("<WakeToRun>true</WakeToRun>"));
    println!("registered: priority 5, no time limit, battery on, IgnoreNew, no wake");

    // ── Start: the engine serves under its supervisor, normal priority ───
    rig.verb("kickstart", &[]);
    let h = rig.serving(None, Duration::from_secs(240));
    assert_eq!(h["role"], "engine");
    assert_eq!(h["supervised"], true);
    assert_eq!(h["version"], env!("CARGO_PKG_VERSION"));
    let engine = pid(&h);
    let supervisor = rig.supervisor().expect("the supervisor wrote its pid");
    let status = rig.verb("status", &[]);
    assert_eq!(status["job"]["state"], "running", "{status}");
    assert_eq!(base_priority(engine), "8", "the engine runs at normal priority");
    assert_eq!(base_priority(supervisor), "8", "the supervisor runs at normal priority");
    println!("serving: engine {engine} under supervisor {supervisor}, both at normal priority (8)");

    // The runner can't end a process Task Scheduler started (access is
    // denied across the task's logon session), so the engine is made to
    // crash and stall from inside: TEST_CRASH and TEST_STALL, each read once
    // by the next engine when it serves (only in a test's Nebo folder).
    let restart_with = |file: &str, body: &str| {
        std::fs::write(rig.data().join(file), body).unwrap();
        rig.quit();
        rig.down(Duration::from_secs(60));
        rig.supervisor_gone(Duration::from_secs(20));
        rig.verb("kickstart", &[]);
    };

    // ── A crash: started again ───────────────────────────────────────────
    let crashed = Instant::now();
    restart_with("TEST_CRASH", "");
    wait_for("the crashed engine is started again", Duration::from_secs(240), || rig.supervisor_log().contains("engine exited; starting it again"), &rig);
    let h = rig.serving(None, Duration::from_secs(120));
    let engine = pid(&h);
    println!("crash: aborted, started again and serving as {engine} after {:?}", crashed.elapsed());

    // ── A stall: the watchdog exits 70, the supervisor starts it again ──
    restart_with("TEST_STALL", "900");
    let stalled = Instant::now();
    wait_for("the stalled engine exits 70", Duration::from_secs(480), || rig.supervisor_log().contains("Some(70)"), &rig);
    let h = rig.serving(None, Duration::from_secs(120));
    let engine = pid(&h);
    println!("stall: exit 70, started again and serving as {engine} after {:?}", stalled.elapsed());

    // ── Quit: both stay down ─────────────────────────────────────────────
    rig.quit();
    rig.down(Duration::from_secs(60));
    rig.supervisor_gone(Duration::from_secs(20));
    let quiet = Instant::now();
    while quiet.elapsed() < Duration::from_secs(65) {
        assert!(rig.health().is_none(), "the engine came back after Quit");
        std::thread::sleep(Duration::from_secs(1));
    }
    assert!(!alive(engine));
    assert_eq!(rig.verb("status", &[])["job"]["state"], "loaded");
    println!("quit: engine and supervisor down, not started again in 65 s");

    // ── A held port: exit 75 until it is free ────────────────────────────
    let holder = TcpListener::bind(("127.0.0.1", rig.port)).unwrap();
    rig.verb("kickstart", &[]);
    wait_for("the engine exits 75 on the held port", Duration::from_secs(240), || rig.supervisor_log().contains("Some(75)"), &rig);
    drop(holder);
    let h = rig.serving(None, Duration::from_secs(60));
    let engine = pid(&h);
    println!("held port: exit 75, then served as {engine} once free");

    // ── The task ended (Task Scheduler's End, a hard stop of the
    // supervisor): the engine goes with it, nothing left running ────────
    let supervisor = rig.supervisor().expect("supervisor");
    let ended = Command::new("schtasks").args(["/End", "/TN", TASK]).output().unwrap();
    assert!(ended.status.success(), "schtasks /End: {}", String::from_utf8_lossy(&ended.stdout));
    rig.down(Duration::from_secs(60));
    wait_for("the supervisor and the engine are gone", Duration::from_secs(30), || !alive(supervisor) && !alive(engine), &rig);
    println!("task ended: supervisor {supervisor} and engine {engine} gone");

    // ── An update through the task: kept when it answers ─────────────────
    let build = PathBuf::from(env!("CARGO_BIN_EXE_nebo"));
    let garbage = rig.root.join("garbage.exe");
    std::fs::write(&garbage, b"not a program").unwrap();
    let previous = rig.data().join("updates").join("previous-setup.cmd");
    let gone = Command::new("cmd").args(["/C", "exit 0"]).spawn().unwrap();
    let dead = gone.id();
    let _ = gone.wait_with_output();
    let helper = |setup: PathBuf, gate_secs: u64| updater::WindowsHelper {
        self_task: r"\NeboAI\Nebo Update Engine Test".into(),
        pid: dead,
        setup: setup.display().to_string(),
        previous: previous.display().to_string(),
        relaunch: String::new(),
        task: TASK.into(),
        port: rig.port,
        old_version: "0.0.0-test".into(),
        updating: rig.data().join("UPDATING").display().to_string(),
        gate_secs,
        marker: rig.data().join("UPDATE_FAILED.json").display().to_string(),
        log: rig.data().join("logs").join("update.log").display().to_string(),
    };
    let update_log = || std::fs::read_to_string(rig.data().join("logs").join("update.log")).unwrap_or_default();
    let h = helper(installer(&rig, "good", &build), 180);
    updater::start_helper_task(&h.self_task, &updater::build_windows_helper(&h)).expect("helper task");
    wait_for("the update completes", Duration::from_secs(300), || update_log().contains("update complete"), &rig);
    let served = rig.serving(None, Duration::from_secs(30));
    assert!(previous.exists(), "the installer is kept as the way back");
    assert!(!rig.data().join("UPDATE_FAILED.json").exists());
    println!("update: the task's engine answered as {}; installer kept", served["version"]);

    // ── An update that never answers: rolled back ────────────────────────
    rig.quit();
    rig.down(Duration::from_secs(60));
    rig.supervisor_gone(Duration::from_secs(20));
    let h = helper(installer(&rig, "broken", &garbage), 45);
    updater::start_helper_task(&h.self_task, &updater::build_windows_helper(&h)).expect("helper task");
    wait_for(
        "the rollback finishes",
        Duration::from_secs(400),
        || rig.data().join("UPDATE_FAILED.json").exists(),
        &rig,
    );
    let marker = std::fs::read_to_string(rig.data().join("UPDATE_FAILED.json")).unwrap();
    assert!(marker.contains("rolled back"), "{marker}");
    assert!(update_log().contains("rolling back"), "{}", update_log());
    let served = rig.serving(None, Duration::from_secs(180));
    assert_eq!(served["version"], env!("CARGO_PKG_VERSION"));
    assert!(!rig.data().join("UPDATING").exists());
    println!("rollback: the broken install never answered; the previous one serves again with the marker");

    // ── Unregister: nothing left ─────────────────────────────────────────
    let removed = rig.verb("uninstall", &["--app-removed"]);
    assert_eq!(removed["status"], "notRegistered", "{removed}");
    rig.down(Duration::from_secs(60));
    rig.supervisor_gone(Duration::from_secs(20));
    let query = Command::new("schtasks").args(["/Query", "/TN", TASK]).output().unwrap();
    assert!(!query.status.success(), "the task is gone");
    assert_eq!(rig.verb("status", &[])["job"]["state"], "absent");
    println!("unregistered: engine stopped, task gone");
}
