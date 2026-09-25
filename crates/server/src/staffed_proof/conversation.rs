//! The owner's conversation stays open (Batch B, PRD-Turn-Controller G4):
//! work fans out to coworkers, teams and helpers, nothing the employee
//! starts holds the owner's turn, and every result comes back as a
//! notification that is combined and reported to the conversation the work
//! came from.
//!
//! These scenarios run real turns on the one server: the model is a script
//! (`Company`) swapped in for the scenario, which answers each thread from
//! its own words and holds a worker's answer until the scenario lets it go.
//! Replies to a loop conversation land in a stand-in channel (`Loop`).

use super::*;
use tools::Origin;

use std::collections::HashMap;

/// What one model call does: optionally wait on a gate, then send events.
struct Step {
    gate: Option<&'static str>,
    events: Vec<ai::StreamEvent>,
}

impl Step {
    fn say(text: impl Into<String>) -> Self {
        Step {
            gate: None,
            events: vec![ai::StreamEvent::text(text)],
        }
    }

    fn held(gate: &'static str, text: impl Into<String>) -> Self {
        Step {
            gate: Some(gate),
            events: vec![ai::StreamEvent::text(text)],
        }
    }

    fn call(calls: Vec<(&str, Value)>) -> Self {
        let events = calls
            .into_iter()
            .map(|(name, input)| {
                ai::StreamEvent::tool_call(ai::ToolCall {
                    id: format!("call-{}", uuid::Uuid::new_v4().simple()),
                    name: name.to_string(),
                    input,
                })
            })
            .collect();
        Step { gate: None, events }
    }
}

/// A thread as the script reads it.
struct Thread<'a> {
    req: &'a ai::ChatRequest,
}

impl Thread<'_> {
    /// The first user message that names a marker: whose thread this is.
    fn opener(&self) -> &str {
        self.req
            .messages
            .iter()
            .filter(|m| m.role == "user")
            .map(|m| m.content.as_str())
            .find(|c| c.contains("MARK-") || c.contains("OWNER-"))
            .unwrap_or("")
    }

    /// Everything after the model's last answer: what this step reads new.
    fn since_last_answer(&self) -> Vec<&ai::Message> {
        let from = self
            .req
            .messages
            .iter()
            .rposition(|m| m.role == "assistant")
            .map(|i| i + 1)
            .unwrap_or(0);
        self.req.messages[from..].iter().collect()
    }

    /// The step reads the results of its own tool calls.
    fn has_tool_results(&self) -> bool {
        self.since_last_answer()
            .iter()
            .any(|m| m.tool_results.is_some() || m.role == "tool")
    }

    /// Anything in the thread says `words` (its identity row names whose
    /// thread it is).
    fn says(&self, words: &str) -> bool {
        self.req.messages.iter().any(|m| m.content.contains(words))
    }

    fn new_text(&self) -> String {
        self.since_last_answer()
            .iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The model already said `words` in this thread.
    fn answered(&self, words: &str) -> bool {
        self.req
            .messages
            .iter()
            .any(|m| m.role == "assistant" && m.content.contains(words))
    }

    /// Every `*-RESULT` word the thread has brought in and the model has not
    /// said yet, in order, once. A row that lands while a call is in flight
    /// is stored before that call's answer, so "since the last answer"
    /// would miss it; a model reads the whole thread.
    fn unreported_results(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for m in self.req.messages.iter().filter(|m| m.role != "assistant") {
            for word in m.content.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-')) {
                if word.ends_with("-RESULT") && !self.answered(word) && !out.iter().any(|w| w == word) {
                    out.push(word.to_string());
                }
            }
        }
        out
    }
}

type Rule = Box<dyn Fn(&Thread<'_>) -> Option<Step> + Send + Sync>;

/// The company's model for one scenario.
struct Company {
    rules: Vec<Rule>,
    gates: std::sync::Mutex<HashMap<&'static str, Arc<tokio::sync::Semaphore>>>,
    /// Every call's opener, in order.
    calls: std::sync::Mutex<Vec<String>>,
}

impl Company {
    fn new(rules: Vec<Rule>) -> Arc<Self> {
        Arc::new(Self {
            rules,
            gates: Default::default(),
            calls: Default::default(),
        })
    }

    fn gate(&self, name: &'static str) -> Arc<tokio::sync::Semaphore> {
        self.gates
            .lock()
            .unwrap()
            .entry(name)
            .or_insert_with(|| Arc::new(tokio::sync::Semaphore::new(0)))
            .clone()
    }

    /// Let every call held on `name` answer, now and later.
    fn open(&self, name: &'static str) {
        self.gate(name).add_permits(10_000);
    }

    fn calls_naming(&self, marker: &str) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.contains(marker))
            .count()
    }
}

