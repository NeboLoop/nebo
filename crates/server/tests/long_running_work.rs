//! Long-running work end to end on a real server (Batch E, the design for
//! E1–E4 and E13–E18): a scratch home, no hub, and a model on loopback that
//! answers from a script.
//!
//! - A temporary workflow the employee makes in the owner's conversation
//!   runs once, reports its outcome to the owner and to that conversation,
//!   and leaves the workflow list; its run stays on the record.
//! - The owner then asks for the same work every Monday at 8: the finished
//!   run's work is saved and scheduled through the one create path.

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

/// The run id a finished temporary workflow reported, from its notification.
fn reported_run(messages: &[Value]) -> Option<String> {
    messages.iter().filter_map(|m| m["content"].as_str()).find_map(|c| {
        let rest = c.split_once("Temporary work you started has ended")?.1;
        let id = rest.split_once("(run ")?.1.split_once(')')?.0;
        Some(id.to_string())
    })
}

/// Say `prompt` to employee `agent_id` in its app conversation.
async fn say(server: &TestServer, agent_id: &str, prompt: &str) {
    let (mut ws, _) = connect_async(format!("ws://127.0.0.1:{}/ws", server.port)).await.expect("ws");
    let msg = json!({"type": "chat", "data": {"session_id": format!("agent:{agent_id}:web"), "prompt": prompt}});
    ws.send(Message::Text(msg.to_string().into())).await.unwrap();
}

/// The employee's workflows, as the app lists them.
async fn workflows(server: &TestServer, agent_id: &str) -> Value {
    server.get(&format!("/agents/{agent_id}/workflows")).await.json::<Value>().await.unwrap()["workflows"].clone()
}

/// Whether a main call's thread says `text`.
fn heard(calls: &Calls, text: &str) -> bool {
    calls.lock().unwrap().iter().any(|(p, b)| {
        main_call(p) && b["messages"].as_array().is_some_and(|ms| ms.iter().any(|m| m["content"].as_str().is_some_and(|c| c.contains(text))))
    })
}

/// E13/E14: the owner's direction becomes a temporary workflow, which runs
/// once, reports and disappears, its run kept; "every Monday at 8" saves
/// the same work on a schedule. Must never: a temporary workflow left in
/// the list after its outcome reached the owner, a run record lost with it,
/// or a weekly workflow rebuilt from scratch.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_temporary_workflow_reports_disappears_and_is_saved_to_run_every_monday() {
    let direction = json!({
        "name": "Budget check",
        "lifetime": "temporary",
        "definition": json!({"activities": [{"id": "budget", "intent": "MARK-BUDGET Find this month's marketing budget in the books."}]}).to_string(),
    });
    let (server, calls) = server_with(Arc::new(move |purpose, messages| {
        if !main_call(purpose) {
            return Reply::Text("ok");
        }
        if fresh(messages, "MARK-DIRECTION") {
            return Reply::Call("create_workflow", direction.clone());
        }
        if fresh(messages, "MARK-WEEKLY") {
            let run = reported_run(messages).unwrap_or_default();
            return Reply::Call(
                "create_workflow",
                json!({"name": "Weekly budget", "from_run": run, "lifetime": "saved",
                       "definition": json!({"trigger": {"type": "schedule", "cron": "0 8 * * MON"}}).to_string()}),
            );
        }
        if messages.iter().any(|m| m["content"].as_str().is_some_and(|c| c.contains("MARK-BUDGET"))) {
            return Reply::Text("BUDGET-RESULT: $1,200 for marketing this month.");
        }
        Reply::Text("Noted.")
    }))
    .await;
    let planner = hire_blank(&server, "Planner").await;
    set_mode(&server, &planner, "full_access").await;
    let store = server.db_store();

    // The owner's direction: the employee makes a temporary workflow, and it
    // starts as it is made.
    say(&server, &planner, "MARK-DIRECTION check the marketing budget and tell me").await;
    let run_id = eventually(60, "the temporary workflow's run", async || {
        store.list_workflow_runs(&format!("agent:{planner}"), 10, 0).ok()?.into_iter().next().map(|r| r.id)
    })
    .await;

    // It ran once, its outcome reached the owner's Inbox and the owner's
    // conversation, and it left the workflow list.
    eventually(60, "the temporary workflow to leave the list", async || {
        workflows(&server, &planner).await.get("budget-check").is_none().then_some(())
    })
    .await;
    let run = store.get_workflow_run(&run_id).unwrap().expect("the run stays on the record");
    assert_eq!(run.status, "completed");
    let user = store.ensure_local_user_id().unwrap();
    let inbox = store.get_notification(&format!("temporary:{run_id}"), &user).unwrap().expect("the outcome is in the Inbox");
    assert!(inbox.body.unwrap_or_default().contains("BUDGET-RESULT"), "the outcome itself");
    assert!(inbox.title.contains("finished"), "{}", inbox.title);
    eventually(60, "the conversation to hear the outcome", async || {
        (heard(&calls, "Temporary work you started has ended") && heard(&calls, "BUDGET-RESULT")).then_some(())
    })
    .await;
    assert!(
        store.temporary_work(db::TemporaryKind::Workflow, &planner, "budget-check").unwrap().is_none(),
        "nothing temporary is left"
    );

    // "Give me a report each week on Monday at 8:00 am": the same work,
    // saved and scheduled through the one create path.
    say(&server, &planner, "MARK-WEEKLY give me a report each week on monday at 8:00 am").await;
    let weekly = eventually(60, "the weekly workflow", async || workflows(&server, &planner).await.get("weekly-budget").cloned()).await;
    assert_eq!(weekly["temporary"], false, "saved, not temporary: {weekly}");
    assert_eq!(weekly["trigger"]["type"], "schedule", "{weekly}");
    assert!(weekly["trigger"]["cron"].as_str().unwrap_or("").contains("MON"), "{weekly}");
    assert!(weekly["activities"].to_string().contains("MARK-BUDGET"), "the same work, not rebuilt: {weekly}");
    assert!(weekly["isActive"].as_bool().unwrap_or(false), "{weekly}");
    let job = store
        .list_cron_jobs(100, 0)
        .unwrap()
        .into_iter()
        .find(|j| j.name == format!("agent-{planner}-weekly-budget"))
        .expect("the Monday schedule is registered");
    assert_eq!(job.enabled, Some(1), "the schedule is on");
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
            return Reply::Call(
                "create_team",
                json!({"name": "Budget team", "members": ["Marketer"], "lead": "Bookkeeper", "lifetime": "temporary"}),
            );
        }
        if just_heard(messages, "Temporary team \"Budget team\" exists") {
            return Reply::Call(
                "assign_task",
                json!({"to": "Budget team", "subject": "MARK-TEAMWORK Find the marketing budget and what it buys.", "done_means": "A number and a plan"}),
            );
        }
        if just_heard(messages, "Assigned to the Budget team") {
            return Reply::Call(
                "assign_task",
                json!({"to": "Budget team", "subject": "MARK-SECOND another job"}),
            );
        }
        if messages.iter().any(|m| m["content"].as_str().is_some_and(|c| c.contains("MARK-TEAMWORK"))) && !messages.iter().any(|m| m["content"].as_str().is_some_and(|c| c.contains("MARK-SECOND"))) {
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

    // One piece of work: the second assignment is refused.
    eventually(60, "the second assignment to be refused", async || {
        heard(&calls, "already has its one piece of work").then_some(())
    })
    .await;

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
