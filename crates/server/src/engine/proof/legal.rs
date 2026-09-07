//! Proof scenarios: legal, real estate, professional services (uc47–uc51).
//! See `mod.rs`.

use super::*;
use serde_json::json;

fn msg(email: &str, text: &str) -> serde_json::Value {
    json!({"email": email, "message": text})
}

fn rfc3339(t: i64) -> String {
    chrono::DateTime::from_timestamp(t, 0).unwrap().to_rfc3339()
}

/// uc47 — Matter intake that waits on client documents. The matter waits
/// on the client with a two-week nudge; the documents start the next turn
/// the moment they arrive; the nudge that was due never fires; a second
/// send of the same documents is a duplicate, not a second turn. Must
/// never: a "still waiting on your documents" after they arrived.
#[test]
fn uc47_matter_intake_waits_on_client_documents() {
    let mut w = World::new();
    let b = World::binding("paralegal", "intake", "matter", 14 * DAY);
    let (case, _) = w.open(&b, "client@x.com", "new matter: lease dispute", "m1");
    w.turn(&case, &waits("awaiting_documents", "requested the lease and the notices", "signal", "14d", "the client's documents"));
    let nudge = w.wait(&case).unwrap().deadline.unwrap();
    assert_eq!(nudge, w.t + 14 * DAY);

    w.t += 4 * DAY;
    assert!(matches!(w.arrive(&b, "email", "client@x.com", msg("client@x.com", "documents attached: lease.pdf, notice-1.pdf"), "docs-1"), Routed::Signaled { .. }));
    assert_eq!(w.arrive(&b, "email", "client@x.com", msg("client@x.com", "documents attached: lease.pdf, notice-1.pdf"), "docs-1"), Routed::Duplicate);
    assert_eq!(w.tick().children_started, 1);
    let turn = w.queued_turn(&case).unwrap();
    assert!(turn.inputs.as_deref().unwrap().contains("lease.pdf"));
    w.turn(&case, &waits("reviewing", "documents received; conflict check started", "signal", "21d", "conflict check"));

    w.t = nudge;
    let r = w.tick();
    assert_eq!((r.children_started, r.superseded), (0, 1), "the nudge never fires");
    assert!(w.undelivered().is_empty());
}

/// uc48 — Closing timelines with inspection, appraisal, and financing
/// waits. Each stage is a wait on a real date (RFC3339); the date starts
/// the stage's turn; a stage finished early by a message advances without
/// waiting for its date, and that date then passes with nothing. Must
/// never: a stage's turn fired twice, or one stage's turn beside another.
#[test]
fn uc48_a_closing_timeline_is_a_chain_of_dated_waits() {
    let mut w = World::new();
    let b = World::binding("closer", "closings", "closing", 30 * DAY);
    let (case, _) = w.open(&b, "buyer@x.com", "under contract on 12 Elm; closing in 45 days", "c1");
    let inspection = w.t + 7 * DAY;
    let appraisal = w.t + 21 * DAY;
    let financing = w.t + 35 * DAY;
    w.turn(&case, &waits("inspection_pending", "inspection booked", "signal", &rfc3339(inspection), "the inspection date"));
    assert_eq!(w.wait(&case).unwrap().deadline, Some(inspection), "a real date, not a relative one");

    // The inspection date starts the inspection turn, once.
    w.t = inspection;
    assert_eq!(w.tick().children_started, 1);
    assert_eq!(w.tick().children_started, 0, "not twice");
    w.turn(&case, &waits("appraisal_pending", "inspection passed; appraisal ordered", "signal", &rfc3339(appraisal), "the appraisal date"));

    // The appraisal comes back early: the message advances the stage.
    w.t += 5 * DAY;
    w.arrive(&b, "email", "buyer@x.com", msg("buyer@x.com", "appraisal came in at contract price"), "c2");
    assert_eq!(w.tick().children_started, 1);
    w.turn(&case, &waits("financing_pending", "appraisal in; waiting on the lender", "signal", &rfc3339(financing), "financing commitment"));
    w.t = appraisal;
    let r = w.tick();
    assert_eq!((r.children_started, r.superseded), (0, 1), "the appraisal date passes with nothing");

    w.t = financing;
    assert_eq!(w.tick().children_started, 1);
    w.turn(&case, &closes("closed", "financing committed; closed on 12 Elm"));
    assert_eq!(w.run(&case).state, "done");
    let stages: Vec<String> = w.history(&case).iter().filter(|e| e.kind == "turn_result").map(|e| e.payload.clone()).collect();
    assert_eq!(stages.len(), 4);
    assert!(stages[1].contains("inspection passed") && stages[2].contains("appraisal in") && stages[3].contains("closed on 12 Elm"));
}