struct Model(Arc<Company>);

#[async_trait::async_trait]
impl ai::Provider for Model {
    fn id(&self) -> &str {
        "company-script"
    }

    async fn stream(&self, req: &ai::ChatRequest) -> Result<ai::EventReceiver, ai::ProviderError> {
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        // Titles, recaps, memory and checks: nothing to do with the work.
        if req.trace.purpose != "agent_turn" {
            tokio::spawn(async move {
                let _ = tx.send(ai::StreamEvent::text("ok")).await;
                let _ = tx.send(ai::StreamEvent::done()).await;
            });
            return Ok(rx);
        }
        let thread = Thread { req };
        let step = self
            .0
            .rules
            .iter()
            .find_map(|r| r(&thread))
            .unwrap_or_else(|| Step::say("noted"));
        self.0
            .calls
            .lock()
            .unwrap()
            .push(thread.opener().to_string());
        let gate = step.gate.map(|g| self.0.gate(g));
        tokio::spawn(async move {
            if let Some(gate) = gate
                && let Ok(permit) = gate.acquire().await
            {
                permit.forget();
            }
            for e in step.events {
                let _ = tx.send(e).await;
            }
            let _ = tx.send(ai::StreamEvent::done()).await;
        });
        Ok(rx)
    }
}

/// A loop conversation as the owner's phone sees it: every reply the bot
/// sends there.
#[derive(Default)]
struct Loop {
    sent: std::sync::Mutex<Vec<comm::CommMessage>>,
}

#[async_trait::async_trait]
impl comm::ChannelProvider for Loop {
    fn name(&self) -> &str {
        LOOP
    }
    async fn send_response(&self, msg: comm::CommMessage) -> Result<(), comm::CommError> {
        self.sent.lock().unwrap().push(msg);
        Ok(())
    }
}

const LOOP: &str = "proof-loop";

impl Loop {
    /// Every reply sent into `conversation`, as text.
    fn replies(&self, conversation: &str) -> Vec<String> {
        self.sent
            .lock()
            .unwrap()
            .iter()
            .filter(|m| m.conversation_id == conversation)
            .map(|m| m.content.clone())
            .collect()
    }

    fn all(&self, conversation: &str) -> String {
        self.replies(conversation).join("\n")
    }
}

/// The scenario's model and loop, in place on the one server until dropped.
struct Rig<'a> {
    nebo: &'a Nebo,
    company: Arc<Company>,
    loop_: Arc<Loop>,
}

impl<'a> Rig<'a> {
    async fn new(nebo: &'a Nebo, rules: Vec<Rule>) -> Self {
        let company = Company::new(rules);
        nebo.state
            .harness
            .reload_providers(vec![Arc::new(Model(company.clone()))])
            .await;
        let loop_ = Arc::new(Loop::default());
        nebo.state.channel_providers.write().await.insert(
            LOOP.to_string(),
            loop_.clone() as Arc<dyn comm::ChannelProvider>,
        );
        Rig {
            nebo,
            company,
            loop_,
        }
    }

    /// The owner writes in their loop conversation `conversation`, to the
    /// main employee, on session `session_key`.
    async fn owner_says(&self, session_key: &str, conversation: &str, text: &str) {
        self.owner_writes(session_key, "", Some(conversation), text)
            .await;
    }

