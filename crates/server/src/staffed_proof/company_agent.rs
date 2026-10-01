//! The coding agent the company scenarios' linked employees run as
//! (`company.rs`): this test binary again. Kept apart from the scenarios,
//! which are one set with their fixtures; this is not a scenario.

use serde_json::{Value, json};

/// Not a test when the harness runs it: the coding agent a linked employee
/// runs as (`NEBO_PROOF_COMPANY` names the file it writes what it was told
/// to). A prompt that says HOLD starts a long refactor: a tool call running
/// and words said, until it is cancelled. One that says BRIEF is answered
/// "BRIEF-DONE." three seconds later. Any other prompt is answered at
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
                // The whole prompt, every block (a turn's briefing rides
                // before the words it was asked).
                let text = message["params"]["prompt"]
                    .as_array()
                    .map(|blocks| blocks.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n\n"))
                    .unwrap_or_default();
                note(json!({ "prompt": text, "session": session }));
                if text.contains("HOLD") {
                    update(
                        &session,
                        json!({ "sessionUpdate": "tool_call", "toolCallId": "call_1", "title": "cargo test",
                        "kind": "execute", "status": "in_progress" }),
                    );
                    say(&session, "Working through the refactor.");
                    held.insert(session, id.clone());
                } else if text.contains("BRIEF") {
                    // A short piece of work: answered on its own after a
                    // moment, so a message can arrive while it runs.
                    let (id, session) = (id.clone(), session.clone());
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_secs(3));
                        let mut out = std::io::stdout().lock();
                        let chunk = json!({ "jsonrpc": "2.0", "method": "session/update", "params": { "sessionId": session,
                            "update": { "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "BRIEF-DONE." } } } });
                        writeln!(out, "{chunk}").unwrap();
                        writeln!(out, "{}", json!({ "jsonrpc": "2.0", "id": id, "result": { "stopReason": "end_turn" } })).unwrap();
                        out.flush().unwrap();
                    });
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
