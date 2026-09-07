//! The parity matrix: every behaviour of the seven mechanisms the engine
//! replaced — scheduled jobs, heartbeats, sub-agent tasks, workflow runs,
//! parked approvals, inbound dedupe, and the source-event / webhook entry
//! — proven to work the same or better on the engine. This is the
//! "nothing lost" half of the proof; the use cases are the "does what we
//! say" half.

use super::operations::{job, local};
use super::*;
use serde_json::json;

/// Scheduled jobs. Before: a poll every minute over a last-run column,
/// and a laptop closed for a day replayed every missed fire. Now: one
/// pending timer per job; fires once when due; re-arms from the consumed
/// occurrence; a missed run catches up once inside the window and is
/// skipped beyond it; a reschedule replaces the timer; disabling removes
/// it; a one-shot fires once and leaves no timer behind.
#[test]
fn parity_scheduled_jobs() {
    let w = World::new();
    let created = local(2026, 8, 23, 8, 0);
    let j = job(&w.s, "briefing", "0 0 9 * * *", created);
    assert_eq!(tick(&w.s, created + 60, &idle, &no_steer).armed, 1);
    assert_eq!(tick(&w.s, created + 65, &idle, &no_steer).armed, 0, "one timer per job, not one per tick");
    assert_eq!(tick(&w.s, local(2026, 8, 23, 8, 59), &idle, &no_steer).fired, 0, "not before it is due");
    assert_eq!(tick(&w.s, local(2026, 8, 23, 9, 0) + 5, &idle, &no_steer).fired, 1);
    let fires = w.s.engine_queued_runs_of_kind("task", 10).unwrap();
    assert_eq!(fires.len(), 1);
    assert_eq!(fires[0].external_ref.as_deref(), Some("cron:1"));
    assert_eq!(tick(&w.s, local(2026, 8, 23, 9, 0) + 10, &idle, &no_steer).armed, 1, "tomorrow's, from the consumed one");
    assert_eq!(w.s.engine_pending_timers("binding").unwrap()[0].due_at, Some(local(2026, 8, 24, 9, 0)));

    // Closed for a week: one late fire inside the window, the rest never replayed.
    w.s.engine_set_run_state(&fires[0].id, "done", local(2026, 8, 23, 9, 1), None).unwrap();
    let back = local(2026, 8, 31, 9, 20);
    let r = tick(&w.s, back, &idle, &no_steer);
    assert_eq!((r.skipped, r.fired), (1, 0), "the 24th's timer is far past the window: skipped");
    let r = tick(&w.s, back + 5, &idle, &no_steer);
    assert_eq!((r.armed, r.fired), (1, 1), "today's, twenty minutes late, fires once");
    assert_eq!(w.s.engine_count_runs_for_ref("cron:1").unwrap(), 2, "the 23rd and the 31st ran; the 24th–30th never did");

    // Rescheduled: replaced. Disabled: gone.
    w.s.upsert_cron_job("briefing", "0 30 9 * * *", "echo hi", "shell", None, None, None, true, None, None).unwrap();
    tick(&w.s, back + 10, &idle, &no_steer);
    let pending = w.s.engine_pending_timers("binding").unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].due_at, Some(local(2026, 8, 31, 9, 30)), "today's 9:30 has not passed yet");
    w.s.set_cron_job_enabled(j.id, false).unwrap();
    tick(&w.s, back + 15, &idle, &no_steer);
    assert!(w.s.engine_pending_timers("binding").unwrap().is_empty());

    // A one-shot: fires once, then nothing is armed.
    let w2 = World::new();
    job(&w2.s, "launch", "0 5 10 23 8 * 2026", local(2026, 8, 23, 9, 0));
    assert_eq!(tick(&w2.s, local(2026, 8, 23, 9, 0) + 30, &idle, &no_steer).armed, 1);
    assert_eq!(tick(&w2.s, local(2026, 8, 23, 10, 5) + 1, &idle, &no_steer).fired, 1);
    let r = tick(&w2.s, local(2026, 8, 23, 10, 5) + 6, &idle, &no_steer);
    assert_eq!((r.armed, r.fired), (0, 0), "a one-shot leaves no timer behind");
    assert!(w2.s.engine_pending_timers("binding").unwrap().is_empty());
}

