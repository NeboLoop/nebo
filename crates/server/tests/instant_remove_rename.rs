//! Removing and renaming an employee answer as soon as the roster reads the
//! change: the remove drops only that employee from the loaded roster (no
//! rescan of every agent), and touches only that employee's own folders —
//! the owner's own copy of a marketplace employee, kept under the same slug
//! in `user/agents`, stays where it is. A rename reads under its new name
//! at once.

mod common;

use common::TestServer;
use common::model::eventually;
use serde_json::{Value, json};

const PACKAGE_ID: &str = "6cd4d527-3f29-40f9-8fab-a4b8ccff8add";

async fn roster(server: &TestServer) -> Vec<Value> {
    let body: Value = server.get("/agents").await.json().await.unwrap();
    body["agents"].as_array().cloned().unwrap_or_default()
}

fn named<'a>(roster: &'a [Value], name: &str) -> Option<&'a Value> {
    roster.iter().find(|a| a["name"] == name)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn removing_an_employee_drops_only_its_own_folders_and_answers_at_once() {
    if !common::in_own_process().await {
        return;
    }
    let server = TestServer::boot().await;

    // A marketplace package, and the owner's own employee built from it,
    // kept under the same slug in the user folder with a name of its own.
    let package = server.data_dir.join("nebo").join("agents").join("design-studio").join("0.3.6");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(package.join("AGENT.md"), "---\nname: Design Studio\n---\n# Design Studio").unwrap();
    std::fs::write(
        package.join("manifest.json"),
        json!({"id": PACKAGE_ID, "name": "Design Studio", "slug": "design-studio", "version": "0.3.6", "type": "agent"}).to_string(),
    )
    .unwrap();
    let mine = server.data_dir.join("user").join("agents").join("design-studio");
    std::fs::create_dir_all(&mine).unwrap();
    std::fs::write(mine.join("AGENT.md"), "---\nname: My Design Studio\n---\n# My Design Studio").unwrap();

    let id = eventually(30, "the package is on the roster", async || {
        let r = roster(&server).await;
        let both = named(&r, "Design Studio").is_some() && named(&r, "My Design Studio").is_some();
        both.then(|| named(&r, "Design Studio").unwrap()["id"].as_str().unwrap().to_string())
    })
    .await;
    assert_eq!(id, PACKAGE_ID);

    let resp = server.delete(&format!("/agents/{id}")).await;
    assert_eq!(resp.status(), 200);

    // Read straight after the answer: the removed employee is off the
    // roster, the owner's own one is on it, and its folder is untouched.
    let r = roster(&server).await;
    assert!(named(&r, "Design Studio").is_none(), "the removed employee is off the roster at once");
    assert!(named(&r, "My Design Studio").is_some(), "the owner's own employee stays");
    assert!(mine.join("AGENT.md").exists(), "the owner's own folder under the same slug is never removed");
    assert!(!package.exists(), "the package's own folder went to the trash");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn renaming_an_employee_reads_under_its_new_name_at_once() {
    if !common::in_own_process().await {
        return;
    }
    let server = TestServer::boot().await;
    let resp = server
        .post_json("/agents", &json!({"blank": true, "name": "Rhea"}))
        .await;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    let id = body["agent"]["id"].as_str().unwrap().to_string();

    let resp = server.put_json(&format!("/agents/{id}"), &json!({"name": "Rhea Lane"})).await;
    assert_eq!(resp.status(), 200);

    let r = roster(&server).await;
    let row = r.iter().find(|a| a["id"] == id).expect("the employee is on the roster");
    assert_eq!(row["name"], "Rhea Lane", "the roster reads the new name straight after the answer");
}
