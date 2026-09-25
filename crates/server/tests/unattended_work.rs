//! Work that runs while the owner isn't in the conversation, end to end on
//! a real server: a scratch home, no hub, and a model on loopback that
//! answers from a script.
//!
//! - A workflow step the permission check parks is answered once from the
//!   web Inbox and once from the phone's Inbox (the answer call the hub
//!   relays through the tunnel); both times the run resumes at the parked
//!   call and completes.
//! - An employee's scheduled job runs as that employee: its identity and
//!   rules, the job's message with its instructions in the thread, and the
//!   employee's own permissions.
//! - A coworker asked by another employee acts under its own permissions:
//!   the asker's web access is never lent to it.

mod common;

use std::sync::Arc;

use common::TestServer;
use common::model::{Reply, eventually, has_result, hire_blank, main_call, open_asks, says, server_with, set_mode};
use serde_json::{Value, json};

/// Run the flow once, wait for its one ask, answer it from `via`, and see
/// the run finish with the file written.
async fn run_and_answer(server: &TestServer, agent_id: &str, target: &std::path::Path, via: &str) {
    let started = server.post_json(&format!("/agents/{agent_id}/workflows/close-books/run"), &json!({})).await;
    assert_eq!(started.status(), 200, "the run starts");
    let run_id = started.json::<Value>().await.unwrap()["runId"].as_str().expect("runId").to_string();

    let ask = eventually(60, "the step's ask", async || open_asks(server).await.into_iter().next()).await;
    let store = server.db_store();
    assert_eq!(
        store.get_workflow_run(&run_id).unwrap().expect("the run").status,
        "awaiting_approval",
        "the run is parked on the ask, not failed"
    );
    assert!(!target.exists(), "nothing ran before the answer");

    let id = ask["id"].as_str().expect("ask id");
    let answered = server
        .post_json(&format!("/permissions/asks/{id}/answer"), &json!({"answer": "this_once", "via": via}))
        .await;
    assert_eq!(answered.status(), 200, "the {via} answer is taken");

    // A released run is queued to resume, which reads as "interrupted"
    // (about to resume) until the engine picks it up: not an end.
    let status = eventually(60, "the run to finish", async || {
        let run = store.get_workflow_run(&run_id).ok()??;
        (!["awaiting_approval", "running", "pending", "interrupted"].contains(&run.status.as_str())).then_some(run.status)
    })
    .await;
    assert_eq!(status, "completed", "answered from {via}, the run resumes and completes");
    assert_eq!(std::fs::read_to_string(target).unwrap(), "approved", "the parked call ran once, as approved");
}

/// U2: a gated workflow step, answered from the web Inbox and from the hub.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_parked_workflow_step_resumes_when_answered_from_the_web_inbox_or_the_hub() {
    let out = tempfile::tempdir().unwrap();
    let target = out.path().join("ledger-close.txt");
    let path = target.to_string_lossy().into_owned();
    let (server, _) = server_with(Arc::new(move |purpose, messages| {
        if main_call(purpose) && !has_result(messages) {
            Reply::Call("write_file", json!({"path": path, "content": "approved"}))
        } else if main_call(purpose) {
            Reply::Text("Done.")
        } else {
            Reply::Text("ok")
        }
    }))
    .await;

    let agent_md = "---\nname: Closer\ndescription: Closes the books\n---\n\n# Closer\n";
    let hired = server
        .post_json(
            "/agents",
            &json!({
                "agentMd": agent_md,
                "name": "Closer",
                "description": "Closes the books",
                "agentJson": {
                    "skills": [],
                    "workflows": {
                        "close-books": {
                            "description": "Write the close note",
                            "trigger": { "type": "manual" },
                            "activities": [
                                { "id": "write-note", "type": "custom", "intent": "Write the close note to the ledger file." }
                            ],
                            "connections": [
                                { "from": "__trigger__", "to": "write-note" },
                                { "from": "write-note", "to": "__emit__" }
                            ]
                        }
                    }
                }
            }),
        )
        .await;
    assert_eq!(hired.status(), 200);
    let agent_id = hired.json::<Value>().await.unwrap()["agent"]["id"].as_str().expect("agent id").to_string();
    // Ask mode: every change asks the owner first.
    set_mode(&server, &agent_id, "ask").await;

    run_and_answer(&server, &agent_id, &target, "inbox").await;
    std::fs::remove_file(&target).unwrap();
    run_and_answer(&server, &agent_id, &target, "mobile").await;
}

