//! The ask, proven through the real server: an unattended run parks only
//! the step that asks, the one card reaches the Inbox, the phone's answer
//! resumes that step alone, and an ask nobody answers that no workflow run
//! is parked on never expires: it comes back to the owner as a reminder and
//! waits. (A parked workflow run's ask ends the run after a day.)
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
    fn describe(&self, input: &Value) -> String {
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

/// An ask nobody answers (a heartbeat's: no workflow run is parked on it)
/// never expires and never counts as a No. When its
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

/// On the owner's own call the employee asks aloud and his spoken yes is the
/// answer (live 2026-09-28: told nothing about where to answer, the employee
/// sent the owner, on his phone, to "the desktop app", then to support).
/// The parked step tells the model to ask him now; the call is told the
/// ask with its real id. An id the voice model made up answers nothing and
/// names the real one (live 2026-09-29: "0", four times). His answer counts
/// only after he spoke; his words ("yes") are read as one of the card's
/// answers by the one decision, then go through the ask's one answer path
/// (answered via voice), the server's engine resumes the parked call once,
/// and the employee hears it ran. A client can't claim a spoken answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn owners_call_ask_is_asked_aloud_and_answered_by_voice() {
    let nebo = session().await;
    let agent = format!("vc-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let key = format!("agent:{agent}:thread:{}", uuid::Uuid::new_v4());
    let (names, ran) = heartbeat_steps(&nebo, &agent.replace('-', "_")).await;
    let mut ctx = ToolContext::new(Origin::User).with_session(&key, "voice");
    ctx.door = Door::Voice;
    let asks = &nebo.state.permission_asks;
    let state = &nebo.state;
    let (jev, _) = crate::handlers::asks::tests::table_jev(&[("yes", "option_2")]).await;

    // The step parks, and the model is told to ask the owner now, by voice.
    let parked = nebo.tool(&ctx, &names[1], json!({ "to": "+15550142" })).await;
    let ask_id = parked.parked_ask.clone().expect("the step parks on the owner");
    assert!(parked.content.contains("ask them now, in one short spoken question"), "{}", parked.content);
    assert!(parked.content.contains("Their spoken yes or no on this call is the answer"), "{}", parked.content);
    assert!(!parked.content.to_lowercase().contains("desktop"), "{}", parked.content);
    let waiting = crate::handlers::asks::waiting(state, Some(&key)).await;
    let w = waiting.iter().find(|w| w.card.id == ask_id).expect("the ask waits on the owner");
    assert_eq!(w.card.kind, "permission");
    assert_eq!(w.card.options, ["Allow always", "This once", "No"]);
    assert_eq!(w.card.values, ["allow_always", "this_once", "no"], "what a tap on each sends");
    let on_call = crate::handlers::asks::on_call(&w.card);
    assert!(on_call.contains(&format!("ask_id \"{ask_id}\"")), "the call hears the id: {on_call}");
    let created = w.card.created_at;
    let yes = json!({ "ask_id": ask_id, "answer": "yes" });

    // An id the voice model made up answers nothing, and the voice model
    // hears the real one.
    let invented = json!({ "ask_id": "0", "answer": "yes" });
    let (ok, refused) = crate::handlers::voice::answer_ask_on_call(state, Some(&jev), &key, &invented, "yes", created + 5).await;
    assert!(!ok && refused.starts_with("No ask 0 is waiting"), "{refused}");
    assert!(refused.contains(&format!("ask {ask_id}:")), "the real id is named: {refused}");
    assert_eq!(asks.get(&ask_id).unwrap().unwrap().status, agent::harness::permissions::AskStatus::Open);
    // Not yet: the owner hasn't spoken since it was asked.
    let (ok, early) = crate::handlers::voice::answer_ask_on_call(state, Some(&jev), &key, &yes, "yes", created).await;
    assert!(!ok && early.contains("hasn't answered since this was asked"), "{early}");
    // Not a client claiming a spoken answer.
    let (status, _) = nebo
        .post(&format!("/permissions/asks/{ask_id}/answer"), &json!({ "answer": "this_once", "via": "voice" }))
        .await;
    assert!(status >= 400, "a client can't claim a spoken answer: {status}");
    assert_eq!(asks.get(&ask_id).unwrap().unwrap().status, agent::harness::permissions::AskStatus::Open);
    assert_eq!(count(&ran)[1], 0);

    // He says yes: read as "This once", answered via voice, and the parked
    // call runs once.
    let (ok, heard) = crate::handlers::voice::answer_ask_on_call(state, Some(&jev), &key, &yes, "yes", created + 2).await;
    assert!(ok && heard.starts_with("Answered \"This once\""), "{heard}");
    let row = nebo.store().get_permission_ask(&ask_id).unwrap().unwrap();
    assert_eq!((row.answer.as_deref(), row.answered_via.as_deref()), (Some("this_once"), Some("voice")));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        if told(&nebo, &key).iter().any(|t| t.contains(&ask_id) && t.contains("allowed, this once\nIt ran:\nDONE")) {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "the answer never resumed the ask: {:?}", told(&nebo, &key));
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(count(&ran)[1], 1, "the parked call ran once");
    let (ok, again) = crate::handlers::voice::answer_ask_on_call(state, Some(&jev), &key, &yes, "yes", created + 9).await;
    assert!(!ok && again.contains(&format!("No ask {ask_id} is waiting")), "{again}");
    assert_eq!(count(&ran)[1], 1);
}

/// A send that was attempted and whose outcome never came back (a held
/// ledger row) reaches a final state: the engine raises one card asking the
/// owner whether it went out, in the conversation that sent it; it takes
/// only "it went out" or "it didn't go out", and his answer settles the row
/// once. Live 2026-09-28: three held rows sat pending for a day with no way
/// to close them, logged every five seconds.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_held_send_is_settled_by_the_owners_answer() {
    let nebo = session().await;
    let agent = format!("hs-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let key = format!("agent:{agent}:thread:{}", uuid::Uuid::new_v4());
    let store = nebo.store();
    let asks = &nebo.state.permission_asks;
    let held = |to: &str| {
        let id = store
            .engine_effect_pending(&key, "messaging", &format!("send:{key}:mail.message.send:{to}"), "gmail", "", to)
            .unwrap();
        store.engine_effect_attempted(id).unwrap();
        store.engine_effect_held(id, "the plugin timed out").unwrap();
        id
    };
    let (went, didnt) = (held("pat@example.com"), held("sam@example.com"));

    // One card per held row, however often the engine looks.
    crate::engine::settle_held_sends(store, asks);
    crate::engine::settle_held_sends(store, asks);
    let open = nebo.get_ok(&format!("/permissions/asks?session={key}")).await;
    let cards = open["asks"].as_array().expect("asks").clone();
    assert_eq!(cards.len(), 2, "{open}");
    let card_for = |effect: i64| {
        let id = agent::harness::permissions::send_check_id(effect);
        cards.iter().find(|c| c["id"] == id.as_str()).cloned().unwrap_or_else(|| panic!("no card for {effect}: {open}"))
    };
    let card = card_for(went);
    assert_eq!(card["kind"], "send_check");
    assert_eq!(card["sessionKey"], key.as_str(), "in the conversation that sent it");
    assert!(card["sentence"].as_str().unwrap().starts_with("an email to pat@example.com through gmail"), "{card}");
    assert_eq!((card["allowAlways"].as_bool(), card["thisOnce"].as_bool()), (Some(false), Some(false)));

    // A permission's answer doesn't fit the question.
    let (status, _) = nebo
        .post(&format!("/permissions/asks/{}/answer", card["id"].as_str().unwrap()), &json!({ "answer": "this_once", "via": "chat" }))
        .await;
    assert!(status >= 400, "{status}");

    // It went out: completed, and the person is one the employee works with.
    // It didn't: failed, so it may be sent again.
    for (effect, answer) in [(went, "sent"), (didnt, "not_sent")] {
        let answered = nebo
            .post_ok(&format!("/permissions/asks/{}/answer", card_for(effect)["id"].as_str().unwrap()), &json!({ "answer": answer, "via": "chat" }))
            .await;
        assert_eq!((answered["status"].as_str(), answered["answer"].as_str()), (Some("answered"), Some(answer)), "{answered}");
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let state = |id: i64| store.engine_get_effect(id).unwrap().unwrap().state;
    while state(went) == "pending" || state(didnt) == "pending" {
        assert!(tokio::time::Instant::now() < deadline, "the answers never settled the rows");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!((state(went).as_str(), state(didnt).as_str()), ("completed", "failed"));
    assert!(store.known_counterparties(&agent, &["pat@example.com".to_string()]).unwrap().contains("pat@example.com"));
    crate::engine::settle_held_sends(store, asks);
    assert!(nebo.get_ok(&format!("/permissions/asks?session={key}")).await["asks"].as_array().unwrap().is_empty(), "settled rows get no new card");
}

/// Not a proof when the harness runs it: the coding agent
/// [`a_linked_agents_ask_is_answered_once_in_its_own_turn`] hires (this test
/// binary again, with `NEBO_PROOF_CODER` naming the file it writes what it
/// was told to). Every message, it runs `git status`: in its bypass mode
/// without asking, in any other after asking its own permission.
#[test]
fn fake_coding_agent() {
    use std::io::{BufRead, Write};
    let Ok(told) = std::env::var("NEBO_PROOF_CODER") else {
        return;
    };
    let note = |line: Value| {
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&told).unwrap();
        writeln!(file, "{line}").unwrap();
    };
    let send = |frame: Value| {
        let mut out = std::io::stdout().lock();
        writeln!(out, "{frame}").unwrap();
        out.flush().unwrap();
    };
    let update = |update: Value| {
        send(json!({ "jsonrpc": "2.0", "method": "session/update", "params": { "sessionId": "s-1", "update": update } }));
    };
    // What the call and the turn did once it may run (or may not).
    let finish = |call: &str, prompt: &Value, ran: bool| {
        let (status, text, said) = if ran {
            ("completed", "nothing to commit", "Ran it.")
        } else {
            ("failed", "not allowed", "Declined.")
        };
        update(json!({ "sessionUpdate": "tool_call_update", "toolCallId": call, "status": status,
            "content": [{ "type": "content", "content": { "type": "text", "text": text } }] }));
        update(json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": said } }));
        send(json!({ "jsonrpc": "2.0", "id": prompt, "result": { "stopReason": "end_turn" } }));
    };
    // The harness printed "test ... " without a newline: end that line.
    send(Value::Null);
    let (mut prompt_id, mut mode, mut prompts) = (Value::Null, "default".to_owned(), 0);
    for line in std::io::stdin().lock().lines() {
        let Ok(message) = serde_json::from_str::<Value>(&line.unwrap()) else {
            continue;
        };
        let id = message["id"].clone();
        let reply = |result: Value| send(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
        let call = format!("call_{prompts}");
        match message["method"].as_str() {
            Some("initialize") => reply(json!({ "protocolVersion": 1, "agentCapabilities": { "loadSession": false },
                "agentInfo": { "name": "proof-coder" } })),
            Some("session/new") => reply(json!({ "sessionId": "s-1", "modes": { "currentModeId": "default", "availableModes": [
                { "id": "default", "name": "Default", "_meta": { "kind": "standard" } },
                { "id": "acceptEdits", "name": "Accept edits", "_meta": { "kind": "standard" } },
                { "id": "bypassPermissions", "name": "Bypass permissions", "_meta": { "kind": "full_access" } },
            ] } })),
            Some("session/set_mode") => {
                mode = message["params"]["modeId"].as_str().unwrap_or_default().to_owned();
                note(json!({ "mode": mode }));
                reply(json!({}));
            }
            Some("session/prompt") => {
                // The owner's message: the prompt's last block (a fresh
                // session's first prompt starts with its briefing).
                let owners = message["params"]["prompt"].as_array().and_then(|p| p.last()).map_or(Value::Null, |b| b["text"].clone());
                note(json!({ "prompt": owners }));
                prompts += 1;
                prompt_id = id.clone();
                let call = format!("call_{prompts}");
                update(json!({ "sessionUpdate": "tool_call", "toolCallId": call, "title": "git status", "kind": "execute",
                    "status": "pending", "rawInput": { "command": "git status" } }));
                if mode == "bypassPermissions" {
                    finish(&call, &prompt_id, true);
                } else {
                    send(json!({ "jsonrpc": "2.0", "id": 900 + prompts, "method": "session/request_permission", "params": {
                        "sessionId": "s-1",
                        "toolCall": { "toolCallId": call, "title": "git status", "kind": "execute", "status": "pending",
                            "rawInput": { "command": "git status" } },
                        "options": [
                            { "optionId": "once", "name": "Allow", "kind": "allow_once" },
                            { "optionId": "always", "name": "Always", "kind": "allow_always" },
                            { "optionId": "deny", "name": "Reject", "kind": "reject_once" },
                        ],
                    } }));
                }
            }
            None if id == json!(900 + prompts) => {
                let chosen = message["result"]["outcome"]["optionId"].as_str().unwrap_or_default().to_owned();
                note(json!({ "answer": chosen }));
                finish(&call, &prompt_id, chosen != "deny");
            }
            _ => {}
        }
    }
}

/// Titles, recaps and memory: answered, and never by the linked agent.
struct Quiet;

#[async_trait::async_trait]
impl ai::Provider for Quiet {
    fn id(&self) -> &str {
        "proof-quiet"
    }

    async fn stream(&self, _req: &ai::ChatRequest) -> Result<ai::EventReceiver, ai::ProviderError> {
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        tokio::spawn(async move {
            let _ = tx.send(ai::StreamEvent::text("ok")).await;
            let _ = tx.send(ai::StreamEvent::done()).await;
        });
        Ok(rx)
    }
}

/// What the fake coding agent was told, in order.
fn coder_told(path: &std::path::Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// The assistant rows of a conversation.
fn replies(nebo: &Nebo, chat: &str) -> Vec<String> {
    nebo.store()
        .get_chat_messages(chat)
        .unwrap()
        .into_iter()
        .filter(|m| m.role == "assistant")
        .map(|m| m.content)
        .collect()
}

async fn until(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    while !cond() {
        assert!(tokio::time::Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Live 2026-09-28: on his call the owner's linked Claude Code asked to run
/// a command; the voice model was handed the ask's sentence, not its id, its
/// answer went nowhere, it sent the task again, and the agent asked again.
/// A linked coding agent's own permission request is one card with every
/// option it offered, answered once by the owner anywhere: out loud on his
/// call (by the ask's id, after he spoke, in that conversation only) or on
/// the card from another device. The option of the answer's kind goes back
/// to the agent and the same turn goes on; a second answer changes nothing.
/// In Full Access the agent runs in its own no-prompt mode and nothing asks.
/// Through the real server, the real linked provider and host, and a coding
/// agent process; no model.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_linked_agents_ask_is_answered_once_in_its_own_turn() {
    const BOT: &str = "5e1f0000-0000-4000-8000-0000000c0de5";
    let nebo = session().await;
    let root = tempfile::tempdir().unwrap();
    let told_path = root.path().join("told.jsonl");
    let local = ai::LocalHost::open(
        Arc::new(|| Some(BOT.to_owned())),
        root.path().join("link"),
        root.path().join("home"),
        Some(root.path().join("nebo-link")),
    )
    .unwrap();
    let coder = nebo_runtimes::RuntimeCommand {
        program: std::env::current_exe().unwrap().to_string_lossy().into_owned(),
        args: ["harness::permissions::proof::fake_coding_agent", "--exact", "--nocapture", "--test-threads=1"]
            .map(String::from)
            .to_vec(),
        env: vec![("NEBO_PROOF_CODER".into(), told_path.to_string_lossy().into_owned())],
    };
    let hosted = local.host(nebo_runtimes::acp::Agent::ClaudeCode, coder).await.unwrap();
    let relay = ai::Relay::Hub { api_url: "http://127.0.0.1:9".into(), token: Arc::new(|| None) };
    let linked = ai::LinkedProvider::new(relay, nebo.store().clone(), Some(local.clone()), "Nebo proof", 0);
    nebo.state
        .harness
        .reload_providers(vec![Arc::new(Quiet) as Arc<dyn ai::Provider>, Arc::new(linked)])
        .await;

    let agent = format!("lc-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    nebo.store()
        .create_agent(&agent, Some("linked"), "Proof Coder", "", "---\nname: Proof Coder\n---\n", "{}", None, None)
        .unwrap();
    nebo.store()
        .upsert_entity_config("agent", &agent, &json!({ "modelPreference": ai::LinkedProvider::model_id(BOT, &hosted.id) }))
        .unwrap();
    let employee = types::permissions::Scope::Employee(agent.clone());
    nebo.store().set_permission_mode(&employee, types::permissions::Mode::Ask).unwrap();
    let chat = uuid::Uuid::new_v4().to_string();
    nebo.store().create_chat(&chat, "Proof").unwrap();
    let key = format!("agent:{agent}:thread:{chat}");
    let state = &nebo.state;
    let idle = || !state.harness.is_session_busy(&key);

    // On his call: the run parks on the agent's ask and the voice model
    // hears it with its id and every option.
    let spoken = crate::handlers::voice::run_delegated_task(state, &key, "check the repo", "Check the repo.", None, None).await;
    assert_eq!(spoken.told, ["call_1"], "the call is told it, once");
    let spoken = spoken.text;
    assert!(spoken.contains("The answers it takes: Allow once, Always allow, or Deny."), "{spoken}");
    assert!(spoken.contains("ask_id \"call_1\""), "the voice model hears the ask's id: {spoken}");
    let card = state.run_registry.pending_ask_for_session(&key).await.expect("the card is on the run");
    let widget = &card.widgets.as_ref().unwrap()[0];
    assert_eq!(widget["options"], json!(["Allow once", "Always allow", "Deny"]), "every option the agent offered");

    // Not before he spoke, not by its sentence, not words that are no answer.
    let (jev, _) = crate::handlers::asks::tests::table_jev(&[("always, don't ask me again", "option_2")]).await;
    let say = |id: &str, answer: &str| json!({ "ask_id": id, "answer": answer });
    let voice = |input: Value, heard: &'static str, at: i64| {
        let (jev, key) = (&jev, &key);
        async move { crate::handlers::voice::answer_ask_on_call(state, Some(jev), key, &input, heard, at).await }
    };
    let (_, early) = voice(say("call_1", "always"), "always, don't ask me again", card.created_at).await;
    assert!(early.contains("hasn't answered since this was asked"), "{early}");
    let (_, by_sentence) = voice(say("git status The answers it takes: Allow once", "always"), "always", card.created_at + 2).await;
    assert!(by_sentence.contains("ask call_1:"), "it is told the id to use: {by_sentence}");
    let (ok, new_task) = voice(say("call_1", "what's the weather"), "what's the weather", card.created_at + 2).await;
    assert!(!ok && new_task.contains("isn't an answer"), "{new_task}");
    assert!(coder_told(&told_path).iter().all(|t| t.get("answer").is_none()), "nothing answered the agent yet");

    // He says "always": the agent gets its always option and the same turn
    // goes on to its end.
    let mut hub = state.hub.subscribe();
    let (ok, heard) = voice(say("call_1", "always"), "always, don't ask me again", card.created_at + 2).await;
    assert!(ok && heard.starts_with("Answered \"Always allow\""), "{heard}");
    until("the turn goes on to its end", || replies(&nebo, &chat).iter().any(|r| r.contains("Ran it."))).await;
    let settled = loop {
        let e = hub.recv().await.unwrap();
        if e.event_type == "ask_answered" {
            break e;
        }
    };
    assert_eq!((settled.payload["request_id"].as_str(), settled.payload["value"].as_str()), (Some("call_1"), Some("allow_always")));
    assert!(!crate::chat_dispatch::answer_ask(state, "call_1", "Deny".into()).await, "a second answer changes nothing");
    let (ok, again) = voice(say("call_1", "no"), "no", card.created_at + 9).await;
    assert!(!ok && again.contains("No ask call_1 is waiting"), "{again}");

    until("the turn is over", idle).await;

    // Asked again next message, the card is answered from another device
    // (the phone's tap is its label): the agent gets that option, once.
    let spoken = crate::handlers::voice::run_delegated_task(state, &key, "and the other repo", "And the other repo.", None, None).await.text;
    assert!(spoken.contains("ask_id \"call_2\""), "{spoken}");
    assert!(crate::chat_dispatch::answer_ask(state, "call_2", "Deny".into()).await);
    assert!(!crate::chat_dispatch::answer_ask(state, "call_2", "Allow once".into()).await, "the first answer wins");
    until("the declined turn ends", || replies(&nebo, &chat).iter().any(|r| r.contains("Declined."))).await;

    until("the turn is over", idle).await;

    // Full Access: the agent's own no-prompt mode, and no ask reaches him.
    nebo.store().set_permission_mode(&employee, types::permissions::Mode::FullAccess).unwrap();
    let spoken = crate::handlers::voice::run_delegated_task(state, &key, "once more", "Once more.", None, None).await.text;
    assert!(spoken.contains("Ran it."), "{spoken}");
    assert!(state.run_registry.pending_ask_for_session(&key).await.is_none());

    assert_eq!(
        coder_told(&told_path),
        vec![
            json!({ "mode": "default" }),
            json!({ "prompt": "check the repo" }),
            json!({ "answer": "always" }),
            json!({ "mode": "default" }),
            json!({ "prompt": "and the other repo" }),
            json!({ "answer": "deny" }),
            json!({ "mode": "bypassPermissions" }),
            json!({ "prompt": "once more" }),
        ],
        "each task sent once after the employee's mode, each ask answered once by its kind"
    );
}

/// Not a proof when the harness runs it: the coding agent
/// [`a_linked_agent_calls_nebos_tools_as_each_turn`] hires (this test binary
/// again, with `NEBO_PROOF_TOOL_AGENT` naming the file it writes what it was
/// given and what Nebo answered to). It takes HTTP MCP servers; every
/// message, it lists `nebo-agent`'s tools and calls one with the header it
/// was given when its session opened, then ends the turn.
#[test]
fn fake_tool_agent() {
    use std::io::{BufRead, Read, Write};
    let Ok(told) = std::env::var("NEBO_PROOF_TOOL_AGENT") else {
        return;
    };
    let note = |line: Value| {
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&told).unwrap();
        writeln!(file, "{line}").unwrap();
    };
    let send = |frame: Value| {
        let mut out = std::io::stdout().lock();
        writeln!(out, "{frame}").unwrap();
        out.flush().unwrap();
    };
    // One JSON-RPC call to the MCP server `server`: its HTTP status and body.
    let mcp = |server: &Value, body: Value| -> (u16, Value) {
        let url = server["url"].as_str().unwrap_or_default();
        let rest = url.strip_prefix("http://").unwrap_or_default();
        let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
        let headers: String = server["headers"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|h| format!("{}: {}\r\n", h["name"].as_str().unwrap_or_default(), h["value"].as_str().unwrap_or_default()))
            .collect();
        let body = body.to_string();
        let Ok(mut stream) = std::net::TcpStream::connect(authority) else {
            return (0, Value::Null);
        };
        let request = format!(
            "POST /{path} HTTP/1.1\r\nHost: {authority}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(request.as_bytes()).unwrap();
        let mut response = String::new();
        let _ = stream.read_to_string(&mut response);
        let status = response.split(' ').nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
        let json = response.split_once("\r\n\r\n").and_then(|(_, b)| serde_json::from_str(b.trim()).ok()).unwrap_or(Value::Null);
        (status, json)
    };
    send(Value::Null);
    let mut server = Value::Null;
    for line in std::io::stdin().lock().lines() {
        let Ok(message) = serde_json::from_str::<Value>(&line.unwrap()) else {
            continue;
        };
        let id = message["id"].clone();
        let reply = |result: Value| send(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
        match message["method"].as_str() {
            Some("initialize") => reply(json!({ "protocolVersion": 1,
                "agentCapabilities": { "loadSession": false, "mcpCapabilities": { "http": true } },
                "agentInfo": { "name": "proof-tool-agent" } })),
            Some("session/new") => {
                server = message["params"]["mcpServers"]
                    .as_array()
                    .and_then(|s| s.iter().find(|s| s["name"] == "nebo-agent"))
                    .cloned()
                    .unwrap_or(Value::Null);
                note(json!({ "given": server }));
                reply(json!({ "sessionId": "s-1" }));
            }
            Some("session/prompt") => {
                let (notified, _) = mcp(&server, json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
                let (listed, tools) = mcp(&server, json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }));
                let count = tools["result"]["tools"].as_array().map_or(0, Vec::len);
                let (called, answer) = mcp(&server, json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                    "params": { "name": "no_such_tool", "arguments": {} } }));
                let text = answer["result"]["content"][0]["text"].as_str().unwrap_or_default().to_owned();
                note(json!({ "notified": notified, "listed": listed, "tools": count, "called": called, "answer": text }));
                send(json!({ "jsonrpc": "2.0", "method": "session/update", "params": { "sessionId": "s-1", "update": {
                    "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": format!("Listed {count} tools.") } } } }));
                reply(json!({ "stopReason": "end_turn" }));
            }
            _ => {}
        }
    }
}

/// Customers asked for it many times: a coding agent Nebo hosts (Claude
/// Code, Codex, ...) acts on Nebo with Nebo's own tools. Its session is
/// given Nebo's `/agent/mcp` as `nebo-agent` once, when it opens, with the
/// chat's one credential; each turn grants that credential the turn's run
/// and the grant ends with the turn. So the agent's tools work on its first
/// turn and on every turn after in the same session, and between turns the
/// credential executes nothing. Through the real server, the real linked
/// provider and host, and a coding agent process; no model.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_linked_agent_calls_nebos_tools_as_each_turn() {
    const BOT: &str = "5e1f0000-0000-4000-8000-0000000c0de6";
    let nebo = session().await;
    let root = tempfile::tempdir().unwrap();
    let told_path = root.path().join("told.jsonl");
    let local = ai::LocalHost::open(
        Arc::new(|| Some(BOT.to_owned())),
        root.path().join("link"),
        root.path().join("home"),
        Some(root.path().join("nebo-link")),
    )
    .unwrap();
    let coder = nebo_runtimes::RuntimeCommand {
        program: std::env::current_exe().unwrap().to_string_lossy().into_owned(),
        args: ["harness::permissions::proof::fake_tool_agent", "--exact", "--nocapture", "--test-threads=1"]
            .map(String::from)
            .to_vec(),
        env: vec![("NEBO_PROOF_TOOL_AGENT".into(), told_path.to_string_lossy().into_owned())],
    };
    let hosted = local.host(nebo_runtimes::acp::Agent::Codex, coder).await.unwrap();
    let relay = ai::Relay::Hub { api_url: "http://127.0.0.1:9".into(), token: Arc::new(|| None) };
    let linked = ai::LinkedProvider::new(relay, nebo.store().clone(), Some(local.clone()), "Nebo proof", nebo.port);
    nebo.state
        .harness
        .reload_providers(vec![Arc::new(Quiet) as Arc<dyn ai::Provider>, Arc::new(linked)])
        .await;

    let agent = format!("lc-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    nebo.store()
        .create_agent(&agent, Some("linked"), "Proof Tools", "", "---\nname: Proof Tools\n---\n", "{}", None, None)
        .unwrap();
    nebo.store()
        .upsert_entity_config("agent", &agent, &json!({ "modelPreference": ai::LinkedProvider::model_id(BOT, &hosted.id) }))
        .unwrap();
    let chat = uuid::Uuid::new_v4().to_string();
    nebo.store().create_chat(&chat, "Proof").unwrap();
    let key = format!("agent:{agent}:thread:{chat}");
    let state = &nebo.state;

    for (turn, prompt) in [(1, "list your tools"), (2, "and again")] {
        let spoken = crate::handlers::voice::run_delegated_task(state, &key, prompt, prompt, None, None).await.text;
        assert!(spoken.contains("tools."), "turn {turn}: {spoken}");
        until("the turn is over", || !state.harness.is_session_busy(&key)).await;
    }

    let told = coder_told(&told_path);
    let given: Vec<&Value> = told.iter().filter_map(|t| t.get("given")).collect();
    assert_eq!(given.len(), 1, "one session, given the server once: {told:?}");
    assert_eq!(given[0]["type"], "http");
    assert_eq!(given[0]["url"], format!("http://127.0.0.1:{}/agent/mcp", nebo.port));
    let header = &given[0]["headers"][0];
    assert_eq!(header["name"], "X-Nebo-Run-Credential");
    let token = header["value"].as_str().expect("the chat's credential");
    let turns: Vec<&Value> = told.iter().filter(|t| t.get("listed").is_some()).collect();
    assert_eq!(turns.len(), 2, "{told:?}");
    for (n, turn) in turns.iter().enumerate() {
        assert_eq!(turn["notified"], 202, "turn {}: a notification is accepted with no reply, as MCP says: {turn}", n + 1);
        assert_eq!(turn["listed"], 200, "turn {}: the credential it was given is live: {turn}", n + 1);
        assert!(turn["tools"].as_u64().unwrap() > 0, "turn {}: Nebo's tools are listed: {turn}", n + 1);
        assert_eq!(turn["called"], 200, "turn {}: {turn}", n + 1);
        let answer = turn["answer"].as_str().unwrap();
        assert!(!answer.is_empty() && !answer.contains("run has ended"), "turn {}: the call ran as the turn: {answer}", n + 1);
    }
    assert!(state.tool_credentials.grant(token).is_none(), "between turns the credential executes nothing");
}

/// The owner's chats and calls while another employee's ask waits:
/// Flip-Flap's chat with a call on it, and his chief of staff's chat with a
/// call on it. Each chat holds one real reply.
struct OwnerElsewhere {
    flip_chat: String,
    flip_key: String,
    flip_call: tokio::sync::mpsc::Receiver<crate::handlers::voice::CallNews>,
    pa_chat: String,
    pa_call: tokio::sync::mpsc::Receiver<crate::handlers::voice::CallNews>,
}

fn owner_elsewhere(nebo: &Nebo) -> OwnerElsewhere {
    let store = nebo.store();
    let chat = |agent: &str, reply: &str| {
        let chat = uuid::Uuid::new_v4().to_string();
        store.create_chat(&chat, "Proof").unwrap();
        store.create_chat_message(&uuid::Uuid::new_v4().to_string(), &chat, "assistant", reply, None).unwrap();
        let key = format!("agent:{agent}:thread:{chat}");
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        nebo.state.live_calls.lock().unwrap().insert(key.clone(), tx);
        (chat, key, rx)
    };
    let (flip_chat, flip_key, flip_call) = chat("flip-flap", "Try \"TAP TO FLY!\" again.");
    let (pa_chat, _, pa_call) = chat("assistant", "Morning. Two things on your plate.");
    OwnerElsewhere { flip_chat, flip_key, flip_call, pa_chat, pa_call }
}

impl OwnerElsewhere {
    /// Every call he is on is told of `ask_id` only as a notice: no id to
    /// answer it with, no answer asked for.
    async fn told_only_of(&mut self, ask_id: &str) {
        for call in [&mut self.flip_call, &mut self.pa_call] {
            let news = tokio::time::timeout(Duration::from_secs(10), call.recv())
                .await
                .expect("the call is told of it")
                .unwrap();
            let crate::handlers::voice::CallNews::Notice(w) = news else { panic!("put to another chat's call: {news:?}") };
            assert_eq!(w.id, ask_id);
            let said = crate::handlers::asks::notice_on_call(&w);
            assert!(!said.contains(ask_id) && !said.contains("answer_ask"), "{said}");
        }
    }

    /// Not a word of it in either chat: each holds only its own reply.
    fn chats_untouched(&self, nebo: &Nebo) {
        for (chat, reply) in [(&self.flip_chat, "Try \"TAP TO FLY!\" again."), (&self.pa_chat, "Morning. Two things on your plate.")] {
            let rows: Vec<String> = nebo.store().get_chat_messages(chat).unwrap().into_iter().map(|m| m.content).collect();
            assert_eq!(rows, [reply], "nothing landed in the chat he is in");
        }
    }
}

/// Live 2026-10-02, 7:12 AM: the owner was talking with Flip-Flap when
/// Bookkeeper's workflow step parked on his OK. The ask came into Flip-Flap's
/// conversation ("Bookkeeper asks: OK to go ahead with …? Allow always, this
/// once, or no?"), he said "No.", and Flip-Flap decided it: "Told Bookkeeper
/// no." Now the ask blocks a run, so it reaches him at once — its Inbox item,
/// a push of its own, and on each call only a notice — and it never lands in
/// Flip-Flap's chat or his chief of staff's; Flip-Flap's call can't answer
/// it. His answer from the Inbox card reaches Bookkeeper's run.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_workflow_ask_never_lands_in_the_chat_the_owner_is_in() {
    let nebo = session().await;
    let state = &nebo.state;
    let agent = format!("bk-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let run = format!("run-{}", uuid::Uuid::new_v4().simple());
    let key = format!("agent:{agent}:workflow:{run}:triage::1");
    let (names, _ran) = heartbeat_steps(&nebo, &agent.replace('-', "_")).await;
    let mut ctx = ToolContext::new(Origin::Workflow).with_session(&key, "workflow");
    ctx.door = Door::Workflow;
    let mut owner = owner_elsewhere(&nebo);
    let mut hub = state.hub.subscribe();

    let parked = nebo.tool(&ctx, &names[1], json!({ "to": "+15550142" })).await;
    let ask_id = parked.parked_ask.clone().expect("the step parks on the owner");
    nebo.store().link_permission_ask_run(&ask_id, &run).unwrap();

    // It blocks Bookkeeper's run and no chat raised it.
    let waiting = crate::handlers::asks::waiting(state, None).await;
    let w = waiting.iter().find(|w| w.card.id == ask_id).expect("it waits on the owner");
    assert!(w.card.blocking && w.card.chat_id.is_empty(), "{:?}", w.card);

    // Each call he is on hears only a notice; Flip-Flap's call can't decide it.
    owner.told_only_of(&ask_id).await;
    let (jev, _) = crate::handlers::asks::tests::table_jev(&[("No.", "option_3")]).await;
    let (ok, said) = crate::handlers::voice::answer_ask_on_call(
        state,
        Some(&jev),
        &owner.flip_key,
        &json!({ "ask_id": ask_id, "answer": "No." }),
        "No.",
        w.card.created_at + 5,
    )
    .await;
    assert!(!ok && said.contains("never on this call"), "{said}");
    assert_eq!(nebo.store().get_permission_ask(&ask_id).unwrap().unwrap().status, "open", "Flip-Flap decided nothing");

    // Its Inbox item: who is waiting on him, opening the assistant's chat,
    // where it is relayed. The app's loud signal is its blocking notice, so
    // the row itself is quiet.
    let user = nebo.store().ensure_local_user_id().unwrap();
    let inbox = nebo.store().get_notification(&format!("permission-ask:{ask_id}"), &user).unwrap().expect("the Inbox has it");
    assert!(inbox.title.ends_with(" is waiting on you"), "{}", inbox.title);
    assert_eq!(inbox.action_url.as_deref(), Some("/assistant"));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut quiet = false;
    while tokio::time::Instant::now() < deadline && !quiet {
        let Ok(Ok(e)) = tokio::time::timeout(Duration::from_millis(200), hub.recv()).await else { continue };
        assert!(
            !(e.event_type == "notification" && e.payload["id"] == format!("permission-ask:{ask_id}")),
            "the row is quiet"
        );
        quiet = e.event_type == "notification_created" && e.payload["id"] == format!("permission-ask:{ask_id}");
    }
    assert!(quiet, "the Inbox row was broadcast");
    owner.chats_untouched(&nebo);

    // He answers No on the Inbox card: Bookkeeper's run hears it.
    let answered = nebo
        .post_ok(&format!("/permissions/asks/{ask_id}/answer"), &json!({ "answer": "no", "via": "inbox" }))
        .await;
    assert_eq!(answered["status"], "declined");
    let approval = nebo.get_ok(&format!("/agents/workflow-runs/{run}/approval")).await;
    assert_eq!(approval["status"], "denied", "{approval}");
    owner.chats_untouched(&nebo);
}

/// The same rule for a coworker's run: a question Bookkeeper asks the owner
/// while handling a colleague's request waits in Bookkeeper's own thread
/// with that colleague. Flip-Flap's and the chief of staff's calls are only
/// told of it, neither chat gets a word of it, Flip-Flap's call can't answer
/// it, and the owner's answer reaches Bookkeeper's run.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_coworker_ask_never_lands_in_the_chat_the_owner_is_in() {
    let nebo = session().await;
    let state = &nebo.state;
    let agent = format!("bk-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let key = format!("agent:{agent}:coworker:controller");
    let mut owner = owner_elsewhere(&nebo);
    let handle = state
        .run_registry
        .register(crate::run_registry::RegisterParams {
            session_key: key.clone(),
            entity_id: agent.clone(),
            entity_name: "Bookkeeper".into(),
            origin: "comm".into(),
            channel: "coworker".into(),
            cancel_token: tokio_util::sync::CancellationToken::new(),
            parent_run_id: None,
        })
        .await;
    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    state.ask_channels.lock().await.insert("req-bk".into(), tx);
    let event = ai::StreamEvent::ask_request(
        "req-bk",
        "Post the draft entry to the offset account?",
        Some(json!([{ "type": "options", "options": ["Yes", "No"] }])),
    );
    let ask = crate::chat_dispatch::announce_ask(state, &key, &types::permissions::Door::Coworker { from: "controller".into() }, &event).await;
    assert_eq!(ask.request_id, "req-bk");
    let waiting = crate::handlers::asks::waiting(state, None).await;
    let w = waiting.iter().find(|w| w.card.id == "req-bk").expect("it waits on the owner");
    assert!(w.card.blocking, "a parked question blocks its run");
    assert_eq!(w.card.session_key, key);

    owner.told_only_of("req-bk").await;
    let (jev, _) = crate::handlers::asks::tests::table_jev(&[("Yes.", "option_1")]).await;
    let (ok, said) = crate::handlers::voice::answer_ask_on_call(
        state,
        Some(&jev),
        &owner.flip_key,
        &json!({ "ask_id": "req-bk", "answer": "Yes." }),
        "Yes.",
        w.card.created_at + 5,
    )
    .await;
    assert!(!ok && said.contains("never on this call"), "{said}");
    owner.chats_untouched(&nebo);

    // His answer from the card reaches Bookkeeper's run.
    assert!(crate::chat_dispatch::answer_ask(state, "req-bk", "Yes".into()).await);
    let got = tokio::time::timeout(Duration::from_secs(5), rx).await.expect("the run hears it").unwrap();
    assert_eq!(got, "Yes");
    owner.chats_untouched(&nebo);
    drop(handle);
}

/// Registers a run in `key` for `agent` and parks it on `widgets` through
/// the one announce, via `door`. Returns the run's handle and the receiver
/// its answer reaches.
async fn park_card(
    nebo: &Nebo,
    agent: &str,
    key: &str,
    door: &Door,
    request_id: &str,
    prompt: &str,
    widgets: Value,
) -> (crate::run_registry::RunHandle, tokio::sync::oneshot::Receiver<String>) {
    let state = &nebo.state;
    let handle = state
        .run_registry
        .register(crate::run_registry::RegisterParams {
            session_key: key.to_string(),
            entity_id: agent.to_string(),
            entity_name: agent.to_string(),
            origin: "comm".into(),
            channel: "coworker".into(),
            cancel_token: tokio_util::sync::CancellationToken::new(),
            parent_run_id: None,
        })
        .await;
    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    state.ask_channels.lock().await.insert(request_id.into(), tx);
    let event = ai::StreamEvent::ask_request(request_id, prompt, Some(widgets));
    crate::chat_dispatch::announce_ask(state, key, door, &event).await;
    (handle, rx)
}

/// The owner asked the assistant to have the Bookkeeper reconcile the
/// books; the Bookkeeper's turn (a request passed on through the assistant)
/// parks on QuickBooks' connect card. The card is relayed to the
/// assistant's chat and its notification opens there, and it is the
/// Bookkeeper's own card: the account it connects is the Bookkeeper's,
/// never the assistant's (live 2026-10-05: a QuickBooks card connected
/// Chief instead of the Bookkeeper). The owner's Connect in the assistant's
/// chat answers the Bookkeeper's run on the same path. The assistant's
/// conversation gets not a word of it: it relays, it never answers.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_card_relayed_through_the_assistant_connects_the_asking_employee() {
    let nebo = session().await;
    let state = &nebo.state;
    let bk = format!("bk-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let key = format!("agent:{bk}:coworker:assistant");
    let owner = owner_elsewhere(&nebo);
    let card = json!([{
        "type": "connect_account", "plugin": "books", "agentId": bk,
        "label": "Example Books account", "employee": "Bookkeeper",
    }]);
    let (handle, rx) = park_card(
        &nebo,
        &bk,
        &key,
        &Door::Coworker { from: "assistant".into() },
        "req-connect",
        "I need Bookkeeper's Example Books account connected to continue.",
        card,
    )
    .await;

    let waiting = crate::handlers::asks::waiting(state, None).await;
    let w = &waiting.iter().find(|w| w.card.id == "req-connect").expect("it waits on the owner").card;
    assert!(w.relayed, "work the owner is not in comes to the assistant");
    assert_eq!(w.agent_id, bk, "the employee who asked");
    let widget = &w.widgets.as_ref().expect("the card itself is relayed")[0];
    assert_eq!(widget["type"], "connect_account");
    assert_eq!(widget["agentId"], bk.as_str(), "the card connects the employee who asked, never the assistant");
    assert_eq!(widget["employee"], "Bookkeeper");
    assert_eq!(
        crate::handlers::asks::push_item(w)["link"],
        tools::owner_notify::link::assistant(),
        "its notification opens the assistant's chat"
    );

    // Connect in the assistant's chat answers the Bookkeeper's run.
    assert!(crate::chat_dispatch::answer_ask(state, "req-connect", "connected".into()).await);
    let got = tokio::time::timeout(Duration::from_secs(5), rx).await.expect("the run hears it").unwrap();
    assert_eq!(got, "connected");
    owner.chats_untouched(&nebo);
    drop(handle);
}

/// A card shows where the owner asked: one raised in his own chat with an
/// employee (typed or spoken) stays in that chat. It is not relayed to the
/// assistant, and its notification opens that chat, never pulling him away.
/// The same card raised by work he is not in (a schedule, a heartbeat, a
/// colleague's hand-off, a team seat) is relayed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_card_raised_in_a_direct_owner_employee_chat_is_not_relayed() {
    let nebo = session().await;
    let state = &nebo.state;
    let bk = format!("bk-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let chat = uuid::Uuid::new_v4().to_string();
    nebo.store().create_chat(&chat, "Books").unwrap();
    let key = format!("agent:{bk}:thread:{chat}");
    let card = json!([{ "type": "install_plugin", "plugin": "books", "name": "Example Books", "code": "PLUG-BOOKS" }]);
    let (handle, _rx) =
        park_card(&nebo, &bk, &key, &Door::Chat, "req-direct", "Install Example Books on the card.", card).await;
    let waiting = crate::handlers::asks::waiting(state, None).await;
    let w = &waiting.iter().find(|w| w.card.id == "req-direct").expect("it waits on the owner").card;
    assert!(!w.relayed, "the owner asked in this chat: the card stays here");
    let link = crate::handlers::asks::push_item(w)["link"].as_str().unwrap().to_string();
    assert_ne!(link, tools::owner_notify::link::assistant(), "nothing pulls him to the assistant");
    assert!(link.starts_with(&format!("/{bk}")), "{link}");
    drop(handle);

    // The rule, door by door.
    use crate::handlers::asks::relayed;
    assert!(!relayed(&Door::Voice, &key), "a call in the chat is the owner's own conversation");
    for door in [Door::Schedule, Door::Heartbeat, Door::Workflow, Door::LocalApi, Door::Coworker { from: "chief".into() }] {
        assert!(relayed(&door, &key), "{} is work the owner is not in", door.label());
    }
    assert!(relayed(&Door::Chat, &format!("agent:{bk}:coworker:team:t1")), "a team seat");
    assert!(relayed(&Door::Chat, &format!("subagent:{key}:task-1")), "a helper");
}

// ── The Inbox: one item model, one answer path ─────────────────────────

/// The owner's Inbox items, one part of it.
fn inbox(nebo: &Nebo, view: db::models::InboxView) -> Vec<db::models::Notification> {
    let user = nebo.store().ensure_local_user_id().unwrap();
    nebo.store().list_user_notifications(&user, view, None, 200, 0).unwrap()
}

fn item(nebo: &Nebo, id: &str) -> db::models::Notification {
    let user = nebo.store().ensure_local_user_id().unwrap();
    nebo.store().get_notification(id, &user).unwrap().unwrap_or_else(|| panic!("{id} is in the Inbox"))
}

/// A question from work the owner is not in is a Needs-you item with its
/// answers on the card. Answered from the Inbox (the phone's tap), the run
/// hears the answer and the item moves to History with it. A question in
/// the owner's own chat stays that chat's card: no Inbox item.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_relayed_question_is_a_needs_you_item_answered_on_its_card() {
    let nebo = session().await;
    let state = &nebo.state;
    let bk = format!("bk-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let req = format!("req-{}", uuid::Uuid::new_v4().simple());
    let card = json!([{ "type": "options", "multiSelect": false, "options": ["Lumen · $14.20", "Northfield · $16.20"] }]);
    let (handle, rx) = park_card(
        &nebo,
        &bk,
        &format!("agent:{bk}:cron:restock"),
        &Door::Schedule,
        &req,
        "Which supplier should I reorder AirWash from?",
        card.clone(),
    )
    .await;
    let id = crate::handlers::asks::push_id(&req);
    let waiting = item(&nebo, &id);
    assert!(waiting.needs_you && waiting.status == "open", "{waiting:?}");
    assert_eq!(
        waiting.options.iter().map(|o| o.label.as_str()).collect::<Vec<_>>(),
        ["Lumen · $14.20", "Northfield · $16.20"]
    );
    assert!(inbox(&nebo, db::models::InboxView::NeedsYou).iter().any(|n| n.id == id));

    crate::handlers::notification::answer_item(state, &waiting, "Lumen · $14.20", "mobile").await.unwrap();
    let got = tokio::time::timeout(Duration::from_secs(5), rx).await.expect("the run hears it").unwrap();
    assert_eq!(got, "Lumen · $14.20");
    let answered = item(&nebo, &id);
    assert_eq!((answered.status.as_str(), answered.answer.as_deref()), ("answered", Some("Lumen · $14.20")));
    assert!(!inbox(&nebo, db::models::InboxView::NeedsYou).iter().any(|n| n.id == id), "it left Needs you");
    assert!(inbox(&nebo, db::models::InboxView::History).iter().any(|n| n.id == id), "it is in History");
    drop(handle);

    // In the owner's own chat: the card stays there.
    let chat = uuid::Uuid::new_v4().to_string();
    nebo.store().create_chat(&chat, "Restock").unwrap();
    let direct = format!("req-{}", uuid::Uuid::new_v4().simple());
    let (handle, _rx) =
        park_card(&nebo, &bk, &format!("agent:{bk}:thread:{chat}"), &Door::Chat, &direct, "Which one?", card).await;
    let user = nebo.store().ensure_local_user_id().unwrap();
    assert!(nebo.store().get_notification(&crate::handlers::asks::push_id(&direct), &user).unwrap().is_none());
    drop(handle);
}

/// An employee's own question (push_notification with options) is a
/// Needs-you item; its answer settles it with the option's words and goes
/// to the session that asked, on the wake rail. An update has nothing to
/// answer, and an item put away is History, never deleted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_employees_own_question_is_answered_into_its_conversation() {
    let nebo = session().await;
    let state = &nebo.state;
    let emp = format!("store-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let session_key = format!("agent:{emp}:cron:stock");
    let c = ToolContext { session_key: session_key.clone(), ..Default::default() };
    let r = state
        .tools
        .execute(
            &c,
            "push_notification",
            json!({
                "title": "Which supplier for AirWash?",
                "message": "Stock runs out in 6 days.",
                "options": ["Lumen", "Northfield"],
            }),
        )
        .await;
    assert!(!r.is_error, "{}", r.content);
    let asked = inbox(&nebo, db::models::InboxView::NeedsYou)
        .into_iter()
        .find(|n| n.agent_id.as_deref() == Some(emp.as_str()))
        .expect("the question is a Needs-you item");
    assert_eq!(asked.options.len(), 2);

    crate::handlers::notification::answer_item(state, &asked, "Lumen", "web").await.unwrap();
    let settled = item(&nebo, &asked.id);
    assert_eq!((settled.answer.as_deref(), settled.answered_via.as_deref()), (Some("Lumen"), Some("web")));
    let woke = nebo.store().engine_events_for("session", &session_key, 10).unwrap();
    assert!(woke.iter().any(|e| e.payload.contains("Lumen") && e.payload.contains("Which supplier")), "{woke:?}");

    // An update: nothing to answer. Put away, it is History and still there.
    let r = state
        .tools
        .execute(&c, "push_notification", json!({ "title": "Shopify orders synced", "message": "42 new orders." }))
        .await;
    assert!(!r.is_error, "{}", r.content);
    let update = inbox(&nebo, db::models::InboxView::Updates)
        .into_iter()
        .find(|n| n.agent_id.as_deref() == Some(emp.as_str()))
        .expect("the update is in Updates");
    assert!(crate::handlers::notification::answer_item(state, &update, "x", "web").await.is_err());
    let user = nebo.store().ensure_local_user_id().unwrap();
    nebo.store().archive_notification(&update.id, &user).unwrap();
    assert_eq!(item(&nebo, &update.id).status, "archived", "put away, never deleted");
    assert!(inbox(&nebo, db::models::InboxView::History).iter().any(|n| n.id == update.id));
}

/// A permission ask is a Needs-you item whose answers are its card's; the
/// Inbox answers it through the ask's own path, which moves the item to
/// History with the answer's words and where it came from.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_permission_ask_is_answered_from_the_inbox_through_its_own_path() {
    let nebo = session().await;
    let state = &nebo.state;
    let emp = format!("emp-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let name = format!("probe_{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let ran = Arc::new(AtomicUsize::new(0));
    state.tools.register(Box::new(Probe { name: name.clone(), capability: Some("sms"), ran: ran.clone() })).await;
    let parked = nebo.tool(&heartbeat(&format!("agent:{emp}:heartbeat")), &name, json!({ "to": "+15550142" })).await;
    let ask = parked.parked_ask.expect("the step parks on the owner");
    let id = crate::permission_asks::inbox_id(&ask);
    let waiting = item(&nebo, &id);
    assert!(waiting.needs_you && waiting.status == "open");
    assert!(waiting.options.iter().any(|o| o.value == "this_once"), "{:?}", waiting.options);

    crate::handlers::notification::answer_item(state, &waiting, "this_once", "mobile").await.unwrap();
    let answered = item(&nebo, &id);
    assert_eq!(answered.status, "answered");
    assert_eq!((answered.answer.as_deref(), answered.answered_via.as_deref()), (Some("This once"), Some("mobile")));
}
