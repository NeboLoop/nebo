//! `nebo --engine`: the desktop app's executable as the server alone, no
//! window. It serves, says it is the engine, stops on the shell's Quit with
//! exit 0, and exits 75 when its port is held.
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

fn engine(home: &Home, port: u16) -> Child {
    // The architecture drift gate allows `Command::new` in tests.
    Command::new(env!("CARGO_BIN_EXE_nebo"))
        .arg("--engine")
        .env("NEBO_HOME", &home.0)
        .env("NEBO_PORT", port.to_string())
        .env("NEBO_MCP_API_KEY", KEY)
        .env_remove("NEBO_SUPERVISED")
        .envs([
            ("NEBOAI_API_URL", "http://127.0.0.1:9"),
            ("NEBOAI_JANUS_URL", "http://127.0.0.1:9"),
            ("NEBOAI_COMMS_URL", "http://127.0.0.1:9"),
            ("NEBOAI_TUNNEL_URL", "http://127.0.0.1:9"),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start nebo --engine")
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
    let http = ureq::AgentBuilder::new().timeout(Duration::from_secs(2)).build();

    let deadline = Instant::now() + Duration::from_secs(180);
    let health: serde_json::Value = loop {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("the engine exited before it served: {status}");
        }
        if let Ok(resp) = http.get(&format!("http://127.0.0.1:{port}/health")).call() {
            break serde_json::from_reader(resp.into_reader()).unwrap();
        }
        assert!(Instant::now() < deadline, "the engine did not serve within 180 s");
        std::thread::sleep(Duration::from_millis(250));
    };
    assert_eq!(health["role"], "engine");
    assert_eq!(health["pid"], child.id());
    assert_eq!(health["version"], env!("CARGO_PKG_VERSION"));

    // The shell's sign-in for its window, over HTTP.
    let ticket = http
        .post(&format!("http://127.0.0.1:{port}/api/v1/local-session/ticket"))
        .set("Authorization", &format!("Bearer {KEY}"))
        .call()
        .expect("a sign-in ticket for the shell");
    let ticket: serde_json::Value = serde_json::from_reader(ticket.into_reader()).unwrap();
    assert!(ticket["path"].as_str().is_some_and(|p| p.starts_with("/api/v1/local-session?ticket=")));

    let quit = http
        .post(&format!("http://127.0.0.1:{port}/api/v1/engine/quit"))
        .set("Authorization", &format!("Bearer {KEY}"))
        .call()
        .expect("Quit");
    assert_eq!(quit.status(), 202);
    let status = wait_exit(&mut child, Duration::from_secs(90));
    assert_eq!(status.code(), Some(0), "Quit is a clean exit: {status}");
}
