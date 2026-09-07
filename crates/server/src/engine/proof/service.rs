//! Proof scenarios: customer service and support (uc11–uc18). See `mod.rs`.

use super::*;
use serde_json::json;

fn support() -> CaseBinding<'static> {
    World::binding("cs", "support-ticket", "support", 3 * DAY)
}

fn msg(email: &str, text: &str) -> serde_json::Value {
    json!({"email": email, "message": text})
}

fn rfc3339(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0).unwrap().to_rfc3339()
}

/// uc11 — Ticket ownership that survives restarts and crashes. The process
/// dies mid-turn: the turn resumes once, the case and its key are untouched,
/// a reply meanwhile rides the resumed turn. A second death is poison: the
/// owner is told, the case retries on its ladder. A clean shutdown spends
/// no resume. Never a second case for the same ticket, never a lost reply.
#[test]
fn uc11_ticket_ownership_survives_restarts_and_crashes() {
    let mut w = World::new();
    let b = support();
    let (case, queued) = w.open(&b, "ann@x.com", "my order arrived broken", "t1");
    let turn = w.start(&queued.id);

    // Crash: boot resumes the turn once; the ticket is still hers.
    assert_eq!(recover(&w.s), 1);
    let resumed = w.run(&turn.id);
    assert_eq!((resumed.state.as_str(), resumed.resume_attempted), ("queued", 1));
    assert_eq!(w.run(&case).state, "waiting");
    assert_eq!(open_case_for(&w.s, "support", "email", "ann@x.com").unwrap().id, case);

    // A reply while it is down rides the resumed turn: no second turn, no second case.
    w.t += HOUR;
    assert_eq!(w.arrive(&b, "email", "ann@x.com", msg("ann@x.com", "photos attached"), "t2"), Routed::Signaled { case_id: case.clone() });
    let r = w.tick();
    assert_eq!((r.steered, r.children_started), (1, 0));
    assert!(w.run(&turn.id).inputs.unwrap().contains("photos attached"));
    assert!(w.undelivered().is_empty());

    // It runs and dies again: poison, the owner told, the case still hers.
    w.start(&turn.id);
    assert_eq!(recover(&w.s), 0);
    assert_eq!(w.run(&turn.id).state, "failed");
    assert!(w.card(&turn.id).is_some(), "the owner hears about the double interruption");
    w.fail(&w.run(&turn.id), "interrupted twice");
    let wait = w.wait(&case).unwrap();
    assert!(wait.reason.starts_with("retry 1 of 3"), "{}", wait.reason);
    assert_eq!(wait.deadline, Some(w.t + 60));

    // The retry starts; a clean shutdown suspends it and spends nothing.
    assert_eq!(w.advance(60).children_started, 1);
    let next = w.start(&w.queued_turn(&case).unwrap().id);
    suspend_for_shutdown(&w.s);
    assert_eq!(w.run(&next.id).state, "interrupted");
    recover(&w.s);
    let back = w.run(&next.id);
    assert_eq!((back.state.as_str(), back.resume_attempted), ("queued", 0));
    assert_eq!(open_case_for(&w.s, "support", "email", "ann@x.com").unwrap().id, case, "one case, start to finish");
    assert_eq!(w.s.engine_cases(Some("cs"), 10).unwrap().len(), 1);
}

