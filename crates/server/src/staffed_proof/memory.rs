//! Memory on one Nebo: local memory is shared by every employee on it,
//! private memory is one employee's own, and neither is ever filed under a
//! single conversation with the owner.
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
        .hire("Proof Mem3 Primary", json!({ "workflows": {}, "memory": { "context_isolated": true } }))
        .await;
    assert!(crate::workflow_manager::agent_context_isolated(nebo.store(), &primary), "isolation is on");
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
