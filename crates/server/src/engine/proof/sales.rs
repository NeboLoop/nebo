//! Proof scenarios: sales and lead handling (uc01–uc10). See `mod.rs`.

use super::*;
use serde_json::json;

fn lead() -> CaseBinding<'static> {
    World::binding("ic", "work-lead", "lead", 3 * DAY)
}

fn msg(email: &str, text: &str) -> serde_json::Value {
    json!({"email": email, "message": text})
}

/// uc01 — Work a lead across weeks with one thread of memory instead of a
/// fresh email each time. Three turns over ten days: each later turn reads
/// what the earlier ones did, and the person has one case throughout.
#[test]
fn uc01_one_thread_of_memory_across_weeks() {
    let mut w = World::new();
    let b = lead();
    let (case, _) = w.open(&b, "pat@x.com", "Need a quote for weekly lawn care", "m1");
    w.turn(&case, &waits("contacted", "sent first contact", "signal", "3d", "reply or day 3"));
    assert_eq!(w.wait(&case).unwrap().deadline, Some(w.t + 3 * DAY));

    // Day 3: the deadline starts turn two, which carries turn one's words.
    assert_eq!(w.advance(3 * DAY).children_started, 1);
    let queued = w.queued_turn(&case).unwrap();
    assert!(queued.inputs.as_deref().unwrap().contains("sent first contact"), "turn two reads turn one");
    w.turn(&case, &waits("waiting_on_customer", "sent day-3 touch", "signal", "7d", "reply or day 10"));

    // Day 9: a reply starts turn three, which carries both.
    w.t += 6 * DAY;
    assert!(matches!(w.arrive(&b, "email", "pat@x.com", msg("pat@x.com", "yes, call me"), "m2"), Routed::Signaled { .. }));
    assert_eq!(w.tick().children_started, 1);
    let inputs = w.queued_turn(&case).unwrap().inputs.unwrap();
    assert!(inputs.contains("sent first contact") && inputs.contains("sent day-3 touch") && inputs.contains("yes, call me"));
    assert_eq!(open_case_for(&w.s, "lead", "email", "pat@x.com").unwrap().id, case, "one case all along");
    assert_eq!(w.history(&case).iter().filter(|e| e.kind == "turn_result").count(), 2);
}

/// uc02 — Follow-up cadences that stop the moment the person replies. The
/// reply starts a turn now; the touch that was scheduled for day three
/// never fires, because the turn the reply started replaced that wait.
#[test]
fn uc02_a_reply_stops_the_cadence() {
    let mut w = World::new();
    let b = lead();
    let (case, _) = w.open(&b, "sam@x.com", "Interested", "m1");
    w.turn(&case, &waits("contacted", "sent first contact", "signal", "3d", "day-3 touch"));
    let day3 = w.wait(&case).unwrap().deadline.unwrap();

    w.t += DAY;
    w.arrive(&b, "email", "sam@x.com", msg("sam@x.com", "Let's talk"), "m2");
    assert_eq!(w.tick().children_started, 1, "the reply starts a turn now");
    w.turn(&case, &waits("qualifying", "asked the first question", "signal", "5d", "their answer"));

    // Day three comes: the old touch is a superseded generation, not a send.
    w.t = day3;
    let r = w.tick();
    assert_eq!((r.superseded, r.children_started), (1, 0), "the scheduled touch never fires");
    assert!(w.queued_turn(&case).is_none());
    assert_eq!(w.wait(&case).unwrap().reason, "their answer");
}

/// uc03 — Four form submissions from one person become one case, not
/// four emails: one case, one first turn, the later three ride it.
#[test]
fn uc03_four_submissions_one_case_one_first_turn() {
    let w = World::new();
    let b = lead();
    let (case, turn) = w.open(&b, "alma@x.com", "form 1", "sub-1");
    for (i, text) in ["form 2", "form 3", "form 4"].iter().enumerate() {
        assert_eq!(w.arrive(&b, "email", "alma@x.com", msg("alma@x.com", text), &format!("sub-{}", i + 2)), Routed::Signaled { case_id: case.clone() });
    }
    assert_eq!(w.arrive(&b, "email", "alma@x.com", msg("alma@x.com", "form 4"), "sub-4"), Routed::Duplicate, "a replay is not a fifth");
    let r = w.tick();
    assert_eq!((r.steered, r.children_started), (3, 0));
    let turns = w.s.engine_queued_runs_of_kind("workflow", 10).unwrap();
    assert_eq!(turns.len(), 1, "still exactly one turn");
    assert_eq!(turns[0].id, turn.id);
    let inputs = turns[0].inputs.as_deref().unwrap();
    assert!(inputs.contains("form 1") && inputs.contains("form 2") && inputs.contains("form 3") && inputs.contains("form 4"));
    assert!(w.undelivered().is_empty(), "nothing left over to fire a fifth");
}

