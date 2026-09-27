//! The one test server: booted for real, on a free port, over a temp
//! `NEBO_HOME`. Shared by every integration test in this crate so a second
//! suite never grows a second way to start Nebo.
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
    _temp_dir: tempfile::TempDir,
    _handle: tokio::task::JoinHandle<()>,
}

impl TestServer {
    pub async fn boot() -> Self {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let data_dir = temp_dir.path().to_path_buf();

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
        // Set NEBO_HOME so config::data_dir() resolves to our temp dir
        // SAFETY: single-threaded at this point (before server spawn)
        unsafe {
            std::env::set_var("NEBO_HOME", &data_dir);
        }

        let key = install_key();
        let port = find_free_port();
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
            data_dir: temp_dir.path().to_path_buf(),
            _temp_dir: temp_dir,
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

/// This process's install key. Every server a test binary boots reads it
/// from `NEBO_MCP_API_KEY` (`config::read_install_key`), not from a file in
/// its `NEBO_HOME`: the tests in one binary boot their servers in parallel,
/// and `NEBO_HOME` is one variable for the whole process.
fn install_key() -> String {
    static KEY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    KEY.get_or_init(|| {
        let key = uuid::Uuid::new_v4().simple().to_string();
        // SAFETY: set before any server of this process starts, like NEBO_HOME.
        unsafe { std::env::set_var("NEBO_MCP_API_KEY", &key) };
        key
    })
    .clone()
}

pub fn find_free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}