/// uc12 — Waiting on a customer for information: a nudge at day two,
/// closure at day seven. Both are the case's own deadlines; the nudge is
/// one send with one receipt; the closure starts nothing further.
#[tokio::test]
async fn uc12_nudge_at_day_two_closure_at_day_seven() {
    let mut w = World::new();
    let b = support();
    let (case, _) = w.open(&b, "bo@x.com", "refund please", "t1");
    w.turn(&case, &waits("waiting_on_customer", "asked for the order number", "signal", "2d", "their order number or the day-2 nudge"));
    assert_eq!(w.wait(&case).unwrap().deadline, Some(w.t + 2 * DAY));

    assert_eq!(w.advance(2 * DAY).children_started, 1);
    let nudge = w.start(&w.queued_turn(&case).unwrap().id);
    assert!(nudge.inputs.as_deref().unwrap().contains("case.timer"), "the nudge is the deadline's turn");
    let r = w.send("cs", &nudge.id, "bo@x.com", "Still need your order number", SendOutcome::Sent("Sent.".into(), None)).await;
    assert!(!r.is_error);
    w.finish(&nudge, &waits("waiting_on_customer", "nudged", "signal", "5d", "their answer or the day-7 closure"));
    assert_eq!(w.wait(&case).unwrap().deadline, Some(w.t + 5 * DAY));

    assert_eq!(w.advance(5 * DAY).children_started, 1);
    w.turn(&case, &closes("closed_no_response", "closed at day seven; the door stays open"));
    let run = w.run(&case);
    assert_eq!((run.state.as_str(), run.result.as_deref()), ("done", Some("closed_no_response")));
    assert_eq!(w.history(&case).iter().filter(|e| e.kind == "turn_result").count(), 3);
    assert!(w.history_has(&case, "turn_result", "nudged — 1 send(s) on the ledger this turn"));
    assert!(w.undelivered().is_empty());
    // A month on: only retention housekeeping runs; nothing wakes for the closed case.
    let r = w.advance(30 * DAY);
    assert_eq!((r.claimed, r.children_started, r.fired, r.resumed, r.steered), (0, 0, 0, 0, 0), "a closed case wakes nothing: {r:?}");
}

/// uc13 — Escalation ladders that fire only if the previous step got no
/// answer. Step two fires because step one was silent; the answer arrives
/// before step three's moment, and that moment passes with nothing fired.
#[test]
fn uc13_escalation_steps_fire_only_on_silence() {
    let mut w = World::new();
    let b = support();
    let (case, _) = w.open(&b, "cy@x.com", "site is down", "t1");
    w.turn(&case, &waits("step_1", "acknowledged; asked for details", "signal", "1d", "details or step 2"));
    assert_eq!(w.advance(DAY).children_started, 1, "silence: step two fires");
    w.turn(&case, &waits("step_2", "escalated to a specialist", "signal", "1d", "details or step 3"));
    let step3_at = w.wait(&case).unwrap().deadline.unwrap();

    w.t += 12 * HOUR;
    w.arrive(&b, "email", "cy@x.com", msg("cy@x.com", "here are the logs"), "t2");
    assert_eq!(w.tick().children_started, 1, "the answer starts a turn now");
    w.turn(&case, &waits("investigating", "logs received", "signal", "3d", "the fix"));

    w.t = step3_at;
    let r = w.tick();
    assert_eq!((r.superseded, r.children_started), (1, 0), "step three never fires");
    assert_eq!(w.history(&case).iter().filter(|e| e.kind == "turn_result").count(), 3);
    assert!(!w.history_has(&case, "turn_result", "step_3"));
}

