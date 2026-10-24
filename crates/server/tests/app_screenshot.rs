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
        server.port,
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
        server.port,
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

/// An employee opens its own app's page with `browser_open`, as the owner
/// would type its address: the page opens behind a pass to that app alone
/// (`napp::page_pass`), in the bot's own headless browser, so the employee
/// sees it run, clicks, types and play-tests it. The page reaches its own
/// routes and neither another app's nor Nebo's API; no step's answer names
/// the pass; the page never leaves its own address (a script setting
/// `location`, a link); closing the tab drops the pass; an app the employee
/// may not work on is refused.
///
/// 2026-10-22: an employee built an app, opened its page in the browser,
/// landed on the owner's sign-in and went looking for the install key.
/// Skipped when this machine has no Chrome or Chromium (never on CI).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn browser_open_on_an_own_app_page_opens_it_behind_that_apps_pass() {
    if !common::in_own_process().await {
        return;
    }
    let server = TestServer::boot().await;
    if browser::chrome::find_chrome().is_none() {
        // CI's runners carry Chrome: there this test runs, never skips.
        assert!(std::env::var_os("CI").is_none(), "no Chrome or Chromium on a CI runner: this test must run there");
        eprintln!("no Chrome or Chromium on this machine — skipping");
        return;
    }
    let manager = Arc::new(browser::Manager::new(
        browser::BrowserConfig::default(),
        server.data_dir.to_string_lossy().into_owned(),
        server.port,
    ));
    let store = Arc::new(server.db_store());
    let app = |id: &str, name: &str, built_here: bool, html: &str| {
        let ui = server.data_dir.join("apps").join(id).join("ui");
        std::fs::create_dir_all(&ui).unwrap();
        std::fs::write(ui.join("index.html"), html).unwrap();
        store.create_agent(id, None, name, "An app", "", "", None, None).unwrap();
        store.set_agent_app_fields(id, true, Some(ui.to_str().unwrap()), None, None).unwrap();
        store.set_agent_built_here(id, built_here).unwrap();
    };
    // Another server on this computer, counting the requests that reach it
    // (a connection the browser opens ahead and never asks on is not one):
    // the page frames it, and the frame must never load.
    let other = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let other_port = other.local_addr().unwrap().port();
    let other_hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let hits = other_hits.clone();
    tokio::spawn(async move {
        while let Ok((mut conn, _)) = other.accept().await {
            let hits = hits.clone();
            tokio::spawn(async move {
                use tokio::io::AsyncReadExt;
                let mut buf = [0u8; 16];
                if let Ok(n) = conn.read(&mut buf).await
                    && n > 0
                {
                    hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
            });
        }
    });
    // The page asks, from its own address, for its own store, another
    // app's, and Nebo's own API; it frames this computer's other server and
    // a cloud's metadata address.
    let league = format!(
        "<!doctype html><html><body>\
         <a id=\"out\" href=\"/\" style=\"position:fixed;left:0;top:0;width:300px;height:200px;display:block\">out</a>\
         <iframe src=\"http://127.0.0.1:{other_port}/frame\"></iframe>\
         <iframe src=\"http://169.254.169.254/latest/meta-data/\"></iframe>\
         <h1>League</h1><script>\
         (async () => {{\
           const r = {{}};\
           const ask = async (k, path, init) => {{\
             try {{ r[k] = (await fetch(path.startsWith('http') ? path : '../../../' + path, init)).ok; }} catch (e) {{ r[k] = 'refused'; }}\
           }};\
           await ask('own', 'api/v1/apps/app-league/storage/score', {{ method: 'PUT',\
             headers: {{ 'content-type': 'application/json' }}, body: JSON.stringify({{ value: 3 }}) }});\
           await ask('other', 'api/v1/apps/app-notes/storage');\
           await ask('nebo', 'api/v1/agents');\
           await ask('server', 'http://127.0.0.1:{other_port}/fetch', {{ mode: 'no-cors' }});\
           await ask('metadata', 'http://169.254.169.254/latest/meta-data/', {{ mode: 'no-cors' }});\
           await ask('where', 'api/v1/apps/app-league/storage/where', {{ method: 'PUT',\
             headers: {{ 'content-type': 'application/json' }}, body: JSON.stringify({{ value: location.pathname }}) }});\
           window.__asked = r;\
         }})();\
         </script></body></html>"
    );
    app("app-league", "League", true, &league);
    app("app-notes", "Notes", true, "<!doctype html><html><body>Notes</body></html>");
    app("app-bought", "Bought", false, "<!doctype html><html><body>Bought</body></html>");

    let tools = tools::web_tool::tools(
        tools::web_tool::WebCore::new().with_browser(manager).with_store(store.clone()),
    );
    let tool = |name: &str| tools.iter().find(|t| t.name() == name).unwrap();
    let ctx = tools::ToolContext {
        origin: tools::origin::Origin::User,
        session_key: "agent:app-league:web".into(),
        session_id: "s-league".into(),
        ..Default::default()
    };
    let page = |id: &str| format!("http://127.0.0.1:{}/apps/{id}/ui/index.html", server.port);

    let opened = tool("browser_open").execute_dyn(&ctx, json!({ "url": page("app-league") })).await;
    assert!(!opened.is_error, "{}", opened.content);
    assert!(!opened.content.contains("/k/"), "the pass never reaches the model: {}", opened.content);

    let run_js = tool("browser_run_js");
    let drawn = run_js.execute_dyn(&ctx, json!({ "expression": "document.querySelector('h1')?.textContent" })).await;
    assert!(drawn.content.contains("League"), "the page, not the sign-in: {}", drawn.content);
    let mut asked = String::new();
    for _ in 0..50 {
        let r = run_js
            .execute_dyn(&ctx, json!({ "expression": "JSON.stringify(window.__asked || null)" }))
            .await;
        assert!(!r.is_error, "{}", r.content);
        if r.content.contains("where") {
            asked = r.content;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    assert!(asked.contains(r#""own":true"#), "its own routes: {asked}");
    assert!(!asked.contains(r#""other":true"#), "never another app's: {asked}");
    assert!(!asked.contains(r#""nebo":true"#), "never Nebo's API: {asked}");
    // Nothing of this computer's other server, nor of a cloud's metadata
    // address, was reached: not by a frame, not by a request.
    assert!(asked.contains(r#""server":"refused""#), "never another server here: {asked}");
    assert!(asked.contains(r#""metadata":"refused""#), "never a cloud's metadata address: {asked}");
    assert_eq!(other_hits.load(std::sync::atomic::Ordering::SeqCst), 0, "a frame or a fetch reached this computer's other server");

    // What a step answers names the page by its address alone.
    let at = run_js.execute_dyn(&ctx, json!({ "expression": "location.href" })).await;
    assert!(at.content.contains("/apps/app-league/ui/") && !at.content.contains("/k/"), "{}", at.content);
    let read = tool("browser_read").execute_dyn(&ctx, json!({})).await;
    assert!(!read.content.contains("/k/"), "{}", read.content);
    // The page itself knows its pass (it wrote its address to its store).
    let written = tools::app_data::read(&store, "app-league", "where").unwrap().expect("the page wrote its address");
    let pass = tools::app_data::decode(&written)
        .as_str()
        .and_then(|p| p.split("/k/").nth(1))
        .and_then(|r| r.split('/').next())
        .unwrap_or_else(|| panic!("the page is under a pass: {written}"))
        .to_string();
    let league = napp::page_pass::Scope::App("app-league".into());
    assert!(napp::page_pass::admits(&pass, &league));
    assert!(!napp::page_pass::admits(&pass, &napp::page_pass::Scope::App("app-notes".into())));

    // The page never opens another address: not by a script setting
    // `location` (a cloud's metadata address, Nebo's own API, a public
    // site), not by a link. The load is stopped or refused and the tab is on
    // the app's page again; nothing of the other address reaches the step.
    let reached = |away: &str| {
        let away = away.to_string();
        let ctx = &ctx;
        async move {
            tokio::time::sleep(std::time::Duration::from_millis(800)).await;
            let r = run_js
                .execute_dyn(&ctx, json!({ "expression": "location.href + ' | ' + document.body.innerText" }))
                .await;
            // The address itself, never one that only starts with it (the
            // app's own page starts with Nebo's home address).
            r.content.starts_with(&format!("{away} |")) || r.content.contains("\"agents\"") || r.content.contains(&format!("\"{away}"))
        }
    };
    let nebo_api = format!("http://127.0.0.1:{}/api/v1/agents", server.port);
    // A public site too: the top stays on the app's own address, so the
    // employee never wanders off the app and no other site is opened from
    // the page's address.
    for away in ["http://169.254.169.254/latest/meta-data/", nebo_api.as_str(), "http://example.invalid/"] {
        let reopened = tool("browser_open").execute_dyn(&ctx, json!({ "url": page("app-league") })).await;
        assert!(!reopened.is_error, "{}", reopened.content);
        let _ = run_js.execute_dyn(&ctx, json!({ "expression": format!("location.href = {}", json!(away)) })).await;
        assert!(!reached(away).await, "a script took the page to {away}");
        let at = run_js.execute_dyn(&ctx, json!({ "expression": "location.href" })).await;
        assert!(at.content.contains("/apps/app-league/ui/"), "the page stays on the app's address after {away}: {}", at.content);
    }
    let reopened = tool("browser_open").execute_dyn(&ctx, json!({ "url": page("app-league") })).await;
    assert!(!reopened.is_error, "{}", reopened.content);
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let _ = tool("browser_act").execute_dyn(&ctx, json!({ "action": "click", "coordinate": [50, 50] })).await;
    let home = format!("http://127.0.0.1:{}/", server.port);
    assert!(!reached(&home).await, "a link took the page to Nebo's own page");
    // Each reopen took a pass of its own; the first is gone with its page.
    assert!(!napp::page_pass::admits(&pass, &league), "a page reopened drops the pass it had");

    // Closing the tab drops the pass of the page open now.
    let live = tools::app_data::read(&store, "app-league", "where").unwrap().unwrap_or_default();
    let now = tools::app_data::decode(&live)
        .as_str()
        .and_then(|p| p.split("/k/").nth(1))
        .and_then(|r| r.split('/').next())
        .unwrap_or_default()
        .to_string();
    assert!(napp::page_pass::admits(&now, &league), "the open page's pass: {live}");
    let closed = tool("browser_close_tab").execute_dyn(&ctx, json!({})).await;
    assert!(!closed.is_error, "{}", closed.content);
    assert!(!napp::page_pass::admits(&now, &league), "the pass goes with the tab");

    // An app page's address carries its pass: no request from the page
    // sends that address to another site (`Referer`), whatever the page
    // opens or embeds there.
    let shown = napp::page_pass::grant(league.clone(), std::time::Duration::from_secs(60));
    let served = server
        .client
        .get(format!("http://127.0.0.1:{}/k/{shown}/apps/app-league/ui/index.html", server.port))
        .send()
        .await
        .unwrap();
    napp::page_pass::revoke(&shown);
    assert!(served.status().is_success(), "{}", served.status());
    let policy = served.headers().get("referrer-policy").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
    assert!(
        ["no-referrer", "same-origin", "strict-origin", "strict-origin-when-cross-origin"].contains(&policy.as_str()),
        "an app page's address never goes to another site: Referrer-Policy {policy:?}"
    );

    // Apps it may not work on: another of the owner's (App Developer mode
    // is off) and one from the marketplace.
    let other = tool("browser_open").execute_dyn(&ctx, json!({ "url": page("app-notes") })).await;
    assert!(other.is_error && other.content.contains("App Developer mode is off"), "{}", other.content);
    let bought = tool("browser_open").execute_dyn(&ctx, json!({ "url": page("app-bought") })).await;
    assert!(bought.is_error && bought.content.contains("installed from the marketplace"), "{}", bought.content);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_missing_key_reads_null_quietly_and_a_misplaced_stylesheet_names_both_paths() {
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
        server.port,
    ));
    let store = Arc::new(server.db_store());
    let app = |id: &str, name: &str, files: &[(&str, &str)]| {
        let ui = server.data_dir.join("apps").join(id).join("ui");
        for (path, text) in files {
            let file = ui.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, text).unwrap();
        }
        store.create_agent(id, None, name, "An app", "", "", None, None).unwrap();
        store.set_agent_app_fields(id, true, Some(ui.to_str().unwrap()), None, None).unwrap();
        store.set_agent_built_here(id, true).unwrap();
    };
    app(
        "app-league",
        "League",
        &[(
            "index.html",
            "<!doctype html><html><head><script src=\"../../../sdk/nebo.global.js\"></script></head><body>\
             <h1 id=\"saved\">loading</h1><script>\
             const { nebo } = window.NeboAppSDK;\
             nebo.storage.getItem('never-saved').then(v => {\
               document.getElementById('saved').textContent = v === null ? 'saved: null' : 'saved: ' + JSON.stringify(v);\
               if (v !== null) console.error('a key never saved read ' + JSON.stringify(v));\
             });\
             </script></body></html>",
        )],
    );
    app(
        "app-styled",
        "Styled",
        &[
            (
                "index.html",
                "<!doctype html><html><head><link rel=\"stylesheet\" href=\"./style.css\"></head>\
                 <body><h1>Styled</h1><img src=\"../../../api/v1/no-such-route.png\"></body></html>",
            ),
            ("css/style.css", "h1 { color: teal }"),
        ],
    );

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

    let league = check("app-league").await;
    assert!(!league.is_error, "a key never saved is no error: {}", league.content);
    assert_eq!(league.payload.as_ref().unwrap()["passed"], true);

    let styled = check("app-styled").await;
    assert!(styled.is_error, "{}", styled.content);
    assert!(
        styled.content.contains("the page asked for ui/style.css (the served page writes \"./style.css\"), and there is no such file")
            && styled.content.contains("ui/css/style.css is there"),
        "both paths named: {}",
        styled.content
    );
    // A failed load of one of Nebo's own routes is never read as a file
    // of ui/.
    assert!(styled.content.contains("no-such-route.png"), "{}", styled.content);
    assert!(!styled.content.contains("asked for /api/"), "{}", styled.content);
    assert!(!styled.content.contains("asked for ui/api/"), "{}", styled.content);
}
