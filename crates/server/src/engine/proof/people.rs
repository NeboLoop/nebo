//! Proof scenarios: hiring and people (uc42–uc46). See `mod.rs`.

use super::*;
use serde_json::json;

fn hiring() -> CaseBinding<'static> {
    World::binding("hr", "candidates", "candidate", 7 * DAY)
}

fn msg(email: &str, text: &str) -> serde_json::Value {
    json!({"email": email, "message": text})
}

/// uc42 — Candidate pipelines with one case per applicant across
/// interviews. Two applicants, two cases; each interview result lands in
/// its own case; a second application from the same person joins theirs.
#[test]
fn uc42_one_case_per_applicant_across_interviews() {
    let mut w = World::new();
    let b = hiring();
    let (jo, _) = w.open(&b, "jo@x.com", "applied: engineer", "app-jo");
    let (kai, _) = w.open(&b, "kai@x.com", "applied: engineer", "app-kai");
    assert_ne!(jo, kai);
    for c in [&jo, &kai] {
        w.turn(c, &waits("screening", "scheduled the screen", "signal", "7d", "the screen result"));
    }
    assert_eq!(w.s.engine_cases(Some("hr"), 10).unwrap().len(), 2);

    w.t += DAY;
    assert_eq!(w.arrive(&b, "email", "jo@x.com", json!({"email": "jo@x.com", "stage": "screen", "result": "pass"}), "jo-screen"), Routed::Signaled { case_id: jo.clone() });
    assert_eq!(w.arrive(&b, "email", "jo@x.com", msg("jo@x.com", "applied again from the careers page"), "app-jo-2"), Routed::Signaled { case_id: jo.clone() }, "one case per applicant");
    let r = w.tick();
    assert_eq!((r.children_started, r.steered), (1, 1));
    assert!(w.queued_turn(&kai).is_none(), "Kai's case is untouched");
    w.turn(&jo, &waits("onsite", "screen passed; onsite booked", "signal", "7d", "the onsite result"));

    w.t += 3 * DAY;
    w.arrive(&b, "email", "kai@x.com", json!({"email": "kai@x.com", "stage": "screen", "result": "fail"}), "kai-screen");
    w.tick();
    w.turn(&kai, &closes("declined", "screen not passed; sent a kind no"));
    assert_eq!(w.run(&kai).result.as_deref(), Some("declined"));
    assert_eq!(w.run(&jo).state, "waiting");
    assert!(!w.history(&jo).iter().any(|e| e.payload.contains("kind no")), "nothing of Kai's in Jo's history");
    assert!(!w.history(&kai).iter().any(|e| e.payload.contains("onsite")));
}

/// uc43 — Offer follow-ups that wait for a signature with a deadline. The
/// signature arrives on day three: the case closes accepted, and the
/// day-five follow-up never fires.
#[test]
fn uc43_an_offer_waits_for_a_signature_with_a_deadline() {
    let mut w = World::new();
    let b = hiring();
    let (case, _) = w.open(&b, "lu@x.com", "offer extended", "offer");
    w.turn(&case, &waits("offer_out", "sent the offer", "signal", "5d", "the signature or a follow-up"));
    let follow_up = w.wait(&case).unwrap().deadline.unwrap();
    assert_eq!(follow_up, w.t + 5 * DAY);

    w.t += 3 * DAY;
    w.arrive(&b, "email", "lu@x.com", json!({"email": "lu@x.com", "signed": true}), "signed");
    assert_eq!(w.tick().children_started, 1);
    w.turn(&case, &closes("accepted", "offer signed"));
    assert_eq!(w.run(&case).result.as_deref(), Some("accepted"));

    w.t = follow_up;
    assert_eq!(w.tick().children_started, 0, "no follow-up after the signature");
    assert!(w.undelivered().is_empty());
}

/// uc44 — Employee onboarding checklists spanning weeks. Day one, day
/// seven, day thirty: each is the case's own deadline, each fires once at
/// its moment, and a restart in between leaves the waiting case exactly
/// where it was.
#[test]
fn uc44_an_onboarding_checklist_across_weeks() {
    let mut w = World::new();
    let b = World::binding("hr", "onboarding", "new-hire", 30 * DAY);
    let (case, _) = w.open(&b, "mo@x.com", "start date confirmed", "hire");
    w.turn(&case, &waits("day_0", "sent the welcome pack", "signal", "1d", "day-1 setup check"));
    let day1 = w.wait(&case).unwrap().deadline.unwrap();

    // A restart while waiting: the wait is the same one, the case is still waiting.
    recover(&w.s);
    assert_eq!(w.run(&case).state, "waiting");
    assert_eq!(w.wait(&case).unwrap().deadline, Some(day1));

    w.t = day1;
    assert_eq!(w.tick().children_started, 1);
    w.turn(&case, &waits("day_1", "laptop and accounts checked", "signal", "6d", "week-1 check-in"));
    let day7 = w.wait(&case).unwrap().deadline.unwrap();
    assert_eq!(day7, day1 + 6 * DAY);
    w.t = day7 - HOUR;
    assert_eq!(w.tick().children_started, 0);
    w.t = day7;
    assert_eq!(w.tick().children_started, 1);
    w.turn(&case, &waits("week_1", "week-1 check-in done", "signal", "23d", "day-30 review"));
    let day30 = w.wait(&case).unwrap().deadline.unwrap();
    assert_eq!(day30, day7 + 23 * DAY);
    w.t = day30;
    assert_eq!(w.tick().children_started, 1);
    w.turn(&case, &closes("onboarded", "day-30 review done"));
    assert_eq!(w.history(&case).iter().filter(|e| e.kind == "turn_result").count(), 4);
}

