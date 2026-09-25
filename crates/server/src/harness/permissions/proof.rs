//! The ask, proven through the real server: an unattended run parks only
//! the step that asks, the one card reaches the Inbox, the phone's answer
//! resumes that step alone, and an ask nobody answers never expires: it
//! comes back to the owner as a reminder and waits.
//!
//! The asking step here is outside the employee's job (§2.12.4 case 4),
//! decided by code today; every surfaced case parks through the same ask.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use tools::registry::DynTool;
use tools::{Origin, ToolContext, ToolResult};
use types::permissions::Door;

use crate::staffed_proof::{Nebo, session};

/// A tool that counts the calls that ran.
struct Probe {
    name: String,
    capability: Option<&'static str>,
    ran: Arc<AtomicUsize>,
}

impl DynTool for Probe {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> String {
        String::new()
    }
    fn schema(&self) -> Value {
        json!({ "type": "object" })
    }
    fn capability(&self, _input: &Value) -> Option<&'static str> {
        self.capability
    }
    fn activity(&self, input: &Value) -> String {
        match input["to"].as_str() {
            Some(to) => format!("texting {to}"),
            None => format!("running {}", self.name),
        }
    }
    fn execute_dyn<'a>(
        &'a self,
        _ctx: &'a ToolContext,
        _input: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        self.ran.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { ToolResult::ok("DONE") })
    }
}

/// Three steps of a heartbeat, registered on the server's own registry
/// under names unique to this scenario: `[read, text, tally]`.
async fn heartbeat_steps(nebo: &Nebo, tag: &str) -> ([String; 3], [Arc<AtomicUsize>; 3]) {
    let names = [format!("{tag}_read"), format!("{tag}_text"), format!("{tag}_tally")];
    let ran = [Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0))];
    for (i, capability) in [None, Some("sms"), None].into_iter().enumerate() {
        nebo.state
            .tools
            .register(Box::new(Probe { name: names[i].clone(), capability, ran: ran[i].clone() }))
            .await;
    }
    (names, ran)
}

/// A heartbeat run of `agent`.
fn heartbeat(key: &str) -> ToolContext {
    let mut ctx = ToolContext::new(Origin::System).with_session(key, "heartbeat");
    ctx.door = Door::Heartbeat;
    ctx
}

fn count(ran: &[Arc<AtomicUsize>; 3]) -> [usize; 3] {
    [0, 1, 2].map(|i| ran[i].load(Ordering::SeqCst))
}

/// The notification rows the wake rail took for `key`.
fn told(nebo: &Nebo, key: &str) -> Vec<String> {
    nebo.store()
        .engine_events_for("session", key, 20)
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == agent::harness::delegation::notify::WAKE_KIND)
        .map(|e| e.payload)
        .collect()
}

