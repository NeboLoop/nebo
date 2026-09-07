//! Proof scenarios: marketing and content (uc36–uc41). See `mod.rs`.

use super::*;
use serde_json::json;
use workflow::cases::route_signal;

use super::operations::{job, local};

fn mk(case_type: &str) -> CaseBinding<'static> {
    World::binding("mk", "campaign", case_type, 5 * DAY)
}

fn msg(email: &str, text: &str) -> serde_json::Value {
    json!({"email": email, "message": text})
}


/// uc36 — Drip campaigns that pause per contact on reply and resume on
/// silence. Step two is the case's wait; a reply replaces it with a turn
/// now, whose wait resumes the drip only after five silent days. The
/// scheduled step never fires; the resume fires once.
#[test]
fn uc36_a_drip_pauses_on_reply_and_resumes_on_silence() {
    let mut w = World::new();
    let b = mk("drip");
    let (case, _) = w.open(&b, "d@x.com", "signed up", "s1");
    w.turn(&case, &waits("step1", "sent step 1", "signal", "2d", "step 2"));
    let step2 = w.wait(&case).unwrap().deadline.unwrap();

    w.t += DAY;
    w.arrive(&b, "email", "d@x.com", msg("d@x.com", "thanks, reading it"), "reply");
    assert_eq!(w.tick().children_started, 1, "the reply pauses the drip: a conversation turn now");
    w.turn(&case, &waits("paused", "answered; drip paused", "signal", "5d", "resume the drip after five silent days"));
    let resume = w.wait(&case).unwrap().deadline.unwrap();

    w.t = step2;
    let r = w.tick();
    assert_eq!((r.superseded, r.children_started), (1, 0), "step 2 never fires on its old clock");
    w.t = resume;
    assert_eq!(w.tick().children_started, 1, "silence resumes the drip once");
    assert!(w.queued_turn(&case).unwrap().inputs.unwrap().contains("case.timer"));
    assert_eq!(w.advance(1).children_started, 0);
}

/// uc37 — Content calendars where each piece is a case from brief to
/// publish. A piece is keyed by its id: brief, draft, and approval are one
/// case; another piece is another case; publish closes the piece.
#[test]
fn uc37_each_piece_is_one_case_from_brief_to_publish() {
    let mut w = World::new();
    let b = mk("piece");
    let Routed::Opened { case_id: post } = w.arrive(&b, "crm", "post-2026-09", json!({"crm_id": "post-2026-09", "stage": "brief"}), "b1") else { panic!() };
    w.turn(&post, &waits("briefed", "brief accepted; drafting", "signal", "3d", "the draft"));
    w.t += DAY;
    assert_eq!(w.arrive(&b, "crm", "post-2026-09", json!({"crm_id": "post-2026-09", "stage": "draft"}), "d1"), Routed::Signaled { case_id: post.clone() });
    let Routed::Opened { case_id: video } = w.arrive(&b, "crm", "video-11", json!({"crm_id": "video-11", "stage": "brief"}), "v1") else { panic!() };
    assert_ne!(post, video);
    w.tick();
    w.turn(&post, &waits("drafted", "draft in review", "signal", "3d", "approval"));
    w.t += DAY;
    w.arrive(&b, "crm", "post-2026-09", json!({"crm_id": "post-2026-09", "stage": "approved"}), "a1");
    w.tick();
    w.turn(&post, &closes("published", "published Tuesday 9am"));
    assert!(open_case_for(&w.s, "piece", "crm", "post-2026-09").is_none(), "the piece is done");
    assert_eq!(open_case_for(&w.s, "piece", "crm", "video-11").unwrap().id, video, "the video is still open");
    assert_eq!(w.history(&post).iter().filter(|e| e.kind == "turn_result").count(), 3);
}