/// U3: an employee's scheduled job runs as the employee.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_scheduled_job_runs_as_its_employee_with_its_rules_and_instructions() {
    let out = tempfile::tempdir().unwrap();
    let path = out.path().join("overnight.txt").to_string_lossy().into_owned();
    let (server, calls) = server_with(Arc::new(move |purpose, messages| {
        if main_call(purpose) && says(messages, "Check the overnight entries.") && !has_result(messages) {
            Reply::Call("write_file", json!({"path": path, "content": "checked"}))
        } else if main_call(purpose) {
            Reply::Text("Checked.")
        } else {
            Reply::Text("ok")
        }
    }))
    .await;
    let keeper = hire_blank(&server, "Keeper").await;
    let ruled = server.put_json(&format!("/agents/{keeper}"), &json!({"rules": "Always cite the ledger page."})).await;
    assert_eq!(ruled.status(), 200, "the employee's rules are set");
    set_mode(&server, &keeper, "ask").await;

    let job = server
        .post_json(
            "/tasks",
            &json!({"name": "morning-check", "schedule": "0 7 * * *", "taskType": "agent", "agentId": keeper,
                    "message": "Check the overnight entries.", "instructions": "List any entry over $500."}),
        )
        .await;
    assert!(job.status().is_success(), "the job is scheduled: {}", job.status());
    let fired = server.post_json("/tasks/morning-check/run", &json!({})).await;
    assert!(fired.status().is_success(), "the job fires");

    // It acts under the employee's own permissions: its change asks as Keeper.
    let ask = eventually(60, "the job's ask", async || open_asks(&server).await.into_iter().next()).await;
    assert_eq!(ask["agentId"], keeper, "the job runs under the employee's grant: {ask:#}");
    let key = format!("agent:{keeper}:cron:morning-check");
    assert_eq!(ask["sessionKey"], key, "in the employee's job session");

    let turn = calls
        .lock()
        .unwrap()
        .iter()
        .find(|(p, b)| main_call(p) && says(b["messages"].as_array().unwrap(), "Check the overnight entries."))
        .map(|(_, b)| b["messages"].as_array().unwrap().clone())
        .expect("the job's turn reached the model");
    let owner_row = turn
        .iter()
        .filter_map(|m| m["content"].as_str())
        .find(|c| c.contains("Check the overnight entries."))
        .unwrap();
    assert!(owner_row.contains("List any entry over $500."), "the instructions are in the thread: {owner_row}");
    assert!(says(&turn, "You are Keeper"), "the turn is the employee's");
    assert!(says(&turn, "Always cite the ledger page."), "with the employee's rules");
}

/// U4: a coworker runs under its own permissions, never the asker's. The
/// asker works in Full Access. Asked to write a file, the coworker's call is
/// decided as the coworker, through the coworker door, under the limits a
/// request from another employee carries (it can reply, not change things,
/// as on main): the asker's access is never lent to it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_coworker_acts_under_its_own_permissions_not_the_askers() {
    let out = tempfile::tempdir().unwrap();
    let target = out.path().join("vat-rate.txt");
    let path = target.to_string_lossy().into_owned();
    let (server, _) = server_with(Arc::new(move |purpose, messages| {
        if !main_call(purpose) {
            return Reply::Text("ok");
        }
        if says(messages, "[Coworker message from Supervisor]") {
            if has_result(messages) {
                return Reply::Text("I can't write that file from a coworker's request.");
            }
            return Reply::Call("write_file", json!({"path": path, "content": "VAT 20%"}));
        }
        if says(messages, "Ask Field Researcher to note the VAT rate.") {
            if has_result(messages) {
                return Reply::Text("I've asked Field Researcher.");
            }
            return Reply::Call(
                "send_message",
                json!({"to": "Field Researcher", "message": "Please write the VAT rate to the rates file.", "wait": false}),
            );
        }
        Reply::Text("Hello.")
    }))
    .await;
    let supervisor = hire_blank(&server, "Supervisor").await;
    let researcher = hire_blank(&server, "Field Researcher").await;
    set_mode(&server, &supervisor, "full_access").await;

    let said = server
        .post_json(&format!("/agents/{supervisor}/chat"), &json!({"prompt": "Ask Field Researcher to note the VAT rate."}))
        .await;
    assert!(said.status().is_success(), "the owner's message is taken: {}", said.status());

    let decided = eventually(60, "the coworker's write to be decided", async || {
        let page: Value = server.get("/permissions/activity?limit=50").await.json().await.ok()?;
        page["rows"].as_array()?.iter().find(|r| r["action"] == "Writing vat-rate.txt").cloned()
    })
    .await;
    assert_eq!(decided["employeeId"], researcher, "decided as the coworker: {decided:#}");
    assert_eq!(decided["door"], "coworker", "through the coworker door: {decided:#}");
    assert_ne!(decided["decision"], "allow", "the asker's Full Access is not lent: {decided:#}");
    assert!(!target.exists(), "the coworker never wrote on the asker's access");
}
