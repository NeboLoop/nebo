//! An app from the marketplace reads the owner's files, talks to another
//! website or asks another of his employees only once he allows it; one his
//! own employees built here does as its manifest asks. Every check refuses
//! what is not held, whether or not the app has a sidecar running.
//!
//! Run:
//!   cargo test -p nebo-server --test app_consent

use reqwest::StatusCode;
use serde_json::json;

mod common;
use common::TestServer;

const BOUGHT: &str = "app-bought";
const MINE: &str = "app-mine";
const ASKS: [&str; 4] = ["storage:readwrite", "filesystem:read:Media", "network:example.com", "subagent:emp-1"];

fn install(home: &std::path::Path, id: &str, name: &str) {
    install_asking(home, id, name, &ASKS);
}

fn install_asking(home: &std::path::Path, id: &str, name: &str, asks: &[&str]) {
    let dir = home.join("user/agents").join(name);
    std::fs::create_dir_all(dir.join("ui")).unwrap();
    std::fs::write(dir.join("AGENT.md"), format!("---\nname: {name}\n---\n# {name}")).unwrap();
    let manifest = json!({ "id": id, "name": name, "version": "1.0.0", "type": "app", "permissions": asks });
    std::fs::write(dir.join("manifest.json"), manifest.to_string()).unwrap();
    std::fs::write(dir.join("ui/index.html"), "<html><head></head><body>x</body></html>").unwrap();
}

