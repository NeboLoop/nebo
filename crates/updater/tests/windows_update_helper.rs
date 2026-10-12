//! The Windows update helper, run for real: it waits for the old process,
//! runs the installer, starts Nebo again, and, with the health gate on (an
//! engine the Windows task supervises), keeps the new installer as the way
//! back or rolls back to the previous one and writes the marker.
//!
//! Nothing real is installed: each "installer" is a script that puts a fake
//! engine in a scratch folder, and the fake engine answers `/health` on its
//! own port as one version. Windows only (the house runner, stadium-win).
//!
//! The house runner is a service account (NETWORK SERVICE), so a task there
//! runs in a session of its own; the owner's runs in their logon session.
//! What the helper does is the same either way.
//!
//! Run:
//!   cargo test -p nebo-updater --test windows_update_helper
#![cfg(windows)]

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use nebo_updater::{WindowsHelper, build_windows_helper, start_helper_task};

struct Scratch {
    dir: PathBuf,
    port: u16,
}

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("nebo-update-helper-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("install")).unwrap();
        std::fs::create_dir_all(dir.join("data")).unwrap();
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let s = Self { dir, port };
        // The app's launcher: starts the installed engine, if any, hidden.
        s.write(
            "install/start.cmd",
            "@echo off\r\nif exist \"%~dp0engine.ps1\" start \"\" /b powershell -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File \"%~dp0engine.ps1\"\r\n",
        );
        s
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.dir.join(rel)
    }

    fn write(&self, rel: &str, body: &str) {
        std::fs::write(self.path(rel), body).unwrap();
    }

    /// A fake engine answering `/health` as `version`, its pid in `engine.pid`.
    fn engine(&self, version: &str) -> String {
        format!(
            r#"$l = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, {port})
$l.Start()
Set-Content -Path '{pidfile}' -Value $PID
while ($true) {{
  $c = $l.AcceptTcpClient(); $s = $c.GetStream(); $buf = New-Object byte[] 4096; $null = $s.Read($buf, 0, 4096)
  $body = '{{"status":"ok","role":"engine","version":"{version}"}}'
  $r = [Text.Encoding]::ASCII.GetBytes("HTTP/1.1 200 OK`r`nContent-Type: application/json`r`nContent-Length: $($body.Length)`r`nConnection: close`r`n`r`n$body")
  $s.Write($r, 0, $r.Length); $c.Close()
}}
"#,
            port = self.port,
            pidfile = self.path("engine.pid").display(),
        )
    }

    /// An installer script named `name` that installs `engine` (None: an
    /// install that leaves no working engine) and exits `code`.
    fn installer(&self, name: &str, engine: Option<&str>, code: i32) -> PathBuf {
        let src = format!("{name}.engine.ps1");
        match engine {
            Some(version) => self.write(&src, &self.engine(version)),
            None => self.write(&src, "exit 3\r\n"),
        }
        self.write(
            &format!("{name}.cmd"),
            &format!(
                "@echo off\r\ncopy /y \"{}\" \"{}\" >NUL\r\nexit /b {code}\r\n",
                self.path(&src).display(),
                self.path("install/engine.ps1").display()
            ),
        );
        self.path(&format!("{name}.cmd"))
    }

    fn helper(&self, setup: &Path, previous: &Path, gate_secs: u64) -> WindowsHelper {
        // A process that has already exited: the helper waits for nothing.
        let gone = Command::new("cmd").args(["/C", "exit 0"]).spawn().unwrap();
        let pid = gone.id();
        let _ = gone.wait_with_output();
        WindowsHelper {
            self_task: String::new(),
            pid,
            setup: setup.display().to_string(),
            previous: previous.display().to_string(),
            relaunch: self.path("install/start.cmd").display().to_string(),
            task: r"\NeboAI\Nebo Engine Test (none)".into(),
            port: self.port,
            old_version: "1.0.0".into(),
            updating: String::new(),
            gate_secs,
            marker: self.path("data/UPDATE_FAILED.json").display().to_string(),
            log: self.path("data/logs/update.log").display().to_string(),
        }
    }

    /// Run the helper as the updater does (`-EncodedCommand`) and wait for it.
    fn run(&self, h: &WindowsHelper) -> i32 {
        let script = build_windows_helper(h);
        let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
        use base64::Engine;
        let encoded = base64::engine::general_purpose::STANDARD.encode(utf16);
        let mut child = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-EncodedCommand", &encoded])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(180);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                return status.code().unwrap_or(-1);
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                panic!("the helper did not finish within 180 s; log:\n{}", self.log());
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.path("data/logs/update.log")).unwrap_or_default()
    }

    fn marker(&self) -> Option<String> {
        std::fs::read_to_string(self.path("data/UPDATE_FAILED.json")).ok()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Ok(pid) = std::fs::read_to_string(self.path("engine.pid")) {
            let _ = Command::new("taskkill").args(["/F", "/PID", pid.trim()]).output();
        }
        std::thread::sleep(Duration::from_millis(500));
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn a_new_version_that_answers_is_kept_and_its_installer_becomes_the_way_back() {
    let s = Scratch::new("pass");
    let setup = s.installer("new", Some("2.0.0"), 0);
    let previous = s.path("data/updates/previous-setup.cmd");
    let code = s.run(&s.helper(&setup, &previous, 60));
    let log = s.log();
    assert_eq!(code, 0, "log:\n{log}");
    assert!(log.contains("engine answers as 2.0.0"), "log:\n{log}");
    assert!(log.contains("update complete"), "log:\n{log}");
    assert!(previous.exists(), "the new installer is kept for the next rollback");
    assert!(!setup.exists());
    assert_eq!(s.marker(), None);
}

#[test]
fn a_new_version_that_never_answers_is_rolled_back() {
    let s = Scratch::new("rollback");
    let setup = s.installer("broken", None, 0);
    let previous = s.installer("previous", Some("1.0.0"), 0);
    let code = s.run(&s.helper(&setup, &previous, 20));
    let log = s.log();
    assert_eq!(code, 1, "log:\n{log}");
    assert!(log.contains("did not answer within 20 s"), "log:\n{log}");
    assert!(log.contains("rolling back"), "log:\n{log}");
    assert!(log.contains("engine answers as 1.0.0"), "the previous version is back:\n{log}");
    assert!(s.marker().is_some_and(|m| m.contains("rolled back to the previous version")));
}

#[test]
fn without_the_gate_a_failed_installer_leaves_the_marker_and_nebo_starts() {
    let s = Scratch::new("ungated");
    let setup = s.installer("failing", Some("1.0.0"), 2);
    let previous = s.path("data/updates/previous-setup.cmd");
    let code = s.run(&s.helper(&setup, &previous, 0));
    let log = s.log();
    assert_eq!(code, 0, "log:\n{log}");
    assert!(log.contains("installer exited 2"), "log:\n{log}");
    assert!(log.contains("no health gate"), "log:\n{log}");
    assert!(s.marker().is_some_and(|m| m.contains("reinstall Nebo")));
    // Nebo was started again (the launcher ran the installed engine).
    let http = Instant::now() + Duration::from_secs(20);
    while !s.path("engine.pid").exists() && Instant::now() < http {
        std::thread::sleep(Duration::from_millis(250));
    }
    assert!(s.path("engine.pid").exists(), "the app was started again");
}

/// An engine the Windows task runs gets its helper as a task of its own:
/// the helper finishes there (installer, start, gate) after the process
/// that started it is gone, then deletes its task.
#[test]
fn the_helper_runs_to_the_end_as_its_own_task_and_removes_it() {
    let s = Scratch::new("task");
    let setup = s.installer("new", Some("2.0.0"), 0);
    let previous = s.path("data/updates/previous-setup.cmd");
    let task = format!(r"\NeboAI\Nebo Update Test {}", std::process::id());
    let mut h = s.helper(&setup, &previous, 60);
    h.self_task = task.clone();
    h.updating = s.path("data/UPDATING").display().to_string();
    s.write("data/UPDATING", "2.0.0");
    start_helper_task(&task, &build_windows_helper(&h)).expect("the helper task starts");
    let deadline = Instant::now() + Duration::from_secs(150);
    while !s.log().contains("update complete") && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(500));
    }
    let log = s.log();
    // Whatever happened, no test task is left behind.
    let gone = Instant::now() + Duration::from_secs(20);
    while command::task::query(&task).is_some() && Instant::now() < gone {
        std::thread::sleep(Duration::from_millis(500));
    }
    let left = command::task::query(&task).is_some();
    let _ = command::task::delete(&task);
    assert!(log.contains("engine answers as 2.0.0"), "log:\n{log}");
    assert!(log.contains("update complete"), "log:\n{log}");
    assert!(previous.exists());
    assert!(!left, "the helper deletes its own task");
    assert!(!s.path("data/UPDATING").exists(), "the update marker is gone once the helper is done");
}