/// uc38 — Review solicitation timed to delivery, cancelled on complaint.
/// The review request is the case's wait three days after delivery; a
/// complaint on day one starts a turn that hands the case to the owner;
/// day three passes with no request sent and no turn started.
#[test]
fn uc38_a_complaint_cancels_the_review_request() {
    let mut w = World::new();
    let b = mk("delivery");
    let (case, _) = w.open(&b, "buyer@x.com", "order delivered", "del");
    w.turn(&case, &waits("delivered", "delivered; review request on day 3", "signal", "3d", "send the review request"));
    let review_day = w.wait(&case).unwrap().deadline.unwrap();

    w.t += DAY;
    w.arrive(&b, "email", "buyer@x.com", msg("buyer@x.com", "the item arrived damaged"), "complaint");
    assert_eq!(w.tick().children_started, 1);
    w.turn(&case, &closes("owner_takeover", "complaint: damaged item; handed to the owner, no review request"));
    assert_eq!(w.run(&case).state, "done");

    w.t = review_day;
    let r = w.tick();
    assert_eq!((r.superseded, r.children_started, r.unrouted), (1, 0, 0), "the review request never goes out");
    assert!(w.queued_turn(&case).is_none());
    assert!(w.s.engine_children(&case).unwrap().iter().all(|t| t.state == "done"));
}

/// uc39 — Event RSVPs and reminders that de-duplicate across channels.
/// An RSVP that names an email and a phone together makes them one
/// person; a later message on either channel alone reaches that case. A
/// phone never seen with that email is deliberately someone else.
#[test]
fn uc39_rsvps_deduplicate_across_channels_on_the_deterministic_rule() {
    let w = World::new();
    let b = mk("rsvp");
    let both = [("email".to_string(), "guest@x.com".to_string()), ("phone".to_string(), "+15550001111".to_string())];
    let Routed::Opened { case_id: case } = route_signal(&w.s, &b, &both, &json!({"rsvp": "yes"}), "event", "form-1", w.t).unwrap() else { panic!() };
    assert_eq!(w.arrive(&b, "email", "guest@x.com", json!({"email": "guest@x.com", "message": "+1 guest"}), "mail-1"), Routed::Signaled { case_id: case.clone() });
    assert_eq!(w.arrive(&b, "phone", "+15550001111", json!({"phone": "+15550001111", "message": "running late"}), "sms-1"), Routed::Signaled { case_id: case.clone() });
    assert_eq!(w.s.engine_subject_aliases(&subject_of(&w, &case)).unwrap().len(), 2, "one subject, two aliases");

    // The same email plus a NEW phone in one message: observed together, the
    // phone joins the person. A phone alone, never seen with anyone: a stranger.
    let joined = [("email".to_string(), "guest@x.com".to_string()), ("phone".to_string(), "+15550002222".to_string())];
    assert_eq!(route_signal(&w.s, &b, &joined, &json!({"rsvp": "yes"}), "event", "form-2", w.t + 1).unwrap(), Routed::Signaled { case_id: case.clone() });
    assert_eq!(w.s.engine_subject_aliases(&subject_of(&w, &case)).unwrap().len(), 3);
    let Routed::Opened { case_id: stranger } = w.arrive(&b, "phone", "+15550009999", json!({"phone": "+15550009999", "message": "yes"}), "sms-9") else { panic!("an unknown phone is a new person") };
    assert_ne!(stranger, case);
    let r = w.tick();
    assert_eq!((r.steered, r.children_started), (3, 0), "all three ride the guest's one queued turn");
}

fn subject_of(w: &World, case_id: &str) -> String {
    let inputs: serde_json::Value = serde_json::from_str(w.run(case_id).inputs.as_deref().unwrap()).unwrap();
    inputs["_case"]["subject_id"].as_str().unwrap().to_string()
}

