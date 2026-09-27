//! Memory on one Nebo: local memory is shared by every employee on it,
//! private memory is one employee's own, and neither is ever filed under a
//! single conversation with the owner — unless the owner made the employee
//! Confidential, where every conversation is a sealed matter.
//!
//! These scenarios run real turns on the one server with a scripted model
//! (the conversation rig): the script calls `remember` and `recall` the way a
//! model would, the runner resolves the seat and hands the tools the context
//! it always does, and each scenario then reads the rows the store holds.
//! The checks are on where a row actually is (`memories.user_id`), not only
//! on what a reply says. Each scenario is a fixture in `suites/memory-proof.yaml`;
//! `suites/memory.yaml` holds the model-driven half the gate runs.

use super::conversation::{Rig, Rule, Step, Thread};
use super::*;
use tools::Origin;

/// Every tool result the script read, by the marker of the thread it came in.
#[derive(Default)]
struct Heard(std::sync::Mutex<Vec<(String, String)>>);

impl Heard {
    fn push(&self, marker: &str, t: &Thread<'_>) {
        let results: Vec<String> = t
            .since_last_answer()
            .iter()
            .filter_map(|m| m.tool_results.as_ref())
            .map(|r| r.to_string())
            .collect();
        self.0.lock().unwrap().push((marker.to_string(), results.join("\n")));
    }

    fn of(&self, marker: &str) -> String {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|(m, _)| m == marker)
            .map(|(_, r)| r.clone())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// The script for one thread: its opener names `marker` (a `MARK-` word,
/// what the rig reads as a thread's opener); it makes `call`
/// once, records what came back, and says `DONE-<marker>`.
fn one_call(marker: &'static str, call: (&'static str, Value), heard: Arc<Heard>) -> Rule {
    Box::new(move |t| {
        if !t.opener().contains(marker) {
            return None;
        }
        if t.has_tool_results() {
            heard.push(marker, t);
            return Some(Step::say(format!("DONE-{marker}")));
        }
        Some(Step::call(vec![(call.0, call.1.clone())]))
    })
}

/// The owner writes `text` to employee `agent_id` in the conversation `chat`
/// (`agent:<id>:thread:<chat>`, the key the app gives a thread) and the turn
/// runs to the script's `DONE-<marker>`.
async fn owner_in_chat(rig: &Rig<'_>, agent_id: &str, chat: &str, marker: &str, text: &str) {
    let key = format!("agent:{agent_id}:thread:{chat}");
    rig.owner_writes(&key, agent_id, None, text).await;
    let done = format!("DONE-{marker}");
    rig.until(30, &format!("the turn in {chat} ends"), || {
        rig.thread(&key).iter().any(|m| m.role == "assistant" && m.content.contains(&done))
    })
    .await;
}

/// Every row stored under `key`, as (user_id, value).
fn rows(nebo: &Nebo, key: &str) -> Vec<(String, String)> {
    let conn = rusqlite::Connection::open(nebo.home.join("data").join("nebo.db")).expect("db");
    let mut stmt = conn.prepare("SELECT user_id, value FROM memories WHERE key = ?1").expect("prepare");
    stmt.query_map([key], |r| Ok((r.get(0)?, r.get(1)?)))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows")
}

fn owner(nebo: &Nebo) -> String {
    nebo.store().ensure_local_user_id().expect("owner id")
}

/// One employee saves to local memory when the owner asks, and a different
/// employee, in a conversation of its own, recalls it. The row is in the
/// owner-wide local scope, not in the saving employee's private one, and the
/// saver was told so in the words the model reads.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_employee_saves_to_local_memory_and_another_recalls_it() {
    let nebo = session().await;
    let saver = nebo.hire("Proof Mem1 Office", json!({ "workflows": {} })).await;
    let asker = nebo.hire("Proof Mem1 Scheduler", json!({ "workflows": {} })).await;
    let heard = Arc::new(Heard::default());
    let key = "team/mem1-packing-checklist";
    let rules = vec![
        one_call(
            "MARK-MEM1-SAVE",
            ("remember", json!({ "key": key, "value": "Packing checklist: labels, tape, LANTERN-MEM1 seal.", "layer": "project", "scope": "local" })),
            heard.clone(),
        ),
        one_call("MARK-MEM1-ASK", ("recall", json!({ "query": "packing checklist" })), heard.clone()),
    ];
    let rig = Rig::new(&nebo, rules).await;
    owner_in_chat(&rig, &saver, "mem1-a", "MARK-MEM1-SAVE", "MARK-MEM1-SAVE save the packing checklist to local memory for everyone").await;
    owner_in_chat(&rig, &asker, "mem1-b", "MARK-MEM1-ASK", "MARK-MEM1-ASK what is the packing checklist?").await;

    let owner = owner(&nebo);
    assert_eq!(rows(&nebo, key), vec![(owner.clone(), "Packing checklist: labels, tape, LANTERN-MEM1 seal.".to_string())], "one row, in local memory");
    let saved = heard.of("MARK-MEM1-SAVE");
    assert!(saved.contains("Saved to local memory"), "the saver is told where it went: {saved}");
    let found = heard.of("MARK-MEM1-ASK");
    assert!(found.contains("LANTERN-MEM1"), "another employee recalls it: {found}");
    assert!(found.contains("(local memory"), "and is told it is local memory: {found}");
}

