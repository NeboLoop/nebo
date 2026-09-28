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

/// A Receptionist whose duties stand on a phone line: two watch triggers on
/// the telephony capability, a daily report and a sweep on a heartbeat.
const RECEPTIONIST: &str = r#"{"requires":{"interfaces":["telephony","mail"]},"workflows":{
    "answer-inbound":{"trigger":{"type":"watch","plugin":"telephony","event":"call.incoming"},"activities":[{"id":"a","intent":"answer"}]},
    "callback-watch":{"trigger":{"type":"watch","plugin":"telephony","event":"call.missed"},"activities":[{"id":"a","intent":"call back"}]},
    "front-desk-report":{"trigger":{"type":"schedule","cron":"0 30 17 * * *"},"activities":[{"id":"a","intent":"report"}]},
    "missed-sweep":{"trigger":{"type":"heartbeat","interval":"15m"},"activities":[{"id":"a","intent":"sweep"}]}
}}"#;

/// The Receptionist hired, its four duties armed.
fn receptionist(w: &World) {
    w.s.create_agent("rcp", None, "Receptionist", "", "", RECEPTIONIST, None, None).unwrap();
    for binding in ["answer-inbound", "front-desk-report", "callback-watch", "missed-sweep"] {
        w.s.upsert_agent_workflow("rcp", binding, "heartbeat", "15m", None, None, None, None, None, false).unwrap();
    }
}

/// The owner's need items, oldest first.
fn need_items(w: &World) -> Vec<db::models::Notification> {
    let user = w.s.ensure_local_user_id().unwrap();
    let mut n: Vec<_> = w.s.list_user_notifications(&user, 200, 0).unwrap().into_iter().filter(|n| n.id.starts_with("need:")).collect();
    n.sort_by_key(|n| (n.created_at, n.title.clone()));
    n
}

