//! A new employee is there the moment the owner creates it. Create never
//! waits on the model that drafts its job: tapped before the job line came
//! back, the employee is created at once, holding no job, and the job is
//! drafted in the background as an edit its page offers to add.
//!
//! Run:
//!   cargo test -p nebo-server --test instant_hire

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

mod common;
use common::model::{Reply, Script, eventually, server_with};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn create_answers_before_the_job_is_drafted() {
    if !common::in_own_process().await {
        return;
    }
    // The model holds the job read until the test lets it answer.
    let (release, held) = std::sync::mpsc::channel::<()>();
    let held = Arc::new(Mutex::new(held));
    let script: Script = Arc::new(move |purpose, _| {
        if purpose == "job_needs" {
            let _ = held.lock().unwrap().recv_timeout(Duration::from_secs(30));
            return Reply::Text(r#"{"capabilities": ["web"]}"#);
        }
        Reply::Text("Hello.")
    });
    let (server, _calls) = server_with(script).await;

    let started = Instant::now();
    let created = server
        .post_json(
            "/agents",
            &json!({"blank": true, "name": "Ada", "description": "Looks up competitor prices on the web every morning"}),
        )
        .await;
    let took = started.elapsed();
    assert_eq!(created.status(), 200);
    let created: Value = created.json().await.unwrap();
    let id = created["agent"]["id"].as_str().expect("agent id").to_string();
    assert!(took < Duration::from_secs(5), "create waited on the job read: {took:?}");

    // On the team at once, with its description and no job drafted yet.
    let agent: Value = server.get(&format!("/agents/{id}")).await.json().await.unwrap();
    assert_eq!(agent["agent"]["name"], "Ada");
    assert_eq!(agent["agent"]["description"], "Looks up competitor prices on the web every morning");
    assert!(agent["jobDraft"].is_null(), "nothing drafted while the model is held: {agent}");

    release.send(()).unwrap();
    let draft = eventually(15, "the job drafted for the new employee", async || {
        let agent: Value = server.get(&format!("/agents/{id}")).await.json().await.ok()?;
        agent["jobDraft"].as_object().cloned()
    })
    .await;
    assert!(draft["line"].as_str().is_some_and(|l| !l.is_empty()), "{draft:?}");

    // Adding it is the owner's consent, through the edit's one Save.
    let draft_id = draft["draftId"].as_str().unwrap();
    let saved = server.put_json(&format!("/agents/{id}"), &json!({"draftId": draft_id})).await;
    assert_eq!(saved.status(), 200);
    let agent: Value = server.get(&format!("/agents/{id}")).await.json().await.unwrap();
    assert!(agent["jobDraft"].is_null(), "an added job is no longer offered: {agent}");
}