    /// The owner writes to employee `agent_id` ("" = the main one) on
    /// session `session_key`: in the loop conversation `conversation`, or in
    /// the app when `None`.
    async fn owner_writes(
        &self,
        session_key: &str,
        agent_id: &str,
        conversation: Option<&str>,
        text: &str,
    ) {
        let config = crate::chat_dispatch::ChatConfig {
            session_key: session_key.to_string(),
            prompt: text.to_string(),
            user_id: String::new(),
            channel: LOOP.to_string(),
            origin: Origin::User,
            door: types::permissions::Door::Chat,
            agent_id: agent_id.to_string(),
            cancel_token: tokio_util::sync::CancellationToken::new(),
            lane: types::constants::lanes::MAIN.to_string(),
            comm_reply: conversation.map(|c| crate::chat_dispatch::CommReplyConfig {
                provider: LOOP.to_string(),
                topic: "dm".to_string(),
                conversation_id: c.to_string(),
                handoff_depth: 0,
                approval_relay: true,
                from_agent_id: agent_id.to_string(),
            }),
            entity_config: None,
            images: vec![],
            attachments: vec![],
            entity_name: String::new(),
            origin_agent_id: None,
            mention_context: None,
            tool_scope: None,
            plan_mode: false,
            channel_ctx: None,
            handoff_depth: 0,
            seed_taint: vec![],
            tool_allowlist: None,
            hidden_prompt: false,
            audience: None,
            cwd: None,
            model_override: None,
        };
        crate::chat_dispatch::run_chat(&self.nebo.state, config).await;
    }

    /// The session a tool call below comes from, as a running turn has it.
    fn open_session(&self, key: &str) {
        self.nebo
            .state
            .harness
            .sessions()
            .get_or_create(key, "")
            .expect("session");
    }

    /// The rows of session `key`, as stored.
    fn thread(&self, key: &str) -> Vec<db::models::ChatMessage> {
        let sessions = self.nebo.state.harness.sessions();
        match sessions.resolve_session_id_by_key(key) {
            Ok(id) => sessions.get_messages(&id).unwrap_or_default(),
            Err(_) => Vec::new(),
        }
    }

    /// The notification rows of session `key`.
    fn notifications(&self, key: &str) -> Vec<String> {
        self.thread(key)
            .into_iter()
            .filter(agent::harness::delegation::notify::is_notification_row)
            .map(|m| m.content)
            .collect()
    }

    async fn until(&self, secs: u64, what: &str, cond: impl FnMut() -> bool) {
        self.nebo.wait_until(secs, what, cond).await;
    }
}

/// The next scenario gets the server as it was: no model, no loop, and no
/// call of this one still held.
impl Drop for Rig<'_> {
    fn drop(&mut self) {
        for gate in self.company.gates.lock().unwrap().values() {
            gate.add_permits(10_000);
        }
        let state = &self.nebo.state;
        futures::executor::block_on(async {
            state.harness.reload_providers(Vec::new()).await;
            state.channel_providers.write().await.remove(LOOP);
        });
    }
}

/// The script's rule for a worker's thread: its opener names `mark`; it is
/// held on `gate` and answers `result`.
fn worker(mark: &'static str, gate: &'static str, result: &'static str) -> Rule {
    Box::new(move |t| {
        t.opener()
            .contains(mark)
            .then(|| Step::held(gate, format!("{result}: here is what I found.")))
    })
}