/// uc40 — Webinar follow-ups branching on attendance. Two registrants
/// wait for the webinar's end; the attendance signal starts one turn with
/// "attended" in hand; the other's deadline starts the no-show branch on
/// the timer. Each person gets one turn, on their own branch.
#[test]
fn uc40_follow_ups_branch_on_attendance() {
    let mut w = World::new();
    let b = mk("webinar");
    let (att, _) = w.open(&b, "att@x.com", "registered", "r1");
    let (noshow, _) = w.open(&b, "no@x.com", "registered", "r2");
    for case in [&att, &noshow] {
        w.turn(case, &waits("registered", "confirmation sent", "signal", "2d", "attendance report after the webinar"));
    }
    w.t += DAY + HOUR;
    w.arrive(&b, "email", "att@x.com", json!({"email": "att@x.com", "attended": true, "minutes": 52}), "attendance");
    assert_eq!(w.tick().children_started, 1);
    let attended = w.queued_turn(&att).unwrap();
    assert!(attended.inputs.as_deref().unwrap().contains("\"attended\":true"), "the attended branch has the attendance in hand");
    w.turn(&att, &waits("attended", "sent the recording and the offer", "signal", "5d", "their answer"));

    let r = w.advance(DAY - HOUR);
    assert_eq!((r.children_started, r.superseded), (1, 1), "the no-show's deadline fires; the attendee's old one is superseded");
    let missed = w.queued_turn(&noshow).unwrap();
    assert!(missed.inputs.as_deref().unwrap().contains("case.timer"), "the no-show branch runs on the timer");
    assert!(w.queued_turn(&att).is_none(), "one turn each");
}

/// uc41 — Newsletter production as a scheduled binding with approval
/// before send. The weekly schedule fires one task run; the run parks on
/// the owner's approval; the answer resumes it once; the send's receipt is
/// dated after the approval; a second click is a duplicate.
#[tokio::test]
async fn uc41_a_newsletter_parks_for_approval_before_send() {
    let s = World::new().s;
    // August 3rd 2026 is a Monday: the job is created at eight, fires at nine.
    let created = local(2026, 8, 3, 8, 0);
    job(&s, "newsletter", "0 0 9 * * MON", created);
    let monday = local(2026, 8, 3, 9, 0);
    assert_eq!(tick(&s, created + 60, &idle, &no_steer).armed, 1);
    assert_eq!(s.engine_pending_timers("binding").unwrap()[0].due_at, Some(monday));
    assert_eq!(tick(&s, monday, &idle, &no_steer).fired, 1);
    let run = s.engine_queued_runs_of_kind("task", 10).unwrap().remove(0);
    s.engine_set_run_state(&run.id, "running", monday + 1, None).unwrap();

    // Draft ready: the run parks on the owner. Nothing is sent yet.
    let key = format!("approval:{}", run.id);
    s.engine_declare_wait(&run.id, &NewWait { action: "resume", on_kind: "approval", key: &key, parked: Some("{}"), reason: "newsletter draft needs your approval", ..Default::default() }, monday + 60).unwrap();
    assert!(s.engine_effects_for_run(&run.id).unwrap().is_empty());

    let approved_at = monday + HOUR;
    let answer = |idem: &'static str| NewEvent { kind: "approval", target_type: "run", target_id: &key, payload: r#"{"approved":true}"#, channel: "owner", idem_key: idem, durable: true, ..Default::default() };
    assert!(matches!(s.engine_enqueue_event(&answer("ok-1")).unwrap(), Enqueued::Inserted(_)));
    assert_eq!(s.engine_enqueue_event(&answer("ok-1")).unwrap(), Enqueued::Duplicate, "a second click is one answer");
    let r = tick(&s, approved_at, &idle, &no_steer);
    assert_eq!((r.resumed, r.unrouted), (1, 0));
    assert_eq!(s.engine_get_run(&run.id).unwrap().unwrap().state, "queued");

    let ctx = ToolContext { session_key: format!("agent:mk:workflow:{}:send::0", run.id), ..Default::default() };
    let input = json!({"to": "list@x.com", "body": "September newsletter"});
    let sent = guarded_send(&s, &ctx, "messaging", "mail-app", "mail.message.send", &input, || async { SendOutcome::Sent("Sent.".into(), Some("nl-9".into())) }).await;
    assert!(!sent.is_error, "{}", sent.content);
    let receipt = &s.engine_effects_for_run(&run.id).unwrap()[0];
    assert_eq!((receipt.state.as_str(), receipt.provider_ref.as_deref()), ("completed", Some("nl-9")));
    assert!(receipt.completed_at.unwrap() >= approved_at);
    let again = guarded_send(&s, &ctx, "messaging", "mail-app", "mail.message.send", &json!({"to": "list@x.com", "body": "September newsletter (resend)"}), || async { panic!("one send to the list per run") }).await;
    assert!(again.is_error && again.content.contains("already sent"));
}
