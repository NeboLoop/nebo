//! An app's page reads the owner's files through its own route
//! (`/api/v1/apps/<id>/files/<path>`, `nebo.files.url`), only inside the
//! workspace folders its manifest grants (`filesystem:read:<folder>`).
//!
//! Driven through the real server with an app-view pass, the credential a
//! page carries when Nebo opens it at `/k/<pass>/apps/<id>/ui/` (the mobile
//! app and the tunnel reach the same route by the same rule).
//!
//! Run:
//!   cargo test -p nebo-server --test app_files

use reqwest::StatusCode;

mod common;
use common::TestServer;

const APP: &str = "app-reel";
const OTHER: &str = "app-other";

/// An app on disk, as the loader reads it: its manifest and its page.
fn install(data_dir: &std::path::Path, id: &str, name: &str, permissions: &[&str]) {
    let dir = data_dir.join("user/agents").join(name);
    std::fs::create_dir_all(dir.join("ui")).unwrap();
    std::fs::write(
        dir.join("AGENT.md"),
        format!("---\nname: {name}\n---\n# {name}"),
    )
    .unwrap();
    let manifest = serde_json::json!({
        "id": id, "name": name, "version": "1.0.0", "type": "app", "permissions": permissions,
    });
    std::fs::write(dir.join("manifest.json"), manifest.to_string()).unwrap();
    std::fs::write(
        dir.join("ui/index.html"),
        "<html><head></head><body>reel</body></html>",
    )
    .unwrap();
}

#[tokio::test]
async fn an_app_page_reads_only_its_granted_folder() {
    if !common::in_own_process().await {
        return;
    }
    let home = config::data_dir().unwrap();
    // A relocated root keeps its workspace in its own files folder.
    let workspace = config::workspace_dir().unwrap();
    let film: Vec<u8> = (0..4096u32).map(|i| (i % 251) as u8).collect();
    let project = workspace.join("Media/Projects/reel");
    std::fs::create_dir_all(project.join(".proxies")).unwrap();
    std::fs::write(project.join("render.mp4"), &film).unwrap();
    std::fs::write(project.join(".proxies/render-proxy.mp4"), b"proxy").unwrap();
    std::fs::write(workspace.join("Media/notes.txt"), b"not granted").unwrap();
    install(
        &home,
        APP,
        "Reel",
        &["storage:readwrite", "filesystem:read:Media/Projects"],
    );
    install(&home, OTHER, "Other", &["storage:readwrite"]);

    let server = TestServer::boot().await;
    let client = reqwest::Client::new();
    let pass = napp::app_view::grant(APP, std::time::Duration::from_secs(60));
    let url = |pass: &str, app: &str, path: &str| {
        format!(
            "http://127.0.0.1:{}/k/{pass}/api/v1/apps/{app}/files/{path}",
            server.port
        )
    };
    let film_path = "Media/Projects/reel/render.mp4";

    // The whole film, and a byte range of it, as a media element asks.
    let whole = client.get(url(&pass, APP, film_path)).send().await.unwrap();
    assert_eq!(whole.status(), StatusCode::OK);
    assert_eq!(whole.headers()["content-type"], "video/mp4");
    assert_eq!(whole.headers()["accept-ranges"], "bytes");
    assert!(
        whole.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .starts_with("sandbox"),
        "an owner's file never runs as Nebo's page"
    );
    assert_eq!(whole.bytes().await.unwrap().as_ref(), film.as_slice());
    let part = client
        .get(url(&pass, APP, film_path))
        .header("range", "bytes=100-199")
        .send()
        .await
        .unwrap();
    assert_eq!(part.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(part.headers()["content-range"], "bytes 100-199/4096");
    assert_eq!(part.bytes().await.unwrap().as_ref(), &film[100..200]);
    let head = client
        .head(url(&pass, APP, film_path))
        .send()
        .await
        .unwrap();
    assert_eq!(head.status(), StatusCode::OK);
    assert_eq!(head.headers()["content-length"], "4096");

    // Outside the grant: a folder it doesn't name, a climb out of it.
    for path in [
        "Media/notes.txt",
        "Media/Projects/..%2Fnotes.txt",
        "Media/Projects/reel/..%2F..%2F..%2Fnotes.txt",
    ] {
        let r = client.get(url(&pass, APP, path)).send().await.unwrap();
        assert_eq!(r.status(), StatusCode::FORBIDDEN, "{path}");
    }
    // Inside it, but hidden or missing: the same 404.
    for path in [
        "Media/Projects/reel/.proxies/render-proxy.mp4",
        "Media/Projects/reel/gone.mp4",
    ] {
        let r = client.get(url(&pass, APP, path)).send().await.unwrap();
        assert_eq!(r.status(), StatusCode::NOT_FOUND, "{path}");
    }
    // Read only.
    let put = client
        .put(url(&pass, APP, film_path))
        .body("x")
        .send()
        .await
        .unwrap();
    assert_eq!(put.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(std::fs::read(project.join("render.mp4")).unwrap(), film);

    // Another app's pass reaches nothing of this app's; an app with no grant
    // reads nothing, even with its own pass.
    let other = napp::app_view::grant(OTHER, std::time::Duration::from_secs(60));
    let r = client
        .get(url(&other, APP, film_path))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    let r = client
        .get(url(&other, OTHER, film_path))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);

    // A revoked pass reaches nothing.
    napp::app_view::revoke(&pass);
    let r = client.get(url(&pass, APP, film_path)).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    napp::app_view::revoke(&other);
}