/// A fact an employee keeps for itself stays in its private memory: another
/// employee's recall never returns it, and the employee itself finds it again
/// in a new conversation.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_private_fact_stays_with_its_employee() {
    let nebo = session().await;
    let keeper = nebo.hire("Proof Mem2 Keeper", json!({ "workflows": {} })).await;
    let other = nebo.hire("Proof Mem2 Other", json!({ "workflows": {} })).await;
    let heard = Arc::new(Heard::default());
    let key = "owner/mem2-meeting-length";
    let rules = vec![
        one_call(
            "MARK-MEM2-SAVE",
            ("remember", json!({ "key": key, "value": "Prefers 25-minute meetings, ORCHID-MEM2.", "layer": "tacit" })),
            heard.clone(),
        ),
        one_call("MARK-MEM2-OTHER", ("recall", json!({ "query": "25-minute meetings" })), heard.clone()),
        one_call("MARK-MEM2-AGAIN", ("recall", json!({ "query": "25-minute meetings" })), heard.clone()),
    ];
    let rig = Rig::new(&nebo, rules).await;
    owner_in_chat(&rig, &keeper, "mem2-a", "MARK-MEM2-SAVE", "MARK-MEM2-SAVE remember for yourself that I like 25-minute meetings").await;
    owner_in_chat(&rig, &other, "mem2-b", "MARK-MEM2-OTHER", "MARK-MEM2-OTHER how long do I like meetings?").await;
    owner_in_chat(&rig, &keeper, "mem2-c", "MARK-MEM2-AGAIN", "MARK-MEM2-AGAIN how long do I like meetings?").await;

    let owner = owner(&nebo);
    assert_eq!(rows(&nebo, key).iter().map(|(u, _)| u.clone()).collect::<Vec<_>>(), vec![format!("{owner}:agent:{keeper}")], "one row, in the keeper's private memory");
    let saved = heard.of("MARK-MEM2-SAVE");
    assert!(saved.contains("Saved to your private memory"), "{saved}");
    assert!(!heard.of("MARK-MEM2-OTHER").contains("ORCHID-MEM2"), "another employee never sees it: {}", heard.of("MARK-MEM2-OTHER"));
    let again = heard.of("MARK-MEM2-AGAIN");
    assert!(again.contains("ORCHID-MEM2") && again.contains("(private memory"), "the keeper finds it in a new chat: {again}");
}

/// The owner's primary employee had memory isolation switched on (the switch
/// that also splits its conversations into threads), and every durable fact
/// it learned in the owner's chat was filed under that one chat: a new chat
/// never saw it. An owner chat is not a matter. A fact the employee saves
/// in one of the owner's chats is in its private memory and is recalled in
/// the next chat.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_isolated_employees_facts_from_the_owners_chat_outlive_the_chat() {
    let nebo = session().await;
    let primary = nebo
        .hire("Proof Mem3 Primary", json!({ "workflows": {}, "memory": { "mode": "separate" } }))
        .await;
    assert!(crate::workflow_manager::agent_memory_mode(nebo.store(), &primary).separates_conversations(), "conversations are separate");
    let heard = Arc::new(Heard::default());
    let key = "owner/mem3-standing-order";
    let rules = vec![
        one_call(
            "MARK-MEM3-SAVE",
            ("remember", json!({ "key": key, "value": "Standing order: oat milk, MAPLE-MEM3.", "layer": "tacit" })),
            heard.clone(),
        ),
        one_call("MARK-MEM3-NEW", ("recall", json!({ "query": "standing order" })), heard.clone()),
    ];
    let rig = Rig::new(&nebo, rules).await;
    owner_in_chat(&rig, &primary, "mem3-first", "MARK-MEM3-SAVE", "MARK-MEM3-SAVE remember my standing order is oat milk").await;
    owner_in_chat(&rig, &primary, "mem3-second", "MARK-MEM3-NEW", "MARK-MEM3-NEW what's my standing order?").await;

    let owner = owner(&nebo);
    let stored: Vec<String> = rows(&nebo, key).into_iter().map(|(u, _)| u).collect();
    assert_eq!(stored, vec![format!("{owner}:agent:{primary}")], "the employee's private memory, never a chat's");
    let found = heard.of("MARK-MEM3-NEW");
    assert!(found.contains("MAPLE-MEM3"), "the next chat recalls it: {found}");
}

