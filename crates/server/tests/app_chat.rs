//! An app's page talks to its own employee (`nebo.chat`), and to nothing
//! else of the owner's.
//!
//! The page proves itself with its app's pass (`napp::page_pass`) alone. Its
//! words run as the app's (`Origin::App`) on the app's own conversation:
//! none of the owner's composer powers apply to them (an install code, a
//! slash command, a local picture named in the words, an @mention of another
//! employee), and no other employee can be named to answer. The app's
//! socket carries the words of the app's own conversations, never the
//! owner's chats or another app's.
//!
//! Driven through the real server, with the scripted model.
//!
//! Run:
//!   cargo test -p nebo-server --test app_chat

use std::sync::{Arc, Mutex};

use futures_util::StreamExt;
use reqwest::StatusCode;
use reqwest::redirect::Policy;
use serde_json::{Value, json};
use tokio_tungstenite::{connect_async, tungstenite::Message};

mod common;
use common::TestServer;
use common::model::{Reply, Script, eventually, has_result, hire_blank, main_call, server_with};

const APP: &str = "app-crm";
const OTHER: &str = "app-notes";

/// A 1x1 PNG: a real picture, the kind the owner's composer takes in when
/// its path is named in his words.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00,
    0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63,
    0xF8, 0xCF, 0xC0, 0xF0, 0x1F, 0x00, 0x05, 0x00, 0x01, 0xFF, 0x89, 0x99, 0x3D, 0x1D, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44,
    0xAE, 0x42, 0x60, 0x82,
];

/// An app on disk, as the loader reads it.
fn install(server_dir: &std::path::Path, id: &str, name: &str) {
    let dir = server_dir.join("user/agents").join(name);
    std::fs::create_dir_all(dir.join("ui")).unwrap();
    std::fs::write(dir.join("AGENT.md"), format!("---\nname: {name}\n---\n# {name}")).unwrap();
    let manifest = json!({ "id": id, "name": name, "version": "1.0.0", "type": "app", "permissions": ["storage:readwrite"] });
    std::fs::write(dir.join("manifest.json"), manifest.to_string()).unwrap();
    std::fs::write(dir.join("ui/index.html"), "<html><body>app</body></html>").unwrap();
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().redirect(Policy::none()).build().unwrap()
}

/// The pass the owner's open of the app's page is answered with.
async fn pass_of(server: &TestServer, app: &str) -> String {
    let opened = client()
        .get(format!("http://127.0.0.1:{}/k/{}/apps/{app}/ui/index.html", server.port, server.key))
        .send()
        .await
        .unwrap();
    assert_eq!(opened.status(), StatusCode::SEE_OTHER);
    let location = opened.headers()["location"].to_str().unwrap().to_string();
    location.trim_start_matches("../").strip_prefix("k/").and_then(|r| r.split('/').next()).unwrap().to_string()
}

struct Page {
    base: String,
    pass: String,
}

impl Page {
    fn url(&self, app: &str, rest: &str) -> String {
        format!("{}/k/{}/api/v1/apps/{app}/chat{rest}", self.base, self.pass)
    }

    async fn say(&self, app: &str, body: Value) -> (StatusCode, String) {
        let r = client().post(self.url(app, "")).header("origin", "null").json(&body).send().await.unwrap();
        (r.status(), r.text().await.unwrap_or_default())
    }

    async fn history(&self, app: &str, thread: Option<&str>) -> Value {
        let q = thread.map(|t| format!("?thread={t}")).unwrap_or_default();
        let r = client().get(self.url(app, &q)).header("origin", "null").send().await.unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        r.json().await.unwrap()
    }

