use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use super::*;

fn store(dir: &std::path::Path) -> Arc<db::Store> {
    Arc::new(db::Store::new(&dir.join("t.db").to_string_lossy()).unwrap())
}

fn employee(store: &db::Store, id: &str, name: &str, app: bool) {
    store
        .create_agent(id, Some("user"), name, "", "---\nname: x\n---\n", "{}", None, None)
        .unwrap();
    if app {
        store.set_agent_app_fields(id, true, Some("/tmp/ui"), None, None).unwrap();
    }
}

fn ctx(agent: &str) -> ToolContext {
    ToolContext {
        session_key: format!("agent:{agent}:web"),
        ..Default::default()
    }
}

type Sent = Arc<Mutex<Vec<(String, Value)>>>;

fn tool(store: Arc<db::Store>) -> (AppDataTool, Sent) {
    let sent: Sent = Arc::default();
    let log = sent.clone();
    let broadcaster: Broadcaster = Arc::new(move |name: &str, payload: Value| {
        log.lock().unwrap().push((name.to_string(), payload));
    });
    (AppDataTool::new(store, Some(broadcaster)), sent)
}

async fn call(t: &AppDataTool, agent: &str, input: Value) -> ToolResult {
    t.execute_dyn(&ctx(agent), input).await
}

/// What the page's `storage.setItem(key, value)` sends and the server keeps:
/// the value as text (a string as is, anything else JSON-encoded), and that
/// text JSON-encoded (`put_storage` stores `body.value.to_string()`).
fn page_set(store: &db::Store, app: &str, key: &str, value: &Value) {
    let sent = match value {
        Value::String(s) => Value::String(s.clone()),
        other => Value::String(other.to_string()),
    };
    write(store, app, key, &sent.to_string()).unwrap();
}

/// What the page's `storage.getItem(key)` returns for the stored text.
fn page_get(store: &db::Store, app: &str, key: &str) -> Option<Value> {
    let raw = read(store, app, key).unwrap()?;
    let once: Value = serde_json::from_str(&raw).unwrap();
    Some(match once {
        Value::String(text) => serde_json::from_str(&text).unwrap_or(Value::String(text)),
        other => other,
    })
}

fn contacts() -> Value {
    json!([
        { "name": "John Smith", "phone": { "mobile": "+1 555 0100" } },
        { "name": "Jane Doe", "phone": { "mobile": "+1 555 0199" } }
    ])
}

#[tokio::test]
async fn the_employee_reads_what_the_page_wrote() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(tmp.path());
    employee(&store, "crm", "CRM", true);
    page_set(&store, "crm", "contacts", &contacts());
    page_set(&store, "crm", "title", &json!("My contacts"));

    let (t, _) = tool(store);
    let got = call(&t, "crm", json!({"action": "get", "key": "contacts"})).await;
    assert!(!got.is_error, "{}", got.content);
    let got: Value = serde_json::from_str(&got.content).unwrap();
    assert_eq!(got["value"], contacts());

    let title = call(&t, "crm", json!({"action": "get", "key": "title"})).await;
    let title: Value = serde_json::from_str(&title.content).unwrap();
    assert_eq!(title["value"], json!("My contacts"));
}

#[tokio::test]
async fn the_page_reads_what_the_employee_wrote_and_hears_of_it() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(tmp.path());
    employee(&store, "crm", "CRM", true);
    let (t, sent) = tool(store.clone());

    let r = call(&t, "crm", json!({"action": "set", "key": "contacts", "value": contacts()})).await;
    assert!(!r.is_error, "{}", r.content);
    assert_eq!(page_get(&store, "crm", "contacts"), Some(contacts()));
    // Exactly the bytes the page's own setItem would have stored.
    let raw = read(&store, "crm", "contacts").unwrap().unwrap();
    assert_eq!(raw, Value::String(contacts().to_string()).to_string());

    let r = call(&t, "crm", json!({"action": "set", "key": "note", "value": "call back"})).await;
    assert!(!r.is_error);
    assert_eq!(page_get(&store, "crm", "note"), Some(json!("call back")));

    let r = call(&t, "crm", json!({"action": "delete", "key": "note"})).await;
    assert!(!r.is_error);
    assert_eq!(page_get(&store, "crm", "note"), None);
    assert!(!list(&store, "crm").unwrap().iter().any(|(k, _)| k == "note"));

    let sent = sent.lock().unwrap();
    assert_eq!(sent.len(), 3);
    assert!(sent.iter().all(|(name, _)| name == CHANGED_EVENT));
    assert_eq!(
        sent[0].1,
        json!({"appId": "crm", "keys": ["contacts"], "action": "set", "source": "employee"})
    );
    assert_eq!(sent[2].1["action"], "delete");
}

