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
    if !common::in_own_process().await {
        return;
    }
    let server = TestServer::boot().await;

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
    store.set_agent_built_here("app-flip", true).unwrap();

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

/// The live check that verifies app work (`app_reload(check: true)`)
/// against the real app server: each page loads fresh on a phone and a
/// desktop in the bot's built-in browser. A console error, an uncaught
/// exception and an unhandled rejection each fail it, by name and view; a
/// page that drew nothing fails it; a clean page passes. Skipped when this
/// machine has no Chrome or Chromium.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_live_check_fails_errors_and_a_blank_page_and_passes_a_clean_one() {
    if !common::in_own_process().await {
        return;
    }
    let server = TestServer::boot().await;
    if browser::chrome::find_chrome().is_none() {
        eprintln!("no Chrome or Chromium on this machine — skipping");
        return;
    }
    let manager = Arc::new(browser::Manager::new(
        browser::BrowserConfig::default(),
        server.data_dir.to_string_lossy().into_owned(),
    ));
    let store = Arc::new(server.db_store());
    let app = |id: &str, name: &str, html: &str| {
        let ui = server.data_dir.join("apps").join(id).join("ui");
        std::fs::create_dir_all(&ui).unwrap();
        std::fs::write(ui.join("index.html"), html).unwrap();
        store.create_agent(id, None, name, "An app", "", "", None, None).unwrap();
        store.set_agent_app_fields(id, true, Some(ui.to_str().unwrap()), None, None).unwrap();
        store.set_agent_built_here(id, true).unwrap();
    };
    app(
        "app-broken",
        "Broken",
        "<!doctype html><html><body><h1>Broken</h1>\
         <script>console.error('broken: the board failed');</script>\
         <script>Promise.reject(new Error('broken: no scores'));</script>\
         <script>missingFunction();</script></body></html>",
    );
    app(
        "app-blank",
        "Blank",
        "<!doctype html><html><head><title>Blank</title></head><body><div id=\"root\"></div></body></html>",
    );
    app("app-clean", "Clean", "<!doctype html><html><body><h1>Clean</h1><p>Ready.</p></body></html>");

    let reload = tools::app_dev::tools(store, None, Some(manager))
        .into_iter()
        .find(|t| t.name() == tools::app_dev::APP_RELOAD)
        .unwrap();
    let check = |id: &'static str| {
        let ctx = tools::ToolContext {
            origin: tools::origin::Origin::User,
            session_key: format!("agent:{id}:web"),
            ..Default::default()
        };
        let reload = &reload;
        async move { reload.execute_dyn(&ctx, json!({ "check": true })).await }
    };

    let broken = check("app-broken").await;
    assert!(broken.is_error, "{}", broken.content);
    for (view, line) in [
        ("Phone (390×844)", "broken: the board failed"),
        ("Phone (390×844)", "broken: no scores"),
        ("Phone (390×844)", "missingFunction"),
        ("Desktop (1280×800)", "broken: the board failed"),
    ] {
        assert!(
            broken.content.lines().any(|l| l.contains(view) && l.contains(line)),
            "{view} {line}: {}",
            broken.content
        );
    }
    assert_eq!(broken.payload.as_ref().unwrap()["passed"], false);

    let blank = check("app-blank").await;
    assert!(blank.is_error, "{}", blank.content);
    assert!(blank.content.contains("drew nothing"), "{}", blank.content);

    let clean = check("app-clean").await;
    assert!(!clean.is_error, "{}", clean.content);
    assert_eq!(clean.payload.as_ref().unwrap()["passed"], true);
}
