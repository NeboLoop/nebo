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
            assert_eq!(emit, ["posted"]);
            assert!(def_json.contains("post it"));
        }
        other => panic!("a plain binding runs plain, got {other:?}"),
    }
    assert!(matches!(route_webhook(&w.s, "ic", "missing", Some(body), "m", w.t), Err(types::NeboError::Validation(_))));
    assert!(matches!(route_webhook(&w.s, "nobody", "work-lead", Some(body), "m", w.t), Err(types::NeboError::NotFound)));
}

/// An employee's missing need reaches the owner once, from each of the
/// three places that know it, and the item opens the one place that fixes
/// it; nothing is installed or connected for the owner.
///
/// Pre-flight holds the fires of a binding whose plugin is missing and hands
/// every held fire's need on: the owner is told the first, never the fires
/// after it; a different need is told; the need met (its record clears) is
/// forgotten, so its return is told again.
///
/// A run blocked by a tool refusing for want of an account is told from what
/// the tool named, as data — the refusal's words are never read: a repeat is
/// quiet, a failure or a quiet exit in between changes nothing, each binding
/// is told for itself, and a run that completes means the need was met.
///
/// A run whose own words say the duty cannot be done (a completed run with
/// a prose outcome) is judged in the heartbeat triage call: when the
/// decision says a need is missing the fire is held and the owner told once;
/// when the decision is silent the fire runs and nothing is told.
#[test]
fn a_missing_need_reaches_the_owner_once() {
    let w = World::new();
    let hub = crate::handlers::ws::ClientHub::new();
    let frontmatter = r#"{"requires":{"interfaces":["telephony","mail"]}}"#;
    w.s.create_agent("rcp", None, "Receptionist", "", "", frontmatter, None, None).unwrap();
    for binding in ["answer-inbound", "front-desk-report", "callback-watch", "missed-sweep"] {
        w.s.upsert_agent_workflow("rcp", binding, "heartbeat", "15m", None, None, None, None, None, false).unwrap();
    }
    let user = w.s.ensure_local_user_id().unwrap();
    let inbox = || -> Vec<db::models::Notification> {
        let mut n: Vec<_> = w.s.list_user_notifications(&user, 100, 0).unwrap().into_iter().filter(|n| n.id.starts_with("need:")).collect();
        n.sort_by_key(|n| (n.created_at, n.title.clone()));
        n
    };
    let tell = |binding: &str, need: crate::preflight::Need<'_>| {
        crate::workflow_manager::tell_owner_need(&w.s, &hub, "", "rcp", binding, need);
    };

    // ── Pre-flight: a missing plugin ────────────────────────────────────
    let announce = |need: &str| tell("answer-inbound", crate::preflight::Need::Recorded(need));
    let fire = |id: &str, unmet: Option<&str>| {
        w.s.engine_create_run(&NewRun {
            id,
            kind: "task",
            session_key: "heartbeat-binding-rcp-answer-inbound",
            agent_id: "rcp",
            lane: "main",
            inputs: Some(r#"{"command":"agent:rcp:answer-inbound","trigger":"heartbeat"}"#),
            external_ref: Some("hb:rcp:answer-inbound"),
            ..Default::default()
        })
        .unwrap();
        crate::preflight::admit(&w.s, &w.run(id), "rcp", "answer-inbound", unmet.map(String::from), w.t, &announce)
    };
    let need = "needs a telephony plugin";
    assert!(!fire("f1", Some(need)));
    assert!(!fire("f2", Some(need)));
    assert!(!fire("f3", Some(need)));
    assert_eq!(inbox().len(), 1, "three held fires, one item");
    let first = &inbox()[0];
    assert_eq!(first.title, "Receptionist needs a telephony plugin");
    assert_eq!(
        first.body.as_deref(),
        Some("Receptionist is holding \"answer inbound\" until then. Add the plugin or turn it on in Plugins. The duty goes ahead on its own after that.")
    );
    assert_eq!(first.action_url.as_deref(), Some("/settings/plugins"));
    assert_eq!(first.agent_id.as_deref(), Some("rcp"));
    assert!(!fire("f4", Some("needs the voiceline plugin turned on")));
    assert_eq!(inbox().len(), 2, "a different need is told");
    assert!(fire("f5", None), "the need is met: the fire runs");
    assert!(!fire("f6", Some(need)));
    assert_eq!(inbox().len(), 3, "the need returns: told again");
    // A watch trigger's start records the same need on every start.
    tell("answer-inbound", crate::preflight::Need::Recorded(need));
    assert_eq!(inbox().len(), 3, "a restart tells nothing new");

    // ── A run blocked on what the refusing tool named ───────────────────
    // The refusal's words name nothing: only the data does.
    let named = types::OwnerNeed::Account { plugin: "voiceline".into() };
    let notice = ai::StreamEvent::control_notice("This line cannot be reached right now.", "terminal_tool_error").with_owner_need(Some(named.clone()));
    assert_eq!(notice.owner_need(), Some(named.clone()), "the runner carries the need to the workflow loop");
    let blocked = workflow::WorkflowError::Blocked("This line cannot be reached right now.".into(), Some(named.clone()));
    assert_eq!(blocked.owner_need(), Some(&named));
    let outcome = blocked.standing_outcome().unwrap();
    let runs = std::cell::Cell::new(0);
    let end = |binding: &str, status: &str, error: Option<&str>, need: Option<&types::OwnerNeed>| {
        runs.set(runs.get() + 1);
        let id = format!("wf-{}", runs.get());
        w.s.create_workflow_run(&id, "agent:rcp", "schedule", Some(binding), None, None, None).unwrap();
        if let Some(need) = need {
            w.s.set_workflow_run_owner_need(&id, need).unwrap();
        }
        w.s.complete_workflow_run(&id, status, 0, error, None, None).unwrap();
        crate::workflow_manager::tell_owner_if_blocked(&w.s, &hub, "", "rcp", binding, &id);
    };
    end("front-desk-report", "exited", Some(&outcome), Some(&named));
    assert_eq!(inbox().len(), 4, "the first block is told");
    let item = inbox().into_iter().find(|n| n.title.ends_with("voiceline connected")).unwrap();
    assert_eq!(item.title, "Receptionist needs voiceline connected");
    assert_eq!(
        item.body.as_deref(),
        Some("Receptionist can't do \"front desk report\" until a voiceline account is connected for it. Connect one in Receptionist's accounts. The duty goes ahead on its own after that.")
    );
    assert_eq!(item.action_url.as_deref(), Some("/rcp/settings/accounts?plugin=voiceline"));
    end("front-desk-report", "exited", Some(&outcome), Some(&named));
    end("front-desk-report", "failed", Some("provider error: 503"), None);
    end("front-desk-report", "exited", Some("Nothing to report today."), None);
    end("front-desk-report", "exited", Some(&outcome), Some(&named));
    assert_eq!(inbox().len(), 4, "a repeat, a failure or a quiet exit in between: quiet");
    end("front-desk-report", "exited", Some("blocked: No example account is connected for this employee."), None);
    assert_eq!(inbox().len(), 4, "a refusal that names nothing is never parsed for a need");
    end("callback-watch", "exited", Some(&outcome), Some(&named));
    assert_eq!(inbox().len(), 5, "each binding is told for itself");
    end("front-desk-report", "completed", None, None);
    end("front-desk-report", "exited", Some(&outcome), Some(&named));
    assert_eq!(inbox().len(), 6, "met, then back: told again");

    // ── A prose outcome, judged in the heartbeat triage call ────────────
    use agent::heartbeat_triage::{self as triage, Declared, Gate};
    let t = crate::engine::now();
    let key = "hb:rcp:missed-sweep";
    let prose = "No telephony plugin available for voicemail or call log retrieval. The referenced plugin is not installed, so nothing was read.";
    let prior = |id: &str, ago: i64| {
        w.s.engine_create_run(&NewRun { id, kind: "task", session_key: "hb-x", agent_id: "rcp", lane: "main", external_ref: Some(key), ..Default::default() }).unwrap();
        w.s.engine_set_run_state(id, "done", t - ago + 5, None).unwrap();
        w.s.conn_exec_for_test(&format!("UPDATE engine_runs SET created_at = {c}, started_at = {c} WHERE id = '{id}'", c = t - ago));
        let wf = format!("{id}-wf");
        w.s.create_workflow_run(&wf, "agent:rcp", "heartbeat", Some("missed-sweep"), None, None, None).unwrap();
        w.s.complete_workflow_run(&wf, "completed", 0, None, None, Some(prose)).unwrap();
        w.s.conn_exec_for_test(&format!("UPDATE engine_runs SET created_at = {c}, started_at = {c} WHERE id = '{wf}'", c = t - ago + 1));
    };
    prior("sweep-1", 600);
    // The employee was set up before that run.
    w.s.conn_exec_for_test("UPDATE agents SET updated_at = 0 WHERE id = 'rcp'");
    let queued = |id: &str| {
        w.s.engine_create_run(&NewRun {
            id,
            kind: "task",
            session_key: "heartbeat-binding-rcp-missed-sweep",
            agent_id: "rcp",
            lane: "main",
            inputs: Some(r#"{"command":"agent:rcp:missed-sweep","trigger":"heartbeat"}"#),
            external_ref: Some(key),
            ..Default::default()
        })
        .unwrap();
        w.run(id)
    };
    let fire = queued("sweep-fire-1");
    let b = crate::engine::triage_binding(&w.s, &fire, None, t).expect("a binding fire triage reads");
    assert!(!b.flags.changed_anything(), "{:?}", b.flags);
    assert_eq!(b.duty.as_deref(), Some("missed-sweep"));
    assert_eq!(b.declared, [Declared::Capability("telephony".into()), Declared::Capability("mail".into())]);
    assert!(b.last_outcome.starts_with("No telephony plugin available"));
    let answer = |missing: Option<f64>, which: &str| {
        let mut answers = std::collections::HashMap::from([
            ("worth_a_run".to_string(), ai::Answer { kind: "noul".into(), choice: None, score: None, noul: Some(0.64), confidence: None, probabilities: Default::default() }),
            ("urgent".to_string(), ai::Answer { kind: "noul".into(), choice: None, score: None, noul: Some(0.83), confidence: None, probabilities: Default::default() }),
        ]);
        if let Some(m) = missing {
            answers.insert("missing_need".into(), ai::Answer { kind: "noul".into(), choice: None, score: None, noul: Some(m), confidence: None, probabilities: Default::default() });
            answers.insert("which_need".into(), ai::Answer { kind: "choice".into(), choice: Some(which.into()), score: None, noul: None, confidence: Some(0.9), probabilities: Default::default() });
        }
        ai::Decision { model: "jev-test".into(), answers, usage: Default::default() }
    };
    let judged = |duty: &str, held: &triage::HeldNeed| tell(duty, crate::preflight::Need::Judged(held));

    // Jev silent (no need answered — as when the call fails, triage runs):
    // the fire runs, nothing is told.
    let gate = triage::gate_from(&answer(None, ""), &b);
    assert_eq!(gate, Gate::Run);
    assert!(crate::engine::act_on_gate(&w.s, &fire, &b, &gate, t, &judged));
    assert_eq!(inbox().len(), 6, "silent: nothing told");

    // Jev says a need is missing, and which: held, told once.
    let fire2 = queued("sweep-fire-2");
    let gate = triage::gate_from(&answer(Some(0.93), "telephony"), &b);
    assert!(matches!(gate, Gate::Hold(_)));
    assert!(!crate::engine::act_on_gate(&w.s, &fire2, &b, &gate, t, &judged), "held, not run");
    let held = w.run("sweep-fire-2");
    assert_eq!((held.state.as_str(), held.summary.as_str()), ("done", "skipped"));
    assert_eq!(inbox().len(), 7);
    let item = inbox().into_iter().rev().find(|n| n.body.as_deref().is_some_and(|b| b.contains("missed sweep"))).unwrap();
    assert_eq!(item.title, "Receptionist needs a telephony plugin");
    let fire3 = queued("sweep-fire-3");
    assert!(!crate::engine::act_on_gate(&w.s, &fire3, &b, &gate, t, &judged));
    assert_eq!(inbox().len(), 7, "held again: told once");

    // Unsure which: a plain item quoting only the outcome's first sentence.
    let other = triage::gate_from(&answer(Some(0.93), "other"), &b);
    let fire4 = queued("sweep-fire-4");
    assert!(!crate::engine::act_on_gate(&w.s, &fire4, &b, &other, t, &judged));
    let plain = inbox().into_iter().rev().find(|n| n.title.contains("something connected")).unwrap();
    assert_eq!(plain.title, "Receptionist needs something connected");
    assert_eq!(
        plain.body.as_deref(),
        Some("Receptionist can't do \"missed sweep\" until something is added or connected. Its last run said: \"No telephony plugin available for voicemail or call log retrieval.\" Check Receptionist's accounts and Plugins. The duty goes ahead on its own after that.")
    );
    assert_eq!(plain.action_url.as_deref(), Some("/rcp/settings/accounts"));

    // The owner's words written in code: an employee, never an agent;
    // nothing unsteady.
    for n in inbox() {
        let text = format!("{} {}", n.title, n.body.unwrap_or_default()).to_lowercase();
        for banned in ["agent", "wake", "sleep", "restart", "may be"] {
            assert!(!text.contains(banned), "{banned:?} in {text:?}");
        }
    }
}
