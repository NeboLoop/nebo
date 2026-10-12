//! The engine's API for the desktop shell, another process: `/health`'s
//! handshake, an app window's facts (`GET /api/v1/apps/{id}/desktop`) and
//! "Quit Nebo" (`POST /api/v1/engine/quit`). The last two answer only the
//! shell's own call: the install key as its bearer token, no page behind it.
//! Each run records how it ended in `engine-run.json`, and a stall the last
//! run ended inside (`stall.json`) is taken at start.
//!
//! Run:
//!   cargo test -p nebo-server --test engine_api

use std::time::Duration;

mod common;
use common::TestServer;

fn api(server: &TestServer, path: &str) -> String {
    format!("http://127.0.0.1:{}/api/v1{path}", server.port)
}

#[tokio::test]
async fn health_names_the_engine() {
    if !common::in_own_process().await {
        return;
    }
    let server = TestServer::boot().await;
    let health: serde_json::Value = server.client.get(server.health_url()).send().await.unwrap().json().await.unwrap();
    assert_eq!(health["status"], "ok");
    assert_eq!(health["role"], "engine");
    assert_eq!(health["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(health["pid"], std::process::id());
    assert_eq!(health["supervised"], false, "nothing restarts a test server");
    assert!(health["startedAt"].as_str().is_some_and(|s| !s.is_empty()));
}

#[tokio::test]
async fn desktop_app_answers_only_the_shell() {
    if !common::in_own_process().await {
        return;
    }
    let server = TestServer::boot().await;
    let url = api(&server, "/apps/no-such-app/desktop");

    let bare = server.client.get(&url).send().await.unwrap();
    assert_eq!(bare.status(), 401, "no proof");

    let wrong = server.client.get(&url).bearer_auth("not-the-key").send().await.unwrap();
    assert_eq!(wrong.status(), 401, "a wrong key");

    // An app window's page reaches the engine through the shell's
    // `neboapp://` proxy with the key, and always names its page.
    let page = server
        .client
        .get(&url)
        .bearer_auth(&server.key)
        .header("Origin", "neboapp://no-such-app")
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), 401, "a page behind the shell's proxy");

    // The key in the path (a base URL's credential) is not the shell's call.
    let path_key = server.client.get(server.url("/apps/no-such-app/desktop")).send().await.unwrap();
    assert_eq!(path_key.status(), 401, "the key in the path");

    let shell = server.client.get(&url).bearer_auth(&server.key).send().await.unwrap();
    assert_eq!(shell.status(), 200);
    let body: serde_json::Value = shell.json().await.unwrap();
    assert_eq!(body["offersPublish"], false);
    assert_eq!(body["developerScript"], "");
    assert!(body["uiDir"].is_null());
}

#[tokio::test]
async fn quit_stops_the_engine_for_the_shell_only() {
    if !common::in_own_process().await {
        return;
    }
    let server = TestServer::boot().await;
    let url = api(&server, "/engine/quit");

    assert_eq!(server.client.post(&url).send().await.unwrap().status(), 401, "no proof");
    let page = server
        .client
        .post(&url)
        .bearer_auth(&server.key)
        .header("Origin", "neboapp://some-app")
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), 401, "a page behind the shell's proxy");
    assert!(server.client.get(server.health_url()).send().await.unwrap().status().is_success(), "still serving");

    let run_file = config::data_dir().unwrap().join("engine-run.json");
    assert_eq!(engine_run(&run_file)["cleanExit"], false, "a run in progress");

    let quit = server.client.post(&url).bearer_auth(&server.key).send().await.unwrap();
    assert_eq!(quit.status(), 202);

    // The graceful stop: the port closes and `run` returns.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        if server.client.get(server.health_url()).timeout(Duration::from_secs(2)).send().await.is_err() {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "the engine still serves 60 s after Quit");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // Stopped on purpose: the next run is not a restart.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    while engine_run(&run_file)["cleanExit"] != true {
        assert!(tokio::time::Instant::now() < deadline, "engine-run.json never says cleanExit");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

fn engine_run(path: &std::path::Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(path).expect("engine-run.json")).unwrap()
}

/// The last run ended inside a stall (the watchdog exited it): this run
/// takes `stall.json` (to send as unrecovered) and counts a restart.
#[tokio::test]
async fn a_run_that_ended_in_a_stall_is_a_restart() {
    if !common::in_own_process().await {
        return;
    }
    let dir = config::data_dir().unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    let stall = types::stall::Report { kind: "stalled".into(), at: 1_000, ..Default::default() };
    types::stall::write_unrecovered(&dir, &stall).unwrap();
    std::fs::write(dir.join("engine-run.json"), r#"{"pid":1,"startedAt":900,"aliveAt":990}"#).unwrap();

    let _server = TestServer::boot().await;
    assert!(!dir.join("stall.json").exists(), "taken at start");
    let run = engine_run(&dir.join("engine-run.json"));
    assert_eq!(run["pid"], std::process::id());
    assert_eq!(run["cleanExit"], false);
    assert_eq!(run["restarts"].as_array().map(Vec::len), Some(1));
    let pending = types::stall::pending();
    assert_eq!(pending.len(), 1);
    assert!(pending[0].unrecovered && pending[0].kind == "stalled" && pending[0].at == 1_000);
}
