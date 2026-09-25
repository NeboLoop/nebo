//! Proof scenarios: long-running work (Batch E, E4 and the design for
//! E1–E4 and E13–E18). Every ask is a durable wait in the engine with no
//! expiry, answered by the owner's signal. See `mod.rs`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use agent::harness::permissions::{Answer, AnsweredVia, Ask, AskStatus, AskSurfaces, Asks, Check};
use tools::registry::DynTool;
use tools::{Origin, Registry, ToolResult};
use types::permissions::Door;

use super::*;

/// What reached the owner and the employee.
#[derive(Default)]
struct Owner {
    reminded: Mutex<Vec<String>>,
    told: Mutex<Vec<String>>,
}

impl AskSurfaces for Owner {
    fn card(&self, _ask: &Ask) {}
    fn remind(&self, ask: &Ask) {
        self.reminded.lock().unwrap().push(ask.id.clone());
    }
    fn resolved(&self, _ask: &Ask) {}
    fn notify(&self, _session_key: &str, text: &str) {
        self.told.lock().unwrap().push(text.to_string());
    }
    fn release_run(&self, _run_id: &str, _allowed: bool) {}
}

/// A text message: outside the employee's job, so it asks.
struct Text(Arc<AtomicUsize>);

impl DynTool for Text {
    fn name(&self) -> &str {
        "text"
    }
    fn description(&self) -> String {
        String::new()
    }
    fn schema(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object" })
    }
    fn capability(&self, _input: &serde_json::Value) -> Option<&'static str> {
        Some("sms")
    }
    fn activity(&self, input: &serde_json::Value) -> String {
        format!("texting {}", input["to"].as_str().unwrap_or(""))
    }
    fn execute_dyn<'a>(
        &'a self,
        _ctx: &'a ToolContext,
        _input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { ToolResult::ok("SENT") })
    }
}

/// One process of Nebo over a database file: the store, the one check and
/// its asks, the registry, and what reached the owner.
struct Process {
    store: Arc<Store>,
    asks: Arc<Asks>,
    reg: Arc<Registry>,
    owner: Arc<Owner>,
}

impl Process {
    async fn boot(path: &std::path::Path, ran: &Arc<AtomicUsize>) -> Process {
        let store = Arc::new(Store::new(&path.to_string_lossy()).expect("store"));
        store.add_employee_counterparty("ops", "+15550142", "sent").unwrap();
        let check = Arc::new(Check::new(store.clone()));
        let asks = check.asks();
        let owner = Arc::new(Owner::default());
        asks.attach(owner.clone());
        let reg = Arc::new(Registry::new(check));
        reg.register(Box::new(Text(ran.clone()))).await;
        Process { store, asks, reg, owner }
    }

    /// One engine tick at `t`, then the asks it woke, as `drive` runs them.
    async fn tick(&self, t: i64) -> TickReport {
        let report = tick(&self.store, t, &idle, &no_steer);
        for task in crate::engine::resume_asks(&self.store, &self.asks, &self.reg, t) {
            task.await.unwrap();
        }
        report
    }

    fn reminded(&self) -> usize {
        self.owner.reminded.lock().unwrap().len()
    }
}

/// E4 — "An approval can take until Thursday." An employee's step asks the
/// owner on Monday. Nobody answers: on Tuesday and again on Thursday the
/// card comes back to the owner, and the ask stays open — never a No,
/// nothing run. Nebo restarts in between; the wait is still there. On
/// Friday, four days on, the owner answers: the step runs once and the
/// employee hears it. Must never: an ask that expires, a silence that
/// counts as No, a reminder that runs the call, a restart that loses the
/// wait.
#[tokio::test]
async fn e4_an_ask_waits_four_days_across_a_restart_and_the_answer_resumes_it() {
    let path = std::env::temp_dir().join(format!("nebo-proof-{}.db", uuid::Uuid::new_v4()));
    let ran = Arc::new(AtomicUsize::new(0));
    let monday = 1_700_000_000;
    let p = Process::boot(&path, &ran).await;

    let mut ctx = ToolContext::new(Origin::System).with_session("agent:ops:heartbeat", "s1");
    ctx.door = Door::Heartbeat;
    let parked = p.reg.execute(&ctx, "text", serde_json::json!({ "to": "+15550142" })).await;
    let id = parked.parked_ask.expect("the step outside the job asks");
    assert_eq!(ran.load(Ordering::SeqCst), 0);
    let run = p.store.engine_get_run(&id).unwrap().expect("the ask is a run in the engine");
    assert_eq!((run.kind.as_str(), run.state.as_str()), ("ask", "waiting"));
    let created = p.asks.get(&id).unwrap().unwrap().created_at;
    let at = |secs: i64| created.max(monday) + secs;

    // Monday evening: nothing is due.
    assert_eq!(p.tick(at(8 * HOUR)).await.resumed, 0);
    assert_eq!(p.reminded(), 0);

    // Tuesday: the first reminder; still open, still waiting.
    assert_eq!(p.tick(at(DAY + 60)).await.resumed, 1, "the wait's timer woke the ask");
    assert_eq!(p.reminded(), 1);
    assert_eq!(p.asks.get(&id).unwrap().unwrap().status, AskStatus::Open);
    assert_eq!(p.store.engine_get_run(&id).unwrap().unwrap().state, "waiting");

    // Nebo restarts on Wednesday: the ask and its next reminder survive.
    drop(p);
    let p = Process::boot(&path, &ran).await;
    crate::engine::recover(&p.store);
    assert_eq!(p.asks.open(None).unwrap().len(), 1, "the ask survived the restart");
    assert_eq!(p.store.engine_get_run(&id).unwrap().unwrap().state, "waiting");
    assert_eq!(p.tick(at(2 * DAY)).await.resumed, 0, "the next reminder is not due yet");

    // Thursday: reminded again. Friday, four days on: still open.
    assert_eq!(p.tick(at(3 * DAY + 120)).await.resumed, 1);
    assert_eq!(p.reminded(), 1, "this process reminded once (Thursday)");
    assert_eq!(p.tick(at(4 * DAY)).await.resumed, 0);
    let four_days_on = p.asks.get(&id).unwrap().unwrap();
    assert_eq!(four_days_on.status, AskStatus::Open, "never expired, never a No");
    assert_eq!(ran.load(Ordering::SeqCst), 0, "no reminder ran the call");
    assert!(p.owner.told.lock().unwrap().is_empty(), "the employee was told nothing yet");

    // Friday: the owner answers from the phone. The answer is the signal.
    p.asks.answer(&id, Answer::ThisOnce, AnsweredVia::Mobile).unwrap();
    let report = p.tick(at(4 * DAY + 30)).await;
    assert_eq!(report.resumed, 1, "the answer woke the ask's wait");
    assert_eq!(ran.load(Ordering::SeqCst), 1, "the step ran once");
    let told = p.owner.told.lock().unwrap().clone();
    assert_eq!(told.len(), 1);
    assert!(told[0].contains("allowed, this once\nIt ran:\nSENT"), "{}", told[0]);
    assert_eq!(p.store.engine_get_run(&id).unwrap().unwrap().state, "done");

    // Later ticks change nothing: the ask's timers are gone with its wait.
    p.tick(at(12 * DAY)).await;
    assert_eq!((ran.load(Ordering::SeqCst), p.reminded()), (1, 1));
}
