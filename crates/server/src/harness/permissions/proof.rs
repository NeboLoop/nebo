//! The ask, proven through the real server: an unattended run parks only
//! the step that asks, the one card reaches the Inbox, the phone's answer
//! resumes that step alone, and an ask nobody answers expires as a No.
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

/// An ask nobody answers: past the 72-hour window the sweep expires it as a
/// No, never as a yes. The parked call never runs, the employee is told it
/// was declined, and its next try at the same call is a plain refusal to
/// plan around, not a repeat ask or a silent retry.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ask_expires_as_no() {
    let nebo = session().await;
    let agent = format!("ex-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let key = format!("agent:{agent}:heartbeat");
    let (names, ran) = heartbeat_steps(&nebo, &agent.replace('-', "_")).await;
    let ctx = heartbeat(&key);
    let parked = nebo.tool(&ctx, &names[1], json!({ "to": "+15550177" })).await;
    let ask_id = parked.parked_ask.expect("parked");
    let asks = &nebo.state.permission_asks;
    let created = asks.get(&ask_id).unwrap().unwrap().created_at;
    let window = agent::harness::permissions::ask::EXPIRES_AFTER_SECS;
    assert_eq!(window, 72 * 3600, "the default window");

    // Not a minute early; then exactly once.
    asks.expire_due(created + window - 60);
    assert_eq!(nebo.get_ok(&format!("/permissions/asks/{ask_id}")).await["status"], "open");
    asks.expire_due(created + window + 1);
    asks.expire_due(created + window + 120);
    let card = nebo.get_ok(&format!("/permissions/asks/{ask_id}")).await;
    assert_eq!(card["status"], "expired", "{card}");
    let row = nebo.store().get_permission_ask(&ask_id).unwrap().unwrap();
    assert_eq!((row.status.as_str(), row.answer.as_deref()), ("expired", Some("no")));
    assert_eq!(count(&ran)[1], 0, "the parked call never ran");
    let own = nebo.store().permission_rules_in(&types::permissions::Scope::Employee(agent.clone())).unwrap();
    assert!(own.is_empty(), "nothing was granted: {own:?}");

    // The employee is told, once, that it counts as declined.
    let expired: Vec<_> = told(&nebo, &key).into_iter().filter(|t| t.contains(&ask_id)).collect();
    assert_eq!(expired.len(), 1, "{expired:?}");
    assert!(expired[0].contains("no answer in 72 hours, so it counts as declined\nIt did not run."), "{}", expired[0]);

    // Its next step at the same call: a plain refusal, never a repeat ask.
    let again = nebo.tool(&ctx, &names[1], json!({ "to": "+15550177" })).await;
    assert!(again.is_error && again.parked_ask.is_none(), "{}", again.content);
    assert!(again.content.starts_with("The owner already said no to this"), "{}", again.content);
    assert_eq!(count(&ran)[1], 0);

    // An answer after expiry takes nothing back.
    let late = nebo
        .post_ok(&format!("/permissions/asks/{ask_id}/answer"), &json!({ "answer": "this_once", "via": "mobile" }))
        .await;
    assert_eq!(late["status"], "expired");
    assert_eq!(count(&ran)[1], 0);
}
