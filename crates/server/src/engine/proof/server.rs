//! The paths that used to need a running server — turn timeouts, heartbeat
//! window placement, the owner's approval answer, the hub's webhook door —
//! given a store-level shape and proven here. The thin async wrappers that
//! remain only gather what the runtime knows (idle time, live agents) and
//! hand it in.

use super::operations::local;
use super::*;
use crate::heartbeat::Enabled;
use serde_json::json;
use workflow::cases::{route_webhook, Webhook};

fn lead() -> CaseBinding<'static> {
    World::binding("ic", "work-lead", "lead", 3 * DAY)
}

/// A turn running past the hour is timed out whatever it is doing; one
/// running ten minutes with no activity for eleven is stuck; a fresh turn
/// and a busy long one are not touched. A turn queued past ten minutes is
/// on the alert list. The timed-out turn is cancelled and settles as a
/// failure: the case retries on the ladder and the owner sees why.
#[test]
fn turns_time_out_by_the_clock_and_by_silence() {
    let mut w = World::new();
    let b = lead();
    let (old, q1) = w.open(&b, "old@x.com", "hi", "m1");
    let (silent, q2) = w.open(&b, "silent@x.com", "hi", "m2");
    let (busy, q3) = w.open(&b, "busy@x.com", "hi", "m3");
    let (fresh, q4) = w.open(&b, "fresh@x.com", "hi", "m4");
    let (queued, q5) = w.open(&b, "queued@x.com", "hi", "m5");
    let start = w.t;
    // A queued turn's age is its row's creation; the world's clock sets it.
    w.s.conn_exec_for_test(&format!("UPDATE engine_runs SET created_at = {start} WHERE id = '{}'", q5.id));
    w.start(&q1.id);
    w.t = start + 20 * 60;
    w.start(&q2.id);
    w.start(&q3.id);
    w.t = start + 61 * 60;
    w.start(&q4.id);
    let t = w.t;
    let idle = |session: &str| -> Option<u64> {
        if session.contains(&q2.id) {
            Some(11 * 60)
        } else if session.contains(&q3.id) {
            Some(30)
        } else {
            None
        }
    };
    let stuck = turns_to_time_out(&w.s, t, &idle);
    let ids: Vec<&str> = stuck.iter().map(|(r, _)| r.id.as_str()).collect();
    assert!(ids.contains(&q1.id.as_str()), "past the hour");
    assert!(ids.contains(&q2.id.as_str()), "idle past ten minutes");
    assert!(!ids.contains(&q3.id.as_str()), "busy is not stuck");
    assert!(!ids.contains(&q4.id.as_str()), "fresh is not stuck");
    assert!(stuck.iter().any(|(r, why)| r.id == q1.id && why.contains("more than 60 minutes")));
    assert!(stuck.iter().any(|(r, why)| r.id == q2.id && why.contains("no activity for 11 minutes")));
    let late: Vec<String> = turns_queued_too_long(&w.s, t).iter().map(|r| r.parent_run_id.clone().unwrap()).collect();
    assert!(late.contains(&queued), "queued past ten minutes is on the alert list");
    let _ = (&silent, &busy, &fresh);

    // The wrapper cancels a stuck turn; the loop then settles it as a
    // failed turn with the timeout as its reason.
    w.s.engine_set_run_state(&q1.id, "cancelled", t, None).unwrap();
    w.fail(&w.run(&q1.id), "timed out: running for more than 60 minutes");
    let case = w.run(&old);
    assert_eq!(case.state, "waiting");
    assert!(w.history_has(&old, "turn_failed", "timed out"));
    assert!(w.wait(&old).unwrap().reason.starts_with("retry 1 of 3"));
}

/// Heartbeat timers are placed inside the entity's window: a fire due at
/// 18:20 against an 08:00–18:00 window lands at 08:00 the next day; one
/// due inside the window keeps its time; an entity with no window is due
/// exactly an interval after its last fire; an entity that never fired is
/// due now. A binding heartbeat of a live agent gets its timer; a binding
/// of an agent that is not live gets none.
#[test]
fn heartbeats_are_armed_inside_their_windows() {
    let w = World::new();
    let now = local(2026, 9, 7, 17, 50);
    let windowed = Enabled { entity_type: "agent".into(), entity_id: "ic".into(), interval_secs: 1800, window: Some(("08:00".into(), "18:00".into())), last_fired_at: Some(now - 60) };
    let inside = Enabled { entity_type: "agent".into(), entity_id: "day".into(), interval_secs: 600, window: Some(("08:00".into(), "18:00".into())), last_fired_at: Some(now - 60) };
    let open = Enabled { entity_type: "agent".into(), entity_id: "night".into(), interval_secs: 1800, window: None, last_fired_at: Some(now - 60) };
    let never = Enabled { entity_type: "team".into(), entity_id: "ops".into(), interval_secs: 3600, window: None, last_fired_at: None };
    assert_eq!(arm_entity_heartbeats(&w.s, now, &[windowed, inside, open, never]), 4);
    let due = |target: &str| w.s.engine_pending_timers("entity").unwrap().into_iter().find(|e| e.target_id == target).and_then(|e| e.due_at).unwrap();
    assert_eq!(due("heartbeat:agent:ic"), local(2026, 9, 8, 8, 0), "18:20 is outside the window: next opening");
    assert_eq!(due("heartbeat:agent:day"), now - 60 + 600, "inside the window: on time");
    assert_eq!(due("heartbeat:agent:night"), now - 60 + 1800, "no window: an interval after the last fire");
    assert_eq!(due("heartbeat:team:ops"), now, "never fired: due now");
    assert_eq!(arm_entity_heartbeats(&w.s, now + 5, &[]), 0, "nothing enabled: the timers go");
    assert!(w.s.engine_pending_timers("entity").unwrap().is_empty());

    w.s.create_agent("ic", None, "Intake", "", "", "{}", None, None).unwrap();
    w.s.create_agent("gone", None, "Gone", "", "", "{}", None, None).unwrap();
    w.s.upsert_agent_workflow("ic", "day-monitor", "heartbeat", "30m|08:00-18:00", None, None, None, Some("[]"), None, false).unwrap();
    w.s.upsert_agent_workflow("gone", "day-monitor", "heartbeat", "30m", None, None, None, Some("[]"), None, false).unwrap();
    let bindings = w.s.list_active_heartbeat_workflows().unwrap();
    assert_eq!(bindings.len(), 2);
    let live: std::collections::HashSet<String> = ["ic".to_string()].into_iter().collect();
    assert_eq!(arm_binding_heartbeats(&w.s, now, bindings, &live), 1, "one timer, for the live agent");
    let timers = w.s.engine_pending_timers("binding").unwrap();
    assert_eq!(timers.len(), 1);
    assert_eq!(timers[0].target_id, "hb:ic:day-monitor");
    assert_eq!(timers[0].due_at, Some(local(2026, 9, 8, 8, 0)), "an interval from now is 18:20: placed at the window's opening");
}

