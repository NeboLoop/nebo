//! A model on loopback that answers from a script, and a server whose only
//! provider it is: work that runs end to end with no network and no real
//! model. Shared by the suites that drive real turns and workflow runs.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::TestServer;

/// What the scripted model answers a call with.
pub enum Reply {
    Text(&'static str),
    Call(&'static str, Value),
}

/// Given a call's purpose and its messages, the reply.
pub type Script = Arc<dyn Fn(&str, &[Value]) -> Reply + Send + Sync>;

/// Every call the model got: its purpose and its body.
pub type Calls = Arc<Mutex<Vec<(String, Value)>>>;

/// Whether this is a turn's own call (not a title, recap or memory call).
pub fn main_call(purpose: &str) -> bool {
    purpose == "agent_turn" || purpose.starts_with("workflow")
}

/// Whether the conversation already holds a tool result.
pub fn has_result(messages: &[Value]) -> bool {
    messages.iter().any(|m| m["role"] == "tool")
}

/// Whether any message says `text`.
pub fn says(messages: &[Value], text: &str) -> bool {
    messages.iter().any(|m| m["content"].as_str().is_some_and(|c| c.contains(text)))
}

/// An OpenAI-compatible model on loopback, answering from `script`.
pub async fn fake_model(script: Script) -> (u16, Calls) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let calls: Calls = Arc::default();
    let seen = calls.clone();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let (seen, script) = (seen.clone(), script.clone());
            tokio::spawn(async move {
                let Some((purpose, body)) = read_request(&mut sock).await else {
                    return;
                };
                let messages = body["messages"].as_array().cloned().unwrap_or_default();
                let reply = script(&purpose, &messages);
                seen.lock().unwrap().push((purpose, body));
                let chunk = |delta: Value, finish: Value| {
                    format!(
                        "data: {}\n\n",
                        json!({"id": "x", "object": "chat.completion.chunk", "created": 0, "model": "m",
                               "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]})
                    )
                };
                let (delta, finish) = match reply {
                    Reply::Text(text) => (json!({"role": "assistant", "content": text}), "stop"),
                    Reply::Call(name, args) => (
                        json!({"role": "assistant", "tool_calls": [{"index": 0, "id": format!("call_{}", uuid::Uuid::new_v4().simple()), "type": "function",
                               "function": {"name": name, "arguments": args.to_string()}}]}),
                        "tool_calls",
                    ),
                };
                let sse = format!("{}{}data: [DONE]\n\n", chunk(delta, Value::Null), chunk(json!({}), json!(finish)));
                let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n{sse}");
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.flush().await;
            });
        }
    });
    (port, calls)
}

/// One HTTP request: its `x-purpose` header and its JSON body.
pub async fn read_request(sock: &mut tokio::net::TcpStream) -> Option<(String, Value)> {
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

/// A server on a scratch home with the scripted model as its only
/// provider. No hub is reachable: nothing here can act as anyone's bot.
pub async fn server_with(script: Script) -> (TestServer, Calls) {
    for var in ["NEBOAI_API_URL", "NEBOAI_JANUS_URL", "NEBOAI_COMMS_URL", "NEBOAI_TUNNEL_URL"] {
        // SAFETY: set before the server starts, like NEBO_HOME in `boot`.
        unsafe { std::env::set_var(var, "http://127.0.0.1:9") };
    }
    let (model_port, calls) = fake_model(script).await;
    let server = TestServer::boot().await;
    let created = server
        .post_json(
            "/providers",
            &json!({"name": "loopback", "provider": "deepseek", "apiKey": "test",
                    "model": "loopback-model", "baseUrl": format!("http://127.0.0.1:{model_port}")}),
        )
        .await;
    assert!(created.status().is_success(), "{}", created.status());
    (server, calls)
}

/// Poll `probe` every 100 ms for up to `secs` seconds.
pub async fn eventually<T>(secs: u64, what: &str, mut probe: impl AsyncFnMut() -> Option<T>) -> T {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        if let Some(v) = probe().await {
            return v;
        }
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// The asks waiting on the owner.
pub async fn open_asks(server: &TestServer) -> Vec<Value> {
    let asks: Value = server.get("/permissions/asks").await.json().await.unwrap_or_default();
    asks["asks"].as_array().cloned().unwrap_or_default()
}

/// Hire a blank employee named `name` (its job: nothing yet) and activate
/// it. Returns its id.
pub async fn hire_blank(server: &TestServer, name: &str) -> String {
    let hired = server.post_json("/agents", &json!({"blank": true, "name": name})).await;
    assert_eq!(hired.status(), 200, "hiring {name}");
    let id = hired.json::<Value>().await.unwrap()["agent"]["id"].as_str().expect("agent id").to_string();
    let active = server.post_json(&format!("/agents/{id}/activate"), &json!({})).await;
    assert_eq!(active.status(), 200, "activating {name}");
    id
}

pub async fn set_mode(server: &TestServer, agent_id: &str, mode: &str) {
    let r = server.put_json(&format!("/agents/{agent_id}/permissions"), &json!({"mode": mode})).await;
    assert_eq!(r.status(), 200, "mode {mode}");
}

