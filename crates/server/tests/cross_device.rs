//! One conversation open on two devices. The desktop shows the thread while
//! the phone carries it on (or the other way round): every client looking at
//! the conversation gets the owner's message, the turn's stream and its end
//! live, whichever client started it. A client that was away catches up from
//! the thread's history, once, when it comes back.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::model::{Reply, main_call, server_with};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

const REPLY: &str = "All eight clips are in the cut.";
const OWNER: &str = "Use all 8 clips at their full length.";

async fn connect(url: &str, client_id: &str) -> Socket {
    let (mut ws, _) = connect_async(url).await.expect("ws");
    let hello = json!({"type": "connect", "data": {"client_id": client_id}});
    ws.send(Message::Text(hello.to_string().into())).await.unwrap();
    ws
}

/// The events about conversation `key` this socket hears, in order, until
/// the turn's end (or the deadline).
async fn until_turn_ends(ws: &mut Socket, key: &str) -> Vec<(String, Value)> {
    let mut heard = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    while let Ok(Some(Ok(frame))) = tokio::time::timeout_at(deadline, ws.next()).await {
        let Message::Text(text) = frame else { continue };
        let Ok(ev) = serde_json::from_str::<Value>(&text) else { continue };
        if ev["data"]["session_id"].as_str() != Some(key) {
            continue;
        }
        let kind = ev["type"].as_str().unwrap_or("").to_string();
        let done = (kind == "chat_complete" && ev["data"]["stop_reason"].is_null()) || kind == "chat_error";
        heard.push((kind, ev["data"].clone()));
        if done {
            break;
        }
    }
    heard
}

async fn new_chat(server: &common::TestServer) -> String {
    let chat: Value = server.post_json("/chats", &json!({"agentId": "assistant"})).await.json().await.unwrap();
    chat["id"].as_str().expect("chat id").to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_the_phone_starts_reaches_the_desktop_watching_the_thread() {
    let (server, _calls) = server_with(Arc::new(|purpose: &str, _: &[Value]| {
        Reply::Text(if main_call(purpose) { REPLY } else { "ok" })
    }))
    .await;
    let chat = new_chat(&server).await;
    let key = format!("agent:assistant:thread:{chat}");

    // The desktop has the thread open; the phone sends into it.
    let mut desktop = connect(&server.ws_url(), "desktop-window").await;
    let mut phone = connect(&server.ws_url(), "phone").await;
    let send = json!({"type": "chat", "message_id": "m-phone-1",
                      "data": {"agent_id": "assistant", "session_id": &key, "prompt": OWNER}});
    phone.send(Message::Text(send.to_string().into())).await.unwrap();

    let heard = until_turn_ends(&mut desktop, &key).await;
    let kinds: Vec<&str> = heard.iter().map(|(k, _)| k.as_str()).collect();

    // The owner's message, named as the phone's, before the work on it.
    let said = heard.iter().find(|(k, _)| k == "chat_user_message").expect("the owner's message reached the desktop");
    assert_eq!(said.1["content"], OWNER);
    assert_eq!(said.1["client_id"], "phone", "named as the phone's, so the desktop shows it as sent from elsewhere");
    let at = |k: &str| kinds.iter().position(|x| *x == k).unwrap_or_else(|| panic!("no {k}: {kinds:?}"));
    assert!(at("chat_user_message") < at("chat_created"), "{kinds:?}");
    assert!(at("chat_created") < at("chat_stream"), "{kinds:?}");

    // The reply streamed to the desktop as it was written, and the turn ended there.
    let streamed: String = heard
        .iter()
        .filter(|(k, _)| k == "chat_stream")
        .filter_map(|(_, d)| d["content"].as_str())
        .collect();
    assert_eq!(streamed, REPLY, "{kinds:?}");
    assert_eq!(kinds.last(), Some(&"chat_complete"), "{kinds:?}");
    assert!(!kinds.contains(&"chat_error"), "{kinds:?}");

    // The phone heard the same turn.
    let phone_heard = until_turn_ends(&mut phone, &key).await;
    assert!(phone_heard.iter().any(|(k, _)| k == "chat_complete"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_client_that_was_away_catches_up_from_the_thread_once_it_is_back() {
    let (server, _calls) = server_with(Arc::new(|purpose: &str, _: &[Value]| {
        Reply::Text(if main_call(purpose) { REPLY } else { "ok" })
    }))
    .await;
    let chat = new_chat(&server).await;
    let key = format!("agent:assistant:thread:{chat}");

    // The desktop's socket is down (asleep, a dropped line) while the phone
    // carries the conversation on.
    let mut phone = connect(&server.ws_url(), "phone").await;
    let send = json!({"type": "chat", "message_id": "m-phone-2",
                      "data": {"agent_id": "assistant", "session_id": &key, "prompt": OWNER}});
    phone.send(Message::Text(send.to_string().into())).await.unwrap();
    let heard = until_turn_ends(&mut phone, &key).await;
    assert_eq!(heard.last().map(|(k, _)| k.as_str()), Some("chat_complete"), "{heard:?}");

    // Back: it dials again and reads the thread, the way the app's resync
    // does, and has everything the live events carried.
    let _desktop = connect(&server.ws_url(), "desktop-window").await;
    let history: Value = server.get(&format!("/chats/{chat}/messages")).await.json().await.unwrap();
    let rows = history["messages"].as_array().cloned().unwrap_or_default();
    let said = |role: &str, text: &str| {
        rows.iter().any(|m| m["role"] == role && m["content"].as_str().is_some_and(|c| c.contains(text)))
    };
    assert!(said("user", OWNER), "the owner's message is in the thread: {rows:?}");
    assert!(said("assistant", REPLY), "the reply is in the thread: {rows:?}");
    assert!(history["activeRun"].is_null(), "nothing is still running: {history}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_quiet_socket_hears_the_heartbeat_its_clients_watch_for() {
    let (server, _calls) = server_with(Arc::new(|_: &str, _: &[Value]| Reply::Text("ok"))).await;
    let mut ws = connect(&server.ws_url(), "desktop-window").await;
    // Nothing happens on the bot; within a beat (20 s) the socket still hears
    // from it, so a client can tell a quiet line from a dead one.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut pinged = false;
    while let Ok(Some(Ok(frame))) = tokio::time::timeout_at(deadline, ws.next()).await {
        let Message::Text(text) = frame else { continue };
        if serde_json::from_str::<Value>(&text).is_ok_and(|ev| ev["type"] == "ping") {
            pinged = true;
            break;
        }
    }
    assert!(pinged, "no heartbeat within 30 s");
    // The answer is taken quietly and the socket stays up.
    ws.send(Message::Text(json!({"type": "pong"}).to_string().into())).await.unwrap();
    ws.send(Message::Text(json!({"type": "ping"}).to_string().into())).await.unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut ponged = false;
    while let Ok(Some(Ok(frame))) = tokio::time::timeout_at(deadline, ws.next()).await {
        let Message::Text(text) = frame else { continue };
        if serde_json::from_str::<Value>(&text).is_ok_and(|ev| ev["type"] == "pong") {
            ponged = true;
            break;
        }
    }
    assert!(ponged, "the socket is still answering");
}