/// The owner's question fans out to three coworkers and two helpers at
/// once, and the owner's next message is answered at once while all five
/// work. Every result then lands as a notification in the owner's session
/// and is reported, combined, to the loop conversation the owner asked in.
/// One coworker answers through a helper of its own: its final answer
/// reaches the owner too.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_owner_is_answered_while_the_company_works() {
    let nebo = session().await;
    for name in ["Proof Books", "Proof Marketing", "Proof Buying"] {
        nebo.hire(name, json!({ "workflows": {} })).await;
    }
    const OWNER: &str = "proof:dm:owner-b";
    const CONV: &str = "conv-owner-b";
    let rules: Vec<Rule> = vec![
        // The owner's thread.
        Box::new(|t| {
            if !t.opener().contains("OWNER-B1") {
                return None;
            }
            let fresh = t.new_text();
            if fresh.contains("OWNER-B2") {
                return Some(Step::say("ANSWER-B2: we open at nine."));
            }
            if t.has_tool_results() {
                return Some(Step::say(
                    "Asked the team and two helpers; I'll report back.",
                ));
            }
            if fresh.contains("OWNER-B1") {
                return Some(Step::call(vec![
                    (
                        "send_message",
                        json!({"to": "Proof Books", "message": "MARK-BOOKS what is the budget?"}),
                    ),
                    (
                        "send_message",
                        json!({"to": "Proof Marketing", "message": "MARK-MKT what can we run?"}),
                    ),
                    (
                        "send_message",
                        json!({"to": "Proof Buying", "message": "MARK-BUY what is low?"}),
                    ),
                    (
                        "delegate",
                        json!({"description": "count the till", "prompt": "MARK-H1 count the till"}),
                    ),
                    (
                        "delegate",
                        json!({"description": "read the reviews", "prompt": "MARK-H2 read the reviews"}),
                    ),
                ]));
            }
            let results = t.unreported_results();
            (!results.is_empty()).then(|| Step::say(format!("REPORT {}", results.join(" "))))
        }),
        // Books answers through a helper of its own.
        Box::new(|t| {
            if !t.opener().contains("MARK-BOOKS") {
                return None;
            }
            if t.has_tool_results() {
                return Some(Step::say("Started on the books."));
            }
            let fresh = t.new_text();
            if t.says("BOOKS-HELPER-RESULT") && !t.answered("BOOKS-RESULT") {
                return Some(Step::say(
                    "BOOKS-RESULT: the budget is 2,000 (the ledger helper checked).",
                ));
            }
            fresh.contains("MARK-BOOKS").then(|| {
                Step::call(vec![("delegate", json!({"description": "check the ledger", "prompt": "MARK-BKH check the ledger"}))])
            })
        }),
        worker("MARK-BKH", "books-helper", "BOOKS-HELPER-RESULT"),
        worker("MARK-MKT", "marketing", "MKT-RESULT"),
        worker("MARK-BUY", "buying", "BUY-RESULT"),
        worker("MARK-H1", "h1", "H1-RESULT"),
        worker("MARK-H2", "h2", "H2-RESULT"),
    ];
    let rig = Rig::new(&nebo, rules).await;

    rig.owner_says(OWNER, CONV, "OWNER-B1 how is the business doing?")
        .await;
    rig.until(
        20,
        "the first turn ends with the owner told what started",
        || rig.loop_.all(CONV).contains("I'll report back"),
    )
    .await;
    rig.until(20, "all five are working", || {
        ["MARK-BKH", "MARK-MKT", "MARK-BUY", "MARK-H1", "MARK-H2"]
            .iter()
            .all(|m| rig.company.calls_naming(m) > 0)
    })
    .await;

    // The owner writes again while all five work: answered at once.
    rig.owner_says(OWNER, CONV, "OWNER-B2 what time do we open?")
        .await;
    rig.until(
        20,
        "the owner's second message is answered while the work runs",
        || rig.loop_.all(CONV).contains("ANSWER-B2"),
    )
    .await;
    assert!(
        !rig.loop_.all(CONV).contains("-RESULT"),
        "nothing has finished yet: {:?}",
        rig.loop_.replies(CONV)
    );

    for gate in ["books-helper", "marketing", "buying", "h1", "h2"] {
        rig.company.open(gate);
    }
    let expected = [
        "BOOKS-RESULT",
        "MKT-RESULT",
        "BUY-RESULT",
        "H1-RESULT",
        "H2-RESULT",
    ];
    rig.until(
        60,
        "every result is reported to the loop conversation",
        || {
            let said = rig.loop_.all(CONV);
            said.contains("REPORT") && expected.iter().all(|r| said.contains(r))
        },
    )
    .await;
    let notes = rig.notifications(OWNER).join("\n");
    for r in expected {
        assert_eq!(
            notes.matches(r).count(),
            1,
            "{r} came back as one notification: {notes}"
        );
    }
    assert!(
        rig.loop_
            .sent
            .lock()
            .unwrap()
            .iter()
            .all(|m| m.conversation_id == CONV),
        "every reply went to the conversation the owner asked in"
    );
}

