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

/// E13 — A temporary workflow's outcome reaches the owner, once, before it
/// is deleted. Its one run has ended: the outcome is written to the owner's
/// Inbox and the session that started the work is told, with the run named
/// so the same work can be saved. Delivering it again (a crash between the
/// report and the delete) repeats nothing. Must never: a temporary workflow
/// deleted before its outcome is written, or an outcome told twice.
#[test]
fn e13_a_temporary_workflows_outcome_reaches_the_owner_once_before_it_goes() {
    let w = World::new();
    let chat = "agent:ops:web";
    w.s.mark_temporary(db::TemporaryKind::Workflow, "ops", "budget-check", chat).unwrap();
    w.s.engine_create_run(&NewRun { id: "run-b", kind: "workflow", session_key: "agent:ops:workflow:run-b", agent_id: "ops", lane: "main", ..Default::default() }).unwrap();
    assert_eq!(w.s.claim_temporary_run(db::TemporaryKind::Workflow, "ops", "budget-check", "run-b").unwrap(), db::TemporaryClaim::Claimed);
    assert!(w.s.ended_temporary_work().unwrap().is_empty(), "still running: nothing to finish");

    w.s.engine_set_run_result("run-b", "BUDGET-RESULT: $1,200 a month", None).unwrap();
    w.s.engine_set_run_state("run-b", "done", w.t, None).unwrap();
    let ended = w.s.ended_temporary_work().unwrap();
    assert_eq!(ended.len(), 1, "its one run ended: it is finished next");
    let (work, run) = &ended[0];
    for _ in 0..2 {
        crate::engine::report_temporary_outcome(&w.s, work, run, None).unwrap();
    }

    let user = w.s.ensure_local_user_id().unwrap();
    let inbox = w.s.get_notification("temporary:run-b", &user).unwrap().expect("the outcome is in the Inbox");
    assert_eq!(inbox.title, "The budget check workflow finished");
    assert_eq!(inbox.body.as_deref(), Some("BUDGET-RESULT: $1,200 a month"));
    let (told, _) = w.s.engine_claim_session_events(chat, w.t).unwrap();
    assert_eq!(told.len(), 1, "told once: {told:?}");
    assert_eq!(told[0].kind, crate::engine::TEMPORARY_WORK_ENDED);
    assert!(told[0].payload.contains("(run run-b)") && told[0].payload.contains("from_run: \"run-b\""), "{}", told[0].payload);
    assert!(told[0].payload.contains("BUDGET-RESULT"), "{}", told[0].payload);
}

/// E16 — A temporary team's one piece of work is the assignment its lead
/// takes. While the lead works it the team stands; when the lead closes it,
/// the team is due to disband, and its outcome is the lead's summary, told
/// once to the owner and to the session that assembled the team. A
/// persistent team is never listed. Must never: a temporary team disbanded
/// while its work is open, or a persistent team disbanded at all.
#[test]
fn e16_a_temporary_team_is_due_to_disband_when_its_lead_closes_its_work() {
    let w = World::new();
    let chat = "agent:assistant:web";
    let lead_first = [db::TeamMember::local("bk"), db::TeamMember::local("mk")];
    let team = w.s.create_team("t-budget", "Budget team", "", &lead_first, "bk", None).unwrap();
    w.s.create_team("t-ops", "Ops", "", &lead_first, "bk", None).unwrap();
    w.s.mark_temporary(db::TemporaryKind::Team, "", &team.id, chat).unwrap();

    let req = workflow::cases::NewAssignmentRequest {
        assigner_agent_id: "assistant",
        assigner_name: "Nebo",
        assigner_session_key: chat,
        parent_run_id: None,
        assignee_agent_id: "bk",
        subject: "Find the marketing budget and what it buys",
        done_means: "A number and a plan",
        due: None,
    };
    let assignment = workflow::cases::open_assignment(&w.s, &req, w.t).unwrap();
    let case = w.s.engine_run_for_key("case:assignment", &assignment).unwrap().expect("the lead's case");
    assert_eq!(w.s.claim_temporary_run(db::TemporaryKind::Team, "", &team.id, &case.id).unwrap(), db::TemporaryClaim::Claimed);
    assert!(w.s.ended_temporary_work().unwrap().is_empty(), "the lead is working it: the team stands");

    w.turn(&case.id, &closes("done", "TEAM-RESULT: $900 for marketing, enough for two local ads"));
    let ended = w.s.ended_temporary_work().unwrap();
    assert_eq!(ended.len(), 1, "one temporary team is due to disband; the persistent one never is");
    let (work, run) = &ended[0];
    assert_eq!((work.name.as_str(), run.id.as_str()), (team.id.as_str(), case.id.as_str()));
    crate::engine::report_temporary_outcome(&w.s, work, run, None).unwrap();
    let user = w.s.ensure_local_user_id().unwrap();
    let inbox = w.s.get_notification(&format!("temporary:{}", case.id), &user).unwrap().expect("the outcome is in the Inbox");
    assert_eq!(inbox.title, "The Budget team finished");
    assert!(inbox.body.unwrap_or_default().contains("TEAM-RESULT"), "the lead's summary is the outcome");
    let told: Vec<_> = w.s.engine_claim_session_events(chat, w.t).unwrap().0.into_iter().filter(|e| e.kind == crate::engine::TEMPORARY_WORK_ENDED).collect();
    assert_eq!(told.len(), 1);
    assert!(told[0].payload.contains("It was a temporary team, so it has disbanded."), "{}", told[0].payload);
}
