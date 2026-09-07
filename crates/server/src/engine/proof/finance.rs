//! Proof scenarios: finance and money movement (uc19–uc27). See `mod.rs`.
//!
//! Money here is the effect ledger: a row written BEFORE the attempt under
//! a key the provider will honour, completed or failed by what the
//! provider said, and never retried when the provider said nothing.

use super::*;
use chrono::{Local, TimeZone};
use serde_json::json;

fn invoices() -> CaseBinding<'static> {
    World::binding("ar", "collect-invoice", "invoice", 7 * DAY)
}

fn msg(email: &str, text: &str) -> serde_json::Value {
    json!({"email": email, "message": text})
}

fn run(w: &World, id: &str, agent: &str) -> EngineRun {
    w.s.engine_create_run(&NewRun { id, kind: "workflow", session_key: &format!("agent:{agent}:workflow:{id}"), agent_id: agent, lane: "main", ..Default::default() }).unwrap();
    w.run(id)
}

fn approval<'a>(target: &'a str, payload: &'a str, idem: &'a str) -> NewEvent<'a> {
    NewEvent { kind: "approval", target_type: "run", target_id: target, payload, channel: "owner", idem_key: idem, durable: true, ..Default::default() }
}

fn local(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> i64 {
    Local.with_ymd_and_hms(y, mo, d, h, mi, s).single().unwrap().timestamp()
}

/// uc19 — Invoice collection that dunns on a schedule and stops on
/// payment. Two silent weeks are two dunning turns started by the clock;
/// the payment starts the closing turn; the dunning that was due next
/// never fires. Must never: a dunning message after payment.
#[test]
fn uc19_dunning_on_a_schedule_stops_on_payment() {
    let mut w = World::new();
    let b = invoices();
    let (case, _) = w.open(&b, "ap@example.com", "invoice 1042 issued, net 7", "inv-1042");
    w.turn(&case, &waits("open", "sent invoice 1042", "signal", "7d", "payment or day 7"));
    assert_eq!(w.advance(7 * DAY).children_started, 1);
    assert!(w.queued_turn(&case).unwrap().inputs.as_deref().unwrap().contains("case.timer"), "dunning 1 is the clock's");
    w.turn(&case, &waits("overdue", "dunning 1 sent", "signal", "7d", "payment or day 14"));
    let dunning2 = w.wait(&case).unwrap().deadline.unwrap();

    w.t += 3 * DAY;
    assert!(matches!(w.arrive(&b, "email", "ap@example.com", msg("ap@example.com", "paid 1042 today"), "pay-1042"), Routed::Signaled { .. }));
    assert_eq!(w.tick().children_started, 1, "the payment starts the closing turn now");
    w.turn(&case, &closes("paid", "payment received; case closed"));
    assert_eq!(w.run(&case).state, "done");

    w.t = dunning2;
    let r = w.tick();
    assert_eq!((r.children_started, r.superseded), (0, 1), "dunning 2 never fires");
    assert!(w.queued_turn(&case).is_none());
    assert!(w.undelivered().is_empty());
    assert_eq!(w.history(&case).iter().filter(|e| e.kind == "turn_result").count(), 3);
}

/// uc20 — Effectively-once charges with a provider idempotency key written
/// before the attempt. The row exists, pending, under the provider's key
/// before anything is attempted; the same key asked again is the same row;
/// once completed it is never charged again. Must never: two charges for
/// one key.
#[test]
fn uc20_a_charge_is_recorded_under_the_providers_key_before_the_attempt() {
    let w = World::new();
    run(&w, "wf-charge", "ar");
    let id = w.s.engine_effect_pending("wf-charge", "financial", "charge:inv-1042", "stripe", "pi_idem_1042", "ap@example.com").unwrap();
    let row = w.s.engine_get_effect(id).unwrap().unwrap();
    assert_eq!((row.state.as_str(), row.attempts, row.provider_key.as_str()), ("pending", 0, "pi_idem_1042"), "on the books before the attempt");
    assert_eq!(row.counterparty.as_deref(), Some("ap@example.com"));
    assert!(row.completed_at.is_none());

    assert_eq!(w.s.engine_effect_pending("wf-charge", "financial", "charge:inv-1042", "stripe", "pi_idem_1042", "ap@example.com").unwrap(), id, "the same key is the same row");
    w.s.engine_effect_attempted(id).unwrap();
    w.s.engine_effect_completed(id, Some("ch_9f"), Some("charged $120.00"), w.t).unwrap();

    // A relaunched turn asks again: same row, already completed, no attempt.
    assert_eq!(w.s.engine_effect_pending("wf-charge", "financial", "charge:inv-1042", "stripe", "pi_idem_1042", "ap@example.com").unwrap(), id);
    let row = w.s.engine_get_effect(id).unwrap().unwrap();
    assert_eq!((row.state.as_str(), row.attempts, row.provider_ref.as_deref()), ("completed", 1, Some("ch_9f")));
    assert_eq!(w.receipts("wf-charge").len(), 1, "one charge on the ledger");
    assert!(w.s.engine_pending_effects().unwrap().is_empty(), "nothing left to reconcile");
}

/// uc21 — Authorize now, capture later, as two linked effects. The capture
/// row names the authorization it draws on (the provider's reference), so
/// the ledger shows the pair as one story; each is effectively-once on its
/// own key. Must never: a capture with no authorization behind it.
#[test]
fn uc21_authorize_now_capture_later_as_two_linked_effects() {
    let mut w = World::new();
    run(&w, "wf-order-7", "ar");
    let auth = w.s.engine_effect_pending("wf-order-7", "financial", "charge:ord-7:authorize", "stripe", "pi_ord_7", "buyer@x.com").unwrap();
    w.s.engine_effect_attempted(auth).unwrap();
    w.s.engine_effect_completed(auth, Some("auth_7"), Some("authorized $80.00"), w.t).unwrap();
    let auth_ref = w.s.engine_get_effect(auth).unwrap().unwrap().provider_ref.unwrap();

    // Days later the goods ship: the capture is keyed to the authorization.
    w.t += 3 * DAY;
    let capture = w.s.engine_effect_pending("wf-order-7", "financial", &format!("charge:ord-7:capture:{auth_ref}"), "stripe", &auth_ref, "buyer@x.com").unwrap();
    assert_ne!(capture, auth);
    w.s.engine_effect_attempted(capture).unwrap();
    w.s.engine_effect_completed(capture, Some("ch_7"), Some("captured $80.00"), w.t).unwrap();

    let rows = w.receipts("wf-order-7");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].provider_ref.as_deref(), Some("auth_7"));
    assert_eq!(rows[1].provider_key, "auth_7", "the capture names the authorization");
    assert!(rows[1].idem_key.ends_with("auth_7"));
    assert!(rows[1].completed_at.unwrap() > rows[0].completed_at.unwrap());
    assert_eq!(w.s.engine_effect_pending("wf-order-7", "financial", &format!("charge:ord-7:capture:{auth_ref}"), "stripe", &auth_ref, "buyer@x.com").unwrap(), capture, "a second capture is the same capture");
}

