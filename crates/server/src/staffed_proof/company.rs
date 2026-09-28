//! The primary employee is the owner's point person for the whole company
//! (owner, 2026-09-28): it sees what every employee is doing, native and
//! linked, reads one employee's current work, and talks to any of them, a
//! linked employee in the conversation it is already working in included.
//! Live that day, on his call, it said of a linked coding employee that was
//! mid-turn: "I can't see Codex. Status only covers this conversation."
//!
//! Through the real server: the run registry, the permission asks, the
//! coworker rail, the wake rail, the real linked provider and a real host
//! on this computer running a coding agent process (this test binary again,
//! `company_agent::fake_company_agent`). The primary employee's model is a script that
//! relays what it hears; no network, no real bot.

use super::*;
use ai::Provider;
use tools::Origin;

/// What the fake coding agent was told, in order.
fn told(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// The primary employee's model: it tells the owner what came back.
struct Relay;

#[async_trait::async_trait]
impl ai::Provider for Relay {
    fn id(&self) -> &str {
        "proof-relay"
    }

    async fn stream(&self, req: &ai::ChatRequest) -> Result<ai::EventReceiver, ai::ProviderError> {
        // The reply as the notification carries it, without the reminder's
        // wrapping.
        let heard = req.messages.iter().rev().find_map(|m| {
            let from = m.content.find("[Reply from")?;
            let reply = &m.content[from..];
            Some(
                reply
                    .split("</system-reminder>")
                    .next()
                    .unwrap_or(reply)
                    .trim()
                    .replace('\n', " "),
            )
        });
        let text = match (req.trace.purpose, heard) {
            ("agent_turn", Some(heard)) => format!("RELAYED {heard}"),
            _ => "ok".to_string(),
        };
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        tokio::spawn(async move {
            let _ = tx.send(ai::StreamEvent::text(text)).await;
            let _ = tx.send(ai::StreamEvent::done()).await;
        });
        Ok(rx)
    }
}

/// A linked employee hosted on this computer, running
/// `company_agent::fake_company_agent`,
/// with the real linked provider in place on the one server.
struct Linked {
    _root: tempfile::TempDir,
    _host: Arc<ai::LocalHost>,
    told: PathBuf,
    provider: ai::LinkedProvider,
    bot: &'static str,
    agent: String,
    id: String,
}

impl Linked {
    async fn hire(nebo: &Nebo, bot: &'static str, name: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let told = root.path().join("told.jsonl");
        let host = ai::LocalHost::open(
            Arc::new(move || Some(bot.to_owned())),
            root.path().join("link"),
            root.path().join("home"),
            Some(root.path().join("nebo-link")),
        )
        .unwrap();
        let coder = nebo_runtimes::RuntimeCommand {
            program: std::env::current_exe()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            args: [
                "staffed_proof::company_agent::fake_company_agent",
                "--exact",
                "--nocapture",
                "--test-threads=1",
            ]
            .map(String::from)
            .to_vec(),
            env: vec![(
                "NEBO_PROOF_COMPANY".into(),
                told.to_string_lossy().into_owned(),
            )],
        };
        let hosted = host
            .host(nebo_runtimes::acp::Agent::ClaudeCode, coder)
            .await
            .unwrap();
        let relay = ai::Relay::Hub {
            api_url: "http://127.0.0.1:9".into(),
            token: Arc::new(|| None),
        };
        let provider = ai::LinkedProvider::new(
            relay,
            nebo.store().clone(),
            Some(host.clone()),
            "Nebo proof",
        );
        nebo.state
            .harness
            .reload_providers(vec![
                Arc::new(Relay) as Arc<dyn ai::Provider>,
                Arc::new(provider.clone()),
            ])
            .await;
        let id = format!("lx-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
        nebo.store()
            .create_agent(
                &id,
                Some("linked"),
                name,
                "",
                &format!("---\nname: {name}\n---\n"),
                "{}",
                None,
                None,
            )
            .unwrap();
        nebo.store()
            .upsert_entity_config(
                "agent",
                &id,
                &json!({ "modelPreference": ai::LinkedProvider::model_id(bot, &hosted.id) }),
            )
            .unwrap();
        Linked {
            _root: root,
            _host: host,
            told,
            provider,
            bot,
            agent: hosted.id,
            id,
        }
    }

    /// A conversation with it, titled `title`, started outside the
    /// assistant (the owner's own thread, or his phone): `prompt` sent, and
    /// its stream, which is cancelled with `cancel`.
    async fn converse(
        &self,
        nebo: &Nebo,
        title: &str,
        prompt: &str,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> ai::EventReceiver {
        let chat = uuid::Uuid::new_v4().to_string();
        let key = format!("agent:{}:thread:{chat}", self.id);
        nebo.store()
            .create_chat_for_session(&chat, &key, title, None)
            .unwrap();
        let req = ai::ChatRequest {
            messages: vec![ai::Message {
                role: "user".into(),
                content: prompt.into(),
                ..Default::default()
            }],
            model: format!("{}/{}", self.bot, self.agent),
            chat_id: chat,
            cancel_token: Some(cancel.clone()),
            ..ai::ChatRequest::new(ai::RequestTrace {
                agent_id: self.id.clone(),
                ..ai::RequestTrace::new("agent_turn")
            })
        };
        self.provider.stream(&req).await.unwrap()
    }

    fn told(&self) -> Vec<Value> {
        told(&self.told)
    }

    fn prompts(&self) -> Vec<Value> {
        self.told()
            .into_iter()
            .filter(|t| t.get("prompt").is_some())
            .collect()
    }

    /// Off the roster, and the server's providers as they were.
    async fn leave(self, nebo: &Nebo) {
        nebo.store().delete_agent(&self.id).unwrap();
        nebo.state.harness.reload_providers(Vec::new()).await;
    }
}

/// The first text the stream says.
async fn first_words(rx: &mut ai::EventReceiver) -> String {
    loop {
        let event = tokio::time::timeout(Duration::from_secs(60), rx.recv())
            .await
            .expect("it spoke")
            .expect("the turn is on");
        assert_ne!(
            event.event_type,
            ai::StreamEventType::Error,
            "{:?}",
            event.error
        );
        if event.event_type == ai::StreamEventType::Text {
            return event.text;
        }
    }
}

/// The stream to its end.
async fn to_the_end(mut rx: ai::EventReceiver) -> Vec<ai::StreamEvent> {
    let mut events = Vec::new();
    while let Some(e) = tokio::time::timeout(Duration::from_secs(60), rx.recv())
        .await
        .expect("the turn ended")
    {
        events.push(e);
    }
    events
}

/// A conversation of `agent`'s with the owner, titled `title`, as the app
/// opens one: `agent:<id>:thread:<chat>`.
fn thread(nebo: &Nebo, agent: &str, title: &str) -> String {
    let chat = uuid::Uuid::new_v4().to_string();
    let key = format!("agent:{agent}:thread:{chat}");
    nebo.store()
        .create_chat_for_session(&chat, &key, title, None)
        .unwrap();
    nebo.state
        .harness
        .sessions()
        .get_or_create(&key, "")
        .unwrap();
    key
}

/// The primary employee's conversation with the owner, and the context a
/// turn of its runs tools with.
fn assistant_thread(nebo: &Nebo, title: &str) -> (String, tools::ToolContext) {
    let key = thread(nebo, tools::team_tool::PRIMARY_AGENT_ID, title);
    let ctx = tools::ToolContext::new(Origin::User).with_session(key.clone(), "s1");
    (key, ctx)
}

/// "What is everyone doing?" has one answer for the whole company: a
/// native employee mid-run (where, what, how long), one stopped on a
/// question for the owner, an idle one, and a linked employee mid-prompt in
/// a conversation it was given elsewhere, read from the computer it runs
/// on, with the conversation's id. The asking turn itself is not reported.
/// get_employee then reads the linked employee's current work: the request,
/// its calls, its latest words; reading sends it nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_point_person_sees_what_everyone_is_doing() {
    const BOT: &str = "5e1f0000-0000-4000-8000-0000000c0a55";
    let nebo = session().await;
    let coder = Linked::hire(&nebo, BOT, "Proof Point Coder").await;
    let books = nebo
        .hire("Proof Point Books", json!({ "workflows": {} }))
        .await;
    let desk = nebo
        .hire("Proof Point Desk", json!({ "workflows": {} }))
        .await;
    let idle = nebo
        .hire("Proof Point Researcher", json!({ "workflows": {} }))
        .await;

    // The coding employee mid-prompt, in a conversation the owner started.
    let cancel = tokio_util::sync::CancellationToken::new();
    let mut working = coder
        .converse(
            &nebo,
            "Refactor billing",
            "HOLD refactor the billing module",
            &cancel,
        )
        .await;
    assert_eq!(
        first_words(&mut working).await,
        "Working through the refactor."
    );

    // The bookkeeper mid-run in its own conversation; the front desk
    // stopped on a question for the owner.
    let books_key = thread(&nebo, &books, "Close September");
    let books_run = nebo
        .state
        .run_registry
        .register(crate::run_registry::RegisterParams {
            session_key: books_key.clone(),
            entity_id: books.clone(),
            entity_name: "Proof Point Books".into(),
            origin: "user".into(),
            channel: "web".into(),
            cancel_token: tokio_util::sync::CancellationToken::new(),
            parent_run_id: None,
        })
        .await;
    books_run.start_tool("read_file");
    books_run.show_activity("Reading ledger.csv");
    let desk_key = thread(&nebo, &desk, "Call back Dana");
    let _desk_run = nebo
        .state
        .run_registry
        .register(crate::run_registry::RegisterParams {
            session_key: desk_key.clone(),
            entity_id: desk.clone(),
            entity_name: "Proof Point Desk".into(),
            origin: "user".into(),
            channel: "web".into(),
            cancel_token: tokio_util::sync::CancellationToken::new(),
            parent_run_id: None,
        })
        .await;
    let parked = crate::handlers::chat::PendingAsk {
        request_id: "ask-proof-desk".into(),
        prompt: "Text Dana at the new number?".into(),
        widgets: None,
        created_at: chrono::Utc::now().timestamp(),
    };
    assert!(nebo.state.run_registry.park_ask(&desk_key, parked).await);

    // The owner asks the point person.
    let (asking, ctx) = assistant_thread(&nebo, "Owner");
    let _asking_run = nebo
        .state
        .run_registry
        .register(crate::run_registry::RegisterParams {
            session_key: asking.clone(),
            entity_id: tools::team_tool::PRIMARY_AGENT_ID.into(),
            entity_name: "Assistant".into(),
            origin: "user".into(),
            channel: "web".into(),
            cancel_token: tokio_util::sync::CancellationToken::new(),
            parent_run_id: None,
        })
        .await;
    let all = nebo.tool(&ctx, "list_employees", json!({})).await;
    assert!(!all.is_error, "{}", all.content);
    let now = all
        .content
        .split("Right now:")
        .nth(1)
        .unwrap_or_else(|| panic!("no account of right now: {}", all.content));
    assert!(
        now.contains("- Proof Point Coder (linked, on this computer): working\n    · the conversation \"Refactor billing\" (conversation: s-1): working on a request, a tool running"),
        "the linked employee, from its own computer: {now}"
    );
    assert!(now.contains("- Proof Point Books: working\n    · the conversation \"Close September\": Reading ledger.csv, for "), "{now}");
    assert!(
        now.contains("- Proof Point Desk: waiting on you\n    · the conversation \"Call back Dana\": thinking, for ") && now.contains("asks you: \"Text Dana at the new number?\""),
        "{now}"
    );
    assert!(now.contains("- Proof Point Researcher: idle"), "{now}");
    assert!(
        !now.contains("\"Owner\""),
        "the asking turn is not the company's work: {now}"
    );

    // "How far along is it?"
    let one = nebo
        .tool(&ctx, "get_employee", json!({ "name": "Proof Point Coder" }))
        .await;
    assert!(!one.is_error, "{}", one.content);
    let now = one
        .content
        .split("Now: ")
        .nth(1)
        .unwrap_or_else(|| panic!("no current work: {}", one.content));
    assert!(
        now.starts_with("Proof Point Coder (linked, on this computer): working"),
        "{now}"
    );
    assert!(
        now.contains("Working on: \"HOLD refactor the billing module\""),
        "{now}"
    );
    assert!(now.contains("Calls: cargo test (running)"), "{now}");
    assert!(
        now.contains("Latest words: \"Working through the refactor.\""),
        "{now}"
    );
    assert_eq!(
        coder.prompts().len(),
        1,
        "reading sent it nothing: {:?}",
        coder.told()
    );
    assert!(
        coder.told().iter().all(|t| t.get("cancel").is_none()),
        "reading stopped nothing"
    );

    cancel.cancel();
    to_the_end(working).await;
    drop(books_run);
    for id in [&books, &desk, &idle] {
        nebo.store().delete_agent(id).unwrap();
    }
    coder.leave(&nebo).await;
}

/// "Ask it how far along it is": the point person sends into the
/// conversation the linked employee already has, not a new one; the
/// message and the answer are in the employee's own thread for the owner
/// to open; the answer comes back to the point person's conversation and
/// is told to the owner, on his call too.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_point_person_relays_into_a_linked_employees_conversation_and_hears_back() {
    const BOT: &str = "5e1f0000-0000-4000-8000-0000000c0a56";
    let nebo = session().await;
    let coder = Linked::hire(&nebo, BOT, "Proof Point Relay").await;
    let earlier = coder
        .converse(
            &nebo,
            "Billing cleanup",
            "clean up billing",
            &tokio_util::sync::CancellationToken::new(),
        )
        .await;
    to_the_end(earlier).await;
    assert_eq!(
        coder
            .told()
            .iter()
            .filter(|t| t.get("new").is_some())
            .count(),
        1
    );

    let (asking, ctx) = assistant_thread(&nebo, "Owner call");
    // The owner is on a call in that conversation.
    let (heard_tx, mut heard) = tokio::sync::mpsc::channel(4);
    nebo.state
        .live_calls
        .lock()
        .unwrap()
        .insert(asking.clone(), heard_tx);

    let sent = nebo
        .tool(&ctx, "send_message", json!({ "to": "Proof Point Relay", "message": "How many files are left?", "conversation": "s-1" }))
        .await;
    assert!(
        !sent.is_error
            && sent
                .content
                .starts_with("Message sent to Proof Point Relay."),
        "{}",
        sent.content
    );

    let replied = |nebo: &Nebo| {
        let sessions = nebo.state.harness.sessions();
        sessions
            .resolve_session_id_by_key(&asking)
            .ok()
            .and_then(|id| sessions.get_messages(&id).ok())
            .unwrap_or_default()
    };
    nebo.wait_until(
        60,
        "the answer is told in the point person's conversation",
        || {
            replied(&nebo).iter().any(|m| {
                m.role == "assistant"
                    && m.content.contains("RELAYED")
                    && m.content.contains("ANSWER: 3 files left.")
            })
        },
    )
    .await;
    let rows = replied(&nebo);
    assert!(
        rows.iter()
            .any(|m| m.content.contains("[Reply from Proof Point Relay]")
                && m.content.contains("ANSWER: 3 files left.")),
        "the reply came back to the point person: {rows:?}"
    );
    let on_call = tokio::time::timeout(Duration::from_secs(30), heard.recv())
        .await
        .expect("the call heard it")
        .unwrap();
    assert!(on_call.contains("ANSWER: 3 files left."), "{on_call}");

    // Into the conversation it already had: one session, both prompts in it.
    let prompts = coder.prompts();
    assert_eq!(prompts.len(), 2, "{prompts:?}");
    assert!(prompts.iter().all(|p| p["session"] == "s-1"), "{prompts:?}");
    assert!(
        prompts[1]["prompt"]
            .as_str()
            .unwrap()
            .contains("How many files are left?"),
        "{prompts:?}"
    );
    assert_eq!(
        coder
            .told()
            .iter()
            .filter(|t| t.get("new").is_some())
            .count(),
        1,
        "no new conversation: {:?}",
        coder.told()
    );

    // The exchange is in the employee's own thread.
    let thread = format!(
        "agent:{}:coworker:{}",
        coder.id,
        tools::team_tool::PRIMARY_AGENT_ID
    );
    let sessions = nebo.state.harness.sessions();
    let id = sessions.resolve_session_id_by_key(&thread).unwrap();
    let chat = nebo
        .store()
        .get_chat(&sessions.active_chat_id(&id))
        .unwrap()
        .unwrap();
    assert_eq!(chat.linked_chat_id.as_deref(), Some("s-1"));
    let rows = sessions.get_messages(&id).unwrap();
    assert!(
        rows.iter()
            .any(|m| m.role == "user" && m.content.contains("How many files are left?")),
        "{rows:?}"
    );
    assert!(
        rows.iter()
            .any(|m| m.role == "assistant" && m.content.contains("ANSWER: 3 files left.")),
        "{rows:?}"
    );

    // "new" starts another conversation.
    let fresh = nebo
        .tool(&ctx, "send_message", json!({ "to": "Proof Point Relay", "message": "Start the docs.", "conversation": "new" }))
        .await;
    assert!(!fresh.is_error, "{}", fresh.content);
    nebo.wait_until(60, "a new conversation is opened for it", || {
        coder
            .told()
            .iter()
            .filter(|t| t.get("new").is_some())
            .count()
            == 2
    })
    .await;
    nebo.wait_until(60, "its answer there is told too", || {
        replied(&nebo)
            .iter()
            .filter(|m| m.role == "assistant" && m.content.contains("RELAYED"))
            .count()
            == 2
    })
    .await;

    nebo.state.live_calls.lock().unwrap().remove(&asking);
    coder.leave(&nebo).await;
}

/// Never two prompts in flight on one linked conversation: while the
/// linked employee's turn runs there, a message into it is not sent, and
/// the point person is told so plainly, with what to do instead. A
/// conversation it doesn't have, or one named for an employee that has no
/// conversations of its own, sends nothing either.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_busy_linked_conversation_is_never_sent_a_second_prompt() {
    const BOT: &str = "5e1f0000-0000-4000-8000-0000000c0a57";
    let nebo = session().await;
    let coder = Linked::hire(&nebo, BOT, "Proof Point Busy").await;
    let native = nebo
        .hire("Proof Point Clerk", json!({ "workflows": {} }))
        .await;
    let cancel = tokio_util::sync::CancellationToken::new();
    let mut working = coder
        .converse(&nebo, "Refactor", "HOLD refactor it all", &cancel)
        .await;
    first_words(&mut working).await;

    let (_asking, ctx) = assistant_thread(&nebo, "Owner");
    let busy = nebo
        .tool(&ctx, "send_message", json!({ "to": "Proof Point Busy", "message": "Also fix the tests.", "conversation": "s-1" }))
        .await;
    assert!(busy.is_error, "{}", busy.content);
    assert!(
        busy.content
            .contains("in the middle of a turn in that conversation"),
        "{}",
        busy.content
    );
    assert!(
        busy.content.contains("Nothing was sent") && busy.content.contains("conversation: \"new\""),
        "{}",
        busy.content
    );

    let unknown = nebo
        .tool(
            &ctx,
            "send_message",
            json!({ "to": "Proof Point Busy", "message": "x", "conversation": "s-404" }),
        )
        .await;
    assert!(
        unknown.is_error && unknown.content.contains("has no conversation s-404"),
        "{}",
        unknown.content
    );
    let native_one = nebo
        .tool(
            &ctx,
            "send_message",
            json!({ "to": "Proof Point Clerk", "message": "x", "conversation": "s-1" }),
        )
        .await;
    assert!(
        native_one.is_error && native_one.content.contains("is not a linked employee"),
        "{}",
        native_one.content
    );

    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        coder.prompts().len(),
        1,
        "one prompt in flight, ever: {:?}",
        coder.told()
    );
    assert!(
        coder.told().iter().all(|t| t.get("cancel").is_none()),
        "its turn was never disturbed"
    );

    cancel.cancel();
    to_the_end(working).await;
    nebo.store().delete_agent(&native).unwrap();
    coder.leave(&nebo).await;
}