/// B1: a message to a coworker never waits. It returns a receipt at once
/// while the coworker works, it may run beside other calls, and the reply
/// comes back to the sender's session as a notification that wakes it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_coworker_message_returns_at_once_and_the_reply_wakes_the_sender() {
    let nebo = session().await;
    nebo.hire("Proof Clerk", json!({ "workflows": {} })).await;
    const SENDER: &str = "proof:dm:sender-b1";
    let rules: Vec<Rule> = vec![
        worker("MARK-CLERK", "clerk", "CLERK-RESULT"),
        Box::new(|t| {
            t.new_text()
                .contains("CLERK-RESULT")
                .then(|| Step::say("HEARD the clerk"))
        }),
    ];
    let rig = Rig::new(&nebo, rules).await;
    rig.open_session(SENDER);
    let input = json!({"to": "Proof Clerk", "message": "MARK-CLERK file the invoice"});
    assert!(
        nebo.state
            .tools
            .concurrency_safe("send_message", &input)
            .await,
        "several go out side by side"
    );
    let ctx = tools::ToolContext::new(Origin::User).with_session(SENDER, "s1");
    let sent = tokio::time::timeout(
        Duration::from_secs(10),
        nebo.tool(&ctx, "send_message", input),
    )
    .await
    .expect("the send returned while the coworker is still working");
    assert!(!sent.is_error, "{}", sent.content);
    assert!(
        sent.content.contains("notification"),
        "a receipt, not a reply: {}",
        sent.content
    );
    assert!(!sent.content.contains("CLERK-RESULT"));

    rig.company.open("clerk");
    rig.until(
        30,
        "the reply lands as a notification in the sender's session",
        || {
            rig.notifications(SENDER)
                .iter()
                .any(|n| n.contains("CLERK-RESULT"))
        },
    )
    .await;
    rig.until(30, "the idle sender is woken by it", || {
        rig.thread(SENDER)
            .iter()
            .any(|m| m.role == "assistant" && m.content.contains("HEARD"))
    })
    .await;
}

/// B2: a team's replies reach the session that posted: the lead's answer,
/// and a named member's, come back to the poster as notifications, not only
/// into the team.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_team_reply_reaches_the_poster() {
    let nebo = session().await;
    let lead = nebo
        .hire("Proof Team Lead", json!({ "workflows": {} }))
        .await;
    let member = nebo
        .hire("Proof Team Member", json!({ "workflows": {} }))
        .await;
    nebo.store()
        .create_team(
            "proof-team-b2",
            "Proof Floor",
            "run the floor",
            &[db::TeamMember::local(&lead), db::TeamMember::local(&member)],
            &lead,
            None,
        )
        .unwrap();
    const POSTER: &str = "proof:dm:poster-b2";
    let rules: Vec<Rule> = vec![
        Box::new(|t| {
            (t.opener().contains("MARK-TEAM") && t.says("You are Proof Team Lead."))
                .then(|| Step::say("LEAD-RESULT: Monday is set."))
        }),
        Box::new(|t| {
            (t.opener().contains("MARK-TEAM") && t.says("You are Proof Team Member."))
                .then(|| Step::say("MEMBER-RESULT: I take Tuesday."))
        }),
    ];
    let rig = Rig::new(&nebo, rules).await;
    rig.open_session(POSTER);
    let ctx = tools::ToolContext::new(Origin::User).with_session(POSTER, "s1");
    let posted = nebo
        .tool(
            &ctx,
            "send_message",
            json!({"to": "Proof Floor", "message": "MARK-TEAM plan the week", "mention": ["Proof Team Lead", "Proof Team Member"]}),
        )
        .await;
    assert!(!posted.is_error, "{}", posted.content);
    rig.until(
        30,
        "the lead's and the named member's replies come back to the poster",
        || {
            let notes = rig.notifications(POSTER).join("\n");
            notes.contains("LEAD-RESULT") && notes.contains("MEMBER-RESULT")
        },
    )
    .await;
    let notes = rig.notifications(POSTER).join("\n");
    assert!(
        notes.contains("[Reply from Proof Team Lead in team \"Proof Floor\"]"),
        "{notes}"
    );
}

/// B3: an employee holds no fixed number of turn slots: three of its
/// conversations run at once, each waiting on its own work.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_employee_works_three_conversations_at_once() {
    let nebo = session().await;
    let desk = nebo.hire("Proof Desk", json!({ "workflows": {} })).await;
    let rig = Rig::new(&nebo, vec![worker("MARK-DESK", "desk", "DESK-RESULT")]).await;
    for n in 1..=3 {
        rig.owner_writes(
            &format!("agent:{desk}:web-{n}"),
            &desk,
            None,
            &format!("MARK-DESK-{n} check drawer {n}"),
        )
        .await;
    }
    rig.until(
        20,
        "all three conversations are in a model call at once",
        || rig.company.calls_naming("MARK-DESK") == 3,
    )
    .await;
    rig.company.open("desk");
}

