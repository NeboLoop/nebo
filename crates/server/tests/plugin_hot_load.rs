//! A plugin copied into the user plugins folder by hand is usable at once:
//! the filesystem watcher activates it through the same path a marketplace
//! install takes, so its `plugin__<slug>` tool is registered and callable
//! with nothing else refreshing it. A copy seen halfway never counts.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::TestServer;
use common::model::{Reply, eventually, hire_blank, main_call, server_with, set_mode};
use futures_util::SinkExt;
use serde_json::{Value, json};
use tokio_tungstenite::{connect_async, tungstenite::Message};

const SLUG: &str = "hand-media";
const TOOL: &str = "plugin__hand-media";
const OUTPUT: &str = "HAND-MEDIA-OK";

/// The last message the model was sent that isn't a reminder.
fn last(messages: &[Value]) -> &Value {
    messages.iter().rev().find(|m| !says(m, "<system-reminder>")).unwrap_or(&Value::Null)
}

fn says(m: &Value, text: &str) -> bool {
    m["content"].as_str().is_some_and(|c| c.contains(text))
}

/// Copy the plugin in the way a person does, a piece at a time: the
/// manifest, then part of the binary, then the rest.
async fn copy_in_by_hand(server: &TestServer) {
    let dir = server.data_dir.join("user").join("plugins").join(SLUG).join("0.1.0");
    std::fs::create_dir_all(&dir).unwrap();
    let script = format!("#!/bin/sh\necho {OUTPUT} \"$@\"\n");
    let mut platforms = serde_json::Map::new();
    platforms.insert(
        napp::plugin::current_platform_key(),
        json!({"binaryName": SLUG, "sha256": "", "signature": "", "size": script.len(), "downloadUrl": ""}),
    );
    let manifest = json!({"id": SLUG, "slug": SLUG, "name": "Hand Media", "version": "0.1.0", "platforms": platforms});
    std::fs::write(dir.join("plugin.json"), manifest.to_string()).unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;
    let binary = dir.join(SLUG);
    std::fs::write(&binary, &script.as_bytes()[..8]).unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;
    std::fs::write(&binary, script.as_bytes()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_plugin_copied_in_by_hand_is_callable_right_after_hot_load_and_its_toggle_is_honest() {
    let (server, calls) = server_with(Arc::new(|purpose, messages| {
        if !main_call(purpose) {
            return Reply::Text("ok");
        }
        let m = last(messages);
        if m["role"] == "user" && says(m, "MARK-MEDIA") {
            return Reply::Call(vec![("find_tools", json!({"query": format!("select:{TOOL}")}))]);
        }
        if m["role"] == "tool" && says(m, OUTPUT) {
            return Reply::Text("Done.");
        }
        if m["role"] == "tool" {
            return Reply::Call(vec![(TOOL, json!({"command": "hello"}))]);
        }
        Reply::Text("Noted.")
    }))
    .await;

    copy_in_by_hand(&server).await;

    // Hot-loaded: the tool is registered with nothing else refreshing it.
    eventually(30, "the plugin's tool to register", async || {
        let tools: Value = server.get("/integrations/tools").await.json().await.ok()?;
        tools["tools"].as_array()?.iter().any(|t| t["name"] == TOOL).then_some(())
    })
    .await;

    // And callable: an employee loads it and runs it.
    let employee = hire_blank(&server, "Editor").await;
    set_mode(&server, &employee, "full_access").await;
    // Its introduction turn ends first: a message typed into a running turn
    // is answered with its tools off.
    let key = format!("agent:{employee}:web");
    eventually(60, "the introduction to end", async || {
        let active: Value = server.get("/runs/active").await.json().await.ok()?;
        (!active["runs"].as_array()?.iter().any(|r| r["sessionKey"] == key.as_str())).then_some(())
    })
    .await;
    let (mut ws, _) = connect_async(server.ws_url()).await.expect("ws");
    let msg = json!({"type": "chat", "data": {"session_id": key, "prompt": "MARK-MEDIA run the media plugin"}});
    ws.send(Message::Text(msg.to_string().into())).await.unwrap();

    eventually(60, "the plugin's output to reach the model", async || {
        calls
            .lock()
            .unwrap()
            .iter()
            .any(|(p, b)| main_call(p) && b["messages"].as_array().is_some_and(|ms| ms.iter().any(|m| m["role"] == "tool" && says(m, OUTPUT))))
            .then_some(())
    })
    .await;
    let loaded = calls.lock().unwrap().iter().any(|(p, b)| {
        main_call(p)
            && b["messages"]
                .as_array()
                .is_some_and(|ms| ms.iter().any(|m| m["role"] == "tool" && says(m, "No deferred tool matches")))
    });
    assert!(!loaded, "find_tools could not see the hand-copied plugin's tool");

    // The toggle never reports a state it did not save: a plugin with no
    // install record (copied in by hand) gets a clear refusal, and an
    // unknown plugin a 404. (One server per binary: boots race on NEBO_HOME.)
    let r = server.post_json(&format!("/plugins/{SLUG}/toggle"), &json!({})).await;
    assert_eq!(r.status(), 400);
    let body: Value = r.json().await.unwrap();
    assert!(body.to_string().contains("by hand"), "{body}");
    assert!(body.get("enabled").is_none(), "no state reported: {body}");
    let r = server.post_json("/plugins/no-such-plugin/toggle", &json!({})).await;
    assert_eq!(r.status(), 404);
}