/// uc04 — Appointment reminders that cancel themselves when the booking
/// changes. The reminder is the case's wait; a rebooking turn declares a
/// new one, and the old moment passes with nothing sent.
#[test]
fn uc04_a_rebooking_cancels_the_old_reminder() {
    let mut w = World::new();
    let b = lead();
    let (case, _) = w.open(&b, "kim@x.com", "book me Tuesday", "m1");
    w.turn(&case, &waits("booked", "booked Tuesday 10am", "signal", "2d", "T-24h reminder for Tuesday"));
    let old_reminder = w.wait(&case).unwrap().deadline.unwrap();

    w.t += DAY;
    w.arrive(&b, "email", "kim@x.com", msg("kim@x.com", "can we do Friday instead?"), "m2");
    assert_eq!(w.tick().children_started, 1);
    w.turn(&case, &waits("booked", "moved to Friday 10am", "signal", "4d", "T-24h reminder for Friday"));
    let new_reminder = w.wait(&case).unwrap().deadline.unwrap();
    assert_ne!(old_reminder, new_reminder);

    w.t = old_reminder;
    let r = w.tick();
    assert_eq!((r.superseded, r.children_started), (1, 0), "Tuesday's reminder never fires");
    w.t = new_reminder;
    assert_eq!(w.tick().children_started, 1, "Friday's fires once");
    assert_eq!(w.tick().children_started, 0);
}

/// uc05 — Reactivation of leads that went quiet ninety days ago. A case
/// closed as unresponsive reopens on the person's next message, with its
/// history intact, and the message starts the next turn.
#[test]
fn uc05_a_quiet_lead_reactivates_into_the_same_case() {
    let mut w = World::new();
    let b = lead();
    let (case, _) = w.open(&b, "lee@x.com", "maybe next quarter", "m1");
    w.turn(&case, &closes("unresponsive", "no reply after four touches; door stays open"));
    assert_eq!(w.run(&case).state, "done");

    w.t += 90 * DAY;
    let routed = w.arrive(&b, "email", "lee@x.com", msg("lee@x.com", "ready now — still available?"), "m2");
    assert_eq!(routed, Routed::Reopened { case_id: case.clone() });
    assert_eq!(w.run(&case).state, "waiting");
    let turn = w.queued_turn(&case).expect("the message starts the next turn");
    let inputs = turn.inputs.unwrap();
    assert!(inputs.contains("door stays open"), "the old history is in the turn");
    assert!(inputs.contains("ready now"));
    assert!(w.history_has(&case, "reopened", "unresponsive"));
}

/// uc06 — Quote follow-up that waits for the customer, then a deadline,
/// then escalates. Two silent deadlines start two touches; the third turn
/// hands the case to the owner and closes it. Never two turns at once.
#[test]
fn uc06_wait_then_deadline_then_escalate() {
    let mut w = World::new();
    let b = lead();
    let (case, _) = w.open(&b, "ray@x.com", "send me a quote", "m1");
    w.turn(&case, &waits("waiting_on_customer", "sent the quote", "signal", "3d", "their answer or day 3"));
    assert_eq!(w.advance(3 * DAY).children_started, 1);
    let second = w.queued_turn(&case).unwrap();
    assert!(second.inputs.as_deref().unwrap().contains("case.timer"), "started by the deadline, not a person");
    w.turn(&case, &waits("waiting_on_customer", "nudged once", "signal", "3d", "their answer or day 6"));
    assert_eq!(w.advance(3 * DAY).children_started, 1);
    let third = w.start(&w.queued_turn(&case).unwrap().id);
    // While the third turn runs, another deadline cannot start a fourth.
    assert_eq!(w.advance(3 * DAY).children_started, 0, "one live turn per case");
    w.finish(&third, &closes("owner_takeover", "two silent deadlines; handed to the owner"));
    let case_run = w.run(&case);
    assert_eq!((case_run.state.as_str(), case_run.result.as_deref()), ("done", Some("owner_takeover")));
    assert_eq!(w.history(&case).iter().filter(|e| e.kind == "turn_result").count(), 3);
}