/// B7: a question an unattended seat sends up its reporting line parks
/// nothing: the call returns at once, and the manager's answer comes back
/// as a notification that wakes the asking run.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_question_up_the_line_returns_at_once_and_the_answer_wakes_the_run() {
    let nebo = session().await;
    let manager = nebo.hire("Proof Manager", json!({ "workflows": {} })).await;
    let report = nebo.hire("Proof Report", json!({ "workflows": {} })).await;
    nebo.put_ok(
        &format!("/agents/{report}"),
        &json!({ "reportsTo": manager }),
    )
    .await;
    let asking = format!("agent:{report}:cron");
    let rig = Rig::new(&nebo, vec![worker("MARK-UP", "manager", "UP-RESULT")]).await;
    rig.open_session(&asking);
    let ctx = tools::ToolContext::new(Origin::System).with_session(asking.clone(), "s1");
    let asked = tokio::time::timeout(
        Duration::from_secs(10),
        nebo.tool(
            &ctx,
            "ask_owner",
            json!({"question": "MARK-UP which vendor should we use?"}),
        ),
    )
    .await
    .expect("the question returned while the manager is still deciding");
    assert!(
        !asked.is_error && asked.content.contains("Proof Manager"),
        "{}",
        asked.content
    );
    rig.company.open("manager");
    rig.until(
        30,
        "the manager's answer lands in the asking run's session",
        || {
            rig.notifications(&asking)
                .iter()
                .any(|n| n.contains("UP-RESULT"))
        },
    )
    .await;
}

/// B13: a turn woken by a helper's result replies to the conversation the
/// work came from, here the owner's loop conversation, not only the app.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_woken_turn_replies_where_the_work_came_from() {
    let nebo = session().await;
    const OWNER: &str = "proof:dm:owner-b13";
    const CONV: &str = "conv-owner-b13";
    let rules: Vec<Rule> = vec![
        Box::new(|t| {
            if !t.opener().contains("OWNER-B13") {
                return None;
            }
            if t.has_tool_results() {
                return Some(Step::say("Started a helper."));
            }
            if t.new_text().contains("OWNER-B13") {
                return Some(Step::call(vec![(
                    "delegate",
                    json!({"description": "price the order", "prompt": "MARK-R1 price the order"}),
                )]));
            }
            let results = t.unreported_results();
            (!results.is_empty()).then(|| Step::say(format!("REPORT {}", results.join(" "))))
        }),
        worker("MARK-R1", "r1", "R1-RESULT"),
    ];
    let rig = Rig::new(&nebo, rules).await;
    rig.owner_says(OWNER, CONV, "OWNER-B13 price the Rivera order")
        .await;
    rig.until(20, "the first turn ends", || {
        rig.loop_.all(CONV).contains("Started a helper")
    })
    .await;
    rig.company.open("r1");
    rig.until(
        30,
        "the woken turn's report reaches the loop conversation",
        || rig.loop_.all(CONV).contains("REPORT R1-RESULT"),
    )
    .await;
}

