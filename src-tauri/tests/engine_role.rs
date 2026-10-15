//! `nebo --engine`: the desktop app's executable as the server alone, no
//! window. It serves, says it is the engine, stops on the shell's Quit with
//! exit 0, and exits 75 when its port is held, leaving the engine that holds
//! it serving (one engine per Nebo). Run by the OS's service it outlives the
//! app that opened it; run as the app's child it stops with the app.
//!
//! Every run has its own Nebo folder, install key and port, and no hub.
//!
//! Run:
//!   cargo test -p nebo --test engine_role

use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

struct Home(PathBuf);

impl Home {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("nebo-engine-role-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const KEY: &str = "engine-role-test-key";

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// `nebo --engine` on `port`. `supervisor`: who runs it (`NEBO_SUPERVISED`),
/// with its stdin a pipe this test holds, as the app and the OS's services
/// start it; None for a bare engine.
fn engine_as(home: &Home, port: u16, supervisor: Option<&str>) -> Child {
    // The architecture drift gate allows `Command::new` in tests.
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_nebo"));
    match supervisor {
        Some(s) => cmd.env("NEBO_SUPERVISED", s).stdin(Stdio::piped()),
        None => cmd.env_remove("NEBO_SUPERVISED").stdin(Stdio::null()),
    };
    cmd.arg("--engine")
        .env("NEBO_HOME", &home.0)
        .env("NEBO_PORT", port.to_string())
        .env("NEBO_MCP_API_KEY", KEY)
        .envs([
            ("NEBOAI_API_URL", "http://127.0.0.1:9"),
            ("NEBOAI_JANUS_URL", "http://127.0.0.1:9"),
            ("NEBOAI_COMMS_URL", "http://127.0.0.1:9"),
            ("NEBOAI_TUNNEL_URL", "http://127.0.0.1:9"),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start nebo --engine")
}

fn engine(home: &Home, port: u16) -> Child {
    engine_as(home, port, None)
}

fn http() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(Duration::from_secs(2)).build()
}

/// The engine's `/health` once it serves (it may migrate a fresh Nebo folder
/// first).
fn wait_serving(child: &mut Child, port: u16) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("the engine exited before it served: {status}");
        }
        if let Ok(resp) = http().get(&format!("http://127.0.0.1:{port}/health")).call() {
            return serde_json::from_reader(resp.into_reader()).unwrap();
        }
        assert!(Instant::now() < deadline, "the engine did not serve within 180 s");
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn quit(port: u16) {
    let quit = http()
        .post(&format!("http://127.0.0.1:{port}/api/v1/engine/quit"))
        .set("Authorization", &format!("Bearer {KEY}"))
        .call()
        .expect("Quit");
    assert_eq!(quit.status(), 202);
}

fn wait_exit(child: &mut Child, within: Duration) -> ExitStatus {
    let deadline = Instant::now() + within;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("the engine did not exit within {within:?}");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn a_held_port_exits_75() {
    let home = Home::new("held");
    let holder = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = holder.local_addr().unwrap().port();
    let mut child = engine(&home, port);
    let status = wait_exit(&mut child, Duration::from_secs(60));
    assert_eq!(status.code(), Some(75), "port held: {status}");
}

#[test]
fn the_engine_serves_without_a_window_and_quits_with_0() {
    let home = Home::new("serve");
    let port = free_port();
    let mut child = engine(&home, port);
    let health = wait_serving(&mut child, port);
    assert_eq!(health["role"], "engine");
    assert_eq!(health["pid"], child.id());
    assert_eq!(health["version"], env!("CARGO_PKG_VERSION"));

    // The shell's sign-in for its window, over HTTP.
    let ticket = http()
        .post(&format!("http://127.0.0.1:{port}/api/v1/local-session/ticket"))
        .set("Authorization", &format!("Bearer {KEY}"))
        .call()
        .expect("a sign-in ticket for the shell");
    let ticket: serde_json::Value = serde_json::from_reader(ticket.into_reader()).unwrap();
    assert!(ticket["path"].as_str().is_some_and(|p| p.starts_with("/api/v1/local-session?ticket=")));

    quit(port);
    let status = wait_exit(&mut child, Duration::from_secs(90));
    assert_eq!(status.code(), Some(0), "Quit is a clean exit: {status}");
}

/// A second engine for the same Nebo folder and port (an app opened while
/// the OS's engine serves, a handover in flight) exits 75 and leaves the
/// first serving: one engine, never two on one database.
#[test]
fn a_second_engine_leaves_the_first_serving() {
    let home = Home::new("second");
    let port = free_port();
    let mut first = engine(&home, port);
    let health = wait_serving(&mut first, port);
    let mut second = engine(&home, port);
    let status = wait_exit(&mut second, Duration::from_secs(60));
    assert_eq!(status.code(), Some(75), "the second engine: {status}");
    assert_eq!(wait_serving(&mut first, port)["pid"], health["pid"], "the first engine still serves");
    quit(port);
    wait_exit(&mut first, Duration::from_secs(90));
}

/// Quitting the app: an engine the OS runs (launchd here; the Windows
/// task's supervisor holds its stdin for itself) keeps serving when the
/// process that started it goes; the app's own child stops with it, the
/// graceful way.
#[test]
fn a_service_engine_outlives_the_app_and_a_child_stops_with_it() {
    let home = Home::new("outlives");
    let port = free_port();
    let mut service = engine_as(&home, port, Some("launchd"));
    let health = wait_serving(&mut service, port);
    drop(service.stdin.take());
    std::thread::sleep(Duration::from_secs(3));
    assert!(service.try_wait().unwrap().is_none(), "the service engine stopped with the app");
    assert_eq!(wait_serving(&mut service, port)["pid"], health["pid"]);
    quit(port);
    assert_eq!(wait_exit(&mut service, Duration::from_secs(90)).code(), Some(0));

    let mut child = engine_as(&home, port, Some("shell"));
    wait_serving(&mut child, port);
    drop(child.stdin.take());
    let status = wait_exit(&mut child, Duration::from_secs(90));
    assert_eq!(status.code(), Some(0), "the app's child stops the graceful way: {status}");
}