/// uc07 — Referral tracking from introduction to close. Three signals
/// over weeks are one case; once won, the person's next message opens a
/// new case linked to the won one, never a reopening of it.
#[test]
fn uc07_a_referral_from_introduction_to_close() {
    let mut w = World::new();
    let b = lead();
    let (case, _) = w.open(&b, "ref@x.com", "Jo introduced us", "intro");
    w.turn(&case, &waits("contacted", "thanked Jo, asked for a call", "signal", "7d", "a call"));
    w.t += 2 * DAY;
    w.arrive(&b, "email", "ref@x.com", msg("ref@x.com", "met today"), "met");
    w.tick();
    w.turn(&case, &waits("qualifying", "met; sending terms", "signal", "7d", "signature"));
    w.t += 5 * DAY;
    w.arrive(&b, "email", "ref@x.com", msg("ref@x.com", "signed"), "signed");
    w.tick();
    w.turn(&case, &closes("won", "signed; referral credited to Jo"));

    w.t += 30 * DAY;
    let Routed::Opened { case_id: next } = w.arrive(&b, "email", "ref@x.com", msg("ref@x.com", "another project"), "again") else { panic!("won never reopens: a new case") };
    assert_ne!(next, case);
    let inputs: serde_json::Value = serde_json::from_str(w.run(&next).inputs.as_deref().unwrap()).unwrap();
    assert_eq!(inputs["_case"]["previous_case"]["id"], json!(case));
    assert_eq!(inputs["_case"]["previous_case"]["closed_as"], json!("won"));
    assert_eq!(w.run(&case).state, "done", "the won case stays closed");
}

/// uc08 — Trade-show lead sequences with a per-contact pause window. Two
/// contacts, two cases, two clocks: one asked for two weeks, the other
/// gets day three; day three touches exactly one of them.
#[test]
fn uc08_per_contact_pause_windows() {
    let mut w = World::new();
    let b = lead();
    let (a, _) = w.open(&b, "a@show.com", "booth 12", "a1");
    let (c, _) = w.open(&b, "c@show.com", "booth 12", "c1");
    w.turn(&a, &waits("contacted", "asked to pause two weeks", "signal", "14d", "their pause"));
    w.turn(&c, &waits("contacted", "sent first contact", "signal", "3d", "day-3 touch"));
    assert_ne!(w.wait(&a).unwrap().deadline, w.wait(&c).unwrap().deadline);

    assert_eq!(w.advance(3 * DAY).children_started, 1);
    assert!(w.queued_turn(&c).is_some() && w.queued_turn(&a).is_none(), "only the day-3 contact is touched");
    w.turn(&c, &waits("contacted", "day-3 touch", "signal", "3d", "day-6"));
    assert_eq!(w.advance(11 * DAY).children_started, 2, "day 14: the paused one wakes, and c's day-6 fired too");
    assert!(w.queued_turn(&a).is_some());
}