/// What the model is told matches where the row is: "local memory" only for a
/// row in the local scope, "private memory" only for a row in the
/// employee's own. A run the owner did not start (a coworker's message, a
/// workflow) cannot put anything in local memory, is told so, and nothing
/// lands there.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_words_the_model_reads_name_the_scope_the_row_is_in() {
    let nebo = session().await;
    let clerk = nebo.hire("Proof Mem4 Clerk", json!({ "workflows": {} })).await;
    let heard = Arc::new(Heard::default());
    let rules = vec![
        one_call("MARK-MEM4-LOCAL", ("remember", json!({ "key": "team/mem4-local", "value": "Door code policy: ask the owner, MEM4-L.", "scope": "local" })), heard.clone()),
        one_call("MARK-MEM4-PRIV", ("remember", json!({ "key": "owner/mem4-private", "value": "Likes short replies, MEM4-P.", "scope": "private" })), heard.clone()),
    ];
    let rig = Rig::new(&nebo, rules).await;
    owner_in_chat(&rig, &clerk, "mem4-a", "MARK-MEM4-LOCAL", "MARK-MEM4-LOCAL save this for everyone").await;
    owner_in_chat(&rig, &clerk, "mem4-b", "MARK-MEM4-PRIV", "MARK-MEM4-PRIV keep this to yourself").await;
    drop(rig);

    let owner = owner(&nebo);
    let private = format!("{owner}:agent:{clerk}");
    for (marker, key, scope, words) in [
        ("MARK-MEM4-LOCAL", "team/mem4-local", owner.as_str(), "Saved to local memory"),
        ("MARK-MEM4-PRIV", "owner/mem4-private", private.as_str(), "Saved to your private memory"),
    ] {
        let stored: Vec<String> = rows(&nebo, key).into_iter().map(|(u, _)| u).collect();
        assert_eq!(stored, vec![scope.to_string()], "{key} is where the words say");
        let said = heard.of(marker);
        assert!(said.contains(words), "{marker}: {said}");
        let other = if words.contains("local") { "private memory" } else { "local memory" };
        assert!(!said.contains(other), "{marker} never names the other scope: {said}");
    }

    // Not the owner's own turn: a coworker's message, a workflow step.
    for origin in [Origin::Comm, Origin::Workflow] {
        let mut ctx = Nebo::ctx(&clerk, origin);
        ctx.user_id = private.clone();
        let key = format!("team/mem4-unattended-{origin:?}").to_lowercase();
        let refused = nebo
            .tool(&ctx, "remember", json!({ "key": key, "value": "Vendor list moved to the shared drive.", "scope": "local" }))
            .await;
        assert!(refused.is_error && refused.content.starts_with("Not saved to local memory"), "{origin:?}: {}", refused.content);
        assert!(rows(&nebo, &key).is_empty(), "{origin:?}: nothing landed anywhere");
    }
}