/// E15 (owner rule 2026-09-15): the employee the owner talks to directs a
/// team without naming anyone. The lead answers, alone, and the reply
/// comes back to the poster as a notification; the other member only reads
/// the post. A team with no lead refuses such a post, with the reason, and
/// nothing is recorded or sent.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_employees_team_post_goes_to_the_lead_and_a_leaderless_team_refuses_it() {
    let nebo = session().await;
    let assistant = nebo.hire("Proof E15 Assistant", json!({ "workflows": {} })).await;
    let lead = nebo.hire("Proof E15 Lead", json!({ "workflows": {} })).await;
    let member = nebo.hire("Proof E15 Member", json!({ "workflows": {} })).await;
    let members = [db::TeamMember::local(&lead), db::TeamMember::local(&member)];
    nebo.store()
        .create_team("proof-team-e15", "Proof Marketing Team", "every campaign", &members, &lead, None)
        .unwrap();
    nebo.store()
        .create_team("proof-team-e15-open", "Proof Sales Team", "every deal", &members, "", None)
        .unwrap();
    let member_ran = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let rules: Vec<Rule> = vec![
        Box::new(|t| {
            (t.opener().contains("MARK-E15") && t.says("You are Proof E15 Lead."))
                .then(|| Step::say("E15-LEAD-RESULT: here is what fits the budget."))
        }),
        Box::new({
            let member_ran = member_ran.clone();
            move |t| {
                (t.opener().contains("MARK-E15") && t.says("You are Proof E15 Member.")).then(|| {
                    member_ran.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Step::say("E15-MEMBER-RESULT")
                })
            }
        }),
    ];
    let rig = Rig::new(&nebo, rules).await;
    let poster = format!("agent:{assistant}:web");
    rig.open_session(&poster);
    let ctx = tools::ToolContext::new(Origin::User).with_session(poster.clone(), "s1");
    let posted = nebo
        .tool(
            &ctx,
            "send_message",
            json!({"to": "Proof Marketing Team", "message": "MARK-E15 what can we afford to run?"}),
        )
        .await;
    assert!(!posted.is_error, "{}", posted.content);
    assert!(posted.content.contains("Proof E15 Lead"), "the lead was asked: {}", posted.content);
    rig.until(30, "the lead's answer comes back to the poster", || {
        rig.notifications(&poster).join("\n").contains("E15-LEAD-RESULT")
    })
    .await;
    assert_eq!(member_ran.load(std::sync::atomic::Ordering::SeqCst), 0, "the member only read the post");

    let before = nebo.store().list_team_messages("proof-team-e15-open", 50).unwrap().len();
    let refused = nebo
        .tool(
            &ctx,
            "send_message",
            json!({"to": "Proof Sales Team", "message": "MARK-E15 who takes the Rivera deal?"}),
        )
        .await;
    assert!(refused.is_error, "{}", refused.content);
    assert!(refused.content.contains("has no lead") && refused.content.contains("NOT sent"), "{}", refused.content);
    assert_eq!(
        nebo.store().list_team_messages("proof-team-e15-open", 50).unwrap().len(),
        before,
        "nothing was recorded"
    );
}

/// Parity 1.9: the owner's next message answers the question card that is
/// open in the conversation, as typing answers Claude Code's
/// AskUserQuestion. The parked call gets the message as its answer and the
/// turn goes on; it is never queued behind a card nobody will click.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_owners_next_message_answers_the_open_question() {
    let nebo = session().await;
    const OWNER: &str = "agent:proof-q:web";
    let rules: Vec<Rule> = vec![Box::new(|t| {
        if !t.opener().contains("OWNER-Q") {
            return None;
        }
        if t.has_tool_results() {
            let answered = t
                .since_last_answer()
                .iter()
                .filter_map(|m| m.tool_results.as_ref())
                .any(|r| r.to_string().contains("teal, please"));
            return Some(Step::say(if answered { "ANSWER-Q teal it is." } else { "ANSWER-Q no answer." }));
        }
        t.new_text().contains("OWNER-Q").then(|| {
            Step::call(vec![(
                "ask_owner",
                json!({"question": "Which color for the banner?", "options": ["blue", "green"]}),
            )])
        })
    })];
    let rig = Rig::new(&nebo, rules).await;
    rig.owner_writes(OWNER, "", None, "OWNER-Q make the sale banner").await;
    rig.until(20, "the question is open on the conversation", || {
        futures::executor::block_on(nebo.state.run_registry.pending_ask_for_session(OWNER)).is_some()
    })
    .await;

    rig.owner_writes(OWNER, "", None, "teal, please").await;
    rig.until(20, "the turn goes on with the owner's message as the answer", || {
        rig.thread(OWNER).iter().any(|m| m.role == "assistant" && m.content.contains("ANSWER-Q"))
    })
    .await;
    let said: Vec<String> = rig.thread(OWNER).into_iter().filter(|m| m.role == "assistant").map(|m| m.content).collect();
    assert!(said.iter().any(|c| c.contains("ANSWER-Q teal it is.")), "{said:?}");
    assert!(
        futures::executor::block_on(nebo.state.run_registry.pending_ask_for_session(OWNER)).is_none(),
        "the card is closed"
    );
}