#[tokio::test]
async fn an_employee_reaches_only_its_own_app() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(tmp.path());
    employee(&store, "crm", "CRM", true);
    employee(&store, "notes", "Notes", true);
    employee(&store, "chief", "Chief of Staff", false);
    page_set(&store, "crm", "contacts", &contacts());
    let (t, sent) = tool(store.clone());

    // Another app sees its own (empty) store, never the CRM's.
    let other = call(&t, "notes", json!({"action": "list"})).await;
    assert!(!other.is_error);
    let other: Value = serde_json::from_str(&other.content).unwrap();
    assert_eq!(other["total"], 0);
    let r = call(&t, "notes", json!({"action": "set", "key": "contacts", "value": []})).await;
    assert!(!r.is_error);
    assert_eq!(page_get(&store, "crm", "contacts"), Some(contacts()));
    assert_eq!(sent.lock().unwrap()[0].1["appId"], "notes");

    // An employee that is not an app has no store and is told to ask.
    let chief = call(&t, "chief", json!({"action": "get", "key": "contacts"})).await;
    assert!(chief.is_error);
    assert!(chief.content.contains("send_message"), "{}", chief.content);

    // And is never offered the tool.
    assert_eq!(withheld(&store, "chief"), vec![APP_DATA.to_string()]);
    assert!(withheld(&store, "crm").is_empty());
    assert_eq!(withheld(&store, ""), vec![APP_DATA.to_string()]);
}

#[tokio::test]
async fn query_finds_a_record_inside_a_list_by_field_or_text() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(tmp.path());
    employee(&store, "crm", "CRM", true);
    page_set(&store, "crm", "contacts", &contacts());
    page_set(&store, "crm", "contact:3", &json!({"name": "John Appleseed", "phone": {"mobile": "+1 555 0123"}}));
    let (t, _) = tool(store);

    let by_name = call(&t, "crm", json!({"action": "query", "where": {"name": "john smith"}})).await;
    let by_name: Value = serde_json::from_str(&by_name.content).unwrap();
    assert_eq!(by_name["total"], 1);
    assert_eq!(by_name["items"][0]["key"], "contacts");
    assert_eq!(by_name["items"][0]["index"], 0);
    assert_eq!(by_name["items"][0]["value"]["phone"]["mobile"], "+1 555 0100");

    let johns = call(&t, "crm", json!({"action": "query", "text": "JOHN"})).await;
    let johns: Value = serde_json::from_str(&johns.content).unwrap();
    assert_eq!(johns["total"], 2);

    let by_path = call(&t, "crm", json!({"action": "query", "where": {"phone.mobile": "0199"}})).await;
    let by_path: Value = serde_json::from_str(&by_path.content).unwrap();
    assert_eq!(by_path["items"][0]["value"]["name"], "Jane Doe");

    let prefixed = call(&t, "crm", json!({"action": "query", "prefix": "contact:", "text": "john", "limit": 1})).await;
    let prefixed: Value = serde_json::from_str(&prefixed.content).unwrap();
    assert_eq!(prefixed["total"], 1);
    assert_eq!(prefixed["items"][0]["key"], "contact:3");

    let listed = call(&t, "crm", json!({"action": "list", "prefix": "contact:"})).await;
    let listed: Value = serde_json::from_str(&listed.content).unwrap();
    assert_eq!(listed["total"], 1);
}

#[test]
fn an_emptied_key_from_before_deletes_reads_as_absent() {
    let tmp = tempfile::tempdir().unwrap();
    let store = store(tmp.path());
    write(&store, "crm", "old", "").unwrap();
    assert_eq!(read(&store, "crm", "old").unwrap(), None);
    assert!(list(&store, "crm").unwrap().is_empty());
}

#[test]
fn encode_and_decode_are_the_pages_set_and_get() {
    for v in [json!({"a": 1}), json!([1, 2]), json!(42), json!(true), json!("hello"), json!(null)] {
        assert_eq!(decode(&encode(&v)), v, "{v}");
    }
    // Text a page stored some other way still reads.
    assert_eq!(decode("plain"), json!("plain"));
    assert_eq!(decode("{\"a\":1}"), json!({"a": 1}));
}

#[tokio::test]
async fn a_chat_key_is_each_chats_own_and_the_page_reads_it_by_chat() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    employee(&store, "studio", "Design Studio", true);
    let (t, sent) = tool(store.clone());
    let in_chat = |chat: &str| ToolContext {
        session_key: format!("agent:studio:thread:{chat}"),
        ..Default::default()
    };
    let set = |design: &str| json!({ "action": "set", "key": "chat:design", "value": design });

    let first = t.execute_dyn(&in_chat("c1"), set("d-1")).await;
    assert!(!first.is_error, "{}", first.content);
    assert!(!t.execute_dyn(&in_chat("c2"), set("d-2")).await.is_error);

    // The page opened from each chat reads that chat's own
    assert_eq!(page_get(&store, "studio", "chat:c1:design"), Some(json!("d-1")));
    assert_eq!(page_get(&store, "studio", "chat:c2:design"), Some(json!("d-2")));
    // and hears of it by the key it reads
    assert!(sent.lock().unwrap().iter().any(|(_, p)| p["keys"][0] == "chat:c1:design"));

    // Each chat reads back its own, by the same key
    let got = t.execute_dyn(&in_chat("c2"), json!({ "action": "get", "key": "chat:design" })).await;
    assert!(got.content.contains("d-2") && !got.content.contains("d-1"), "{}", got.content);

    // Outside the owner's chats a chat key has no chat to belong to
    let web = call(&t, "studio", json!({ "action": "set", "key": "chat:design", "value": "x" })).await;
    assert!(web.is_error, "{}", web.content);
    // and app-wide keys are unchanged
    assert!(!call(&t, "studio", json!({ "action": "set", "key": "designs", "value": [] })).await.is_error);
}