/// With no global store (this Nebo is not enrolled in one), nothing the model
/// is sent names global memory: not the memory tools, not their results, not
/// the prompt. Saving and recalling still work.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn with_no_global_store_nothing_names_global_memory() {
    let nebo = session().await;
    let clerk = nebo.hire("Proof Mem5 Clerk", json!({ "workflows": {} })).await;
    let memory_url = config::memory_url();
    assert!(
        !nebo.store().list_mcp_integrations().unwrap().iter().any(|i| i.server_url.as_deref() == Some(memory_url.as_str())),
        "no global store on this Nebo"
    );
    for def in nebo.state.tools.list().await.into_iter().filter(|d| ["remember", "recall", "forget"].contains(&d.name.as_str())) {
        let text = format!("{} {}", def.description, def.input_schema).to_lowercase();
        assert!(!text.contains("global"), "{} names global: {text}", def.name);
    }

    let heard = Arc::new(Heard::default());
    let sent = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let seen = sent.clone();
    let saw: Rule = Box::new(move |t| {
        if t.opener().contains("MARK-MEM5") {
            seen.lock().unwrap().push(format!("{}\n{}", t.req.system, t.req.messages.iter().map(|m| m.content.as_str()).collect::<Vec<_>>().join("\n")));
        }
        None
    });
    let rules = vec![
        saw,
        one_call("MARK-MEM5-SAVE", ("remember", json!({ "key": "team/mem5", "value": "Supplies are reordered on Mondays, MEM5-S.", "scope": "local" })), heard.clone()),
        one_call("MARK-MEM5-ASK", ("recall", json!({ "query": "reordered" })), heard.clone()),
    ];
    let rig = Rig::new(&nebo, rules).await;
    owner_in_chat(&rig, &clerk, "mem5-a", "MARK-MEM5-SAVE", "MARK-MEM5-SAVE save this for the team").await;
    owner_in_chat(&rig, &clerk, "mem5-b", "MARK-MEM5-ASK", "MARK-MEM5-ASK when are supplies reordered?").await;

    let results = format!("{}\n{}", heard.of("MARK-MEM5-SAVE"), heard.of("MARK-MEM5-ASK"));
    assert!(results.contains("Saved to local memory") && results.contains("MEM5-S"), "it all works: {results}");
    assert!(!results.to_lowercase().contains("global"), "no result names global: {results}");
    for prompt in sent.lock().unwrap().iter() {
        let lower = prompt.to_lowercase();
        assert!(!lower.contains("global memory") && !lower.contains("shared memory"), "the prompt names a global store that is not there");
    }
}

/// A global store this Nebo is enrolled in is reached through its one door:
/// the MCP integration at the platform's memory address, whose tools the
/// seat knows as the global store's. A save one employee makes there is read
/// by a second bot, another client of the same store. The store is a stand-in
/// on loopback (an in-memory fact list speaking MCP's JSON-RPC).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_enrolled_global_store_carries_a_save_to_a_second_bot() {
    let nebo = session().await;
    let clerk = nebo.hire("Proof Mem6 Clerk", json!({ "workflows": {} })).await;
    let store_url = global_store_stand_in().await;

    // Enrolled: the integration row sits at the memory address (what makes it
    // THE global store for the seat); its transport is the stand-in.
    let id = "proof-global-store";
    let memory_url = config::memory_url();
    nebo.store()
        .create_mcp_integration(id, "Global memory", "global", Some(&memory_url), "neboai", None, None)
        .unwrap();
    nebo.state.bridge.connect(id, "global", &store_url, None, None).await.expect("connect");
    let walled = agent::harness::seat::company_memory_tools(nebo.store(), &nebo.state.tools, &clerk).await;
    assert!(walled.contains("mcp__global__memory_remember"), "the seat knows the global store's tools: {walled:?}");

    let mut ctx = Nebo::ctx(&clerk, Origin::User);
    ctx.owner_request = true;
    let saved = nebo
        .tool(&ctx, "mcp__global__memory_remember", json!({ "src_id": 1, "predicate": "note", "text": "Office closes at noon on Fridays, GLOBAL-MEM6.", "evidence": "the owner said so", "source_ref": "chat" }))
        .await;

    // Leave the server as it was before reading, whatever the read says.
    nebo.state.bridge.disconnect(id).await;
    nebo.store().delete_mcp_integration(id).unwrap();
    assert!(!saved.is_error, "{}", saved.content);

    // A second bot: its own client of the same store.
    let second = reqwest::Client::new();
    let read: Value = second
        .post(&store_url)
        .json(&json!({ "jsonrpc": "2.0", "id": 9, "method": "tools/call", "params": { "name": "memory_recall", "arguments": { "src_id": 1 } } }))
        .send()
        .await
        .expect("second bot reads")
        .json()
        .await
        .expect("json");
    assert!(read.to_string().contains("GLOBAL-MEM6"), "the second bot reads what the first saved: {read}");
}

