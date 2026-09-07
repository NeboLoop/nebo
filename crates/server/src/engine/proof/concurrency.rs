//! Concurrency: many employees at once, and one employee with many things
//! in flight at once — cases, tasks, heartbeats, approvals. The engine
//! serializes exactly two things: one live turn per case, and one turn per
//! session. Everything else runs side by side, and the store takes the
//! contention. Every scenario here asserts that nothing is lost, nothing
//! is doubled, and nothing errors under pressure.

use super::*;
use serde_json::json;
use std::sync::Arc;

fn lead(agent: &'static str) -> CaseBinding<'static> {
    World::binding(agent, "work-lead", "lead", 3 * DAY)
}

/// Four employees, forty people, eight threads arriving at once, one tick
/// thread running the whole time. Every person gets exactly one case and
/// one first turn; no employee's work waits on another's.
#[test]
fn four_employees_forty_people_arrive_at_once() {
    let store = Arc::new(fresh_store());
    let t = 1_700_000_000;
    let ticking = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let ticker = {
        let (s, ticking) = (store.clone(), ticking.clone());
        std::thread::spawn(move || {
            let mut reports = Vec::new();
            while ticking.load(std::sync::atomic::Ordering::Relaxed) {
                reports.push(tick(&s, t, &idle, &no_steer));
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            reports
        })
    };
    let agents = ["ic", "closer", "support", "billing"];
    let workers: Vec<_> = (0..8)
        .map(|i| {
            let s = store.clone();
            std::thread::spawn(move || {
                let agent = agents[i % 4];
                let b = lead(agent);
                for p in 0..5 {
                    let email = format!("p{}-{}@x.com", i, p);
                    let payload = json!({"email": email, "message": "hi"});
                    let idem = format!("arrive:{i}:{p}");
                    let r = signal_or_open(&s, &b, "email", &email, &payload, "event", &idem, t).expect("no store error under contention");
                    assert!(matches!(r, Routed::Opened { .. }), "{email}: {r:?}");
                }
            })
        })
        .collect();
    for h in workers {
        h.join().unwrap();
    }
    // Let the ticker see the final state, then stop it.
    std::thread::sleep(std::time::Duration::from_millis(50));
    ticking.store(false, std::sync::atomic::Ordering::Relaxed);
    let reports = ticker.join().unwrap();
    assert!(!reports.is_empty());

    let cases = store.engine_cases(None, 100).unwrap();
    let mut seen = std::collections::HashMap::<String, Vec<String>>::new();
    for c in &cases {
        let aliases = c.inputs.as_deref().and_then(|i| serde_json::from_str::<serde_json::Value>(i).ok()).map(|v| v["_case"]["aliases"].to_string()).unwrap_or_default();
        seen.entry(aliases).or_default().push(format!("{} {} {}", &c.id[..8], c.agent_id, c.state));
    }
    let dup: Vec<_> = seen.iter().filter(|(_, v)| v.len() > 1).collect();
    assert_eq!(cases.len(), 40, "one case per person; people with more than one case: {dup:?}");
    for agent in agents {
        assert_eq!(store.engine_cases(Some(agent), 100).unwrap().len(), 10, "{agent} has its ten");
    }
    let turns = store.engine_queued_runs_of_kind("workflow", 100).unwrap();
    assert_eq!(turns.len(), 40, "one first turn each, none doubled by the ticker");
    let mut parents: Vec<_> = turns.iter().map(|t| t.parent_run_id.clone().unwrap()).collect();
    parents.sort();
    parents.dedup();
    assert_eq!(parents.len(), 40);
    assert!(store.engine_undelivered_signals().unwrap().is_empty(), "nothing left owed");
}

/// One employee with everything in flight at once: twenty cases, five
/// tasks, a heartbeat, a parked approval. One tick delivers all of it; the
/// twenty first turns run side by side; each case still has exactly one.
#[test]
fn one_employee_many_things_at_once() {
    let mut w = World::new();
    let b = lead("ic");
    let mut cases = Vec::new();
    for p in 0..20 {
        let email = format!("c{p}@x.com");
        let (case, _) = w.open(&b, &email, "hello", &format!("m{p}"));
        cases.push(case);
    }
    for i in 0..5 {
        w.s.create_pending_task(&format!("task-{i}"), "task", "agent:ic:web", None, "do", None, None, None, 0, None).unwrap();
    }
    w.s.engine_enqueue_event(&NewEvent { kind: "timer", target_type: "entity", target_id: "heartbeat:agent:ic", idem_key: "hb", due_at: Some(w.t), schedule: Some("1800"), ..Default::default() }).unwrap();
    w.s.engine_create_run(&NewRun { id: "wf-park", kind: "workflow", session_key: "agent:ic:workflow:wf-park", agent_id: "ic", lane: "main", ..Default::default() }).unwrap();
    w.s.engine_declare_wait("wf-park", &NewWait { action: "resume", on_kind: "approval", key: "approval:wf-park", parked: Some("{}"), reason: "sign", ..Default::default() }, w.t).unwrap();
    w.s.engine_enqueue_event(&NewEvent { kind: "approval", target_type: "run", target_id: "approval:wf-park", payload: "yes", idem_key: "ok", durable: true, ..Default::default() }).unwrap();

    let r = w.tick();
    assert_eq!((r.fired, r.resumed, r.unrouted), (1, 1, 0));
    assert_eq!(w.s.engine_queued_runs_of_kind("workflow", 100).unwrap().len(), 21, "twenty first turns and the resumed workflow");
    assert_eq!(w.s.engine_queued_runs_of_kind("task", 100).unwrap().len(), 5);
    assert_eq!(w.s.engine_queued_runs_of_kind("heartbeat", 100).unwrap().len(), 1);

    // All twenty turns run at once (distinct sessions); a second message to
    // each person rides its own running turn, never a second turn.
    for case in &cases {
        let turn = w.queued_turn(case).unwrap();
        w.start(&turn.id);
    }
    for (p, case) in cases.iter().enumerate() {
        let email = format!("c{p}@x.com");
        assert_eq!(w.arrive(&b, "email", &email, json!({"email": email, "message": "again"}), &format!("again{p}")), Routed::Signaled { case_id: case.clone() });
    }
    let live = |k: &str| Some(format!("{k}:run::0"));
    let r = tick(&w.s, w.t, &live, &no_steer);
    assert_eq!((r.steered, r.children_started), (20, 0), "twenty steered into twenty running turns");
    for case in &cases {
        assert_eq!(w.s.engine_children(case).unwrap().len(), 1, "one turn per case, still");
    }
    // The turns settle in any order; each case waits on its own clock.
    for (p, case) in cases.iter().enumerate() {
        let turn = w.s.engine_children(case).unwrap().remove(0);
        w.t += 1;
        w.finish(&turn, &waits("contacted", "sent", "signal", &format!("{}d", 1 + p % 3), "reply"));
        assert_eq!(w.run(case).state, "waiting");
    }
    let deadlines: std::collections::HashSet<i64> = cases.iter().map(|c| w.wait(c).unwrap().deadline.unwrap()).collect();
    assert!(deadlines.len() >= 3, "independent clocks");
}

/// A burst larger than one tick's claim batch is drained over ticks with
/// nothing lost and nothing doubled: a hundred and twenty people in one
/// second become a hundred and twenty cases and a hundred and twenty turns.
#[test]
fn a_burst_is_drained_over_ticks_without_loss() {
    let w = World::new();
    let b = lead("ic");
    let mut opened = 0;
    for p in 0..120 {
        let email = format!("b{p}@x.com");
        if matches!(w.arrive(&b, "email", &email, json!({"email": email}), &format!("burst{p}")), Routed::Opened { .. }) {
            opened += 1;
        }
    }
    assert_eq!(opened, 120);
    // Their first turns are queued at arrival; a second wave of replies is
    // what the ticks must drain: 120 signals, 50 per tick.
    for p in 0..120 {
        let email = format!("b{p}@x.com");
        w.arrive(&b, "email", &email, json!({"email": email, "message": "reply"}), &format!("reply{p}"));
    }
    let mut steered = 0;
    let mut ticks = 0;
    while steered < 120 {
        let r = w.tick();
        steered += r.steered;
        ticks += 1;
        assert!(ticks <= 10, "drained in a bounded number of ticks");
        assert_eq!(r.children_started, 0, "the replies ride the queued first turns");
    }
    assert_eq!(steered, 120);
    assert!(ticks >= 3, "more than one tick's batch");
    assert_eq!(w.s.engine_queued_runs_of_kind("workflow", 200).unwrap().len(), 120, "still one turn per person");
    assert!(w.s.engine_undelivered_signals().unwrap().is_empty());
}

/// The store under contention: sixteen threads writing waits, events, and
/// ledger rows while a tick thread runs. No write fails, every row lands,
/// and the ledger's one-message-per-person rule holds across threads.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn the_store_and_the_ledger_hold_under_contention() {
    let store = Arc::new(fresh_store());
    let t = 1_700_000_000;
    for i in 0..16 {
        store.engine_create_run(&NewRun { id: &format!("run-{i}"), kind: "workflow", session_key: &format!("agent:ic:workflow:run-{i}"), agent_id: "ic", lane: "main", ..Default::default() }).unwrap();
    }
    let ticking = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let ticker = {
        let (s, ticking) = (store.clone(), ticking.clone());
        std::thread::spawn(move || {
            while ticking.load(std::sync::atomic::Ordering::Relaxed) {
                tick(&s, t, &idle, &no_steer);
            }
        })
    };
    let mut handles = Vec::new();
    for i in 0..16 {
        let s = store.clone();
        handles.push(tokio::spawn(async move {
            let run = format!("run-{i}");
            let ctx = World::ctx("ic", &run);
            let mut sent = 0;
            for k in 0..25 {
                s.engine_enqueue_event(&NewEvent { kind: "note", target_type: "run", target_id: &run, payload: "x", idem_key: &format!("n:{i}:{k}"), durable: true, ..Default::default() }).expect("enqueue under contention");
                s.engine_declare_wait(&run, &NewWait { action: "resume", on_kind: "signal", key: &format!("k{i}"), reason: "w", ..Default::default() }, t).expect("wait under contention");
                // Every thread tries to message the same person four times in
                // four wordings; the ledger lets one through per run.
                let input = json!({"to": "same@person.com", "body": format!("wording {k}")});
                let r = guarded_send(&s, &ctx, "messaging", "mail-app", "mail.message.send", &input, || async { SendOutcome::Sent("ok".into(), None) }).await;
                if !r.is_error {
                    sent += 1;
                }
            }
            sent
        }));
    }
    let mut total_sent = 0;
    for h in handles {
        total_sent += h.await.unwrap();
    }
    ticking.store(false, std::sync::atomic::Ordering::Relaxed);
    ticker.join().unwrap();
    assert_eq!(total_sent, 16, "one message to the person per run, sixteen runs");
    for i in 0..16 {
        let run = format!("run-{i}");
        let completed = store.engine_effects_for_run(&run).unwrap().iter().filter(|e| e.state == "completed").count();
        assert_eq!(completed, 1, "{run}");
        assert_eq!(store.engine_waits_for_run(&run).unwrap().len(), 25, "{run}: every wait landed");
    }
}