    /// The employee's replies in the conversation, once it has `n`.
    async fn replies(&self, app: &str, thread: Option<&str>, n: usize) -> Vec<Value> {
        eventually(60, "the employee's replies", async || {
            let h = self.history(app, thread).await;
            let replies: Vec<Value> = h["messages"].as_array()?.iter().filter(|m| m["role"] == "assistant").cloned().collect();
            (replies.len() >= n && h["running"] == false).then_some(replies)
        })
        .await
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_app_page_talks_to_its_own_employee_alone() {
    if !common::in_own_process().await {
        return;
    }
    let home = config::data_dir().unwrap();
    install(&home, APP, "CRM");
    install(&home, OTHER, "Notes");
    let picture = home.join("secret.png");
    std::fs::write(&picture, PNG).unwrap();

    // The tool the employee calls for MARK-TOOL: one it was offered.
    let tool: Arc<Mutex<String>> = Arc::default();
    let offered = tool.clone();
    let script: Script = Arc::new(move |purpose, messages| {
        if !main_call(purpose) {
            return Reply::Text("ok");
        }
        // The page's words: what the user side said since the model last
        // answered (the words, and any reminder the house adds after them).
        let since = messages.iter().rposition(|m| m["role"] == "assistant").map_or(0, |i| i + 1);
        let last: String = messages[since..]
            .iter()
            .filter(|m| m["role"] == "user")
            .map(|m| m["content"].as_str().map(str::to_string).unwrap_or_else(|| m["content"].to_string()))
            .collect::<Vec<_>>()
            .join("\n");
        if last.contains("MARK-TOOL") && !has_result(messages) {
            let name: &'static str = Box::leak(offered.lock().unwrap().clone().into_boxed_str());
            Reply::Call(vec![(name, json!({}))])
        } else if last.contains("MARK-SLOW") {
            std::thread::sleep(std::time::Duration::from_secs(4));
            Reply::Text("SLOW-REPLY")
        } else if last.contains("MARK-OTHER") {
            Reply::Text("OTHER-REPLY")
        } else if last.contains("MARK-OWNER") {
            Reply::Text("OWNER-REPLY")
        } else {
            Reply::Text("APP-REPLY")
        }
    });
    let (server, calls) = server_with(script).await;
    let pam = hire_blank(&server, "Pam").await;
    let base = format!("http://127.0.0.1:{}", server.port);
    let page = Page { base: base.clone(), pass: pass_of(&server, APP).await };
    let other = Page { base: base.clone(), pass: pass_of(&server, OTHER).await };

    // The app's socket, opened with its pass, hears everything it is sent.
    let heard: Arc<Mutex<Vec<Value>>> = Arc::default();
    let (mut socket, _) = connect_async(format!("ws://127.0.0.1:{}/k/{}/ws/app/{APP}", server.port, page.pass)).await.expect("the app's socket");
    let sink = heard.clone();
    tokio::spawn(async move {
        while let Some(Ok(frame)) = socket.next().await {
            if let Message::Text(text) = frame
                && let Ok(v) = serde_json::from_str::<Value>(&text)
            {
                sink.lock().unwrap().push(v);
            }
        }
    });

    // An employee the owner turned off stays off: the page can't turn it on.
    assert_eq!(server.post_json(&format!("/agents/{APP}/deactivate"), &json!({})).await.status(), 200);
    let (status, body) = page.say(APP, json!({ "message": "hi" })).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body.contains("turned off"), "{body}");
    for app in [APP, OTHER] {
        assert_eq!(server.post_json(&format!("/agents/{app}/activate"), &json!({})).await.status(), 200);
    }

    // Said and answered, in the app's own conversation.
    let (status, body) = page.say(APP, json!({ "message": "MARK-HELLO what's on my board?", "context": { "deal": "d-7" } })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let replies = page.replies(APP, None, 1).await;
    assert_eq!(replies[0]["content"], "APP-REPLY");
    let history = page.history(APP, None).await;
    assert_eq!(history["sessionId"], format!("agent:{APP}:page"));
    let first = &history["messages"][0];
    assert_eq!((first["role"].as_str(), first["content"].as_str()), (Some("user"), Some("MARK-HELLO what's on my board?")));
    // The page's context reached the model beside the words.
    assert!(calls.lock().unwrap().iter().any(|(_, b)| b.to_string().contains("d-7")), "the app's context reached the model");

    // Its socket heard the words and the reply of its conversation.
    eventually(10, "the socket hears the reply", async || {
        let h = heard.lock().unwrap();
        let ours = |kind: &str| h.iter().any(|e| e["type"] == kind && e["data"]["session_id"] == format!("agent:{APP}:page"));
        (ours("chat_user_message") && ours("chat_stream") && ours("chat_complete")).then_some(())
    })
    .await;

    // A thread of its own is a conversation of its own; a name that could
    // reach another key is refused.
    assert_eq!(page.say(APP, json!({ "message": "MARK-THREAD", "thread": "deal-7" })).await.0, StatusCode::OK);
    page.replies(APP, Some("deal-7"), 1).await;
    assert_eq!(page.history(APP, None).await["messages"].as_array().unwrap().len(), 2, "the thread's words stay in the thread");
    for bad in ["a:b", "../x", "x y"] {
        assert_eq!(page.say(APP, json!({ "message": "hi", "thread": bad })).await.0, StatusCode::BAD_REQUEST, "{bad}");
    }
    assert_eq!(page.say(APP, json!({ "message": "   " })).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(page.say(APP, json!({ "message": "hi", "scope": "never-declared" })).await.0, StatusCode::BAD_REQUEST);

    // None of the owner's composer powers: an install code, a slash command,
    // a picture named by its path and an @mention all reach the app's
    // employee as words, and the employee itself answers each.
    let picture_words = format!("look at {}", picture.display());
    for words in ["NEBO-ABCD-1234", "/clear", picture_words.as_str(), "@Pam MARK-MENTION what do you think?"] {
        let before = page.replies(APP, None, 1).await.len();
        assert_eq!(page.say(APP, json!({ "message": words })).await.0, StatusCode::OK, "{words}");
        page.replies(APP, None, before + 1).await;
        assert!(calls.lock().unwrap().iter().any(|(_, b)| b.to_string().contains(words.split(' ').next().unwrap())), "{words} reached the model as words");
    }
    let bodies: String = calls.lock().unwrap().iter().map(|(_, b)| b.to_string()).collect();
    assert!(!bodies.contains("image_url") && !bodies.contains("data:image"), "no picture of the owner's was taken in");
    let pam_chats: Value = server.get(&format!("/agents/{pam}/chats")).await.json().await.unwrap_or_default();
    assert!(!pam_chats.to_string().contains("MARK-MENTION"), "the mention never reached Pam: {pam_chats}");
    // Naming another employee to answer is not a thing the page can do.
    let before = page.replies(APP, None, 1).await.len();
    assert_eq!(page.say(APP, json!({ "message": "MARK-AGENT", "agent": pam })).await.0, StatusCode::OK);
    page.replies(APP, None, before + 1).await;
    assert!(!server.get(&format!("/agents/{pam}/chats")).await.text().await.unwrap().contains("MARK-AGENT"));

    // What the employee does on the way (a tool call and its result) stays
    // with the owner: the page hears the words alone.
    let first_tool = calls
        .lock()
        .unwrap()
        .iter()
        .rev()
        .find(|(p, _)| main_call(p))
        .and_then(|(_, b)| {
            // One that answers at once: never a tool that waits on the owner.
            b["tools"].as_array()?.iter().filter_map(|t| t["function"]["name"].as_str()).find(|n| !n.starts_with("ask")).map(str::to_string)
        })
        .expect("the employee is offered a tool");
    *tool.lock().unwrap() = first_tool;
    let before = page.replies(APP, None, 1).await.len();
    assert_eq!(page.say(APP, json!({ "message": "MARK-TOOL look it up" })).await.0, StatusCode::OK);
    page.replies(APP, None, before + 1).await;
    let tool_ran = calls.lock().unwrap().iter().any(|(p, b)| main_call(p) && has_result(b["messages"].as_array().map(Vec::as_slice).unwrap_or(&[])));
    let last_roles: Vec<String> = calls
        .lock()
        .unwrap()
        .iter()
        .rev()
        .take(3)
        .map(|(p, b)| format!("{p}: {}", b["messages"].as_array().map(|m| m.iter().map(|x| x["role"].as_str().unwrap_or("?").to_string()).collect::<Vec<_>>().join(",")).unwrap_or_default()))
        .collect();
    assert!(tool_ran, "the tool ran: {last_roles:?}");

    // One message at a time: while a turn runs, another send and starting
    // over are refused, never queued without their own context.
    let before = page.replies(APP, None, 1).await.len();
    assert_eq!(page.say(APP, json!({ "message": "MARK-SLOW think hard" })).await.0, StatusCode::OK);
    eventually(10, "the slow turn is running", async || (page.history(APP, None).await["running"] == true).then_some(())).await;
    let (status, body) = page.say(APP, json!({ "message": "and another thing", "context": { "deal": "d-9" } })).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let fresh = client().post(page.url(APP, "/new")).header("origin", "null").json(&json!({})).send().await.unwrap();
    assert_eq!(fresh.status(), StatusCode::CONFLICT);
    page.replies(APP, None, before + 1).await;

    // The owner's own doors stay out of the page's conversation: his
    // composer naming it, or a thread key naming its chat, is refused, and
    // it is never one of his chats to open or list.
    let store = server.db_store();
    let page_chat = store.get_session_by_name(&format!("agent:{APP}:page")).unwrap().unwrap().active_chat_id.unwrap();
    let (mut owner_ws, _) = connect_async(server.ws_url()).await.unwrap();
    use futures_util::SinkExt;
    owner_ws.send(Message::Text(json!({"type": "connect", "data": {"client_id": "owner"}}).to_string().into())).await.unwrap();
    for key in [format!("agent:{APP}:page"), format!("agent:{APP}:thread:{page_chat}")] {
        let send = json!({"type": "chat", "data": {"agent_id": APP, "session_id": key, "prompt": "MARK-INTRUDE"}});
        owner_ws.send(Message::Text(send.to_string().into())).await.unwrap();
    }
    let mut refused = 0;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while refused < 2 {
        let Ok(Some(Ok(Message::Text(text)))) = tokio::time::timeout_at(deadline, owner_ws.next()).await else { break };
        let ev: Value = serde_json::from_str(&text).unwrap_or_default();
        if ev["type"] == "chat_error" && ev["data"]["error"].as_str().is_some_and(|e| e.contains("belongs to an app")) {
            refused += 1;
        }
    }
    assert_eq!(refused, 2, "both of the owner's doors into the page's conversation are refused");
    // His REST chat, naming the page's conversation or its chat, is refused
    // too, and says so.
    for key in [format!("agent:{APP}:page"), format!("agent:{APP}:thread:{page_chat}")] {
        let r = server.post_json(&format!("/agents/{APP}/chat"), &json!({ "prompt": "MARK-INTRUDE", "sessionKey": key })).await;
        assert_eq!(r.status(), StatusCode::CONFLICT, "{key}");
    }
    // And so is a compact of it from his socket: nothing of his runs there.
    owner_ws
        .send(Message::Text(json!({"type": "session_compact", "data": {"session_id": format!("agent:{APP}:page"), "instructions": "MARK-INTRUDE"}}).to_string().into()))
        .await
        .unwrap();
    let mut compact_refused = false;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while !compact_refused {
        let Ok(Some(Ok(Message::Text(text)))) = tokio::time::timeout_at(deadline, owner_ws.next()).await else { break };
        let ev: Value = serde_json::from_str(&text).unwrap_or_default();
        compact_refused = ev["type"] == "session_compact"
            && ev["data"]["success"] == false
            && ev["data"]["error"].as_str().is_some_and(|e| e.contains("belongs to an app"));
    }
    assert!(compact_refused, "a compact of the page's conversation is refused");
    assert!(!calls.lock().unwrap().iter().any(|(_, b)| b.to_string().contains("MARK-INTRUDE")), "nothing of his ran there");
    assert_eq!(server.post_json(&format!("/agents/{APP}/chats/{page_chat}/activate"), &json!({})).await.status(), 404);
    let listed = server.get(&format!("/agents/{APP}/chats")).await.text().await.unwrap();
    assert!(!listed.contains(&page_chat), "never among the owner's chats: {listed}");

    // Another app's pass never reaches this app's chat, and the owner's
    // install key is not an app page.
    assert_eq!(other.say(APP, json!({ "message": "hi" })).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(client().get(other.url(APP, "")).send().await.unwrap().status(), StatusCode::UNAUTHORIZED);
    let owner = Page { base: base.clone(), pass: server.key.clone() };
    assert_eq!(owner.say(APP, json!({ "message": "hi" })).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(client().get(format!("{base}/api/v1/apps/{APP}/chat")).send().await.unwrap().status(), StatusCode::UNAUTHORIZED);

    // Another app's conversation and the owner's own chat run, and this
    // app's socket hears neither.
    assert_eq!(other.say(OTHER, json!({ "message": "MARK-OTHER" })).await.0, StatusCode::OK);
    other.replies(OTHER, None, 1).await;
    let (mut desk, _) = connect_async(server.ws_url()).await.unwrap();
    desk.send(Message::Text(json!({"type": "connect", "data": {"client_id": "desk"}}).to_string().into())).await.unwrap();
    let owner_key = format!("agent:{pam}:web");
    let send = json!({"type": "chat", "message_id": "m-1", "data": {"agent_id": pam, "session_id": owner_key, "prompt": "MARK-OWNER"}});
    desk.send(Message::Text(send.to_string().into())).await.unwrap();
    eventually(60, "the owner's chat reaches the model", async || {
        calls.lock().unwrap().iter().any(|(p, b)| main_call(p) && b.to_string().contains("MARK-OWNER")).then_some(())
    })
    .await;
    // Its reply streams, and the last frames have had time to arrive.
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let h = heard.lock().unwrap().clone();
    let all = serde_json::to_string(&h).unwrap();
    assert!(!all.contains("OTHER-REPLY") && !all.contains("MARK-OTHER"), "another app's conversation never reaches this app");
    assert!(!all.contains("OWNER-REPLY") && !all.contains("MARK-OWNER"), "the owner's chat never reaches an app");
    for e in &h {
        if let Some(key) = e["data"]["session_id"].as_str() {
            assert!(key == format!("agent:{APP}:page") || key.starts_with(&format!("agent:{APP}:page:")), "{e}");
        }
        assert!(!matches!(e["type"].as_str(), Some("tool_start" | "tool_result" | "thinking" | "approval_request")), "{e}");
    }

    // Stop: only the page's conversation, and the page hears it.
    let stopped = client().post(page.url(APP, "/stop")).header("origin", "null").json(&json!({})).send().await.unwrap();
    assert_eq!(stopped.status(), StatusCode::OK);
    assert_eq!(client().post(other.url(APP, "/stop")).json(&json!({})).send().await.unwrap().status(), StatusCode::UNAUTHORIZED);

    // Starting over: the next words begin a new conversation; the one
    // before is kept.
    let kept = page.history(APP, None).await["messages"].as_array().unwrap().len();
    assert!(kept > 0);
    let fresh = client().post(page.url(APP, "/new")).header("origin", "null").json(&json!({})).send().await.unwrap();
    assert_eq!(fresh.status(), StatusCode::OK);
    assert_eq!(page.history(APP, None).await["messages"].as_array().unwrap().len(), 0, "a new conversation");
    let session = store.get_session_by_name(&format!("agent:{APP}:page")).unwrap().unwrap();
    let earlier = store.list_chats_by_session(session.name.as_deref().unwrap()).unwrap_or_default();
    assert!(earlier.len() >= 2, "the conversation before is kept beside the new one: {earlier:?}");
    assert_eq!(client().post(other.url(APP, "/new")).json(&json!({})).send().await.unwrap().status(), StatusCode::UNAUTHORIZED);
}
