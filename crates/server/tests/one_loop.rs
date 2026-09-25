//! A chat message runs a turn through the harness, end to end, on a real
//! server with no identity: a scratch home, no hub, and a model on loopback
//! that answers every call. The turn is admitted, the model is called with
//! the harness's system prompt and the owner's words, the reply streams and
//! is stored, the session's facts are typed attachment rows, and the turn
//! ends with `chat_complete`.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::TestServer;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::{connect_async, tungstenite::Message};

const KEY: &str = "agent:assistant:web";
const REPLY: &str = "The invoices are ready.";
const OWNER: &str = "Get the invoices ready.";

/// What the model was asked: each call's purpose header and body.
type Calls = Arc<Mutex<Vec<(String, Value)>>>;

/// An OpenAI-compatible model on loopback. The turn's own call gets
/// `REPLY`; every side call (title, recap, memory) gets "ok".
async fn fake_model() -> (u16, Calls) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let calls: Calls = Arc::default();
    let seen = calls.clone();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let seen = seen.clone();
            tokio::spawn(async move {
                let Some((purpose, body)) = read_request(&mut sock).await else {
                    return;
                };
                let text = if purpose == "agent_turn" { REPLY } else { "ok" };
                seen.lock().unwrap().push((purpose, body));
                let chunk = |delta: Value, finish: Value| {
                    format!(
                        "data: {}\n\n",
                        json!({"id": "x", "object": "chat.completion.chunk", "created": 0, "model": "m",
                               "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]})
                    )
                };
                let sse = format!(
                    "{}{}data: [DONE]\n\n",
                    chunk(json!({"role": "assistant", "content": text}), Value::Null),
                    chunk(json!({}), json!("stop"))
                );
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n{sse}"
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.flush().await;
            });
        }
    });
    (port, calls)
}

/// One HTTP request: its `x-purpose` header and its JSON body.
async fn read_request(sock: &mut tokio::net::TcpStream) -> Option<(String, Value)> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let head_end = loop {
        let n = sock.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_lowercase();
    let header = |name: &str| {
        head.lines()
            .find_map(|l| l.strip_prefix(&format!("{name}: ")).map(|v| v.trim().to_string()))
    };
    let length: usize = header("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    while buf.len() < head_end + length {
        let n = sock.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let body = serde_json::from_slice(&buf[head_end..]).unwrap_or(Value::Null);
    Some((header("x-purpose").unwrap_or_default(), body))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_chat_message_runs_a_turn_through_the_one_loop() {
    // No hub is reachable: nothing here can act as anyone's bot.
    for var in ["NEBOAI_API_URL", "NEBOAI_JANUS_URL", "NEBOAI_COMMS_URL", "NEBOAI_TUNNEL_URL"] {
        // SAFETY: set before the server starts, like NEBO_HOME in `boot`.
        unsafe { std::env::set_var(var, "http://127.0.0.1:9") };
    }
    let (model_port, calls) = fake_model().await;
    let server = TestServer::boot().await;
    let created = server
        .post_json(
            "/providers",
            &json!({"name": "loopback", "provider": "deepseek", "apiKey": "test",
                    "model": "loopback-model", "baseUrl": format!("http://127.0.0.1:{model_port}")}),
        )
        .await;
    assert!(created.status().is_success(), "{}", created.status());

    let (mut ws, _) = connect_async(format!("ws://127.0.0.1:{}/ws", server.port)).await.expect("ws");
    let msg = json!({"type": "chat", "data": {"session_id": KEY, "prompt": OWNER}});
    ws.send(Message::Text(msg.to_string().into())).await.unwrap();

    let mut kinds = Vec::new();
    let mut streamed = String::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    while let Ok(Some(Ok(frame))) = tokio::time::timeout_at(deadline, ws.next()).await {
        let Message::Text(text) = frame else { continue };
        let Ok(ev) = serde_json::from_str::<Value>(&text) else { continue };
        if ev["data"]["session_id"].as_str() != Some(KEY) {
            continue;
        }
        let kind = ev["type"].as_str().unwrap_or("").to_string();
        if kind == "chat_stream" {
            streamed.push_str(ev["data"]["content"].as_str().unwrap_or(""));
        }
        let done = kind == "chat_complete" || kind == "chat_error";
        kinds.push(kind);
        if done {
            break;
        }
    }
    assert_eq!(kinds.last().map(String::as_str), Some("chat_complete"), "{kinds:?}");
    assert!(!kinds.iter().any(|k| k == "chat_error"), "{kinds:?}");
    assert_eq!(streamed, REPLY, "the reply streamed to the owner");

    // The model was called once for the turn, with the harness's prompt and
    // the owner's words.
    let turn_calls: Vec<Value> =
        calls.lock().unwrap().iter().filter(|(p, _)| p == "agent_turn").map(|(_, b)| b.clone()).collect();
    assert_eq!(turn_calls.len(), 1, "one step: the model answered without tools");
    let body = &turn_calls[0];
    let messages = body["messages"].as_array().expect("messages");
    let system = messages
        .iter()
        .find(|m| m["role"] == "system")
        .and_then(|m| m["content"].as_str())
        .expect("a system prompt");
    assert!(system.contains("# How this works") && system.contains("# Helpers"), "the harness's prompt: {system}");
    let said: Vec<&str> = messages.iter().filter(|m| m["role"] == "user").filter_map(|m| m["content"].as_str()).collect();
    assert!(said.iter().any(|m| m.contains(OWNER)), "the owner's words: {said:?}");
    assert!(said.iter().any(|m| m.contains("<system-reminder>")), "the session's facts ride as attachment rows: {said:?}");
    assert!(!system.contains(OWNER), "never in the system prompt");
    // The core tools are declared on the first step; the rest are listed.
    let declared: Vec<&str> = body["tools"]
        .as_array()
        .map(|t| t.iter().filter_map(|d| d["function"]["name"].as_str()).collect())
        .unwrap_or_default();
    for core in ["run_command", "read_file", "write_file", "edit_file", "delegate", "find_tools"] {
        assert!(declared.contains(&core), "{core} is not declared: {declared:?}");
    }

    // The thread holds the owner's row, the typed attachment rows and the reply.
    let store = server.db_store();
    let session = store.get_session_by_name(KEY).unwrap().expect("the session");
    let chat = session.active_chat_id.expect("its conversation");
    let rows = store.get_chat_messages(&chat).unwrap();
    assert!(rows.iter().any(|m| m.role == "user" && m.content == OWNER));
    assert!(rows.iter().any(|m| m.role == "assistant" && m.content == REPLY));
    assert!(
        rows.iter()
            .any(|m| m.metadata.as_deref().is_some_and(|meta| meta.contains("\"attachment\""))),
        "the loop wrote its attachments as typed rows"
    );
}