/// A fire of one of the Receptionist's bindings through pre-flight, as the
/// engine starts it: true runs.
fn preflight(w: &World, hub: &crate::handlers::ws::ClientHub, installed: &[(String, Vec<String>)], id: &str, binding: &str) -> bool {
    w.s.engine_create_run(&NewRun {
        id,
        kind: "task",
        session_key: &format!("heartbeat-binding-rcp-{binding}"),
        agent_id: "rcp",
        lane: "main",
        inputs: Some(&format!(r#"{{"command":"agent:rcp:{binding}","trigger":"heartbeat"}}"#)),
        external_ref: Some(&format!("hb:rcp:{binding}")),
        ..Default::default()
    })
    .unwrap();
    crate::engine::preflight_fire(&w.s, hub, "", installed, &w.run(id), w.t)
}

/// A run of one of the Receptionist's bindings ending: `status`, its
/// error or outcome, and what a refusing tool named, if anything.
fn run_ends(w: &World, hub: &crate::handlers::ws::ClientHub, installed: &[(String, Vec<String>)], id: &str, binding: &str, status: &str, words: Option<&str>, need: Option<&types::OwnerNeed>) {
    w.s.create_workflow_run(id, "agent:rcp", "schedule", Some(binding), None, None, None).unwrap();
    if let Some(need) = need {
        w.s.set_workflow_run_owner_need(id, need).unwrap();
    }
    let (error, output) = if status == "completed" { (None, words) } else { (words, None) };
    w.s.complete_workflow_run(id, status, 0, error, None, output).unwrap();
    crate::workflow_manager::tell_owner_if_blocked(&w.s, hub, "", installed, "rcp", binding, id);
}

/// An employee's missing need reaches the owner once, from each place that
/// knows it, and the item opens the one place that fixes it; nothing is
/// installed or connected for the owner.
///
/// Pre-flight holds the fires of a binding whose capability no installed
/// plugin provides: the owner is told the first, never the fires after it,
/// nor a watch trigger's restart. When a plugin that provides it appears,
/// the next fire resolves the item and runs; when it goes, the need is told
/// once more.
///
/// A run blocked by a tool refusing for want of an account is told from what
/// the tool named, as data — the refusal's words are never read: a repeat, a
/// failure, a quiet exit or a run that completes while the account is still
/// missing are quiet. Another duty blocked on the same account joins the
/// same item, which then lists both duties. Connecting the account resolves
/// the item at the next fire; losing it again is told once more.
///
/// A run whose own words say the duty cannot be done (a completed run with a
/// prose outcome) is judged in the heartbeat triage call: when the decision
/// says a need is missing the fire is held and the duty joins the item that
/// already tells that need; when the decision is silent the fire runs and
/// nothing is told; a need nobody can name is its own item.
#[test]
fn a_missing_need_reaches_the_owner_once() {
    let w = World::new();
    let hub = crate::handlers::ws::ClientHub::new();
    receptionist(&w);
    let inbox = || need_items(&w);
    let installed: std::cell::RefCell<Vec<(String, Vec<String>)>> = Default::default();
    let fire = |id: &str| preflight(&w, &hub, &installed.borrow(), id, "answer-inbound");
    let tell = |binding: &str, need: crate::preflight::Need<'_>| {
        crate::workflow_manager::tell_owner_need(&w.s, &hub, "", &installed.borrow(), "rcp", binding, need);
    };

    // ── Pre-flight: no plugin provides telephony ────────────────────────
    assert!(!fire("f1"));
    assert!(!fire("f2"));
    assert!(!fire("f3"));
    assert_eq!(inbox().len(), 1, "three held fires, one item");
    let first = &inbox()[0];
    assert_eq!(first.title, "Receptionist needs a telephony plugin");
    assert_eq!(
        first.body.as_deref(),
        Some("Receptionist is holding \"answer inbound\" until then. Add the plugin or turn it on in Plugins. The duty goes ahead on its own after that.")
    );
    assert_eq!(first.action_url.as_deref(), Some("/settings/plugins"));
    assert_eq!(first.agent_id.as_deref(), Some("rcp"));
    assert_eq!(w.s.agent_workflow_degraded_reason("rcp", "answer-inbound").unwrap().as_deref(), Some("needs a telephony plugin"));
    // A watch trigger's start names the same need on every start.
    tell("answer-inbound", crate::preflight::Need::Known(&types::OwnerNeed::Capability { capability: "telephony".into() }));
    assert_eq!(inbox().len(), 1, "a restart tells nothing new");

    // A plugin that provides telephony appears: the next fire resolves the
    // item and runs.
    w.s.upsert_installed_plugin("ringer", "ringer", "1.0.0", "", "", "", "").unwrap();
    *installed.borrow_mut() = vec![("ringer".into(), vec!["telephony".into()])];
    assert!(fire("f4"), "the need is met: the fire runs");
    assert!(inbox()[0].read_at.is_some(), "the met need's item is resolved");
    assert!(w.s.owner_needs_of("rcp").unwrap().is_empty());
    // It goes again: told once more, then quiet.
    installed.borrow_mut().clear();
    assert!(!fire("f5"));
    assert!(!fire("f6"));
    assert_eq!(inbox().len(), 2, "the need returns: told once more");

    // ── A run blocked on what the refusing tool named ───────────────────
    w.s.upsert_installed_plugin("voiceline", "voiceline", "1.0.0", "", "", "", "").unwrap();
    installed.borrow_mut().push(("voiceline".into(), vec![]));
    // The refusal's words name nothing: only the data does.
    let named = types::OwnerNeed::Account { plugin: "voiceline".into() };
    let notice = ai::StreamEvent::control_notice("This line cannot be reached right now.", "terminal_tool_error").with_owner_need(Some(named.clone()));
    assert_eq!(notice.owner_need(), Some(named.clone()), "the runner carries the need to the workflow loop");
    let blocked = workflow::WorkflowError::Blocked("This line cannot be reached right now.".into(), Some(named.clone()));
    assert_eq!(blocked.owner_need(), Some(&named));
    let outcome = blocked.standing_outcome().unwrap();
    let runs = std::cell::Cell::new(0);
    let end = |binding: &str, status: &str, words: Option<&str>, need: Option<&types::OwnerNeed>| {
        runs.set(runs.get() + 1);
        run_ends(&w, &hub, &installed.borrow(), &format!("wf-{}", runs.get()), binding, status, words, need);
    };
    end("front-desk-report", "exited", Some(&outcome), Some(&named));
    assert_eq!(inbox().len(), 3, "the first block is told");
    // The account's open item (a resolved one is read).
    let account_item = || inbox().into_iter().find(|n| n.title.ends_with("voiceline connected") && n.read_at.is_none()).unwrap();
    let item = account_item();
    assert_eq!(item.title, "Receptionist needs voiceline connected");
    assert_eq!(
        item.body.as_deref(),
        Some("Receptionist can't do \"front desk report\" until a voiceline account is connected for it. Connect one in Receptionist's accounts. The duty goes ahead on its own after that.")
    );
    assert_eq!(item.action_url.as_deref(), Some("/rcp/settings/accounts?plugin=voiceline"));
    end("front-desk-report", "exited", Some(&outcome), Some(&named));
    end("front-desk-report", "failed", Some("provider error: 503"), None);
    end("front-desk-report", "exited", Some("Nothing to report today."), None);
    end("front-desk-report", "completed", Some("Could not reach the line, so nothing was reported."), None);
    end("front-desk-report", "exited", Some(&outcome), Some(&named));
    assert_eq!(inbox().len(), 3, "a repeat, a failure, a quiet exit or a completed run while it still stands: quiet");
    end("front-desk-report", "exited", Some("blocked: No example account is connected for this employee."), None);
    assert_eq!(inbox().len(), 3, "a refusal that names nothing is never parsed for a need");

    // Another duty blocked on the same account joins the item.
    end("callback-watch", "exited", Some(&outcome), Some(&named));
    end("callback-watch", "exited", Some(&outcome), Some(&named));
    assert_eq!(inbox().len(), 3, "another duty on the same need: the same item");
    let item = account_item();
    assert_eq!(
        item.body.as_deref(),
        Some("Receptionist can't do \"front desk report\" and \"callback watch\" until a voiceline account is connected for it. Connect one in Receptionist's accounts. The duties go ahead on their own after that.")
    );

    // The owner connects it: the next fire of any duty resolves the item.
    w.s.upsert_plugin_account_profile("acct-1", "rcp", "voiceline", "Main line", "/tmp/voiceline-main").unwrap();
    assert!(!fire("f7"), "answer inbound still stands on telephony");
    assert!(inbox().iter().all(|n| !n.title.ends_with("voiceline connected") || n.read_at.is_some()), "the met account's item is resolved");
    assert_eq!(inbox().len(), 3);
    // The account goes: the next block is told once more.
    w.s.delete_plugin_account_profile("rcp", "voiceline", "Main line").unwrap();
    end("front-desk-report", "exited", Some(&outcome), Some(&named));
    assert_eq!(inbox().len(), 4, "met, then back: told once more");
    assert_eq!(account_item().body.as_deref().map(|b| b.contains("\"front desk report\" until")), Some(true), "a new item, for the duty blocked now");

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
    // The employee and its plugins were set up before that run.
    w.s.conn_exec_for_test("UPDATE agents SET updated_at = 0 WHERE id = 'rcp'");
    w.s.conn_exec_for_test("UPDATE plugin_registry SET updated_at = 0");
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
    assert_eq!(inbox().len(), 4, "silent: nothing told");

    // Jev says telephony is missing: held, and the sweep joins the item
    // that already tells it.
    let fire2 = queued("sweep-fire-2");
    let gate = triage::gate_from(&answer(Some(0.93), "telephony"), &b);
    assert!(matches!(gate, Gate::Hold(_)));
    assert!(!crate::engine::act_on_gate(&w.s, &fire2, &b, &gate, t, &judged), "held, not run");
    let held = w.run("sweep-fire-2");
    assert_eq!((held.state.as_str(), held.summary.as_str()), ("done", "skipped"));
    assert_eq!(inbox().len(), 4, "the same need, judged: no new item");
    let telephony = inbox().into_iter().find(|n| n.title == "Receptionist needs a telephony plugin" && n.read_at.is_none()).unwrap();
    assert_eq!(
        telephony.body.as_deref(),
        Some("Receptionist is holding \"answer inbound\" and \"missed sweep\" until then. Add the plugin or turn it on in Plugins. The duties go ahead on their own after that.")
    );
    let fire3 = queued("sweep-fire-3");
    assert!(!crate::engine::act_on_gate(&w.s, &fire3, &b, &gate, t, &judged));
    assert_eq!(inbox().len(), 4, "held again: nothing new");

    // Unsure which: a plain item quoting only the outcome's first sentence.
    let other = triage::gate_from(&answer(Some(0.93), "other"), &b);
    let fire4 = queued("sweep-fire-4");
    assert!(!crate::engine::act_on_gate(&w.s, &fire4, &b, &other, t, &judged));
    assert_eq!(inbox().len(), 5);
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

/// The Receptionist's live loop (2026-09-27/28): one missing phone line,
/// noticed by pre-flight on one duty, by a watch trigger's start on another
/// and by heartbeat triage on a third, each under its own key, was told
/// again every ninety minutes for two days, to the Inbox, the phone and
/// email. One need is one item, whoever notices it.
///
/// The same missing telephony capability, from three sources on three
/// duties and spelled two ways, is one item listing the three duties.
/// Twelve hours of fires, restarts, judgments and completed runs while it
/// stands add nothing. The owner dismisses it: nothing brings it back while
/// it stands. A plugin that provides telephony meets it; the account on
/// that plugin is then the need, and the refusing tool, triage naming the
/// capability and triage naming the plugin by another casing all tell the
/// one account item. Connecting the account resolves it; losing it tells it
/// once more.
#[test]
fn one_missing_need_is_one_notice_whoever_sees_it_and_however_often() {
    use agent::heartbeat_triage::{Declared, HeldNeed};
    let mut w = World::new();
    let hub = crate::handlers::ws::ClientHub::new();
    receptionist(&w);
    let user = w.s.ensure_local_user_id().unwrap();
    let mut installed: Vec<(String, Vec<String>)> = vec![("sheets".into(), vec!["spreadsheet".into()])];
    let clause = "No telephony plugin available for voicemail or call log retrieval.";
    let judged = |which: Declared| HeldNeed { which: Some(which), clause: clause.into() };
    let telephony = types::OwnerNeed::Capability { capability: "telephony".into() };
    let mut n = 0;
    let mut next = || {
        n += 1;
        n
    };

    // Three sources, three duties, one need.
    assert!(!preflight(&w, &hub, &installed, "f-0", "answer-inbound"), "pre-flight holds answer inbound");
    crate::workflow_manager::tell_owner_need(&w.s, &hub, "", &installed, "rcp", "callback-watch", crate::preflight::Need::Known(&telephony));
    crate::workflow_manager::tell_owner_need(&w.s, &hub, "", &installed, "rcp", "missed-sweep", crate::preflight::Need::Judged(&judged(Declared::Capability("Telephony".into()))));
    let items = need_items(&w);
    assert_eq!(items.len(), 1, "one need, one item");
    assert_eq!(items[0].title, "Receptionist needs a telephony plugin");
    assert_eq!(
        items[0].body.as_deref(),
        Some("Receptionist is holding \"answer inbound\", \"callback watch\" and \"missed sweep\" until then. Add the plugin or turn it on in Plugins. The duties go ahead on their own after that.")
    );
    let told = items[0].id.clone();

    // Twelve hours of it while it stands: a fire every quarter hour, the
    // watch restarting, triage holding the sweep, a sweep run completing
    // with the need still standing.
    for _ in 0..48 {
        w.t += 15 * 60;
        let i = next();
        assert!(!preflight(&w, &hub, &installed, &format!("f-{i}"), "answer-inbound"));
        crate::workflow_manager::tell_owner_need(&w.s, &hub, "", &installed, "rcp", "callback-watch", crate::preflight::Need::Known(&telephony));
        crate::workflow_manager::tell_owner_need(&w.s, &hub, "", &installed, "rcp", "missed-sweep", crate::preflight::Need::Judged(&judged(Declared::Capability("telephony".into()))));
        run_ends(&w, &hub, &installed, &format!("sweep-{i}"), "missed-sweep", "completed", Some(clause), None);
    }
    let items = need_items(&w);
    assert_eq!(items.len(), 1, "twelve hours: still one item");
    assert_eq!(items[0].id, told);

    // The owner dismisses it: nothing brings it back while it stands.
    w.s.delete_notification(&told, &user).unwrap();
    for _ in 0..8 {
        w.t += 15 * 60;
        let i = next();
        assert!(!preflight(&w, &hub, &installed, &format!("f-{i}"), "answer-inbound"));
        crate::workflow_manager::tell_owner_need(&w.s, &hub, "", &installed, "rcp", "front-desk-report", crate::preflight::Need::Known(&telephony));
        crate::workflow_manager::tell_owner_need(&w.s, &hub, "", &installed, "rcp", "missed-sweep", crate::preflight::Need::Judged(&judged(Declared::Capability("telephony".into()))));
    }
    assert!(need_items(&w).is_empty(), "dismissed stays dismissed while the need stands, even as another duty is held on it");

    // A plugin that provides telephony is installed: the next fire meets
    // the need and runs.
    w.s.upsert_installed_plugin("voiceline", "VoiceLine", "1.0.0", "", "", "", "").unwrap();
    installed.push(("voiceline".into(), vec!["telephony".into()]));
    let i = next();
    assert!(preflight(&w, &hub, &installed, &format!("f-{i}"), "answer-inbound"), "met: the fire runs");
    assert!(w.s.owner_needs_of("rcp").unwrap().is_empty(), "the met need is forgotten");
    assert!(need_items(&w).is_empty(), "meeting a dismissed need raises nothing");

    // The account on it is now what the duties stand on: the refusing
    // tool, triage naming the capability, and triage naming the plugin in
    // another casing are one item.
    let account = types::OwnerNeed::Account { plugin: "voiceline".into() };
    let i = next();
    run_ends(&w, &hub, &installed, &format!("cb-{i}"), "callback-watch", "exited", Some("blocked"), Some(&account));
    crate::workflow_manager::tell_owner_need(&w.s, &hub, "", &installed, "rcp", "missed-sweep", crate::preflight::Need::Judged(&judged(Declared::Capability("telephony".into()))));
    crate::workflow_manager::tell_owner_need(&w.s, &hub, "", &installed, "rcp", "front-desk-report", crate::preflight::Need::Judged(&judged(Declared::Plugin("VoiceLine".into()))));
    let items = need_items(&w);
    assert_eq!(items.len(), 1, "one account, one item");
    assert_eq!(items[0].title, "Receptionist needs VoiceLine connected");
    assert_eq!(
        items[0].body.as_deref(),
        Some("Receptionist can't do \"callback watch\", \"missed sweep\" and \"front desk report\" until a VoiceLine account is connected for it. Connect one in Receptionist's accounts. The duties go ahead on their own after that.")
    );
    for _ in 0..48 {
        w.t += 15 * 60;
        let i = next();
        run_ends(&w, &hub, &installed, &format!("cb-{i}"), "callback-watch", "exited", Some("blocked"), Some(&account));
        run_ends(&w, &hub, &installed, &format!("sweep-{i}"), "missed-sweep", "completed", Some("Could not reach the line."), None);
        crate::workflow_manager::tell_owner_need(&w.s, &hub, "", &installed, "rcp", "missed-sweep", crate::preflight::Need::Judged(&judged(Declared::Plugin("voiceline".into()))));
    }
    assert_eq!(need_items(&w).len(), 1, "twelve more hours: still one item");

    // Connected: the next fire resolves it. Lost again: told once more.
    w.s.upsert_plugin_account_profile("acct-1", "rcp", "voiceline", "Main line", "/tmp/voiceline-main").unwrap();
    let i = next();
    assert!(preflight(&w, &hub, &installed, &format!("f-{i}"), "answer-inbound"));
    assert!(need_items(&w)[0].read_at.is_some(), "the met need's item is resolved");
    w.s.delete_plugin_account_profile("rcp", "voiceline", "Main line").unwrap();
    for _ in 0..4 {
        let i = next();
        run_ends(&w, &hub, &installed, &format!("cb-{i}"), "callback-watch", "exited", Some("blocked"), Some(&account));
    }
    let items = need_items(&w);
    assert_eq!(items.len(), 2, "lost again: told once more, and only once");
    assert_eq!(items.iter().filter(|n| n.read_at.is_none()).count(), 1, "the new item is the open one");
}
