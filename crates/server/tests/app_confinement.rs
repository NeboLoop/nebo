//! An app's page is confined to its own app.
//!
//! The page runs in an origin of its own (a CSP sandbox, so its origin is
//! opaque) and proves itself with its app's pass (`napp::app_view`) alone:
//! in a browser on this computer, through the tunnel (where the hub forwards
//! its calls unchecked), and in Nebo's own headless browser. The pass opens
//! that app's routes and nothing of the owner's: not his agents, not
//! another app's data. The owner's browser session never counts on a page
//! of opaque origin.
//!
//! Driven through the real server.
//!
//! Run:
//!   cargo test -p nebo-server --test app_confinement

use reqwest::StatusCode;
use reqwest::redirect::Policy;

mod common;
use common::TestServer;

const APP: &str = "app-crm";
const OTHER: &str = "app-notes";

/// An app on disk, as the loader reads it, and its row.
fn install(server_dir: &std::path::Path, id: &str, name: &str) {
    let dir = server_dir.join("user/agents").join(name);
    std::fs::create_dir_all(dir.join("ui")).unwrap();
    std::fs::write(dir.join("AGENT.md"), format!("---\nname: {name}\n---\n# {name}")).unwrap();
    let manifest = serde_json::json!({
        "id": id, "name": name, "version": "1.0.0", "type": "app", "permissions": ["storage:readwrite"],
    });
    std::fs::write(dir.join("manifest.json"), manifest.to_string()).unwrap();
    std::fs::write(
        dir.join("ui/index.html"),
        "<html><head><script type=\"module\" crossorigin src=\"./app.js\"></script></head><body>crm</body></html>",
    )
    .unwrap();
    std::fs::write(dir.join("ui/app.js"), "console.log('crm')").unwrap();
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().redirect(Policy::none()).build().unwrap()
}