/// A global store on loopback: `memory_remember` appends a fact, and
/// `memory_recall` returns every fact for the entity. Returns its URL.
async fn global_store_stand_in() -> String {
    use axum::{Json, Router, routing::post};
    let facts: Arc<std::sync::Mutex<Vec<(i64, String)>>> = Default::default();
    let app = Router::new().route(
        "/mcp",
        post(move |Json(req): Json<Value>| {
            let facts = facts.clone();
            async move {
                let id = req["id"].clone();
                let result = match req["method"].as_str() {
                    Some("tools/list") => json!({ "tools": [
                        { "name": "memory_remember", "description": "Store a fact.", "inputSchema": { "type": "object", "properties": { "src_id": { "type": "integer" }, "predicate": { "type": "string" }, "text": { "type": "string" }, "evidence": { "type": "string" }, "source_ref": { "type": "string" } }, "required": ["src_id", "text"] } },
                        { "name": "memory_recall", "description": "Recall facts.", "inputSchema": { "type": "object", "properties": { "src_id": { "type": "integer" } }, "required": ["src_id"] } }
                    ]}),
                    Some("tools/call") => {
                        let args = &req["params"]["arguments"];
                        let src = args["src_id"].as_i64().unwrap_or(0);
                        let text = match req["params"]["name"].as_str() {
                            Some("memory_remember") => {
                                facts.lock().unwrap().push((src, args["text"].as_str().unwrap_or("").to_string()));
                                "stored".to_string()
                            }
                            _ => facts.lock().unwrap().iter().filter(|(s, _)| *s == src).map(|(_, t)| t.clone()).collect::<Vec<_>>().join("\n"),
                        };
                        json!({ "content": [{ "type": "text", "text": text }] })
                    }
                    _ => json!({}),
                };
                Json(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().unwrap();
    server_runtime().spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}/mcp")
}

/// Today's recipe exchange, replayed: the owner asks one employee to save a
/// recipe "to company memory" (the owner's words for local memory), and a
/// different employee, asked to pull the recipe up, finds it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_recipe_saved_for_everyone_is_found_by_another_employee() {
    let nebo = session().await;
    let first = nebo.hire("Proof Mem7 Assistant", json!({ "workflows": {} })).await;
    let second = nebo.hire("Proof Mem7 Planner", json!({ "workflows": {} })).await;
    let heard = Arc::new(Heard::default());
    let rules = vec![
        one_call(
            "MARK-MEM7-SAVE",
            ("remember", json!({
                "key": "recipes/chicken-tenderloin-bites",
                "value": "Chicken tenderloin bites: cube 1 lb, season with paprika and garlic, air fry at 380F for 9 minutes, shake halfway.",
                "layer": "project",
                "scope": "local"
            })),
            heard.clone(),
        ),
        one_call("MARK-MEM7-ASK", ("recall", json!({ "query": "chicken tenderloin bites" })), heard.clone()),
    ];
    let rig = Rig::new(&nebo, rules).await;
    owner_in_chat(&rig, &first, "mem7-a", "MARK-MEM7-SAVE", "MARK-MEM7-SAVE can you also save that recipe to company memory").await;
    owner_in_chat(&rig, &second, "mem7-b", "MARK-MEM7-ASK", "MARK-MEM7-ASK pull up the chicken tenderloin bites recipe").await;

    let owner = owner(&nebo);
    let stored: Vec<String> = rows(&nebo, "recipes/chicken-tenderloin-bites").into_iter().map(|(u, _)| u).collect();
    assert_eq!(stored, vec![owner], "the recipe is in local memory");
    let found = heard.of("MARK-MEM7-ASK");
    assert!(found.contains("air fry at 380F"), "the other employee finds it: {found}");
}

/// A turn keyed by the owner's latest words, not the thread's opener: the
/// latest message that names `marker` (a `MARK-` word) is the owner's newest
/// in this thread, so several turns of one conversation each get their own
/// script. It makes `calls` once, records what came back, and says
/// `DONE-<marker without MARK->`.
fn turn(marker: &'static str, calls: Vec<(&'static str, Value)>, heard: Arc<Heard>) -> Rule {
    Box::new(move |t| {
        let at = t
            .req
            .messages
            .iter()
            .rposition(|m| m.role == "user" && m.content.contains("MARK-"))?;
        if !t.req.messages[at].content.contains(marker) {
            return None;
        }
        let done = format!("DONE-{}", marker.trim_start_matches("MARK-"));
        // Already answered (a notification arriving later is not the ask).
        if t.req.messages[at..].iter().any(|m| m.role == "assistant" && m.content.contains(&done)) {
            return None;
        }
        if calls.is_empty() {
            return Some(Step::say(done));
        }
        if t.has_tool_results() {
            heard.push(marker, t);
            return Some(Step::say(done));
        }
        Some(Step::call(calls.iter().map(|(n, v)| (*n, v.clone())).collect()))
    })
}

/// The owner writes `text` (naming `marker`) in the conversation `chat` with
/// employee `agent_id`, and the turn runs to the script's `DONE-` word.
async fn owner_turn(rig: &Rig<'_>, agent_id: &str, chat: &str, marker: &str, text: &str) {
    let key = format!("agent:{agent_id}:thread:{chat}");
    rig.owner_writes(&key, agent_id, None, text).await;
    let done = format!("DONE-{}", marker.trim_start_matches("MARK-"));
    rig.until(30, &format!("{marker} ends in {chat}"), || {
        rig.thread(&key).iter().any(|m| m.role == "assistant" && m.content.contains(&done))
    })
    .await;
}

/// Every row whose value names `needle`, as (user_id, value).
fn rows_naming(nebo: &Nebo, needle: &str) -> Vec<(String, String)> {
    let conn = rusqlite::Connection::open(nebo.home.join("data").join("nebo.db")).expect("db");
    let mut stmt = conn.prepare("SELECT user_id, value FROM memories WHERE value LIKE ?1").expect("prepare");
    stmt.query_map([format!("%{needle}%")], |r| Ok((r.get(0)?, r.get(1)?)))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows")
}

/// The owner's worry, as real turns: a Confidential employee (a law office's
/// counsel) holds conversation A about client A and conversation B about
/// client B. What A learns — saved by the employee, or extracted on its own
/// after a turn — is never visible in B, and what B learns never in A: not
/// by recall (words, key, listing), not in what the model is sent (the
/// prompt, the relevant-memories recall), not by searching past
/// conversations, not through a helper B starts. The owner's own
/// conversations are sealed too: both are the owner's. A fact the owner
/// saved to local memory is visible in both. Every row sits where the words
/// say: A's in A's scope, B's in B's, the local fact in local memory, and
/// nothing in the employee's private memory.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_confidential_employees_conversations_never_see_each_other() {
    let nebo = session().await;
    let counsel = nebo.hire("Proof Mem8 Counsel", json!({ "workflows": {} })).await;
    // The owner picks Confidential in the employee's settings.
    nebo.put_ok(&format!("/agents/{counsel}"), &json!({ "memoryMode": "confidential" })).await;
    assert_eq!(crate::workflow_manager::agent_memory_mode(nebo.store(), &counsel), napp::agent::MemoryMode::Confidential);
    assert_eq!(nebo.get_ok(&format!("/agents/{counsel}")).await["memoryMode"], "confidential");

    let heard = Arc::new(Heard::default());
    // Every request the model is sent in a turn, with the owner words it came in.
    let sent = Arc::new(std::sync::Mutex::new(Vec::<(String, String)>::new()));
    let seen = sent.clone();
    let saw: Rule = Box::new(move |t| {
        let owner_words: Vec<&str> = t.req.messages.iter().filter(|m| m.role == "user" && m.content.contains("MARK-MEM8")).map(|m| m.content.as_str()).collect();
        if !owner_words.is_empty() {
            let text = format!("{}\n{}", t.req.system, t.req.messages.iter().map(|m| m.content.as_str()).collect::<Vec<_>>().join("\n"));
            seen.lock().unwrap().push((owner_words.join(" | "), text));
        }
        None
    });
    let asks = |about: &'static str, key: &'static str| {
        vec![
            ("recall", json!({ "query": about })),
            ("recall", json!({ "query": key })),
            ("recall", json!({ "namespace": "project" })),
            ("recall", json!({ "query": "mediation" })),
            ("search_history", json!({ "query": about })),
            ("recall", json!({ "query": "office closes Fridays" })),
        ]
    };
    let rules = vec![
        saw,
        turn("MARK-MEM8-A-SAVE", vec![("remember", json!({ "key": "case/harlow-settlement", "value": "The Harlow settlement offer is 410,000, ALDER-MEM8.", "layer": "project" }))], heard.clone()),
        turn("MARK-MEM8-A-TALK", vec![], heard.clone()),
        turn("MARK-MEM8-LOCAL", vec![("remember", json!({ "key": "office/friday-close", "value": "The office closes at four on Fridays, CEDAR-MEM8.", "scope": "local" }))], heard.clone()),
        turn("MARK-MEM8-B-SAVE", vec![("remember", json!({ "key": "case/pryce-deposition", "value": "The Pryce deposition is on the 14th, BIRCH-MEM8.", "layer": "project" }))], heard.clone()),
        turn("MARK-MEM8-B-ASK", asks("Harlow settlement", "case/harlow-settlement"), heard.clone()),
        turn("MARK-MEM8-A-ASK", asks("Pryce deposition", "case/pryce-deposition"), heard.clone()),
        turn("MARK-MEM8-B-HELP", vec![("delegate", json!({ "description": "check the file", "prompt": "MARK-MEM8-HELPER check the file for the Harlow settlement" }))], heard.clone()),
        turn("MARK-MEM8-HELPER", asks("Harlow settlement", "case/harlow-settlement"), heard.clone()),
    ];
    let rig = Rig::new(&nebo, rules).await;
    // Automatic extraction after A's plain turn learns a fact of its own.
    rig.answer_background(Box::new(|t| {
        let prompt = t.req.messages.first().map(|m| m.content.as_str()).unwrap_or("");
        (t.req.system.starts_with("You are a precise fact extractor") && prompt.contains("MARK-MEM8-A-TALK")).then(|| {
            Step::say(
                json!({ "topics": [{ "key": "harlow-mediation", "value": "The Harlow mediation is set for the 9th, DOGWOOD-MEM8.", "category": "topic", "confidence": 0.95, "explicit": true }] })
                    .to_string(),
            )
        })
    }));

    owner_turn(&rig, &counsel, "mem8-a", "MARK-MEM8-A-SAVE", "MARK-MEM8-A-SAVE client A: remember the Harlow settlement offer is 410,000").await;
    owner_turn(&rig, &counsel, "mem8-a", "MARK-MEM8-A-TALK", "MARK-MEM8-A-TALK also, the Harlow mediation is set for the 9th").await;
    rig.until(30, "extraction files A's mediation fact", || !rows_naming(&nebo, "DOGWOOD-MEM8").is_empty()).await;
    owner_turn(&rig, &counsel, "mem8-local", "MARK-MEM8-LOCAL", "MARK-MEM8-LOCAL save to local memory for everyone: the office closes at four on Fridays").await;
    owner_turn(&rig, &counsel, "mem8-b", "MARK-MEM8-B-SAVE", "MARK-MEM8-B-SAVE client B: remember the Pryce deposition is on the 14th").await;
    owner_turn(&rig, &counsel, "mem8-b", "MARK-MEM8-B-ASK", "MARK-MEM8-B-ASK what do we know about the Harlow settlement and the mediation? when does the office close?").await;
    owner_turn(&rig, &counsel, "mem8-a", "MARK-MEM8-A-ASK", "MARK-MEM8-A-ASK what do we know about the Pryce deposition? when does the office close?").await;
    owner_turn(&rig, &counsel, "mem8-b", "MARK-MEM8-B-HELP", "MARK-MEM8-B-HELP have a helper check the file").await;
    rig.until(60, "the helper ran its lookups", || !heard.of("MARK-MEM8-HELPER").is_empty()).await;

    // Where every row sits.
    let owner = owner(&nebo);
    let a = format!("{owner}:agent:{counsel}:matter:mem8-a");
    let b = format!("{owner}:agent:{counsel}:matter:mem8-b");
    let scopes = |needle: &str| rows_naming(&nebo, needle).into_iter().map(|(u, _)| u).collect::<Vec<_>>();
    assert_eq!(scopes("ALDER-MEM8"), vec![a.clone()], "A's save is in A's conversation");
    assert_eq!(scopes("DOGWOOD-MEM8"), vec![a.clone()], "the fact extracted after A's turn is in A's conversation too");
    assert_eq!(scopes("BIRCH-MEM8"), vec![b.clone()], "B's save is in B's conversation");
    assert_eq!(scopes("CEDAR-MEM8"), vec![owner.clone()], "the local fact is in local memory");
    let private = format!("{owner}:agent:{counsel}");
    let conn = rusqlite::Connection::open(nebo.home.join("data").join("nebo.db")).expect("db");
    let in_private: i64 = conn.query_row("SELECT COUNT(*) FROM memories WHERE user_id = ?1", [&private], |r| r.get(0)).unwrap();
    assert_eq!(in_private, 0, "nothing is written to the employee's private memory");

    // The words the model read.
    let saved = heard.of("MARK-MEM8-A-SAVE");
    assert!(saved.contains("Saved to this conversation's confidential memory"), "the save names its scope: {saved}");
    assert!(heard.of("MARK-MEM8-LOCAL").contains("Saved to local memory"), "{}", heard.of("MARK-MEM8-LOCAL"));
    for (marker, other) in [("MARK-MEM8-B-ASK", ["ALDER-MEM8", "DOGWOOD-MEM8"]), ("MARK-MEM8-A-ASK", ["BIRCH-MEM8", "Pryce deposition is on"]), ("MARK-MEM8-HELPER", ["ALDER-MEM8", "DOGWOOD-MEM8"])] {
        let got = heard.of(marker);
        for word in other {
            assert!(!got.contains(word), "{marker} reached the other conversation's {word}: {got}");
        }
        assert!(got.contains("CEDAR-MEM8"), "{marker} reads local memory: {got}");
    }
    // The other conversation's own words are not found by searching history
    // either (the searched conversation's own words are).
    assert!(!heard.of("MARK-MEM8-B-ASK").contains("410,000"), "{}", heard.of("MARK-MEM8-B-ASK"));

    // What the model was sent, turn by turn: A's facts never in B's
    // requests, B's never in A's.
    for (words, text) in sent.lock().unwrap().iter() {
        let in_b = words.contains("MARK-MEM8-B-") || words.contains("MARK-MEM8-HELPER");
        let in_a = words.contains("MARK-MEM8-A-");
        if in_b && !in_a {
            for word in ["ALDER-MEM8", "DOGWOOD-MEM8", "410,000", "mediation is set"] {
                assert!(!text.contains(word), "a request in B carried A's {word}: {words}");
            }
        }
        if in_a && !in_b {
            for word in ["BIRCH-MEM8", "Pryce deposition is on"] {
                assert!(!text.contains(word), "a request in A carried B's {word}: {words}");
            }
        }
    }
}

/// Separate conversations still share one memory: the mode the old
/// isolation flag maps to keeps what the flag did. A fact the owner has the
/// employee save in one conversation is in its private memory and recalled
/// in another, and a caller's conversation of the same employee still gets
/// its own sealed memory.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn separate_conversations_still_share_one_memory() {
    let nebo = session().await;
    let clerk = nebo.hire("Proof Mem9 Clerk", json!({ "workflows": {} })).await;
    nebo.put_ok(&format!("/agents/{clerk}"), &json!({ "memoryMode": "separate" })).await;
    assert_eq!(nebo.get_ok(&format!("/agents/{clerk}")).await["memoryMode"], "separate");
    let heard = Arc::new(Heard::default());
    let rules = vec![
        turn("MARK-MEM9-SAVE", vec![("remember", json!({ "key": "owner/mem9-parking", "value": "The owner parks in bay 12, FERN-MEM9.", "layer": "tacit" }))], heard.clone()),
        turn("MARK-MEM9-ASK", vec![("recall", json!({ "query": "parks in bay" })), ("search_history", json!({ "query": "bay 12" }))], heard.clone()),
    ];
    let rig = Rig::new(&nebo, rules).await;
    owner_turn(&rig, &clerk, "mem9-a", "MARK-MEM9-SAVE", "MARK-MEM9-SAVE remember I park in bay 12").await;
    owner_turn(&rig, &clerk, "mem9-b", "MARK-MEM9-ASK", "MARK-MEM9-ASK where do I park?").await;

    let owner = owner(&nebo);
    assert_eq!(rows(&nebo, "owner/mem9-parking").into_iter().map(|(u, _)| u).collect::<Vec<_>>(), vec![format!("{owner}:agent:{clerk}")], "the private memory");
    let found = heard.of("MARK-MEM9-ASK");
    assert!(found.contains("FERN-MEM9") && found.contains("(private memory"), "the next conversation recalls it: {found}");
    assert!(found.contains("bay 12"), "and past conversations are searchable: {found}");

    // A caller's conversation of the same employee is sealed, as it always was.
    let mut caller = Nebo::ctx(&clerk, Origin::Caller);
    caller.user_id = agent::memory::resolve_memory_scope(&owner, &clerk, napp::agent::MemoryMode::Separate, Origin::Caller, Some("call-9"), None).user_id;
    assert_eq!(caller.user_id, format!("{owner}:agent:{clerk}:ctx:call-9"));
    let saved = nebo.tool(&caller, "remember", json!({ "key": "caller/mem9", "value": "The caller asked about a refund." })).await;
    assert!(saved.content.starts_with("Saved to this conversation's sealed memory"), "{}", saved.content);
}
