//! The one test server: booted for real, on a free port, over a temp
//! `NEBO_HOME`, in its test's own process (`in_own_process`). Shared by every
//! integration test in this crate so a second suite never grows a second way
//! to start Nebo.
//!
//! Each test binary compiles this module separately, so a helper only one
//! suite needs reads as dead code in the other.
#![allow(dead_code)]

pub mod model;

use std::path::PathBuf;
use std::time::Duration;

use reqwest::Client;
use serde_json::Value;

// ── Test Server ─────────────────────────────────────────────────────

pub struct TestServer {
    pub port: u16,
    /// The install key the test proves itself with, as the owner's own
    /// clients do (`install_key`).
    pub key: String,
    pub client: Client,
    pub data_dir: PathBuf,
    _handle: tokio::task::JoinHandle<()>,
}

/// Set in the environment of a test's own process ([`in_own_process`]).
const OWN_PROCESS: &str = "NEBO_TEST_OWN_PROCESS";

/// Run the calling test in a process of its own: a Nebo of its own, on a
/// home of its own, as the product runs one Nebo per process.
///
/// A Nebo's root (`config::data_dir`, read from `NEBO_HOME` all through the
/// product), its install key and its port are one per process. The tests of
/// one binary run in parallel, so servers booted side by side in the test
/// process shared them: each test's `NEBO_HOME` moved every other server,
/// and a test that ended deleted the home another server was still booting
/// on ("io error: No such file or directory").
///
/// Called first in every test that boots a [`TestServer`]. In the test
/// binary's own run it starts this binary again with only the calling test
/// selected, its home, install key, port and no reachable hub given in that
/// process's environment, waits for it, fails with its output if it failed,
/// and answers `false`: the caller returns. In that process it answers `true` and the
/// test runs, server and all. No process's environment is ever written.
pub async fn in_own_process() -> bool {
    if std::env::var_os(OWN_PROCESS).is_some() {
        return true;
    }
    let test = std::thread::current().name().expect("a test thread is named after its test").to_string();
    let home = tempfile::tempdir().expect("create temp dir");
    let key = uuid::Uuid::new_v4().simple().to_string();
    let out = tokio::process::Command::new(std::env::current_exe().expect("this test binary"))
        .args([test.as_str(), "--exact", "--include-ignored", "--test-threads=1"])
        .env(OWN_PROCESS, "1")
        .env("NEBO_HOME", home.path())
        // The install key every caller of the local API proves itself with
        // (`config::read_install_key`).
        .env("NEBO_MCP_API_KEY", &key)
        // The port the server listens on and every local caller addresses
        // (`napp::plugin::local_port`).
        .env("NEBO_PORT", find_free_port().to_string())
        // No hub is reachable: nothing here can act as anyone's bot.
        .envs([
            ("NEBOAI_API_URL", "http://127.0.0.1:9"),
            ("NEBOAI_JANUS_URL", "http://127.0.0.1:9"),
            ("NEBOAI_COMMS_URL", "http://127.0.0.1:9"),
            ("NEBOAI_TUNNEL_URL", "http://127.0.0.1:9"),
        ])
        .kill_on_drop(true)
        .output()
        .await
        .expect("run the test in its own process");
    assert!(
        out.status.success(),
        "{test}, in its own process: {}\n{}{}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    false
}

impl TestServer {
    /// Boot this process's Nebo, on the home, install key and port
    /// [`in_own_process`] gave it.
    pub async fn boot() -> Self {
        assert!(
            std::env::var_os(OWN_PROCESS).is_some(),
            "a TestServer boots only in a test's own process: begin the test with `if !common::in_own_process().await {{ return; }}`"
        );
        let data_dir = config::data_dir().expect("NEBO_HOME");

        // Create required subdirectories
        std::fs::create_dir_all(data_dir.join("data")).unwrap();
        std::fs::create_dir_all(data_dir.join("nebo").join("skills")).unwrap();
        std::fs::create_dir_all(data_dir.join("nebo").join("tools")).unwrap();
        std::fs::create_dir_all(data_dir.join("nebo").join("workflows")).unwrap();
        std::fs::create_dir_all(data_dir.join("nebo").join("agents")).unwrap();
        std::fs::create_dir_all(data_dir.join("user").join("skills")).unwrap();
        std::fs::create_dir_all(data_dir.join("user").join("tools")).unwrap();
        std::fs::create_dir_all(data_dir.join("user").join("workflows")).unwrap();
        std::fs::create_dir_all(data_dir.join("user").join("agents")).unwrap();

        let key = config::read_install_key().expect("NEBO_MCP_API_KEY");
        let port = napp::plugin::local_port();
        let db_path = data_dir.join("data").join("nebo.db");

        let mut cfg = config::Config::default();
        cfg.port = port;
        cfg.host = "127.0.0.1".to_string();
        cfg.database.sqlite_path = db_path.to_string_lossy().to_string();
        // Use a random JWT secret for test isolation
        cfg.auth.access_secret = uuid::Uuid::new_v4().to_string();

        let handle = tokio::spawn(async move {
            if let Err(e) = nebo_server::run(cfg, true).await {
                eprintln!("server error: {}", e);
            }
        });

        let client = Client::new();

        // Poll /health until ready (max 30s)
        let health_url = format!("http://127.0.0.1:{}/health", port);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            if tokio::time::Instant::now() > deadline {
                panic!("server failed to start within 30s");
            }
            match client.get(&health_url).send().await {
                Ok(resp) if resp.status().is_success() => break,
                _ => tokio::time::sleep(Duration::from_millis(100)).await,
            }
        }

        Self {
            port,
            key,
            client,
            data_dir,
            _handle: handle,
        }
    }

    /// An API route, with the install key as the address's first segment
    /// (`/k/<key>`, the form a process that knows Nebo as a base URL uses),
    /// so the Authorization header stays free for what a route checks
    /// itself (a user's token).
    pub fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}/k/{}/api/v1{}", self.port, self.key, path)
    }

    /// The chat socket, with the install key the same way.
    pub fn ws_url(&self) -> String {
        format!("ws://127.0.0.1:{}/k/{}/ws", self.port, self.key)
    }

    pub fn health_url(&self) -> String {
        format!("http://127.0.0.1:{}/health", self.port)
    }

    pub async fn get(&self, path: &str) -> reqwest::Response {
        self.client.get(&self.url(path)).send().await.unwrap()
    }

    pub async fn post_json(&self, path: &str, body: &Value) -> reqwest::Response {
        self.client
            .post(&self.url(path))
            .json(body)
            .send()
            .await
            .unwrap()
    }

    pub async fn put_json(&self, path: &str, body: &Value) -> reqwest::Response {
        self.client
            .put(&self.url(path))
            .json(body)
            .send()
            .await
            .unwrap()
    }

    pub async fn delete(&self, path: &str) -> reqwest::Response {
        self.client.delete(&self.url(path)).send().await.unwrap()
    }

    /// Get a direct DB store handle for setup/assertions that need DB access
    pub fn db_store(&self) -> db::Store {
        let db_path = self.data_dir.join("data").join("nebo.db");
        db::Store::new(&db_path.to_string_lossy()).expect("open test DB")
    }
}

pub fn find_free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}
