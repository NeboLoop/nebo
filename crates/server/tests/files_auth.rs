//! The owner's files (`/api/v1/files/…`) answer only a caller that proves
//! itself. Nothing on this computer reads `~/NeboAI` through Nebo by asking
//! with nothing; the owner's clients do, and a document opened from his
//! files is opened under a pass for its own folder, so the pictures and
//! styles beside it load and nothing else does.
//!
//! Run:
//!   cargo test -p nebo-server --test files_auth

use reqwest::StatusCode;
use reqwest::redirect::Policy;

mod common;
use common::TestServer;

#[tokio::test]
async fn the_owners_files_answer_only_a_caller_that_proves_itself() {
    if !common::in_own_process().await {
        return;
    }
    let workspace = config::workspace_dir().unwrap();
    std::fs::create_dir_all(workspace.join("Reports/q3/img")).unwrap();
    std::fs::create_dir_all(workspace.join("Finance")).unwrap();
    std::fs::write(workspace.join("README.md"), b"hello").unwrap();
    std::fs::write(workspace.join("Finance/taxes.txt"), b"secret").unwrap();
    std::fs::write(
        workspace.join("Reports/q3/deck.html"),
        "<html><head><link rel=stylesheet href=\"img/s.css\"></head><body><img src=\"img/chart.png\"></body></html>",
    )
    .unwrap();
    std::fs::write(workspace.join("Reports/q3/img/chart.png"), b"png").unwrap();
    std::fs::write(workspace.join("Reports/q3/captions.vtt"), b"WEBVTT").unwrap();

    let server = TestServer::boot().await;
    let base = format!("http://127.0.0.1:{}", server.port);
    let http = reqwest::Client::builder().redirect(Policy::none()).build().unwrap();

    // Asking with nothing: refused, every file, GET and HEAD.
    for path in ["README.md", "Finance/taxes.txt", "Reports/q3/img/chart.png", "Reports/q3/deck.html"] {
        let r = http.get(format!("{base}/api/v1/files/{path}")).send().await.unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED, "GET {path}");
        let r = http.head(format!("{base}/api/v1/files/{path}")).send().await.unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED, "HEAD {path}");
    }

    // The owner's own client reads them; captions go out as captions.
    let keyed = |path: &str| format!("{base}/k/{}/api/v1/files/{path}", server.key);
    let r = http.get(keyed("README.md")).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(r.text().await.unwrap(), "hello");
    let r = http.get(keyed("Reports/q3/captions.vtt")).send().await.unwrap();
    assert_eq!(r.headers()["content-type"], "text/vtt; charset=utf-8");

    // A page from his files is opened again under its folder's pass, and runs
    // sandboxed.
    let r = http.get(keyed("Reports/q3/deck.html")).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    let location = r.headers()["location"].to_str().unwrap().to_string();
    assert!(location.starts_with(&format!("{}k/av", "../".repeat(7))) && location.ends_with("/api/v1/files/Reports/q3/deck.html"), "{location}");
    let pass = location.trim_start_matches("../").strip_prefix("k/").unwrap().split('/').next().unwrap().to_string();
    let under = |path: &str| format!("{base}/k/{pass}/api/v1/files/{path}");
    let page = http.get(under("Reports/q3/deck.html")).send().await.unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    let csp = page.headers()["content-security-policy"].to_str().unwrap().to_string();
    assert!(csp.starts_with("sandbox allow-scripts") && !csp.contains("allow-same-origin"), "{csp}");
    // What it loads from beside it: yes. Anything else: no.
    let r = http.get(under("Reports/q3/img/chart.png")).header("origin", "null").send().await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    for path in ["Finance/taxes.txt", "README.md", "Reports/q3/..%2F..%2FFinance%2Ftaxes.txt"] {
        let r = http.get(under(path)).send().await.unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED, "{path}");
    }
    let r = http.get(format!("{base}/k/{pass}/api/v1/agents")).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED, "a document's pass opens nothing of the owner's");
    let up = http.post(format!("{base}/k/{pass}/api/v1/files/upload")).send().await.unwrap();
    assert_eq!(up.status(), StatusCode::UNAUTHORIZED, "and writes nothing");

    // An image needs no pass from the owner's browser on Nebo's own page.
    let cookie = format!("{}={}", nebo_server::local_access::COOKIE, nebo_server::local_access::session_for(&server.key));
    let r = http
        .get(format!("{base}/api/v1/files/Reports/q3/img/chart.png"))
        .header("cookie", &cookie)
        .header("sec-fetch-site", "same-origin")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    napp::page_pass::revoke(&pass);
}
