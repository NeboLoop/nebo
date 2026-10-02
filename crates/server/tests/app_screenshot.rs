//! `app_screenshot` against the real app server: the bot's built-in browser
//! opens a tiny app's page as the server serves it on 127.0.0.1 and comes
//! back with the picture and the console of that load.
//!
//! 2026-10-02: every app screenshot failed. The browsing browser (Obscura)
//! refuses loopback addresses ("Access to private/internal IP address
//! 127.0.0.1 is not allowed"), and the app is served on 127.0.0.1; past that,
//! it has no paint engine and refuses every screenshot. The app's page now
//! opens in a stock Chromium. Skipped when this machine has none.

mod common;

use std::sync::Arc;

use common::TestServer;
use serde_json::json;
use tools::registry::DynTool;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_screenshot_shows_the_app_as_the_bot_serves_it() {
    let server = TestServer::boot().await;
    // The port the tool addresses the bot on (`napp::plugin::local_port`).
    // SAFETY: the one test in this binary; nothing else reads it yet.
    unsafe { std::env::set_var("NEBO_PORT", server.port.to_string()) };

    let manager = Arc::new(browser::Manager::new(
        browser::BrowserConfig::default(),
        server.data_dir.to_string_lossy().into_owned(),
    ));
    // The page is drawn by a stock Chromium (the bot's browsing browser
    // draws nothing).
    if browser::chrome::find_chrome().is_none() {
        eprintln!("no Chrome or Chromium on this machine — skipping");
        return;
    }

    let ui = server.data_dir.join("apps").join("flip-flap").join("ui");
    std::fs::create_dir_all(&ui).unwrap();
    std::fs::write(
        ui.join("index.html"),
        "<!doctype html><html><head><title>Flip-Flap</title></head>\
         <body style=\"margin:0;background:#d22\"><h1>Flip-Flap</h1><button id=\"start\">START</button>\
         <script>console.error('flip-flap: START has no handler')</script></body></html>",
    )
    .unwrap();
    let store = Arc::new(server.db_store());
    store
        .create_agent("app-flip", None, "Flip-Flap", "A flipping game", "", "", None, None)
        .unwrap();
    store
        .set_agent_app_fields("app-flip", true, Some(ui.to_str().unwrap()), None, None)
        .unwrap();

    let tool = tools::app_publish::AppScreenshotTool::new(store, Some(manager));
    let ctx = tools::ToolContext {
        origin: tools::origin::Origin::User,
        session_key: "agent:app-flip:web".into(),
        ..Default::default()
    };
    let shot = tool
        .execute_dyn(&ctx, json!({ "width": 400, "height": 300, "wait_ms": 1500 }))
        .await;
    assert!(!shot.is_error, "{}", shot.content);
    let picture = shot.image_url.as_deref().expect("the screenshot's file");
    let bytes = std::fs::read(picture).expect("the screenshot is saved");
    assert!(bytes.starts_with(b"\x89PNG"), "a PNG");
    assert!(ai::image_norm::dimensions(&bytes).is_some_and(|(w, h)| w > 0 && h > 0));
    // The app's own script ran: its console line came back with the shot.
    assert!(
        shot.content.contains("flip-flap: START has no handler"),
        "{}",
        shot.content
    );
}