#[tokio::test]
async fn an_installed_app_does_only_what_the_owner_allowed() {
    if !common::in_own_process().await {
        return;
    }
    let home = config::data_dir().unwrap();
    install(&home, BOUGHT, "Bought");
    install(&home, MINE, "Mine");
    install_asking(&home, "app-local", "Local", &["network:127.0.0.1", "network:localhost"]);
    let workspace = config::workspace_dir().unwrap();
    std::fs::create_dir_all(workspace.join("Media")).unwrap();
    std::fs::write(workspace.join("Media/a.png"), b"png").unwrap();

    let server = TestServer::boot().await;
    let store = server.db_store();
    // The one from the marketplace: its package is under Nebo's agents.
    let installed = home.join("nebo/agents/bought.napp");
    store.set_agent_napp_path(BOUGHT, installed.to_str().unwrap()).unwrap();
    store.create_agent("emp-1", Some("agent"), "Bookkeeper", "", "", "{}", None, None).ok();

    let base = format!("http://127.0.0.1:{}", server.port);
    let http = reqwest::Client::new();
    let bought = napp::page_pass::grant(napp::page_pass::Scope::App(BOUGHT.into()), std::time::Duration::from_secs(60));
    let mine = napp::page_pass::grant(napp::page_pass::Scope::App(MINE.into()), std::time::Duration::from_secs(60));
    let file = |pass: &str, app: &str| format!("{base}/k/{pass}/api/v1/apps/{app}/files/Media/a.png");

    // Before the owner answers: nothing beyond its own page and store, with
    // no sidecar running at all (the checks used to allow everything then).
    assert_eq!(http.get(file(&bought, BOUGHT)).send().await.unwrap().status(), StatusCode::FORBIDDEN);
    let fetch = http
        .post(format!("{base}/k/{bought}/api/v1/apps/{BOUGHT}/http/proxy"))
        .json(&json!({ "url": "https://example.com/", "method": "GET" }))
        .send()
        .await
        .unwrap();
    assert_eq!(fetch.status(), StatusCode::FORBIDDEN, "network:");
    let invoke = http
        .post(format!("{base}/k/{bought}/api/v1/apps/{BOUGHT}/agents/invoke"))
        .json(&json!({ "message": "hi", "agent": "emp-1" }))
        .send()
        .await
        .unwrap();
    assert_eq!(invoke.status(), StatusCode::FORBIDDEN, "subagent:");
    let undeclared = http
        .post(format!("{base}/k/{mine}/api/v1/apps/{MINE}/http/proxy"))
        .json(&json!({ "url": "https://other.example/", "method": "GET" }))
        .send()
        .await
        .unwrap();
    assert_eq!(undeclared.status(), StatusCode::FORBIDDEN, "even his own app holds only what it asks for");
    // A domain it holds still never reaches this computer or its network,
    // whatever the grant names.
    let local = napp::page_pass::grant(napp::page_pass::Scope::App("app-local".into()), std::time::Duration::from_secs(60));
    for url in [format!("http://127.0.0.1:{}/health", server.port), format!("http://localhost:{}/health", server.port)] {
        let r = http
            .post(format!("{base}/k/{local}/api/v1/apps/app-local/http/proxy"))
            .json(&json!({ "url": url, "method": "GET" }))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::BAD_GATEWAY, "{url}");
        assert!(r.text().await.unwrap().contains("local or private"), "{url}");
    }
    napp::page_pass::revoke(&local);

    // His own app holds what it asks for.
    assert_eq!(http.get(file(&mine, MINE)).send().await.unwrap().status(), StatusCode::OK);

    // What the owner sees: the requests, in his words, waiting.
    let page = server.get(&format!("/agents/{BOUGHT}/app-permissions")).await;
    assert_eq!(page.status(), StatusCode::OK);
    let page: serde_json::Value = page.json().await.unwrap();
    assert_eq!(page["own"], false);
    let files = page["permissions"].as_array().unwrap().iter().find(|p| p["permission"] == "filesystem:read:Media").unwrap().clone();
    assert_eq!((files["kind"].as_str(), files["target"].as_str(), files["state"].as_str()), (Some("files"), Some("NeboAI › Media"), Some("asking")));
    let helper = page["permissions"].as_array().unwrap().iter().find(|p| p["permission"] == "subagent:emp-1").unwrap().clone();
    assert_eq!(helper["target"], "Bookkeeper");
    // The app's own pass never reaches the owner's answers.
    let r = http.get(format!("{base}/k/{bought}/api/v1/agents/{BOUGHT}/app-permissions")).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    let r = http
        .put(format!("{base}/k/{bought}/api/v1/agents/{BOUGHT}/app-permissions"))
        .json(&json!({ "permission": "filesystem:read:Media", "allowed": true }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);

    // He allows the files: the page reads them. He takes it back: refused.
    let r = server.put_json(&format!("/agents/{BOUGHT}/app-permissions"), &json!({ "permission": "filesystem:read:Media", "allowed": true })).await;
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(http.get(file(&bought, BOUGHT)).send().await.unwrap().status(), StatusCode::OK);
    let r = server.put_json(&format!("/agents/{BOUGHT}/app-permissions"), &json!({ "permission": "filesystem:read:Media", "allowed": false })).await;
    let after: serde_json::Value = r.json().await.unwrap();
    assert!(after["permissions"].as_array().unwrap().iter().any(|p| p["permission"] == "filesystem:read:Media" && p["state"] == "refused"));
    assert_eq!(http.get(file(&bought, BOUGHT)).send().await.unwrap().status(), StatusCode::FORBIDDEN);

    // Only what the app asks for, and never for his own app.
    for (app, perm) in [(BOUGHT, "filesystem:read:Finance"), (BOUGHT, "storage:readwrite"), (MINE, "filesystem:read:Media")] {
        let r = server.put_json(&format!("/agents/{app}/app-permissions"), &json!({ "permission": perm, "allowed": true })).await;
        assert_eq!(r.status(), StatusCode::BAD_REQUEST, "{app} {perm}");
    }

    // An update that asks for something new: asked again, not held.
    let dir = home.join("user/agents/Bought");
    let manifest = json!({ "id": BOUGHT, "name": "Bought", "version": "1.1.0", "type": "app",
        "permissions": ["storage:readwrite", "filesystem:read:Media", "filesystem:read:Docs"] });
    std::fs::write(dir.join("manifest.json"), manifest.to_string()).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        let page: serde_json::Value = server.get(&format!("/agents/{BOUGHT}/app-permissions")).await.json().await.unwrap();
        let docs = page["permissions"].as_array().unwrap().iter().find(|p| p["permission"] == "filesystem:read:Docs").cloned();
        if let Some(docs) = docs {
            assert_eq!(docs["state"], "asking");
            break;
        }
        assert!(std::time::Instant::now() < deadline, "the update's new request shows: {page}");
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    napp::page_pass::revoke(&bought);
    napp::page_pass::revoke(&mine);
}
