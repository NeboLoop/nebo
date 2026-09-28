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

/// On the owner's own call the employee asks aloud and his spoken yes is the
/// answer (live 2026-09-28: told nothing about where to answer, the employee
/// sent the owner, on his phone, to "the desktop app", then to support).
/// The parked step tells the model to ask him now; the voice model hears
/// the ask's id beside the run's reply. An id the voice model made up
/// answers nothing and names the real one. His answer counts only after he
/// spoke and only in the call's own conversation; then it goes through the
/// ask's one answer path (answered via voice), the server's engine resumes
/// the parked call once, and the employee hears it ran. A client can't
/// claim a spoken answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn owners_call_ask_is_asked_aloud_and_answered_by_voice() {
    let nebo = session().await;
    let agent = format!("vc-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let key = format!("agent:{agent}:thread:{}", uuid::Uuid::new_v4());
    let (names, ran) = heartbeat_steps(&nebo, &agent.replace('-', "_")).await;
    let mut ctx = ToolContext::new(Origin::User).with_session(&key, "voice");
    ctx.door = Door::Voice;
    let asks = &nebo.state.permission_asks;

    // The step parks, and the model is told to ask the owner now, by voice.
    let since = chrono::Utc::now().timestamp();
    let parked = nebo.tool(&ctx, &names[1], json!({ "to": "+15550142" })).await;
    let ask_id = parked.parked_ask.clone().expect("the step parks on the owner");
    assert!(parked.content.contains("ask them now, in one short spoken question"), "{}", parked.content);
    assert!(parked.content.contains("Their spoken yes or no on this call is the answer"), "{}", parked.content);
    assert!(!parked.content.to_lowercase().contains("desktop"), "{}", parked.content);
    let waiting = crate::handlers::voice::waiting_on_owner(asks, &key, since);
    assert!(waiting.contains(&format!("ask_id \"{ask_id}\"")), "the voice model hears the id: {waiting}");
    let created = asks.get(&ask_id).unwrap().unwrap().created_at;
    let yes = json!({ "ask_id": ask_id, "answer": "this_once" });

    // An id the voice model made up (live 2026-09-28: "0") answers nothing,
    // and the voice model hears the real one.
    let invented = json!({ "ask_id": "0", "answer": "this_once" });
    let refused = crate::handlers::voice::answer_by_voice(asks, &key, &invented, created + 5);
    assert!(refused.starts_with("No ask 0 is waiting in this conversation"), "{refused}");
    assert!(refused.contains(&format!("ask_id \"{ask_id}\"")), "the real id is named: {refused}");
    assert_eq!(asks.get(&ask_id).unwrap().unwrap().status, agent::harness::permissions::AskStatus::Open);
    // Not yet: the owner hasn't spoken since it was asked.
    let early = crate::handlers::voice::answer_by_voice(asks, &key, &yes, created);
    assert!(early.contains("hasn't answered since this was asked"), "{early}");
    // Not from another conversation.
    let elsewhere = format!("agent:{agent}:thread:other");
    let other = crate::handlers::voice::answer_by_voice(asks, &elsewhere, &yes, created + 5);
    assert!(other.contains("Nothing is waiting"), "{other}");
    // Not from a client claiming a spoken answer.
    let (status, _) = nebo
        .post(&format!("/permissions/asks/{ask_id}/answer"), &json!({ "answer": "this_once", "via": "voice" }))
        .await;
    assert!(status >= 400, "a client can't claim a spoken answer: {status}");
    assert_eq!(asks.get(&ask_id).unwrap().unwrap().status, agent::harness::permissions::AskStatus::Open);
    assert_eq!(count(&ran)[1], 0);

    // He says yes: answered via voice, and the parked call runs once.
    let heard = crate::handlers::voice::answer_by_voice(asks, &key, &yes, created + 2);
    assert!(heard.starts_with("Answered yes"), "{heard}");
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
    let again = crate::handlers::voice::answer_by_voice(asks, &key, &yes, created + 9);
    assert!(again.contains("already answered"), "{again}");
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
                note(json!({ "prompt": message["params"]["prompt"][0]["text"] }));
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
    let linked = ai::LinkedProvider::new(relay, nebo.store().clone(), Some(local.clone()), "Nebo proof");
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
    let spoken = crate::handlers::voice::run_delegated_task(state, &key, "check the repo", "Check the repo.", None).await;
    assert!(spoken.contains("Your options are: Allow once, Always allow, or Deny."), "{spoken}");
    assert!(spoken.contains("ask_id \"call_1\""), "the voice model hears the ask's id: {spoken}");
    let card = state.run_registry.pending_ask_for_session(&key).await.expect("the card is on the run");
    let widget = &card.widgets.as_ref().unwrap()[0];
    assert_eq!(widget["options"], json!(["Allow once", "Always allow", "Deny"]), "every option the agent offered");

    // Not before he spoke, not by its sentence, not from another conversation.
    let say = |id: &str, answer: &str| json!({ "ask_id": id, "answer": answer });
    let voice = |key: String, input: Value, at: i64| async move {
        crate::handlers::voice::answer_ask_by_voice(state, &key, &input, at).await
    };
    let early = voice(key.clone(), say("call_1", "allow_always"), card.created_at).await;
    assert!(early.contains("hasn't answered since this was asked"), "{early}");
    let by_sentence = voice(key.clone(), say("git status Your options are: Allow once", "allow_always"), card.created_at + 2).await;
    assert!(by_sentence.contains("ask_id \"call_1\""), "it is told the id to use: {by_sentence}");
    let elsewhere = voice(format!("agent:{agent}:thread:other"), say("call_1", "allow_always"), card.created_at + 2).await;
    assert!(elsewhere.contains("Nothing is waiting"), "{elsewhere}");
    assert!(coder_told(&told_path).iter().all(|t| t.get("answer").is_none()), "nothing answered the agent yet");

    // He says "always": the agent gets its always option and the same turn
    // goes on to its end.
    let mut hub = state.hub.subscribe();
    let heard = voice(key.clone(), say("call_1", "allow_always"), card.created_at + 2).await;
    assert!(heard.starts_with("Answered yes"), "{heard}");
    until("the turn goes on to its end", || replies(&nebo, &chat).iter().any(|r| r.contains("Ran it."))).await;
    let settled = loop {
        let e = hub.recv().await.unwrap();
        if e.event_type == "ask_answered" {
            break e;
        }
    };
    assert_eq!((settled.payload["request_id"].as_str(), settled.payload["value"].as_str()), (Some("call_1"), Some("allow_always")));
    assert!(!crate::chat_dispatch::answer_ask(state, "call_1", "Deny".into()).await, "a second answer changes nothing");
    let again = voice(key.clone(), say("call_1", "no"), card.created_at + 9).await;
    assert!(again.contains("Nothing is waiting"), "{again}");

    until("the turn is over", idle).await;

    // Asked again next message, the card is answered from another device
    // (the phone's tap is its label): the agent gets that option, once.
    let spoken = crate::handlers::voice::run_delegated_task(state, &key, "and the other repo", "And the other repo.", None).await;
    assert!(spoken.contains("ask_id \"call_2\""), "{spoken}");
    assert!(crate::chat_dispatch::answer_ask(state, "call_2", "Deny".into()).await);
    assert!(!crate::chat_dispatch::answer_ask(state, "call_2", "Allow once".into()).await, "the first answer wins");
    until("the declined turn ends", || replies(&nebo, &chat).iter().any(|r| r.contains("Declined."))).await;

    until("the turn is over", idle).await;

    // Full Access: the agent's own no-prompt mode, and no ask reaches him.
    nebo.store().set_permission_mode(&employee, types::permissions::Mode::FullAccess).unwrap();
    let spoken = crate::handlers::voice::run_delegated_task(state, &key, "once more", "Once more.", None).await;
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