/// The owner's answer is one event per wait generation, recorded only
/// while the run is waiting: the first click resumes the run, the second
/// is a duplicate, and an answer for a run that is not waiting is refused
/// rather than pinned on the wrong moment.
#[test]
fn the_owners_answer_is_one_event_per_wait() {
    let w = World::new();
    let b = lead();
    let (_case, queued) = w.open(&b, "ok@x.com", "hi", "m1");
    let turn = w.start(&queued.id);
    assert!(matches!(w.s.engine_answer_wait(&turn.id, true), Err(types::NeboError::Validation(_))), "a running turn is not waiting");
    w.s.engine_declare_wait(&turn.id, &NewWait { action: "resume", on_kind: "approval", key: &format!("approval:{}", turn.id), parked: Some("{}"), reason: "send the quote", ..Default::default() }, w.t).unwrap();
    assert!(matches!(w.s.engine_answer_wait(&turn.id, true).unwrap(), Enqueued::Inserted(_)));
    assert_eq!(w.s.engine_answer_wait(&turn.id, false).unwrap(), Enqueued::Duplicate, "the second click, even the other button, is the same answer");
    assert_eq!(w.tick().resumed, 1);
    let run = w.run(&turn.id);
    assert_eq!(run.state, "queued");
    assert!(run.woken_by().is_some());
    assert!(matches!(w.s.engine_answer_wait(&turn.id, true), Err(types::NeboError::Validation(_))), "no longer waiting: refused");
    assert!(matches!(w.s.engine_answer_wait("nobody", true), Err(types::NeboError::NotFound)));
}

/// The hub's webhook door: a delivery to a case binding that names a
/// person opens their case; the hub's redelivery of the same message is a
/// duplicate; a second submission reaches the same case; a delivery that
/// names nobody, or to a plain binding, is handed back to run as a plain
/// webhook with the payload in the envelope; a missing binding is an error.
#[test]
fn the_webhook_door_routes_people_to_their_case_and_the_rest_to_a_plain_run() {
    let w = World::new();
    let workflows = r#"{"workflows":{
        "work-lead":{"trigger":{"type":"manual"},"activities":[{"id":"run","intent":"work the lead"}],"case":{"type":"lead","key":"email","default_wait":"3d"}},
        "notify":{"trigger":{"type":"manual"},"activities":[{"id":"run","intent":"post it"}],"emit":"posted"}
    }}"#;
    w.s.create_agent("ic", None, "Intake", "", "", workflows, None, None).unwrap();
    let body = r#"{"email":"hook@x.com","message":"from the website"}"#;
    let Webhook::Case(Routed::Opened { case_id }) = route_webhook(&w.s, "ic", "work-lead", Some(body), "msg-1", w.t).unwrap() else { panic!("opens a case") };
    assert!(matches!(route_webhook(&w.s, "ic", "work-lead", Some(body), "msg-1", w.t).unwrap(), Webhook::Case(Routed::Duplicate)), "the hub's redelivery");
    assert!(matches!(route_webhook(&w.s, "ic", "work-lead", Some(r#"{"email":"HOOK@x.com","message":"again"}"#), "msg-2", w.t + 60).unwrap(), Webhook::Case(Routed::Signaled { case_id: ref c }) if *c == case_id));
    assert_eq!(w.s.engine_queued_runs_of_kind("workflow", 10).unwrap().len(), 1, "one first turn, the second submission rides it");

    match route_webhook(&w.s, "ic", "work-lead", Some(r#"{"note":"no address here"}"#), "msg-3", w.t).unwrap() {
        Webhook::Plain { payload, .. } => assert_eq!(payload["note"], json!("no address here")),
        other => panic!("names nobody: plain, got {other:?}"),
    }
    match route_webhook(&w.s, "ic", "notify", Some("not json at all"), "", w.t).unwrap() {
        Webhook::Plain { payload, emit, def_json, .. } => {
            assert_eq!(payload, json!("not json at all"));
            assert_eq!(emit.as_deref(), Some("posted"));
            assert!(def_json.contains("post it"));
        }
        other => panic!("a plain binding runs plain, got {other:?}"),
    }
    assert!(matches!(route_webhook(&w.s, "ic", "missing", Some(body), "m", w.t), Err(types::NeboError::Validation(_))));
    assert!(matches!(route_webhook(&w.s, "nobody", "work-lead", Some(body), "m", w.t), Err(types::NeboError::NotFound)));
}
