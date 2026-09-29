//! The coding agent the company scenarios' linked employees run as
//! (`company.rs`): this test binary again. Kept apart from the scenarios,
//! which are one set with their fixtures; this is not a scenario.

use serde_json::{Value, json};

/// Not a test when the harness runs it: the coding agent a linked employee
/// runs as (`NEBO_PROOF_COMPANY` names the file it writes what it was told
/// to). A prompt that says HOLD starts a long refactor: a tool call running
/// and words said, until it is cancelled. Any other prompt is answered at
/// once with "ANSWER: 3 files left." Each `session/new` is a session of its
/// own, `s-1`, `s-2`, ..., noted with the folder it was opened in.
#[test]
fn fake_company_agent() {
    use std::io::{BufRead, Write};
    let Ok(told) = std::env::var("NEBO_PROOF_COMPANY") else {
        return;
    };
    let note = |line: Value| {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&told)
            .unwrap();
        writeln!(file, "{line}").unwrap();
    };
    let send = |frame: Value| {
        let mut out = std::io::stdout().lock();
        writeln!(out, "{frame}").unwrap();
        out.flush().unwrap();
    };
    let update = |session: &str, update: Value| {
        send(
            json!({ "jsonrpc": "2.0", "method": "session/update", "params": { "sessionId": session, "update": update } }),
        );
    };
    let say = |session: &str, text: &str| {
        update(
            session,
            json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": text } }),
        );
    };
    // The harness printed "test ... " without a newline: end that line.
    send(Value::Null);
    let mut sessions = 0;
    let mut held: std::collections::HashMap<String, Value> = std::collections::HashMap::new();
    for line in std::io::stdin().lock().lines() {
        let Ok(message) = serde_json::from_str::<Value>(&line.unwrap()) else {
            continue;
        };
        let id = message["id"].clone();
        let reply = |result: Value| send(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
        let session = message["params"]["sessionId"]
            .as_str()
            .unwrap_or("")
            .to_owned();
        match message["method"].as_str() {
            Some("initialize") => reply(
                json!({ "protocolVersion": 1, "agentCapabilities": { "loadSession": false },
                "agentInfo": { "name": "proof-company" } }),
            ),
            Some("session/new") => {
                sessions += 1;
                let session = format!("s-{sessions}");
                note(json!({ "new": session, "cwd": message["params"]["cwd"] }));
                reply(json!({ "sessionId": session }));
            }
            Some("session/prompt") => {
                let text = message["params"]["prompt"][0]["text"]
                    .as_str()
                    .unwrap_or("")
                    .to_owned();
                note(json!({ "prompt": text, "session": session }));
                if text.contains("HOLD") {
                    update(
                        &session,
                        json!({ "sessionUpdate": "tool_call", "toolCallId": "call_1", "title": "cargo test",
                        "kind": "execute", "status": "in_progress" }),
                    );
                    say(&session, "Working through the refactor.");
                    held.insert(session, id.clone());
                } else {
                    say(&session, "ANSWER: 3 files left.");
                    reply(json!({ "stopReason": "end_turn" }));
                }
            }
            Some("session/cancel") => {
                note(json!({ "cancel": session }));
                if let Some(prompt) = held.remove(&session) {
                    send(
                        json!({ "jsonrpc": "2.0", "id": prompt, "result": { "stopReason": "cancelled" } }),
                    );
                }
            }
            _ => {}
        }
    }
}