/// uc14 — Warranty and return cases spanning shipping, receipt, and refund.
/// One case waits for the package for two weeks; the warehouse's receipt
/// starts the refund turn; the refund is a money effect written before it
/// is attempted, one row per RMA however often it is asked for. The engine
/// proves the ledger's mechanics; the provider's charge is a plugin's.
#[test]
fn uc14_a_return_from_label_to_refund() {
    let mut w = World::new();
    let b = World::binding("cs", "returns", "return", 14 * DAY);
    let (case, _) = w.open(&b, "dee@x.com", "blender arrived broken", "r1");
    w.turn(&case, &waits("label_sent", "sent the return label", "signal", "14d", "the package at the warehouse"));
    let two_weeks = w.wait(&case).unwrap().deadline.unwrap();

    w.t += 5 * DAY;
    w.arrive(&b, "email", "dee@x.com", json!({"email": "dee@x.com", "rma": "RMA-1", "event": "package received"}), "wh-1");
    assert_eq!(w.tick().children_started, 1);
    let refund = w.start(&w.queued_turn(&case).unwrap().id);
    let first = w.s.engine_effect_pending(&refund.id, "financial", "refund:RMA-1", "stripe", "re_RMA-1", "dee@x.com").unwrap();
    let again = w.s.engine_effect_pending(&refund.id, "financial", "refund:RMA-1", "stripe", "re_RMA-1", "dee@x.com").unwrap();
    assert_eq!(first, again, "one refund row per RMA, asked twice");
    w.s.engine_effect_completed(first, Some("re_RMA-1"), Some("refunded 49.00"), w.t).unwrap();
    w.finish(&refund, &closes("refunded", "package received; refund issued"));

    let receipts = w.receipts(&refund.id);
    assert_eq!(receipts.len(), 1);
    assert_eq!((receipts[0].state.as_str(), receipts[0].provider_ref.as_deref()), ("completed", Some("re_RMA-1")));
    assert_eq!(w.run(&case).result.as_deref(), Some("refunded"));
    w.t = two_weeks;
    assert_eq!(w.tick().children_started, 0, "the package deadline is gone with the case");
}

/// uc15 — Post-resolution satisfaction checks three days later. Resolved
/// is not closed: the case waits three days, the timer's turn sends the
/// check once — a reworded second send in the same turn is refused — and
/// then closes.
#[tokio::test]
async fn uc15_a_satisfaction_check_three_days_after_resolution() {
    let mut w = World::new();
    let b = support();
    let (case, _) = w.open(&b, "eve@x.com", "wrong size", "t1");
    w.turn(&case, &waits("resolved", "exchange shipped", "signal", "3d", "the satisfaction check"));
    let resolved_at = w.t;
    assert_eq!(w.wait(&case).unwrap().deadline, Some(resolved_at + 3 * DAY));

    assert_eq!(w.advance(3 * DAY).children_started, 1);
    let check = w.start(&w.queued_turn(&case).unwrap().id);
    let r = w.send("cs", &check.id, "eve@x.com", "How did the exchange go?", SendOutcome::Sent("Sent.".into(), None)).await;
    assert!(!r.is_error);
    let r = w.send("cs", &check.id, "eve@x.com", "Quick check-in: all good with the exchange?", SendOutcome::Sent("Sent.".into(), None)).await;
    assert!(r.is_error && r.content.contains("already sent"), "one message to a person per turn");
    w.finish(&check, &closes("done", "satisfaction check sent"));
    assert_eq!(w.receipts(&check.id).iter().filter(|e| e.state == "completed").count(), 1);
    assert_eq!(w.run(&case).state, "done");
}

/// uc16 — Outage communication that updates the same customers as status
/// changes. Each subscriber is a case; each status change reaches every
/// case as one turn; each turn sends that customer exactly one update.
#[tokio::test]
async fn uc16_status_changes_update_the_same_customers() {
    let mut w = World::new();
    let b = World::binding("cs", "outage-updates", "outage", 7 * DAY);
    let (fay, _) = w.open(&b, "fay@x.com", "subscribed to status", "s-fay");
    let (gus, _) = w.open(&b, "gus@x.com", "subscribed to status", "s-gus");
    for c in [&fay, &gus] {
        w.turn(c, &waits("subscribed", "confirmed subscription", "signal", "7d", "a status change"));
    }

    for (n, status) in ["investigating", "resolved"].iter().enumerate() {
        w.t += HOUR;
        for (who, idem) in [("fay@x.com", "fay"), ("gus@x.com", "gus")] {
            let r = w.arrive(&b, "email", who, json!({"email": who, "status": status}), &format!("status-{n}-{idem}"));
            assert!(matches!(r, Routed::Signaled { .. }), "{who}: {r:?}");
        }
        assert_eq!(w.tick().children_started, 2, "one turn per subscriber");
        for (c, who) in [(&fay, "fay@x.com"), (&gus, "gus@x.com")] {
            let turn = w.start(&w.queued_turn(c).unwrap().id);
            let r = w.send("cs", &turn.id, who, &format!("Status: {status}"), SendOutcome::Sent("Sent.".into(), None)).await;
            assert!(!r.is_error);
            let r = w.send("cs", &turn.id, who, &format!("Update — status is now {status}"), SendOutcome::Sent("Sent.".into(), None)).await;
            assert!(r.is_error, "never two updates to one customer in one turn");
            w.finish(&turn, &waits("subscribed", &format!("sent {status}"), "signal", "7d", "the next status change"));
            assert_eq!(w.receipts(&turn.id).iter().filter(|e| e.state == "completed").count(), 1);
        }
    }
    for c in [&fay, &gus] {
        assert_eq!(w.history(c).iter().filter(|e| e.kind == "turn_result").count(), 3);
        assert!(w.history_has(c, "turn_result", "sent resolved"));
    }
    assert!(w.undelivered().is_empty());
}