#[tokio::test]
async fn an_app_page_reaches_its_own_app_alone() {
    if !common::in_own_process().await {
        return;
    }
    let home = config::data_dir().unwrap();
    install(&home, APP, "CRM");
    install(&home, OTHER, "Notes");
    let server = TestServer::boot().await;
    let base = format!("http://127.0.0.1:{}", server.port);
    let http = client();

    // The owner opens the page (his own client, the install key): it is
    // opened again under a pass of its own, relative to where he asked, so
    // a prefix before it (the tunnel's `/t/<bot>`) stays.
    let opened = http
        .get(format!("{base}/k/{}/apps/{APP}/ui/index.html?thread=t1", server.key))
        .send()
        .await
        .unwrap();
    assert_eq!(opened.status(), StatusCode::SEE_OTHER);
    let location = opened.headers()["location"].to_str().unwrap().to_string();
    assert!(location.starts_with("../../../../../k/av"), "{location}");
    assert!(location.ends_with(&format!("/apps/{APP}/ui/index.html?thread=t1")), "{location}");
    let pass = location
        .trim_start_matches("../")
        .strip_prefix("k/")
        .and_then(|r| r.split('/').next())
        .unwrap()
        .to_string();
    assert!(napp::app_view::admits(&pass, APP) && !napp::app_view::admits(&pass, OTHER));

    // The page, under its pass: sandboxed, readable by its own opaque origin,
    // and its files load with the pass its address carries.
    let page = http
        .get(format!("{base}/k/{pass}/apps/{APP}/ui/index.html"))
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    let csp = page.headers()["content-security-policy"].to_str().unwrap().to_string();
    assert!(csp.starts_with("sandbox ") && csp.contains("allow-scripts"), "{csp}");
    assert!(!csp.contains("allow-same-origin"), "the page never shares Nebo's origin: {csp}");
    let html = page.text().await.unwrap();
    assert!(html.contains("data-nebo-bridge") && html.contains("data-nebo-storage"), "{html}");
    let script = http
        .get(format!("{base}/k/{pass}/apps/{APP}/ui/app.js"))
        .header("origin", "null")
        .send()
        .await
        .unwrap();
    assert_eq!(script.status(), StatusCode::OK);
    // The one opaque origin CORS lets through: a page's, with its pass.
    assert_eq!(script.headers()["access-control-allow-origin"], "null");
    let foreign = http
        .get(format!("{base}/k/{pass}/api/v1/apps/{OTHER}/storage"))
        .header("origin", "null")
        .send()
        .await
        .unwrap();
    assert!(foreign.headers().get("access-control-allow-origin").is_none_or(|v| v != "null"), "not on another app's route");

    // Its own routes: yes. Another app's, and the owner's: no.
    let own = |path: &str| format!("{base}/k/{pass}{path}");
    let put = http
        .put(own(&format!("/api/v1/apps/{APP}/storage/contact")))
        .header("origin", "null")
        .json(&serde_json::json!({ "value": { "name": "Ann" } }))
        .send()
        .await
        .unwrap();
    assert_eq!(put.status(), StatusCode::NO_CONTENT);
    let got = http.get(own(&format!("/api/v1/apps/{APP}/storage/contact"))).send().await.unwrap();
    assert_eq!(got.status(), StatusCode::OK);
    for path in [
        format!("/api/v1/apps/{OTHER}/storage"),
        format!("/apps/{OTHER}/ui/index.html"),
        "/api/v1/agents".to_string(),
        "/api/v1/settings".to_string(),
        "/agent/mcp".to_string(),
        "/ws".to_string(),
    ] {
        let r = http.get(own(&path)).send().await.unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED, "{path}");
    }

    // The owner's signed-in browser session, carried by a page of opaque
    // origin (`Origin: null`), opens nothing, even with Fetch Metadata that
    // says otherwise.
    let session = format!("{}={}", nebo_server::local_access::COOKIE, nebo_server::local_access::session_for(&server.key));
    let host = format!("127.0.0.1:{}", server.port);
    for path in ["/api/v1/agents", &format!("/api/v1/apps/{APP}/storage")] {
        let r = http
            .get(format!("{base}{path}"))
            .header("cookie", &session)
            .header("origin", "null")
            .header("sec-fetch-site", "same-origin")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED, "{path}");
    }
    // The session on Nebo's own page still opens Nebo.
    let r = http
        .get(format!("{base}/api/v1/agents"))
        .header("cookie", &session)
        .header("sec-fetch-site", "same-origin")
        .header("host", &host)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    // And it reaches no app's routes: an app's routes take the app's pass.
    let r = http
        .get(format!("{base}/api/v1/apps/{APP}/storage"))
        .header("cookie", &session)
        .header("sec-fetch-site", "same-origin")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED, "an app's store is its page's, not a referer's");

    // A page's localStorage, kept in its store, comes back on its next open.
    let kept = http
        .put(own(&format!("/api/v1/apps/{APP}/storage/localStorage")))
        .json(&serde_json::json!({ "value": { "best": "42", "</script>": "x" } }))
        .send()
        .await
        .unwrap();
    assert_eq!(kept.status(), StatusCode::NO_CONTENT);
    let html = http
        .get(format!("{base}/k/{pass}/apps/{APP}/ui/index.html"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.contains(r#""best":"42""#), "{html}");
    assert!(!html.contains(r#""</script>""#) && html.contains(r#"</script>"#), "kept text never closes the script");

    // A page whose pass is gone (the bot restarted), reopened by the owner:
    // a new pass in the old one's place.
    napp::app_view::revoke(&pass);
    let reopened = http
        .get(format!("{base}/k/{pass}/apps/{APP}/ui/index.html"))
        .header("cookie", &session)
        .header("sec-fetch-site", "none")
        .send()
        .await
        .unwrap();
    assert_eq!(reopened.status(), StatusCode::SEE_OTHER);
    let location = reopened.headers()["location"].to_str().unwrap();
    assert!(location.starts_with("../../../../../k/av") && !location.contains(&pass), "{location}");
    let r = http.get(own(&format!("/api/v1/apps/{APP}/storage"))).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED, "a revoked pass opens nothing");
}

/// Through the tunnel, an app page's own calls reach the bot unchecked by
/// the hub (they carry no hub sign-in: the page's origin is its own), and
/// the bot stamps them as such: the app's pass on its own routes and the
/// SDK's script open, nothing else does, whatever else they carry.
#[tokio::test]
async fn an_unchecked_tunnel_request_opens_only_its_apps_routes() {
    if !common::in_own_process().await {
        return;
    }
    let home = config::data_dir().unwrap();
    install(&home, APP, "CRM");
    install(&home, OTHER, "Notes");
    let server = TestServer::boot().await;
    let base = format!("http://127.0.0.1:{}", server.port);
    let http = client();
    let pass = napp::app_view::grant(APP, std::time::Duration::from_secs(60));
    let unchecked = |req: reqwest::RequestBuilder| {
        req.header("host", "neboai.com")
            .header(comm::tunnel::TUNNEL_UNVERIFIED_HEADER, comm::tunnel::tunnel_auth_secret())
    };

    let r = unchecked(http.get(format!("{base}/k/{pass}/api/v1/apps/{APP}/storage"))).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let r = unchecked(http.get(format!("{base}/k/{pass}/apps/{APP}/ui/index.html"))).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let r = unchecked(http.get(format!("{base}/k/{pass}/sdk/nebo.global.js"))).send().await.unwrap();
    assert!(r.status() == StatusCode::OK || r.status() == StatusCode::NOT_FOUND, "{}", r.status());

    for path in [
        format!("/k/{pass}/api/v1/apps/{OTHER}/storage"),
        format!("/k/{pass}/api/v1/agents"),
        format!("/k/{pass}/api/v1/files/README.md"),
        "/api/v1/agents".to_string(),
        format!("/k/{}/api/v1/agents", server.key),
        "/health".to_string(),
    ] {
        let r = unchecked(http.get(format!("{base}{path}"))).send().await.unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED, "{path}");
    }
    // Even with the install key as a bearer: the hub checked nobody.
    let r = unchecked(http.get(format!("{base}/api/v1/agents")))
        .header("authorization", format!("Bearer {}", server.key))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    // A page opened without its pass is not given one here: only the owner
    // through the hub's sign-in opens a page.
    let r = unchecked(http.get(format!("{base}/apps/{APP}/ui/index.html"))).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);

    // The owner through the hub's sign-in is stamped as before.
    let r = http
        .get(format!("{base}/api/v1/agents"))
        .header("host", "neboai.com")
        .header(comm::tunnel::TUNNEL_AUTH_HEADER, comm::tunnel::tunnel_auth_secret())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    napp::app_view::revoke(&pass);
}