/// uc49 — Listing follow-ups per buyer with a contact window. Two buyers
/// are two cases on one listing; each waits until the window opens (a real
/// time), and nothing starts before it; when it opens, each buyer gets
/// exactly one turn. Must never: a touch outside the window, or one
/// buyer's turn carrying another buyer's thread.
#[test]
fn uc49_listing_follow_ups_per_buyer_inside_a_contact_window() {
    let mut w = World::new();
    let b = World::binding("agent-lisa", "listing-12-elm", "buyer", 3 * DAY);
    let (ann, _) = w.open(&b, "ann@x.com", "saw 12 Elm, interested", "a1");
    let (ben, _) = w.open(&b, "ben@x.com", "is 12 Elm still available?", "b1");
    assert_ne!(ann, ben, "one case per buyer");
    let opens = w.t + 10 * HOUR; // 09:00 tomorrow, say
    w.turn(&ann, &waits("contacted", "replied to Ann; next touch when the window opens", "signal", &rfc3339(opens), "the contact window"));
    w.turn(&ben, &waits("contacted", "replied to Ben; next touch when the window opens", "signal", &rfc3339(opens), "the contact window"));

    // Before the window: nothing.
    w.t = opens - HOUR;
    assert_eq!(w.tick().children_started, 0);
    assert!(w.queued_turn(&ann).is_none() && w.queued_turn(&ben).is_none());

    // The window opens: one turn each, each with its own thread.
    w.t = opens;
    assert_eq!(w.tick().children_started, 2);
    let ann_turn = w.queued_turn(&ann).unwrap();
    let ben_turn = w.queued_turn(&ben).unwrap();
    assert!(ann_turn.inputs.as_deref().unwrap().contains("replied to Ann") && !ann_turn.inputs.as_deref().unwrap().contains("Ben"));
    assert!(ben_turn.inputs.as_deref().unwrap().contains("replied to Ben") && !ben_turn.inputs.as_deref().unwrap().contains("Ann"));
    assert_eq!(open_case_for(&w.s, "buyer", "email", "ann@x.com").unwrap().id, ann);
    assert_eq!(open_case_for(&w.s, "buyer", "email", "ben@x.com").unwrap().id, ben);
}