/// uc17 — Renewal reminders tied to the contract date, not a guess. The
/// turn names the exact moment (RFC3339); the wait carries it, not the
/// binding's default; the moment before fires nothing, the moment fires
/// once.
#[test]
fn uc17_a_renewal_reminder_on_the_contract_date() {
    let mut w = World::new();
    let b = support();
    let (case, _) = w.open(&b, "hal@x.com", "signed the annual plan", "t1");
    let renewal = w.t + 60 * DAY;
    let remind_at = renewal - 30 * DAY;
    w.turn(&case, &waits("active", "contract on file", "signal", &rfc3339(remind_at), "the 30-day renewal reminder"));
    assert_eq!(w.wait(&case).unwrap().deadline, Some(remind_at), "the contract's date, not the default");
    assert_ne!(remind_at, w.t + 3 * DAY);

    w.t = remind_at - 1;
    assert_eq!(w.tick().children_started, 0);
    w.t = remind_at;
    assert_eq!(w.tick().children_started, 1);
    assert_eq!(w.tick().children_started, 0, "once");
    w.turn(&case, &waits("reminded", "sent the renewal reminder", "signal", &rfc3339(renewal), "the renewal date"));
    assert_eq!(w.wait(&case).unwrap().deadline, Some(renewal));
}

/// uc18 — Onboarding sequences that advance only when the customer
/// completes each step. Silence brings a nudge that keeps the same step;
/// only the customer's completion moves the case to the next one, and the
/// pending nudge for the finished step never fires.
#[test]
fn uc18_onboarding_advances_only_on_completion() {
    let mut w = World::new();
    let b = World::binding("cs", "onboarding", "onboarding", 14 * DAY);
    let (case, _) = w.open(&b, "ivy@x.com", "signed up", "o1");
    w.turn(&case, &waits("step_1", "sent step 1", "signal", "14d", "step 1 done or the nudge"));

    assert_eq!(w.advance(14 * DAY).children_started, 1, "silence: a nudge");
    w.turn(&case, &waits("step_1", "nudged step 1", "signal", "14d", "step 1 done or the nudge"));
    let nudge_again = w.wait(&case).unwrap().deadline.unwrap();
    assert_eq!(w.run(&case).summary, "nudged step 1", "still step one");

    w.t += 3 * DAY;
    w.arrive(&b, "email", "ivy@x.com", json!({"email": "ivy@x.com", "step": 1, "done": true}), "o2");
    assert_eq!(w.tick().children_started, 1);
    w.turn(&case, &waits("step_2", "step 1 confirmed; sent step 2", "signal", "14d", "step 2 done or the nudge"));
    assert_eq!(w.wait(&case).unwrap().reason, "step 2 done or the nudge");

    w.t = nudge_again;
    let r = w.tick();
    assert_eq!((r.superseded, r.children_started), (1, 0), "the step-1 nudge never fires");
    assert!(w.history_has(&case, "turn_result", "sent step 2"));
}