/// uc22 — Refunds that require an approval above a threshold. The turn
/// parks on the owner; no refund is on the ledger until the answer; the
/// answer resumes the turn once and the refund follows it. Must never: a
/// refund before the approval, or two refunds for one answer.
#[test]
fn uc22_a_refund_above_the_threshold_parks_for_approval() {
    let mut w = World::new();
    let b = World::binding("cs", "refunds", "refund", 3 * DAY);
    let (case, queued) = w.open(&b, "buyer@x.com", "refund my $450 order", "r1");
    let turn = w.start(&queued.id);
    let key = format!("approval:{}", turn.id);
    w.s.engine_declare_wait(&turn.id, &NewWait { action: "resume", on_kind: "approval", key: &key, parked: Some("{}"), reason: "$450 refund is above the $200 threshold", ..Default::default() }, w.t).unwrap();
    assert!(w.receipts(&turn.id).is_empty(), "nothing on the ledger while parked");
    assert_eq!(w.run(&turn.id).state, "waiting");

    w.t += 2 * HOUR;
    let approved_at = w.t;
    assert!(matches!(w.s.engine_enqueue_event(&approval(&key, r#"{"approved":true}"#, "ok-1")).unwrap(), Enqueued::Inserted(_)));
    assert_eq!(w.s.engine_enqueue_event(&approval(&key, r#"{"approved":true}"#, "ok-1")).unwrap(), Enqueued::Duplicate, "one answer per wait");
    assert_eq!(w.tick().resumed, 1);
    assert_eq!(w.run(&turn.id).state, "queued", "resumed at the parked call");
    assert!(w.run(&turn.id).woken_by().is_some());

    w.t += 60;
    let refund = w.s.engine_effect_pending(&turn.id, "financial", "refund:ord-450", "stripe", "re_idem_450", "buyer@x.com").unwrap();
    w.s.engine_effect_attempted(refund).unwrap();
    w.s.engine_effect_completed(refund, Some("re_1"), Some("refunded $450.00"), w.t).unwrap();
    let rows = w.receipts(&turn.id);
    assert_eq!(rows.len(), 1);
    assert!(rows[0].completed_at.unwrap() > approved_at, "the refund is dated after the approval");
    w.s.engine_set_run_state(&turn.id, "running", w.t, None).unwrap();
    w.finish(&w.run(&turn.id), &closes("refunded", "refund approved and issued"));
    assert!(w.history_has(&case, "turn_result", "1 send(s) on the ledger this turn: stripe#"));
}

/// uc23 — Vendor bill approval chains with a parked wait per approver.
/// Each approver is one wait generation with its own key; an answer for a
/// wait that is not current wakes nothing; the chain runs in order and
/// each answer resumes exactly once. Must never: a later approver's answer
/// counted as an earlier one's.
#[test]
fn uc23_an_approval_chain_is_one_parked_wait_per_approver() {
    let mut w = World::new();
    run(&w, "wf-bill-88", "ap");
    let manager = "approval:wf-bill-88:manager";
    let cfo = "approval:wf-bill-88:cfo";
    w.s.engine_declare_wait("wf-bill-88", &NewWait { action: "resume", on_kind: "approval", key: manager, parked: Some("{}"), reason: "bill 88: manager", ..Default::default() }, w.t).unwrap();

    // The CFO answers early: no wait of hers is current, so nothing wakes.
    w.s.engine_enqueue_event(&approval(cfo, r#"{"approved":true}"#, "cfo-early")).unwrap();
    let r = w.tick();
    assert_eq!((r.resumed, r.unrouted), (0, 1), "an answer for a wait that is not current wakes nothing");
    assert_eq!(w.run("wf-bill-88").state, "waiting", "still on the manager");

    w.s.engine_enqueue_event(&approval(manager, r#"{"approved":true}"#, "mgr-1")).unwrap();
    assert_eq!(w.tick().resumed, 1);
    assert_eq!(w.run("wf-bill-88").state, "queued");

    // The run parks again, on the CFO; her (fresh) answer resumes it once.
    w.t += HOUR;
    w.s.engine_declare_wait("wf-bill-88", &NewWait { action: "resume", on_kind: "approval", key: cfo, parked: Some("{}"), reason: "bill 88: CFO", ..Default::default() }, w.t).unwrap();
    assert_eq!(w.run("wf-bill-88").state, "waiting");
    w.s.engine_enqueue_event(&approval(cfo, r#"{"approved":true}"#, "cfo-1")).unwrap();
    assert_eq!(w.s.engine_enqueue_event(&approval(cfo, r#"{"approved":false}"#, "cfo-1")).unwrap(), Enqueued::Duplicate);
    assert_eq!(w.tick().resumed, 1);
    assert_eq!(w.run("wf-bill-88").state, "queued");
    let waits = w.s.engine_waits_for_run("wf-bill-88").unwrap();
    assert_eq!(waits.iter().map(|x| x.key.as_str()).collect::<Vec<_>>(), vec![manager, cfo], "in order");
    assert!(waits.iter().all(|x| x.superseded_at.is_some()), "each wait answered once and closed");
}

/// uc24 — Payouts that reconcile rather than retry when the provider goes
/// silent. Attempted, never confirmed: the row stays pending, attempts stay
/// at one, the owner is told once with the provider key, and the engine
/// never retries it. When a person reconciles it, it leaves the worklist.
/// Must never: a second attempt at money the provider may have moved.
#[test]
fn uc24_a_silent_provider_is_reconciled_not_retried() {
    let mut w = World::new();
    run(&w, "wf-payout-3", "ap");
    let payout = w.s.engine_effect_pending("wf-payout-3", "financial", "payout:vendor-3:2026-09", "stripe", "po_idem_3", "vendor-3").unwrap();
    w.s.engine_effect_attempted(payout).unwrap();
    // The provider never answered.

    let r = w.advance(60);
    assert_eq!(r.pending_effects, 1);
    let card = w.card(&format!("effect:{payout}")).expect("the owner is told");
    assert!(card.contains("po_idem_3") || card.contains("payout:vendor-3"), "with the key to reconcile by: {card}");
    for _ in 0..3 {
        w.advance(HOUR);
    }
    let row = w.s.engine_get_effect(payout).unwrap().unwrap();
    assert_eq!((row.state.as_str(), row.attempts), ("pending", 1), "never retried");
    assert_eq!(w.s.engine_events_for("run", "wf-payout-3", 50).unwrap().iter().filter(|e| e.kind == "needs_attention").count(), 1, "told once");

    // The owner finds it on the provider's side and reconciles.
    w.s.engine_effect_completed(payout, Some("po_3"), Some("confirmed with the provider by the owner"), w.t).unwrap();
    assert_eq!(w.advance(60).pending_effects, 0);
}

/// uc25 — Expense reimbursements that wait for a receipt. The case waits
/// on the receipt with a deadline; nothing is paid before it arrives; the
/// receipt starts the paying turn; the deadline nudge never fires after.
/// Must never: a reimbursement with no receipt on the case.
#[test]
fn uc25_a_reimbursement_waits_for_the_receipt() {
    let mut w = World::new();
    let b = World::binding("ap", "expenses", "expense", 14 * DAY);
    let (case, _) = w.open(&b, "jo@example.com", "expense claim $62 client lunch", "e1");
    w.turn(&case, &waits("awaiting_receipt", "asked Jo for the receipt", "signal", "14d", "the receipt"));
    let nudge = w.wait(&case).unwrap().deadline.unwrap();
    assert!(w.s.engine_children(&case).unwrap().iter().all(|t| w.receipts(&t.id).is_empty()), "nothing paid without a receipt");

    w.t += 2 * DAY;
    w.arrive(&b, "email", "jo@example.com", msg("jo@example.com", "receipt attached: lunch-0904.pdf"), "e2");
    assert_eq!(w.tick().children_started, 1);
    let paying = w.start(&w.queued_turn(&case).unwrap().id);
    assert!(paying.inputs.as_deref().unwrap().contains("lunch-0904.pdf"));
    let pay = w.s.engine_effect_pending(&paying.id, "financial", "reimburse:jo:lunch-0904", "payroll", "rb_idem_0904", "jo@example.com").unwrap();
    w.s.engine_effect_attempted(pay).unwrap();
    w.s.engine_effect_completed(pay, Some("rb_1"), Some("reimbursed $62.00"), w.t).unwrap();
    w.finish(&w.run(&paying.id), &closes("reimbursed", "receipt received; $62 reimbursed"));
    assert!(w.history_has(&case, "turn_result", "1 send(s) on the ledger this turn: payroll#"));

    w.t = nudge;
    let r = w.tick();
    assert_eq!((r.children_started, r.superseded), (0, 1), "the nudge never fires");
}

/// uc26 — Subscription retries with a bounded catch-up window, never a
/// storm after downtime. An hourly retry job that slept six hours fires at
/// most once late, inside the catch-up window, then resumes its cadence.
/// Must never: six fires for six missed hours.
#[test]
fn uc26_subscription_retries_never_storm_after_downtime() {
    let w = World::new();
    let created = local(2026, 9, 1, 8, 0, 0);
    let job = w.s.create_cron_job("retry-subscriptions", "0 0 * * * *", "echo retry", "shell", None, None, None, true, None, None).unwrap();
    let target = crate::engine::cron_target(&job);
    let Enqueued::Inserted(floor) = w.s.engine_enqueue_event(&NewEvent { kind: "timer", target_type: "binding", target_id: &target, idem_key: &format!("{target}:floor"), due_at: Some(created), ..Default::default() }).unwrap() else { panic!() };
    w.s.engine_complete_event(floor, created).unwrap();

    // 9:00 fires on time; the process then dies until 15:10.
    let r = tick(&w.s, local(2026, 9, 1, 9, 0, 5), &idle, &no_steer);
    assert_eq!((r.armed, r.fired), (1, 1));
    let first = w.s.engine_queued_runs_of_kind("task", 10).unwrap().remove(0);
    w.s.engine_set_run_state(&first.id, "done", local(2026, 9, 1, 9, 0, 30), None).unwrap();
    tick(&w.s, local(2026, 9, 1, 9, 1, 0), &idle, &no_steer);
    assert_eq!(w.s.engine_pending_timers("binding").unwrap()[0].due_at, Some(local(2026, 9, 1, 10, 0, 0)));

    // 15:10: the 10:00 timer is far past the window — skipped; 15:00 is
    // inside it and fires once, late; 11:00–14:00 are never replayed.
    let r = tick(&w.s, local(2026, 9, 1, 15, 10, 0), &idle, &no_steer);
    assert_eq!((r.skipped, r.fired), (1, 0));
    let r = tick(&w.s, local(2026, 9, 1, 15, 10, 5), &idle, &no_steer);
    assert_eq!((r.armed, r.fired, r.skipped), (1, 1, 0), "one late fire");
    let r = tick(&w.s, local(2026, 9, 1, 15, 10, 10), &idle, &no_steer);
    assert_eq!(r.fired, 0);
    assert_eq!(w.s.engine_pending_timers("binding").unwrap()[0].due_at, Some(local(2026, 9, 1, 16, 0, 0)), "back on cadence");
    assert_eq!(w.s.engine_count_runs_for_ref(&target).unwrap(), 2, "9:00 and 15:00 ran; nothing else");
    assert!(CATCH_UP_SECS <= HOUR, "the window is bounded");
}

/// uc27 — Bookkeeping close checklists where each step is a durable run
/// with an audit trail. Steps are child runs of the close under real
/// parent links; each leaves a durable line; a step interrupted by a crash
/// resumes once and a second crash fails it for a person. Must never: a
/// step that silently vanishes on restart.
#[test]
fn uc27_a_close_checklist_is_durable_runs_with_an_audit_trail() {
    let w = World::new();
    let close = "close-2026-08";
    run(&w, close, "books");
    for (i, step) in ["reconcile-bank", "post-accruals", "lock-period"].iter().enumerate() {
        let id = format!("{close}:{step}");
        w.s.engine_create_run(&NewRun { id: &id, kind: "task", session_key: &format!("agent:books:task:{id}"), agent_id: "books", lane: "main", parent_run_id: Some(close), ..Default::default() }).unwrap();
        let _ = w.s.engine_enqueue_event(&NewEvent { kind: "step", target_type: "run", target_id: close, payload: &format!("step {}: {step} queued", i + 1), r#ref: &id, idem_key: &format!("{id}:queued"), durable: true, ..Default::default() });
    }
    let children = w.s.engine_children(close).unwrap();
    assert_eq!(children.len(), 3);
    assert!(children.iter().all(|c| c.parent_run_id.as_deref() == Some(close)), "real parent links");

    // Step one runs and finishes; step two is running when the process dies.
    w.s.engine_set_run_state(&children[0].id, "done", w.t + 10, None).unwrap();
    w.s.engine_set_run_state(&children[1].id, "running", w.t + 20, None).unwrap();
    assert_eq!(recover(&w.s), 1, "the interrupted step resumes once");
    assert_eq!(w.s.run_state(&children[1].id), ("queued", 1));
    w.s.engine_set_run_state(&children[1].id, "running", w.t + 40, None).unwrap();
    recover(&w.s);
    assert_eq!(w.s.run_state(&children[1].id), ("failed", 1), "a second crash fails it for a person");
    assert_eq!(w.s.run_state(&children[0].id).0, "done", "finished steps are untouched");
    assert_eq!(w.s.run_state(&children[2].id).0, "queued");

    let history = w.history(close);
    let steps: Vec<&EngineEvent> = history.iter().filter(|e| e.kind == "step").collect();
    assert_eq!(steps.len(), 3, "every step is on the record");
    assert!(steps[0].payload.contains("reconcile-bank") && steps[2].payload.contains("lock-period"));
    assert_eq!(history.iter().filter(|e| e.kind == "needs_attention").count(), 1, "and the failed step reached the owner, on the same record");
    assert!(w.card(&children[1].id).is_some());
}

trait RunState {
    fn run_state(&self, id: &str) -> (&'static str, i64);
}

impl RunState for Store {
    fn run_state(&self, id: &str) -> (&'static str, i64) {
        let r = self.engine_get_run(id).unwrap().unwrap();
        let state: &'static str = Box::leak(r.state.into_boxed_str());
        (state, r.resume_attempted)
    }
}
