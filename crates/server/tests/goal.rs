//! `/goal` and the goal REST route on a real server: setting a goal starts a
//! turn at once (its kickoff), showing one does not. The test server has no
//! model, so a started turn is seen by the run it attempts ending in
//! `chat_error`.

mod common;

use std::time::Duration;

use common::TestServer;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::{connect_async, tungstenite::Message};

const KEY: &str = "agent:assistant:web";

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// The events for `KEY` until `stop` is seen or `within` passes.
async fn events_until(ws: &mut Socket, stop: &[&str], within: Duration) -> Vec<(String, Value)> {
    let mut seen = Vec::new();
    let deadline = tokio::time::Instant::now() + within;
    while let Ok(Some(Ok(frame))) = tokio::time::timeout_at(deadline, ws.next()).await {
        let Message::Text(text) = frame else { continue };
        let Ok(ev) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let kind = ev["type"].as_str().unwrap_or("").to_string();
        if ev["data"]["session_id"].as_str() != Some(KEY) {
            continue;
        }
        let done = stop.contains(&kind.as_str());
        seen.push((kind, ev["data"].clone()));
        if done {
            break;
        }
    }
    seen
}

async fn say(ws: &mut Socket, prompt: &str) {
    let msg = json!({"type": "chat", "data": {"session_id": KEY, "prompt": prompt}});
    ws.send(Message::Text(msg.to_string().into()))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn setting_a_goal_starts_a_turn_and_showing_it_does_not() {
    let server = TestServer::boot().await;
    let (mut ws, _) = connect_async(format!("ws://127.0.0.1:{}/ws", server.port))
        .await
        .expect("ws");

    // `/goal <end state>`: the goal is set, the owner told, and a turn starts.
    say(&mut ws, "/goal every inbox file has a summary line").await;
    let events = events_until(&mut ws, &["chat_error"], Duration::from_secs(20)).await;
    let kinds: Vec<&str> = events.iter().map(|(k, _)| k.as_str()).collect();
    let status = events
        .iter()
        .find(|(k, _)| k == "goal_status")
        .expect("goal_status");
    assert_eq!(status.1["status"], "active");
    assert_eq!(status.1["condition"], "every inbox file has a summary line");
    assert!(
        events.iter().any(|(k, d)| k == "chat_stream"
            && d["content"]
                .as_str()
                .is_some_and(|c| c.starts_with("Goal set:"))),
        "{kinds:?}"
    );
    assert!(
        kinds.contains(&"chat_error"),
        "the kickoff turn ran: {kinds:?}"
    );
    // The failed kickoff turn finishes.
    events_until(&mut ws, &["chat_complete"], Duration::from_secs(5)).await;

    // `/goal` alone shows it and starts nothing.
    say(&mut ws, "/goal").await;
    let events = events_until(
        &mut ws,
        &["chat_complete", "chat_error"],
        Duration::from_secs(10),
    )
    .await;
    let kinds: Vec<&str> = events.iter().map(|(k, _)| k.as_str()).collect();
    assert!(
        kinds.contains(&"chat_complete") && !kinds.contains(&"chat_error"),
        "{kinds:?}"
    );
    assert!(events.iter().any(|(k, d)| {
        k == "chat_stream"
            && d["content"]
                .as_str()
                .is_some_and(|c| c.starts_with("Goal: every inbox file"))
    }));

    // A clear word clears it; the thread's line follows.
    say(&mut ws, "/goal stop").await;
    let events = events_until(&mut ws, &["chat_complete"], Duration::from_secs(10)).await;
    assert!(
        events
            .iter()
            .any(|(k, d)| k == "goal_status" && d["status"] == "cleared")
    );

    // The REST route is the same action: the goal is set and a turn starts.
    let resp = server
        .put_json(
            &format!("/agent/sessions/{KEY}/goal"),
            &json!({"condition": "the report is sent"}),
        )
        .await;
    assert!(resp.status().is_success(), "{}", resp.status());
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["goal"]["status"], "active");
    let events = events_until(&mut ws, &["chat_error"], Duration::from_secs(20)).await;
    assert!(
        events.iter().any(|(k, _)| k == "chat_error"),
        "the kickoff turn ran"
    );

    let got: Value = server
        .get(&format!("/agent/sessions/{KEY}/goal"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(got["goal"]["condition"], "the report is sent");
    let cleared: Value = server
        .delete(&format!("/agent/sessions/{KEY}/goal"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(cleared["goal"]["status"], "cleared");
}
