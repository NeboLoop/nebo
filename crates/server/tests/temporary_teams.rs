//! A temporary team end to end on a real server (Batch E, E16): a scratch
//! home, no hub, and a model on loopback that answers from a script.
//!
//! Its own test binary: assignments reach the engine through an opener the
//! server installs once per process, so a second server booted in the same
//! process (a sibling test) would receive this one's assignments.

mod common;

use std::sync::Arc;

use common::TestServer;
use common::model::{Calls, Reply, eventually, hire_blank, main_call, server_with, set_mode};
use futures_util::SinkExt;
use serde_json::{Value, json};
use tokio_tungstenite::{connect_async, tungstenite::Message};

/// The last message from the owner that names `marker`, and nothing the
/// turn has done since: the step that answers it is the first.
fn fresh(messages: &[Value], marker: &str) -> bool {
    let Some(at) = messages
        .iter()
        .rposition(|m| m["role"] == "user" && m["content"].as_str().is_some_and(|c| c.contains(marker)))
    else {
        return false;
    };
    !messages[at + 1..].iter().any(|m| m["role"] == "tool" || m["role"] == "assistant")
}

/// Say `prompt` to employee `agent_id` in its app conversation.
async fn say(server: &TestServer, agent_id: &str, prompt: &str) {
    let (mut ws, _) = connect_async(format!("ws://127.0.0.1:{}/ws", server.port)).await.expect("ws");
    let msg = json!({"type": "chat", "data": {"session_id": format!("agent:{agent_id}:web"), "prompt": prompt}});
    ws.send(Message::Text(msg.to_string().into())).await.unwrap();
}

/// Whether a main call's thread says `text`.
fn heard(calls: &Calls, text: &str) -> bool {
    calls.lock().unwrap().iter().any(|(p, b)| {
        main_call(p) && b["messages"].as_array().is_some_and(|ms| ms.iter().any(|m| m["content"].as_str().is_some_and(|c| c.contains(text))))
    })
}

/// Whether a tool result since the model's last answer says `text`: what
/// this step reads new.
fn just_heard(messages: &[Value], text: &str) -> bool {
    let from = messages.iter().rposition(|m| m["role"] == "assistant").map_or(0, |i| i + 1);
    messages[from..].iter().any(|m| m["role"] == "tool" && m["content"].as_str().is_some_and(|c| c.contains(text)))
}

/// E16: for a direction that spans employees with no standing team, the
/// employee assembles a temporary team with a lead and hands it the work.
/// The lead closes it; the outcome reaches the owner and the conversation,
/// and the team disbands. A persistent team is untouched. Must never: a
/// temporary team left standing after its outcome reached the owner, a
/// second piece of work on it, or a persistent team removed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_temporary_team_takes_its_one_piece_of_work_and_disbands() {
    let (server, calls) = server_with(Arc::new(|purpose, messages| {
        if !main_call(purpose) {
            return Reply::Text("ok");
        }
        if fresh(messages, "MARK-TEAM") {
            return Reply::Call(vec![(
                "create_team",
                json!({"name": "Budget team", "members": ["Marketer"], "lead": "Bookkeeper", "lifetime": "temporary"}),
            )]);
        }
        // Two pieces of work in one response: the team takes one.
        if just_heard(messages, "Temporary team \"Budget team\" exists") {
            return Reply::Call(vec![
                ("assign_task", json!({"to": "Budget team", "subject": "MARK-TEAMWORK Find the marketing budget and what it buys.", "done_means": "A number and a plan"})),
                ("assign_task", json!({"to": "Budget team", "subject": "MARK-SECOND another job"})),
            ]);
        }
        if messages.iter().any(|m| m["content"].as_str().is_some_and(|c| c.contains("MARK-TEAMWORK"))) {
            return Reply::Text(
                r#"TEAM-RESULT: $900 for marketing. {"result": {"status": "done", "summary": "TEAM-RESULT: $900 for marketing, enough for two local ads"}, "next": {"action": "close"}}"#,
            );
        }
        Reply::Text("Noted.")
    }))
    .await;
    let planner = hire_blank(&server, "Planner").await;
    let bookkeeper = hire_blank(&server, "Bookkeeper").await;
    let marketer = hire_blank(&server, "Marketer").await;
    set_mode(&server, &planner, "full_access").await;
    let kept = server
        .post_json("/teams", &json!({"name": "Ops", "members": [{"agentId": bookkeeper}, {"agentId": marketer}], "organizerAgentId": bookkeeper}))
        .await;
    assert_eq!(kept.status(), 200, "the persistent team");
    let store = server.db_store();
    let team_named = |name: &str| store.list_teams().unwrap().into_iter().find(|t| t.name == name);

    say(&server, &planner, "MARK-TEAM have the bookkeeper and marketing work out what we can afford").await;
    let team = eventually(60, "the temporary team", async || team_named("Budget team")).await;
    assert_eq!(team.organizer_agent_id, bookkeeper, "assembled with a lead");

    // One piece of work: of the two, one is taken and one refused.
    eventually(60, "the second assignment to be refused", async || {
        heard(&calls, "already has its one piece of work").then_some(())
    })
    .await;
    assert_eq!(store.list_assignments_for_agent(&bookkeeper, false).unwrap().len(), 1, "one piece of work");

    // The lead closes it: the outcome reaches the owner, and the team goes.
    eventually(90, "the temporary team to disband", async || team_named("Budget team").is_none().then_some(())).await;
    let assignment = store.list_assignments_for_agent(&bookkeeper, false).unwrap();
    assert!(assignment.iter().all(|a| a.state != "open"), "its work is closed: {assignment:?}");
    let user = store.ensure_local_user_id().unwrap();
    let outcome = store
        .list_user_notifications(&user, 100, 0)
        .unwrap()
        .into_iter()
        .find(|n| n.id.starts_with("temporary:"))
        .expect("the outcome is in the Inbox");
    assert_eq!(outcome.title, "The Budget team finished");
    assert!(outcome.body.unwrap_or_default().contains("TEAM-RESULT"), "the lead's outcome");
    eventually(60, "the conversation to hear it disbanded", async || {
        heard(&calls, "It was a temporary team, so it has disbanded").then_some(())
    })
    .await;
    assert!(team_named("Ops").is_some(), "the persistent team is untouched");
    assert!(store.list_temporary_work().unwrap().is_empty(), "nothing temporary is left");
}