/// A heartbeat with three independent steps, one outside the employee's
/// job: the other two finish in that same tick, only that step parks as one
/// ask, and the one card reaches the Inbox. The owner answers from the
/// phone: the parked step resumes and completes, the finished steps don't
/// run again, and the employee hears the result as a notification. A later
/// answer from the desktop changes nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn heartbeat_ask_parks_only_that_step() {
    let nebo = session().await;
    let agent = format!("hb-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let key = format!("agent:{agent}:heartbeat");
    let (names, ran) = heartbeat_steps(&nebo, &agent.replace('-', "_")).await;
    let ctx = heartbeat(&key);

    // One tick: every step runs but the one that asks.
    let (read, text, tally) = tokio::join!(
        nebo.tool(&ctx, &names[0], json!({})),
        nebo.tool(&ctx, &names[1], json!({ "to": "+15550142" })),
        nebo.tool(&ctx, &names[2], json!({})),
    );
    assert_eq!((read.content.as_str(), tally.content.as_str()), ("DONE", "DONE"));
    let ask_id = text.parked_ask.clone().expect("the step outside its job parks");
    assert!(text.content.contains("Carry on with anything else"), "{}", text.content);
    assert_eq!(count(&ran), [1, 0, 1], "only the asking step waits");

    // One ask, open, for that step; its card is in the Inbox.
    let open = nebo.get_ok(&format!("/permissions/asks?session={key}")).await;
    let asks = open["asks"].as_array().expect("asks");
    assert_eq!(asks.len(), 1, "{open}");
    let card = &asks[0];
    assert_eq!((card["id"].as_str(), card["status"].as_str()), (Some(ask_id.as_str()), Some("open")));
    assert_eq!(card["sentence"], "texting +15550142");
    assert_eq!(card["allowAlways"], true);
    let user = nebo.store().ensure_local_user_id().unwrap();
    let inbox = nebo
        .store()
        .get_notification(&format!("permission-ask:{ask_id}"), &user)
        .unwrap()
        .expect("the card is in the Inbox");
    assert_eq!(inbox.notification_type, "permission_ask");
    assert!(inbox.title.ends_with(" wants your OK"), "{}", inbox.title);
    assert_eq!(inbox.body.as_deref(), Some("Texting +15550142. It's outside this employee's job."));

    // The owner answers from the phone: that step resumes and completes.
    let answered = nebo
        .post_ok(&format!("/permissions/asks/{ask_id}/answer"), &json!({ "answer": "this_once", "via": "mobile" }))
        .await;
    assert_eq!(answered["status"], "allowed");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while count(&ran)[1] == 0 {
        assert!(tokio::time::Instant::now() < deadline, "the parked step never resumed");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(count(&ran), [1, 1, 1], "the finished steps did not run again");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if told(&nebo, &key).iter().any(|t| t.contains("allowed, this once\nIt ran:\nDONE")) {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "the employee was never told: {:?}", told(&nebo, &key));
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let read_at = nebo.store().get_notification(&format!("permission-ask:{ask_id}"), &user).unwrap().unwrap().read_at;
    assert!(read_at.is_some(), "answered anywhere clears it everywhere");

    // The first answer won: a later one from the desktop changes nothing.
    let late = nebo
        .post_ok(&format!("/permissions/asks/{ask_id}/answer"), &json!({ "answer": "no", "via": "inbox" }))
        .await;
    assert_eq!((late["status"].as_str(), late["answer"].as_str()), (Some("allowed"), Some("this_once")));
    assert_eq!(count(&ran), [1, 1, 1]);
    assert!(nebo.get_ok(&format!("/permissions/asks?session={key}")).await["asks"].as_array().unwrap().is_empty());
}

/// An ask nobody answers never expires and never counts as a No. When its
/// wait's timer wakes it, the card comes back to the top of the owner's
/// Inbox, unread, and the ask waits again for the next reminder. The owner
/// answers days later: the server's own engine loop hears the answer, the
/// parked call runs once and the employee is told.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unanswered_ask_never_expires_and_is_reminded() {
    let nebo = session().await;
    let agent = format!("rm-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let key = format!("agent:{agent}:heartbeat");
    let (names, ran) = heartbeat_steps(&nebo, &agent.replace('-', "_")).await;
    let ctx = heartbeat(&key);
    let parked = nebo.tool(&ctx, &names[1], json!({ "to": "+15550177" })).await;
    let ask_id = parked.parked_ask.expect("parked");
    let asks = &nebo.state.permission_asks;
    let created = asks.get(&ask_id).unwrap().unwrap().created_at;
    let user = nebo.store().ensure_local_user_id().unwrap();
    let inbox_id = format!("permission-ask:{ask_id}");

    // The owner saw the card and left it.
    nebo.store().mark_notification_read(&inbox_id, &user).unwrap();

    // The ask's wait: on the answer, its timer the first reminder a day out.
    let run = nebo.store().engine_get_run(&ask_id).unwrap().expect("the ask waits in the engine");
    let wait = nebo.store().engine_get_wait(run.current_wait_id.unwrap()).unwrap().unwrap();
    assert_eq!(wait.deadline, Some(created + 24 * 3600));

    // Four days on, the reminders have come due twice: each brings the card
    // back unread, and the ask stays open.
    for day in [1, 3] {
        asks.resume(&nebo.state.tools, &ask_id, created + day * 24 * 3600 + 60).unwrap();
        let row = nebo.store().get_notification(&inbox_id, &user).unwrap().expect("the card is in the Inbox");
        assert!(row.read_at.is_none(), "the reminder brings it back unread");
        nebo.store().mark_notification_read(&inbox_id, &user).unwrap();
    }
    let card = nebo.get_ok(&format!("/permissions/asks/{ask_id}")).await;
    assert_eq!(card["status"], "open", "{card}");
    assert!(card.get("expiresAt").is_none(), "an ask has no expiry: {card}");
    let row = nebo.store().get_permission_ask(&ask_id).unwrap().unwrap();
    assert_eq!((row.status.as_str(), row.answer.as_deref()), ("open", None));
    assert_eq!(count(&ran)[1], 0, "the parked call never ran on its own");
    assert!(told(&nebo, &key).iter().all(|t| !t.contains(&ask_id)), "nobody told the employee no");
    let own = nebo.store().permission_rules_in(&types::permissions::Scope::Employee(agent.clone())).unwrap();
    assert!(own.is_empty(), "nothing was granted: {own:?}");

    // The owner answers: the server's engine loop wakes the ask and the
    // call runs, once.
    let answered = nebo
        .post_ok(&format!("/permissions/asks/{ask_id}/answer"), &json!({ "answer": "this_once", "via": "mobile" }))
        .await;
    assert_eq!(answered["status"], "allowed");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        if told(&nebo, &key).iter().any(|t| t.contains(&ask_id) && t.contains("allowed, this once\nIt ran:\nDONE")) {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "the answer never resumed the ask: {:?}", told(&nebo, &key));
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(count(&ran)[1], 1);
    assert_eq!(nebo.store().engine_get_run(&ask_id).unwrap().unwrap().state, "done");
}