/// uc09 — Handoff to a human owner that freezes the case until the owner
/// releases it. The case waits on the owner; a customer reply meanwhile is
/// parked on the case, not lost; the owner's release starts a turn that
/// carries the reply, and the parked reply is not carried twice.
#[test]
fn uc09_a_handoff_freezes_the_case_and_holds_replies() {
    let mut w = World::new();
    let b = lead();
    let (case, _) = w.open(&b, "vip@x.com", "I want to negotiate price", "m1");
    w.turn(&case, &waits("owner_takeover", "price negotiation; handed to the owner", "approval", "", "the owner's release"));
    let frozen = w.wait(&case).unwrap();
    // Frozen on the owner — under the binding's safety deadline, because
    // the engine never lets a case sleep forever.
    assert_eq!((frozen.on_kind.as_str(), frozen.deadline), ("approval", Some(w.t + 3 * DAY)));

    // A reply while frozen: held on the case, never a turn, never dropped.
    w.t += DAY;
    w.arrive(&b, "email", "vip@x.com", msg("vip@x.com", "any update?"), "m2");
    let r = w.tick();
    assert_eq!((r.children_started, r.unrouted, r.parked), (0, 0, 1));
    assert!(w.queued_turn(&case).is_none());
    assert!(w.undelivered().is_empty(), "the reply is on the case, not in the queue");
    assert!(w.run(&case).inputs.unwrap().contains("any update?"));

    // The owner releases: one turn, carrying the held reply.
    w.t += DAY;
    let key = frozen.key.clone();
    w.s.engine_enqueue_event(&NewEvent { kind: "approval", target_type: "run", target_id: &key, payload: "released by the owner", idem_key: "release-1", durable: true, ..Default::default() }).unwrap();
    assert_eq!(w.tick().children_started, 1);
    let turn = w.queued_turn(&case).unwrap();
    assert!(turn.inputs.as_deref().unwrap().contains("any update?"), "the held reply rides the release turn");
    w.turn(&case, &waits("qualifying", "back in the employee's hands", "signal", "3d", "their answer"));
    assert!(!w.run(&case).inputs.unwrap().contains("any update?"), "carried once, then cleared");
}

/// uc10 — A discount over a threshold parks for approval before the email
/// goes out. The turn parks; the owner's answer resumes it exactly once; a
/// second click is a duplicate; the email's receipt is dated after the
/// approval; a customer reply during the park waits its turn.
#[tokio::test]
async fn uc10_a_discount_parks_for_approval_before_the_email() {
    let mut w = World::new();
    let b = lead();
    let (case, queued) = w.open(&b, "big@x.com", "can you do 30% off?", "m1");
    let turn = w.start(&queued.id);
    w.s.engine_declare_wait(&turn.id, &NewWait { action: "resume", on_kind: "approval", key: &format!("approval:{}", turn.id), parked: Some("{}"), reason: "30% discount needs your approval", ..Default::default() }, w.t).unwrap();

    // A reply during the park is deferred behind the parked turn.
    w.t += HOUR;
    w.arrive(&b, "email", "big@x.com", msg("big@x.com", "still there?"), "m2");
    assert_eq!(w.tick().children_started, 0, "the parked turn is the live one");

    // The owner approves; a second click is a duplicate; the turn resumes once.
    w.t += HOUR;
    let approved_at = w.t;
    let answer = |idem: &'static str| NewEvent { kind: "approval", target_type: "run", target_id: "", payload: r#"{"approved":true}"#, channel: "owner", idem_key: idem, durable: true, ..Default::default() };
    let key = format!("approval:{}", turn.id);
    assert!(matches!(w.s.engine_enqueue_event(&NewEvent { target_id: &key, ..answer("ok-1") }).unwrap(), Enqueued::Inserted(_)));
    assert_eq!(w.s.engine_enqueue_event(&NewEvent { target_id: &key, ..answer("ok-1") }).unwrap(), Enqueued::Duplicate);
    assert_eq!(w.tick().resumed, 1);
    assert_eq!(w.run(&turn.id).state, "queued", "resumes at the parked call");

    // The email goes out after the approval, with a receipt.
    w.t += 60;
    let r = w.send("ic", &turn.id, "big@x.com", "30% approved — here is the quote", SendOutcome::Sent("Sent.".into(), Some("m-1".into()))).await;
    assert!(!r.is_error, "{}", r.content);
    let receipt = &w.receipts(&turn.id)[0];
    assert_eq!(receipt.state, "completed");
    assert!(receipt.completed_at.unwrap() > approved_at);
    w.s.engine_set_run_state(&turn.id, "running", w.t, None).unwrap();
    w.finish(&w.run(&turn.id), &waits("quoted", "sent the approved quote", "signal", "3d", "their answer"));
    assert!(w.history_has(&case, "turn_result", "1 send(s) on the ledger this turn: mail-app#"));
}