/// uc50 — Engagement letters that park for signature and resume on
/// receipt. The run parks on the signature; nothing is billed while
/// parked; the signature resumes it exactly once; the same signature sent
/// twice is one; a signature for a letter that is not out wakes nothing.
/// Must never: work billed before the letter is signed.
#[test]
fn uc50_an_engagement_letter_parks_for_signature() {
    let mut w = World::new();
    let b = World::binding("partner", "engagements", "engagement", 7 * DAY);
    let (case, queued) = w.open(&b, "client@x.com", "please send the engagement letter", "e1");
    let turn = w.start(&queued.id);
    let key = format!("signature:{}", turn.id);
    w.s.engine_declare_wait(&turn.id, &NewWait { action: "resume", on_kind: "signal", key: &key, parked: Some("{}"), reason: "engagement letter out for signature", ..Default::default() }, w.t).unwrap();
    assert!(w.receipts(&turn.id).is_empty(), "nothing billed while parked");

    w.t += 2 * DAY;
    let signed = NewEvent { kind: "signal", target_type: "run", target_id: &key, payload: r#"{"signed_by":"client@x.com","document":"engagement-2026-09.pdf"}"#, channel: "esign", idem_key: "sig-1", durable: true, ..Default::default() };
    assert!(matches!(w.s.engine_enqueue_event(&signed).unwrap(), Enqueued::Inserted(_)));
    assert_eq!(w.s.engine_enqueue_event(&signed).unwrap(), Enqueued::Duplicate, "the provider's retry is one signature");
    let signed_at = w.t;
    assert_eq!(w.tick().resumed, 1);
    let resumed = w.run(&turn.id);
    assert_eq!(resumed.state, "queued");
    assert!(resumed.woken_by().is_some(), "the signature is on the run");

    // Now the retainer is billed, dated after the signature.
    w.t += 60;
    let retainer = w.s.engine_effect_pending(&turn.id, "financial", "invoice:retainer:client", "quickbooks", "inv_idem_r1", "client@x.com").unwrap();
    w.s.engine_effect_attempted(retainer).unwrap();
    w.s.engine_effect_completed(retainer, Some("INV-1"), Some("retainer invoiced $2,500"), w.t).unwrap();
    assert!(w.receipts(&turn.id)[0].completed_at.unwrap() > signed_at);
    w.s.engine_set_run_state(&turn.id, "running", w.t, None).unwrap();
    w.finish(&w.run(&turn.id), &waits("engaged", "letter signed; retainer invoiced", "signal", "7d", "retainer payment"));
    assert!(w.history_has(&case, "turn_result", "quickbooks#"));

    // A stray signature later reaches no wait and starts nothing.
    w.s.engine_enqueue_event(&NewEvent { idem_key: "sig-late", ..signed }).unwrap();
    let r = w.tick();
    assert_eq!((r.resumed, r.children_started, r.unrouted), (0, 0, 1));
}

/// uc51 — Court or filing deadlines as durable timers with reminders. The
/// filing date is a real date; reminders are the case's waits at seven
/// days and one day out; the timers are rows that survive a restart and
/// fire on the day, never twice, never early. Must never: a deadline that
/// forgets itself because the machine was off.
#[test]
fn uc51_filing_deadlines_are_durable_timers_with_reminders() {
    let mut w = World::new();
    let b = World::binding("paralegal", "filings", "filing", 30 * DAY);
    let (case, _) = w.open(&b, "counsel@x.com", "answer due in 30 days on Smith v. Jones", "f1");
    let filing = w.t + 30 * DAY;
    let seven_out = filing - 7 * DAY;
    let one_out = filing - DAY;
    w.turn(&case, &waits("calendared", "answer calendared", "signal", &rfc3339(seven_out), "seven-day reminder"));

    // The machine is off for two weeks; on boot the timer is still a row.
    suspend_for_shutdown(&w.s);
    w.t += 14 * DAY;
    recover(&w.s);
    let timers = w.s.engine_pending_timers("wait").unwrap();
    assert_eq!(timers.len(), 1);
    assert_eq!(timers[0].due_at, Some(seven_out), "the reminder survived the restart");
    assert_eq!(w.tick().children_started, 0, "and it is not early");

    w.t = seven_out;
    assert_eq!(w.tick().children_started, 1, "seven days out: the reminder turn");
    assert_eq!(w.tick().children_started, 0, "once");
    w.turn(&case, &waits("reminded", "seven-day reminder sent to counsel", "signal", &rfc3339(one_out), "one-day reminder"));
    w.t = one_out;
    assert_eq!(w.tick().children_started, 1);
    w.turn(&case, &waits("final_reminder", "one-day reminder sent", "signal", &rfc3339(filing), "the filing itself"));
    w.t = filing;
    assert_eq!(w.tick().children_started, 1);
    w.turn(&case, &closes("filed", "answer filed on the day"));
    assert_eq!(w.run(&case).state, "done");
    assert_eq!(w.history(&case).iter().filter(|e| e.kind == "turn_result").count(), 4);
    assert!(w.s.engine_pending_timers("wait").unwrap().is_empty(), "nothing left to fire");
}
