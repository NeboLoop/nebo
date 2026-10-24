//! A real app, confined, still does everything it does: its page loads in
//! the bot's headless browser under its pass (`app_screenshot`,
//! `app_record`), reads its store the way the page does (the SDK's base,
//! `Origin: null`), hears its own changes on its socket, and shares a file.
//!
//! By hand, on a copy of an installed app's folder (the folder is read,
//! never written):
//!   NEBO_STUDIO_DIR="$HOME/Library/Application Support/Nebo/user/agents/Design Studio" \
//!     NEBO_STUDIO_REPORT=/tmp/studio.txt \
//!     cargo test -p nebo-server --test owner_studios -- --ignored

mod common;

use std::path::Path;
use std::sync::Arc;

use common::TestServer;
use futures::StreamExt;
use reqwest::StatusCode;
use serde_json::json;
use tools::registry::DynTool;

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let name = entry.file_name();
        if name == "node_modules" || name == ".git" {
            continue;
        }
        let (src, dst) = (entry.path(), to.join(&name));
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&src, &dst);
        } else {
            std::fs::copy(&src, &dst).unwrap();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "by hand: NEBO_STUDIO_DIR=<an app's folder> cargo test -p nebo-server --test owner_studios -- --ignored"]
async fn a_real_app_still_works_confined() {
    let Some(source) = std::env::var_os("NEBO_STUDIO_DIR") else {
        eprintln!("NEBO_STUDIO_DIR is not set — skipping");
        return;
    };
    if !common::in_own_process().await {
        return;
    }
    let source = Path::new(&source);
    let name = source.file_name().unwrap().to_string_lossy().into_owned();
    let home = config::data_dir().unwrap();
    let dir = home.join("user/agents").join(&name);
    copy_dir(source, &dir);
    let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).unwrap()).unwrap();
    let id = manifest["id"].as_str().unwrap().to_string();

    let server = TestServer::boot().await;
    let store = Arc::new(server.db_store());
    if store.get_agent(&id).ok().flatten().is_none() {
        store.create_agent(&id, None, &name, "", "", "", None, None).unwrap();
    }
    store.set_agent_app_fields(&id, true, Some(dir.join("ui").to_str().unwrap()), None, None).unwrap();
    // His studios are made here (`app_dev::is_own_app`).
    store.set_agent_built_here(&id, true).unwrap();
    let base = format!("http://127.0.0.1:{}", server.port);
    let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();

    // The owner opens it: a pass, and a sandboxed page.
    let opened = http.get(format!("{base}/k/{}/apps/{id}/ui/index.html", server.key)).send().await.unwrap();
    assert_eq!(opened.status(), StatusCode::SEE_OTHER);
    let location = opened.headers()["location"].to_str().unwrap().to_string();
    let pass = location.trim_start_matches("../").strip_prefix("k/").unwrap().split('/').next().unwrap().to_string();
    let page = http.get(format!("{base}/k/{pass}/apps/{id}/ui/index.html")).send().await.unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    println!("CSP: {}", page.headers()["content-security-policy"].to_str().unwrap());

    // The page's own read of its store (Design Studio fetches it directly,
    // from the SDK's base) from its opaque origin.
    let own = |path: &str| format!("{base}/k/{pass}/api/v1/apps/{id}{path}");
    let listing = http.get(own("/storage")).header("origin", "null").send().await.unwrap();
    assert_eq!(listing.status(), StatusCode::OK);
    assert_eq!(listing.headers()["access-control-allow-origin"], "null");

    // Its socket hears its own change.
    let mut ws_req = tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(format!(
        "ws://127.0.0.1:{}/k/{pass}/ws/app/{id}",
        server.port
    ))
    .unwrap();
    ws_req.headers_mut().insert("origin", "null".parse().unwrap());
    let (mut socket, _) = tokio_tungstenite::connect_async(ws_req).await.expect("the app's socket opens with its pass");
    let put = http
        .put(own("/storage/confine-check"))
        .header("origin", "null")
        .json(&json!({ "value": { "ok": true } }))
        .send()
        .await
        .unwrap();
    assert_eq!(put.status(), StatusCode::NO_CONTENT);
    let heard = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while let Some(Ok(msg)) = socket.next().await {
            if let Ok(text) = msg.to_text()
                && text.contains("app_data_changed")
                && text.contains("confine-check")
            {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or(false);
    assert!(heard, "app_data_changed reaches the page's socket");
    tools::app_data::remove(&store, &id, "confine-check").unwrap();

    // Share.
    let shared = http
        .post(own("/share"))
        .header("origin", "null")
        .json(&json!({ "name": "confine-check.html", "content": "<p>ok</p>" }))
        .send()
        .await
        .unwrap();
    let status = shared.status();
    println!("share: {status} {}", shared.text().await.unwrap_or_default());
    assert!(status.is_success(), "share");

    // The bot's headless browser: a screenshot and a short recording, and
    // what the page said on its console.
    if browser::chrome::find_chrome().is_none() {
        eprintln!("no Chrome — screenshot and recording skipped");
        return;
    }
    let manager = Arc::new(browser::Manager::new(browser::BrowserConfig::default(), server.data_dir.to_string_lossy().into_owned(), server.port));
    let ctx = tools::ToolContext {
        origin: tools::origin::Origin::User,
        session_key: format!("agent:{id}:web"),
        ..Default::default()
    };
    let shot = tools::app_publish::AppScreenshotTool::new(store.clone(), Some(manager.clone()))
        .execute_dyn(&ctx, json!({ "width": 1280, "height": 860, "wait_ms": 6000 }))
        .await;
    println!("app_screenshot: error={} {}", shot.is_error, shot.content);
    assert!(!shot.is_error, "{}", shot.content);
    let record = tools::app_record::AppRecordTool::new(store.clone(), Some(manager))
        .execute_dyn(&ctx, json!({ "width": 640, "height": 430, "seconds": 1, "fps": 4 }))
        .await;
    println!("app_record: error={} {}", record.is_error, record.content);
    assert!(!record.is_error, "{}", record.content);
    let console = tools::app_console::AppConsoleTool::new(store).execute_dyn(&ctx, json!({})).await;
    println!("app_console: {}", console.content);
    // The run's own process keeps its output; a report file shows it.
    if let Some(report) = std::env::var_os("NEBO_STUDIO_REPORT") {
        let text = format!("app_screenshot:\n{}\n\napp_record:\n{}\n\napp_console:\n{}\n", shot.content, record.content, console.content);
        std::fs::write(&report, text).unwrap();
        if let Some(picture) = shot.image_url.as_deref() {
            let mut copy = std::path::PathBuf::from(&report);
            copy.set_extension("png");
            std::fs::copy(picture, copy).unwrap();
        }
    }
    for broken in ["401", "Unauthorized", "Could not read the store", "Content Security Policy", "SecurityError", "Failed to fetch"] {
        assert!(!shot.content.contains(broken) && !console.content.contains(broken), "{broken}: {}\n{}", shot.content, console.content);
    }
}