/// uc45 — Reference checks that wait on third parties. The case is the
/// candidate's; each referee's answer names the candidate and lands in it.
/// One reference is not enough to close: the case waits for the second,
/// nudges when it is late, and closes only when both are in.
#[test]
fn uc45_reference_checks_wait_on_third_parties() {
    let mut w = World::new();
    let b = hiring();
    let (case, _) = w.open(&b, "nia@x.com", "offer pending references", "refs");
    w.turn(&case, &waits("references_requested", "asked two referees", "signal", "7d", "the references or a nudge"));

    w.t += 2 * DAY;
    w.arrive(&b, "email", "nia@x.com", json!({"email": "nia@x.com", "referee": "boss@old.com", "answer": "strong hire"}), "ref-1");
    w.tick();
    w.turn(&case, &waits("one_reference", "one reference in", "signal", "5d", "the second reference or a nudge"));
    assert_eq!(w.run(&case).state, "waiting", "one reference does not close it");

    assert_eq!(w.advance(5 * DAY).children_started, 1, "the second is late: a nudge");
    w.turn(&case, &waits("one_reference", "nudged the second referee", "signal", "5d", "the second reference"));

    w.t += DAY;
    w.arrive(&b, "email", "nia@x.com", json!({"email": "nia@x.com", "referee": "peer@old.com", "answer": "would rehire"}), "ref-2");
    assert_eq!(w.tick().children_started, 1);
    w.turn(&case, &closes("references_complete", "both references in"));
    assert_eq!(w.run(&case).result.as_deref(), Some("references_complete"));
    assert!(w.history_has(&case, "turn_result", "nudged the second referee"));
}

/// uc46 — Time-off and approval flows parked on a manager. The turn parks
/// on the manager; nothing is actioned while parked; the employee's nudge
/// meanwhile waits its turn and is not lost; the manager's answer resumes
/// the turn once, a second click is a duplicate.
#[test]
fn uc46_time_off_parked_on_a_manager() {
    let mut w = World::new();
    let b = World::binding("hr", "time-off", "time-off", 7 * DAY);
    let (case, queued) = w.open(&b, "ola@x.com", "PTO June 3–7", "pto");
    let turn = w.start(&queued.id);
    let key = format!("approval:{}", turn.id);
    w.s.engine_declare_wait(&turn.id, &NewWait { action: "resume", on_kind: "approval", key: &key, parked: Some("{}"), reason: "PTO needs the manager's approval", ..Default::default() }, w.t).unwrap();
    assert_eq!(w.run(&turn.id).state, "waiting");
    assert!(w.receipts(&turn.id).is_empty(), "nothing actioned while parked");

    // The employee nudges: deferred behind the parked turn, still owed.
    w.t += DAY;
    w.arrive(&b, "email", "ola@x.com", msg("ola@x.com", "any news on my PTO?"), "nudge");
    assert_eq!(w.tick().children_started, 0);
    assert_eq!(w.undelivered().len(), 1);

    // The manager approves; a second click is the same answer.
    w.t += HOUR;
    let answer = |idem: &'static str| NewEvent { kind: "approval", target_type: "run", target_id: "", payload: r#"{"approved":true}"#, channel: "owner", idem_key: idem, durable: true, ..Default::default() };
    assert!(matches!(w.s.engine_enqueue_event(&NewEvent { target_id: &key, ..answer("ok") }).unwrap(), Enqueued::Inserted(_)));
    assert_eq!(w.s.engine_enqueue_event(&NewEvent { target_id: &key, ..answer("ok") }).unwrap(), Enqueued::Duplicate);
    assert_eq!(w.tick().resumed, 1);
    assert_eq!(w.run(&turn.id).state, "queued");
    w.s.engine_set_run_state(&turn.id, "running", w.t, None).unwrap();
    w.finish(&w.run(&turn.id), &waits("approved", "approved by the manager; calendar updated", "signal", "1d", "their acknowledgement"));

    // The nudge outlives the park and starts the next turn: never lost.
    w.t += db::EVENT_LEASE_SECS + 1;
    assert_eq!(w.tick().children_started, 1);
    assert!(w.queued_turn(&case).unwrap().inputs.unwrap().contains("any news on my PTO?"));
    assert!(w.undelivered().is_empty());
}
