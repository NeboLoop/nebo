//! The one test server: booted for real, on a free port, over a temp
//! `NEBO_HOME`. Shared by every integration test in this crate so a second
//! suite never grows a second way to start Nebo.
//!
//! Each test binary compiles this module separately, so a helper only one
//! suite needs reads as dead code in the other.
#![allow(dead_code)]

use std::path::PathBuf;
use std::time::Duration;

use reqwest::Client;
use serde_json::Value;

// ── Test Server ─────────────────────────────────────────────────────

pub struct TestServer {
    pub port: u16,
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
            client,
            data_dir: temp_dir.path().to_path_buf(),
            _temp_dir: temp_dir,
            _handle: handle,
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}/api/v1{}", self.port, path)
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
