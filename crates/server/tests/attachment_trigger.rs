//! An attachment that lands starts the work.
//!
//! A recording is uploaded through the real door — `POST /api/v1/files/upload`
//! on a real booted server — and the employee whose flow waits for a recording
//! runs, with the file named in its inputs. Nobody types anything. A document
//! uploaded the same way reaches the employee waiting for documents and leaves
//! the recording flow alone: that is the whole point of the kind in the source.
//!
//! Deliberately end to end rather than a unit test of the emit: a subscription
//! that never matches and an event that never leaves the handler both look
//! perfectly healthy in isolation.
//!
//! Run:
//!   cargo test -p nebo-server --test attachment_trigger -- --nocapture

use std::time::Duration;

use serde_json::{Value, json};

mod common;
use common::TestServer;

/// Hire an employee with one flow, waiting on `source`. Returns the agent id.
///
/// The binding is declared the way a package declares one — an `event` trigger
/// naming its sources — so the test rides the same subscription path every
/// other trigger uses.
async fn hire_waiting_for(server: &TestServer, name: &str, source: &str) -> String {
    let agent_md = format!("---\nname: {name}\ndescription: Waits for {source}\n---\n\n# {name}\n");
    let body = json!({
        "agentMd": agent_md,
        "name": name,
        "description": format!("Waits for {source}"),
        "agentJson": {
            "skills": [],
            "workflows": {
                "on-arrival": {
                    "description": format!("Act on {source}"),
                    "trigger": { "type": "event", "sources": [source] },
                    "activities": [
                        { "id": "act", "type": "custom", "intent": "Act on the file that arrived." }
                    ],
                    "connections": [
                        { "from": "__trigger__", "to": "act" },
                        { "from": "act", "to": "__emit__" }
                    ]
                }
            }
        }
    });

    let resp = server.post_json("/agents", &body).await;
    assert_eq!(resp.status(), 200, "hiring {name} failed");
    let created: Value = resp.json().await.unwrap();
    let id = created["agent"]["id"].as_str().expect("agent id").to_string();

    // Activation is what puts the subscription in the dispatcher — the worker
    // registers it inline before this call returns.
    let resp = server
        .post_json(&format!("/agents/{id}/activate"), &json!({}))
        .await;
    assert_eq!(resp.status(), 200, "activating {name} failed");
    id
}

/// Upload a file the way a client does: multipart, naming the employee it
/// landed on and the conversation it came from. Returns the stored file id.
async fn upload(
    server: &TestServer,
    filename: &str,
    mime: &str,
    bytes: Vec<u8>,
    agent_id: &str,
    chat_id: &str,
) -> String {
    let part = reqwest::multipart::Part::bytes(bytes)
        .file_name(filename.to_string())
        .mime_str(mime)
        .unwrap();
    let form = reqwest::multipart::Form::new()
        .part("file", part)
        .text("agentId", agent_id.to_string())
        .text("chatId", chat_id.to_string());

    let resp = reqwest::Client::new()
        .post(server.url("/files/upload"))
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "upload failed");
    let body: Value = resp.json().await.unwrap();
    body["fileId"].as_str().expect("fileId").to_string()
}

/// The runs this employee started, newest first. The run row is written before
/// the run executes, so an arrival is visible here even with no AI provider
/// configured — the trigger is what is under test, not the work.
fn runs_of(server: &TestServer, agent_id: &str) -> Vec<db::models::WorkflowRun> {
    server
        .db_store()
        .list_workflow_runs(&types::keyparser::agent_workflow_id(agent_id), 10, 0)
        .expect("list runs")
}

/// Wait for the arrival to reach the employee waiting for it.
async fn run_after_arrival(server: &TestServer, agent_id: &str, what: &str) -> db::models::WorkflowRun {
    for _ in 0..100 {
        if let Some(run) = runs_of(server, agent_id).into_iter().next() {
            return run;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the {what} never reached the employee waiting for it");
}

#[tokio::test]
async fn an_attachment_starts_the_flow_that_waits_for_its_kind() {
    let server = TestServer::boot().await;

    let recordings = hire_waiting_for(&server, "Recording Listener", "attachment.audio").await;
    let documents = hire_waiting_for(&server, "Document Listener", "attachment.file").await;

    // ── a recording lands ────────────────────────────────────────────
    let spoken = b"not really audio, but named and typed as a recording".to_vec();
    let spoken_size = spoken.len();
    let file_id = upload(
        &server,
        "conversation.m4a",
        "audio/m4a",
        spoken,
        &recordings,
        "chat-7",
    )
    .await;

    let run = run_after_arrival(&server, &recordings, "recording").await;
    assert_eq!(run.trigger_type, "event");
    assert_eq!(
        run.trigger_detail.as_deref(),
        Some("on-arrival:attachment.audio"),
        "the run says which binding and which arrival started it"
    );

    let inputs: Value = serde_json::from_str(run.inputs.as_deref().unwrap_or("{}")).unwrap();
    assert_eq!(inputs["_event_source"], "attachment.audio");
    let payload = &inputs["_event_payload"];
    assert_eq!(payload["file_id"], file_id, "the flow can fetch the file");
    assert_eq!(payload["kind"], "audio");
    assert_eq!(payload["filename"], "conversation.m4a");
    assert_eq!(payload["size"], spoken_size);
    assert_eq!(payload["agent_id"], recordings, "the employee it landed on");
    assert_eq!(payload["chat_id"], "chat-7", "the conversation it came from");

    // ── a document lands ─────────────────────────────────────────────
    upload(
        &server,
        "agreement.pdf",
        "application/pdf",
        b"%PDF-1.7 not really a pdf".to_vec(),
        &documents,
        "chat-9",
    )
    .await;

    let run = run_after_arrival(&server, &documents, "document").await;
    let inputs: Value = serde_json::from_str(run.inputs.as_deref().unwrap_or("{}")).unwrap();
    assert_eq!(inputs["_event_source"], "attachment.file");
    assert_eq!(inputs["_event_payload"]["kind"], "file");
    assert_eq!(inputs["_event_payload"]["filename"], "agreement.pdf");

    // ── and neither woke the other ───────────────────────────────────
    assert_eq!(
        runs_of(&server, &recordings).len(),
        1,
        "the document woke the employee waiting for recordings"
    );
    assert_eq!(
        runs_of(&server, &documents).len(),
        1,
        "the recording woke the employee waiting for documents"
    );
}