/// Heartbeats. Before: an in-memory map that forgot everything on
/// restart, and per-binding loops that waited a full interval before the
/// first fire. Now: a timer row is the heartbeat; it fires one run on the
/// heartbeat lane; the consumed timer is the floor for the next; a timer
/// due while the last run is live is skipped, never stacked; and the
/// rows survive a restart because they are rows.
#[test]
fn parity_heartbeats() {
    let w = World::new();
    let target = "heartbeat:agent:ic";
    let beat = |idem: &'static str, due: i64| NewEvent { kind: "timer", target_type: "entity", target_id: target, idem_key: idem, due_at: Some(due), schedule: Some("1800"), ..Default::default() };
    w.s.engine_enqueue_event(&beat("hb-1", 1_000)).unwrap();
    assert_eq!(tick(&w.s, 1_000, &idle, &no_steer).fired, 1);
    let beats = w.s.engine_queued_runs_of_kind("heartbeat", 10).unwrap();
    assert_eq!((beats.len(), beats[0].lane.as_str(), beats[0].agent_id.as_str()), (1, "heartbeat", "ic"));
    assert_eq!(w.s.engine_last_timer_floor("entity", target).unwrap(), Some(1_000));

    // Restart: the run and the timer floor are rows; nothing is forgotten.
    w.s.engine_set_run_state(&beats[0].id, "running", 1_001, None).unwrap();
    assert_eq!(recover(&w.s), 1, "the interrupted beat resumes once");
    assert_eq!(w.s.engine_last_timer_floor("entity", target).unwrap(), Some(1_000));

    // Due again while the last one is live: skipped, not stacked.
    w.s.engine_set_run_state(&beats[0].id, "running", 1_002, None).unwrap();
    w.s.engine_enqueue_event(&beat("hb-2", 2_800)).unwrap();
    assert_eq!(tick(&w.s, 2_800, &idle, &no_steer).skipped, 1);
    assert_eq!(w.s.engine_queued_runs_of_kind("heartbeat", 10).unwrap().len(), 0);

    // A binding heartbeat fires one task of the bound workflow; an inactive
    // binding fires nothing; a live run is never doubled.
    w.s.create_agent("ic", None, "Intake", "", "", "{}", None, None).unwrap();
    w.s.upsert_agent_workflow("ic", "day-monitor", "heartbeat", r#"{"interval":"30m"}"#, None, None, None, Some("[]"), None, false).unwrap();
    let hb = |idem: &'static str, due: i64| NewEvent { kind: "timer", target_type: "binding", target_id: "hb:ic:day-monitor", idem_key: idem, due_at: Some(due), schedule: Some("1800"), ..Default::default() };
    w.s.engine_enqueue_event(&hb("bhb-1", 5_000)).unwrap();
    let r = tick(&w.s, 5_000, &idle, &no_steer);
    let runs = w.s.engine_runs_for_ref("hb:ic:day-monitor", 10, 0).unwrap();
    assert_eq!((r.fired, runs.len()), (1, 1), "one fire of the bound workflow");
    w.s.engine_set_run_state(&runs[0].id, "running", 5_001, None).unwrap();
    w.s.engine_enqueue_event(&hb("bhb-2", 6_800)).unwrap();
    assert_eq!(tick(&w.s, 6_800, &idle, &no_steer).skipped, 1, "still running: skipped");
}

/// Sub-agent tasks. Before: a parent link that was never written;
/// cancelling a parent cancelled nothing. Now: the link is a row, a
/// cancel takes the whole fan-out (uc53), and a task's retries are
/// bounded — one resume after a crash, then the owner decides.
#[test]
fn parity_subagent_tasks_are_linked_and_bounded() {
    let w = World::new();
    w.s.create_pending_task("root", "subagent", "agent:a:web", None, "fan out", None, None, None, 0, None).unwrap();
    w.s.create_pending_task("child", "subagent", "agent:a:web", None, "part one", None, None, None, 0, Some("root")).unwrap();
    assert_eq!(w.run("child").parent_run_id.as_deref(), Some("root"));
    w.s.engine_set_run_state("child", "running", w.t, None).unwrap();
    assert_eq!(recover(&w.s), 1, "one resume");
    w.s.engine_set_run_state("child", "running", w.t, None).unwrap();
    assert_eq!(recover(&w.s), 0, "not a second");
    assert_eq!(w.run("child").state, "failed");
    assert!(w.card("child").unwrap().contains("interrupted by a restart twice"));
    w.s.cancel_task("root").unwrap();
    w.s.cancel_child_tasks("root").unwrap();
    assert_eq!(w.run("root").state, "cancelled");
    assert_eq!(w.run("child").state, "failed", "a finished child keeps its ending");
}

/// Workflow runs. Before: their own interrupted-and-resume bookkeeping.
/// Now: the engine's one rule — resume once after a restart, then a human
/// decides — for plain workflow runs and case turns alike; a clean
/// shutdown suspends case turns without spending their resume.
#[test]
fn parity_workflow_runs_resume_once() {
    let w = World::new();
    w.s.create_workflow_run("wf-1", "agent:a", "manual", None, Some("{}"), Some("agent:a:workflow:wf-1"), Some("{}")).unwrap();
    w.s.engine_set_run_state("wf-1", "running", w.t, None).unwrap();
    assert_eq!(recover(&w.s), 1);
    let run = w.run("wf-1");
    assert_eq!((run.state.as_str(), run.resume_attempted), ("queued", 1));
    w.s.engine_set_run_state("wf-1", "running", w.t, None).unwrap();
    assert_eq!(recover(&w.s), 0);
    assert_eq!(w.run("wf-1").state, "failed");
    assert!(w.card("wf-1").is_some(), "a human decides");
    assert!(w.s.claim_interrupted_workflow_runs(w.t).unwrap().is_empty(), "a poisoned run is never handed back for relaunch");
}

/// Parked approvals. Before: a side table, and the approve button re-ran
/// the workflow itself. Now: the run's live wait; the owner's answer is an
/// event, one per wait generation; a second click is a duplicate; an
/// answer for a run that is not waiting wakes nothing; the resumed run is
/// re-queued at its parked call with the answer on it.
#[test]
fn parity_parked_approvals_are_the_runs_live_wait() {
    let w = World::new();
    w.s.create_workflow_run("wf-1", "agent:a", "watch", Some("intake:x"), Some("{}"), Some("agent:a:workflow:wf-1"), Some("{}")).unwrap();
    w.s.create_workflow_suspension("wf-1", "a", "intake", "act-2", "", None, "[msgs]", "{}", "crm.write", "Create invoice").unwrap();
    w.s.update_workflow_run("wf-1", Some("awaiting_approval"), None, None, None, None).unwrap();
    let run = w.run("wf-1");
    assert_eq!(run.state, "waiting");
    let wait = w.s.engine_get_wait(run.current_wait_id.unwrap()).unwrap().unwrap();
    assert_eq!((wait.action.as_str(), wait.on_kind.as_str(), wait.reason.as_str()), ("resume", "approval", "Create invoice"));

    let answer = |idem: &'static str| NewEvent { kind: "approval", target_type: "run", target_id: "approval:wf-1", payload: r#"{"approved":true}"#, channel: "owner", idem_key: idem, durable: true, ..Default::default() };
    assert!(matches!(w.s.engine_enqueue_event(&answer("a-1")).unwrap(), Enqueued::Inserted(_)));
    assert_eq!(w.s.engine_enqueue_event(&answer("a-1")).unwrap(), Enqueued::Duplicate, "a second click");
    let r = w.tick();
    assert_eq!((r.resumed, r.unrouted), (1, 0));
    let run = w.run("wf-1");
    assert_eq!(run.state, "queued");
    assert!(run.woken_by().is_some(), "the answer is on the run");
    assert!(w.s.get_workflow_suspension("wf-1").unwrap().is_some(), "the parked state is readable for the resume");
    assert!(w.s.list_workflow_suspensions().unwrap().is_empty(), "nothing is pending");
    w.s.engine_enqueue_event(&NewEvent { idem_key: "a-late", ..answer("a-late") }).unwrap();
    let r = w.tick();
    assert_eq!((r.resumed, r.unrouted), (0, 1), "an answer for a run that is not waiting wakes nothing");
}

/// Inbound dedupe. Before: a table of seen ids, pruned by hand. Now: the
/// idempotency key every event has — one namespace, the same guarantee.
#[test]
fn parity_inbound_dedupe_is_the_idempotency_key() {
    let w = World::new();
    assert!(w.s.engine_mark_seen("comm", "comm:sms:abc").unwrap());
    assert!(!w.s.engine_mark_seen("comm", "comm:sms:abc").unwrap(), "a replay is seen");
    assert!(w.s.engine_mark_seen("event", "event:src:1").unwrap());
    // One namespace with every other event: a signal recorded under a key
    // makes a later "seen" under that key a replay, and vice versa.
    assert!(matches!(w.s.engine_enqueue_event(&NewEvent { kind: "signal", target_type: "run", target_id: "k", idem_key: "shared:1", durable: true, ..Default::default() }).unwrap(), Enqueued::Inserted(_)));
    assert!(!w.s.engine_mark_seen("comm", "shared:1").unwrap());
    assert_eq!(w.s.engine_enqueue_event(&NewEvent { kind: "signal", target_type: "run", target_id: "k", idem_key: "comm:sms:abc", durable: true, ..Default::default() }).unwrap(), Enqueued::Duplicate);
}

/// The source-event and webhook entry. Both routes call the same
/// `route_signal`: an event names a person, the person's open case takes
/// it, a replay of the same moment is a duplicate, and a second employee
/// subscribed to the same source is a separate signal that ownership
/// refuses as a conflict instead of the key swallowing it.
#[test]
fn parity_events_and_webhooks_share_one_router() {
    let w = World::new();
    let ic = World::binding("ic", "work-lead", "lead", 3 * DAY);
    let rival = World::binding("rival", "work-lead", "lead", 3 * DAY);
    let payload = json!({"email": "p@x.com", "message": "hi"});
    let Routed::Opened { case_id } = w.arrive(&ic, "email", "p@x.com", payload.clone(), "event:src:ic:work-lead:1") else { panic!() };
    assert_eq!(w.arrive(&ic, "email", "p@x.com", payload.clone(), "event:src:ic:work-lead:1"), Routed::Duplicate, "the same moment twice");
    assert_eq!(w.arrive(&ic, "email", "p@x.com", payload.clone(), "event:src:ic:work-lead:2"), Routed::Signaled { case_id: case_id.clone() });
    assert_eq!(w.arrive(&rival, "email", "p@x.com", payload, "event:src:rival:work-lead:1"), Routed::Conflict { case_id: case_id.clone(), owner: "ic".into() });
    assert!(w.card(&format!("conflict:{case_id}:rival")).is_some());
    assert_eq!(w.s.engine_queued_runs_of_kind("workflow", 10).unwrap().len(), 1, "one turn, for the owner");
}

/// Cases. Did not exist. One open case per person per case type; aliases
/// normalized; the turn contract strict; a failed turn retried on the
/// ladder (1m, 2m, 4m) then the default with the owner told; a turn that
/// runs too long is on the timeout worklist.
#[test]
fn parity_cases_rules_in_one_flow() {
    let mut w = World::new();
    let lead = World::binding("ic", "work-lead", "lead", 3 * DAY);
    let support = World::binding("ic", "help", "support", 2 * DAY);
    let (case, _) = w.open(&lead, "Pat@X.com", "hi", "l1");
    assert_eq!(w.arrive(&lead, "email", " pat@x.com ", json!({"email": "pat@x.com"}), "l2"), Routed::Signaled { case_id: case.clone() }, "aliases are normalized");
    let Routed::Opened { case_id: ticket } = w.arrive(&support, "email", "pat@x.com", json!({"email": "pat@x.com", "message": "broken"}), "s1") else { panic!() };
    assert_ne!(ticket, case, "one case per type: a lead and a ticket for the same person");
    assert_eq!(open_case_for(&w.s, "lead", "email", "pat@x.com").unwrap().id, case);

    // Strict contract: prose after the object is invalid — recorded, default wait, no retry.
    w.tick();
    let turn = w.turn(&case, "{\"result\":{\"status\":\"x\",\"summary\":\"y\"},\"next\":{\"action\":\"wait\"}} and then some");
    assert_eq!(turn.state, "done");
    assert!(w.history_has(&case, "turn_result", "text after the turn's JSON object"));
    assert_eq!(w.wait(&case).unwrap().deadline, Some(w.t + 3 * DAY));

    // The ladder: 60, 120, 240, then the default and the owner told.
    let mut expect = [60, 120, 240].iter();
    for i in 1..=3 {
        w.t = w.wait(&case).unwrap().deadline.unwrap();
        assert_eq!(w.tick().children_started, 1);
        let t = w.start(&w.queued_turn(&case).unwrap().id);
        w.fail(&t, "boom");
        let wait = w.wait(&case).unwrap();
        assert_eq!(wait.deadline, Some(w.t + expect.next().unwrap()), "retry {i}");
        assert!(wait.reason.starts_with(&format!("retry {i} of 3")));
    }
    w.t = w.wait(&case).unwrap().deadline.unwrap();
    w.tick();
    let t = w.start(&w.queued_turn(&case).unwrap().id);
    w.fail(&t, "boom");
    assert_eq!(w.wait(&case).unwrap().deadline, Some(w.t + 3 * DAY), "gave up: the default");
    assert!(w.card(&t.id).unwrap().contains("gave up after 3 retries"));

    // A turn running for an hour is on the worklist; a fresh one is not.
    w.t = w.wait(&case).unwrap().deadline.unwrap();
    w.tick();
    let stale = w.start(&w.queued_turn(&case).unwrap().id);
    assert!(w.s.engine_turns_in_state_since("running", w.t - 1).unwrap().is_empty());
    assert_eq!(w.s.engine_turns_in_state_since("running", w.t + HOUR).unwrap().iter().map(|r| r.id.clone()).collect::<Vec<_>>(), [stale.id]);
}
