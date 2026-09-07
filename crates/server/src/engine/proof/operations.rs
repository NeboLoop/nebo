//! Proof scenarios: operations and scheduling (uc28–uc35). See `mod.rs`.

use super::*;
use chrono::{Local, TimeZone};
use serde_json::json;

pub(super) fn local(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
    Local.with_ymd_and_hms(y, mo, d, h, mi, 0).single().unwrap().timestamp()
}

/// A scheduled job whose floor is `floor`: one consumed timer at that
/// moment, the way a job that has fired before carries its floor.
pub(super) fn job(s: &Store, name: &str, schedule: &str, floor: i64) -> db::models::CronJob {
    let j = s.create_cron_job(name, schedule, "echo hi", "shell", None, None, None, true, None, None).unwrap();
    let target = cron_target(&j);
    let Enqueued::Inserted(id) = s
        .engine_enqueue_event(&NewEvent { kind: "timer", target_type: "binding", target_id: &target, idem_key: &format!("{target}:floor"), due_at: Some(floor), ..Default::default() })
        .unwrap()
    else {
        panic!("floor timer")
    };
    s.engine_complete_event(id, floor).unwrap();
    j
}

fn ops(case_type: &str) -> CaseBinding<'static> {
    World::binding("ops", "run-ops", case_type, 7 * DAY)
}

/// uc28 — Recurring reports that fire once when the laptop wakes, not
/// fifty times. A daily 09:00 report sleeps through thirty days: on waking
/// twenty minutes after nine it fires once; on waking mid-afternoon it
/// skips today and arms tomorrow. Never a replay of the missed month.
#[test]
fn uc28_a_recurring_report_fires_once_on_waking() {
    let s = World::new().s;
    let j = job(&s, "daily-report", "0 0 9 * * *", local(2026, 7, 1, 8, 0));
    let target = cron_target(&j);

    // Thirty days asleep; awake at 09:20. One late fire, inside the window.
    let wake = local(2026, 7, 31, 9, 20);
    let r = tick(&s, wake, &idle, &no_steer);
    assert_eq!((r.armed, r.fired, r.skipped), (1, 1, 0));
    assert_eq!(s.engine_count_runs_for_ref(&target).unwrap(), 1, "one fire, not thirty");
    let run = s.engine_queued_runs_of_kind("task", 10).unwrap().remove(0);
    s.engine_set_run_state(&run.id, "done", wake + 60, None).unwrap();
    assert_eq!(tick(&s, wake + 65, &idle, &no_steer).armed, 1);
    assert_eq!(s.engine_pending_timers("binding").unwrap()[0].due_at, Some(local(2026, 8, 1, 9, 0)));

    // Asleep again; awake three days later at 15:00 — five hours past
    // nine, beyond the catch-up window: skipped, tomorrow armed, nothing ran.
    let r = tick(&s, local(2026, 8, 3, 15, 0), &idle, &no_steer);
    assert_eq!((r.fired, r.skipped), (0, 1));
    let r = tick(&s, local(2026, 8, 3, 15, 0) + 5, &idle, &no_steer);
    assert_eq!((r.armed, r.fired), (1, 0));
    assert_eq!(s.engine_pending_timers("binding").unwrap()[0].due_at, Some(local(2026, 8, 4, 9, 0)));
    assert_eq!(s.engine_count_runs_for_ref(&target).unwrap(), 1, "still one");
    assert!(CATCH_UP_SECS < 5 * HOUR);
}

/// uc29 — Heartbeat monitors that respect a working window and never
/// stack. A thirty-minute heartbeat armed at 22:00 under an 08:00–18:00
/// window is placed at 08:00 tomorrow, not at 22:30; when it fires while
/// the previous beat is still running, it is skipped, not stacked.
#[test]
fn uc29_heartbeats_keep_the_window_and_never_stack() {
    let s = World::new().s;
    let target = "heartbeat:agent:monitor".to_string();
    let window = ("08:00".to_string(), "18:00".to_string());
    let armed_at = local(2026, 8, 20, 22, 0);
    let wanted = [Wanted {
        target: target.clone(),
        schedule: "1800".into(),
        due: Box::new(move |consumed| Some(crate::heartbeat::next_in_window((consumed.unwrap_or(armed_at) + 1800).max(armed_at), Some(&window)))),
    }];
    assert_eq!(reconcile_timers(&s, armed_at, "entity", "heartbeat:", &wanted), 1);
    let pending = s.engine_pending_timers("entity").unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].due_at, Some(local(2026, 8, 21, 8, 0)), "placed at the window's opening, not 22:30");
    assert_eq!(reconcile_timers(&s, armed_at + 5, "entity", "heartbeat:", &wanted), 0, "held, not re-armed every tick");

    // 08:00: one beat. Re-armed from the consumed one: 08:30. While the
    // first still runs, 08:30 is skipped — never a second beat beside it.
    let open = local(2026, 8, 21, 8, 0);
    let r = tick(&s, open, &idle, &no_steer);
    assert_eq!((r.fired, r.skipped), (1, 0));
    let beats = s.engine_queued_runs_of_kind("heartbeat", 10).unwrap();
    assert_eq!(beats.len(), 1);
    assert_eq!(beats[0].lane, "heartbeat");
    assert_eq!(s.engine_last_timer_floor("entity", &target).unwrap(), Some(open));
    assert_eq!(reconcile_timers(&s, open + 5, "entity", "heartbeat:", &wanted), 1);
    assert_eq!(s.engine_pending_timers("entity").unwrap()[0].due_at, Some(open + 1800));
    s.engine_set_run_state(&beats[0].id, "running", open + 1, None).unwrap();
    let r = tick(&s, open + 1800, &idle, &no_steer);
    assert_eq!((r.fired, r.skipped), (0, 1), "the live beat blocks the next");
    assert_eq!(s.engine_queued_runs_of_kind("heartbeat", 10).unwrap().len(), 0);
}

/// uc30 — Maintenance schedules keyed to equipment, one open case per
/// machine. Two readings for one press are one case; a second press is a
/// second case; the key is the machine, not the message.
#[test]
fn uc30_one_open_case_per_machine() {
    let w = World::new();
    let b = ops("maintenance");
    let Routed::Opened { case_id: press7 } = w.arrive(&b, "asset", "press-7", json!({"asset": "press-7", "reading": "vibration high"}), "r1") else { panic!() };
    assert_eq!(w.arrive(&b, "asset", "press-7", json!({"asset": "press-7", "reading": "vibration higher"}), "r2"), Routed::Signaled { case_id: press7.clone() });
    let Routed::Opened { case_id: press8 } = w.arrive(&b, "asset", "press-8", json!({"asset": "press-8", "reading": "oil low"}), "r3") else { panic!() };
    assert_ne!(press7, press8);
    assert_eq!(open_case_for(&w.s, "maintenance", "asset", "press-7").unwrap().id, press7);
    assert_eq!(open_case_for(&w.s, "maintenance", "asset", "press-8").unwrap().id, press8);
    let r = w.tick();
    assert_eq!((r.steered, r.children_started), (1, 0), "the second reading rides press 7's queued turn");
    assert_eq!(w.s.engine_queued_runs_of_kind("workflow", 10).unwrap().len(), 2, "one turn per machine");
    w.turn(&press7, &closes("serviced", "bearing replaced"));
    assert!(open_case_for(&w.s, "maintenance", "asset", "press-7").is_none(), "press 7 released; press 8 untouched");
    assert_eq!(w.run(&press8).state, "waiting");
}

/// uc31 — Inventory reorder cases that wait for the supplier's
/// confirmation. The case waits on the supplier's message with a chase
/// deadline; the confirmation starts the turn and the chase never fires.
#[test]
fn uc31_a_reorder_waits_for_the_supplier() {
    let mut w = World::new();
    let b = ops("reorder");
    let Routed::Opened { case_id: case } = w.arrive(&b, "sku", "widget-9", json!({"sku": "widget-9", "on_hand": 3}), "low") else { panic!() };
    w.turn(&case, &waits("ordered", "PO sent to the supplier", "signal", "5d", "the supplier's confirmation, chase on day 5"));
    let chase = w.wait(&case).unwrap().deadline.unwrap();

    w.t += 2 * DAY;
    assert_eq!(w.arrive(&b, "sku", "widget-9", json!({"sku": "widget-9", "supplier": "confirmed, ships Friday"}), "conf"), Routed::Signaled { case_id: case.clone() });
    assert_eq!(w.tick().children_started, 1, "the confirmation is the next turn");
    assert!(w.queued_turn(&case).unwrap().inputs.unwrap().contains("ships Friday"));
    w.turn(&case, &waits("confirmed", "supplier confirmed; awaiting receipt", "signal", "7d", "receipt"));

    w.t = chase;
    let r = w.tick();
    assert_eq!((r.superseded, r.children_started), (1, 0), "the chase never fires");
}

/// uc32 — Field-service dispatch that follows a job from booking to
/// sign-off. Booking, en route, and sign-off are three signals into one
/// case, three turns in order; while a turn runs nothing starts beside it.
#[test]
fn uc32_dispatch_from_booking_to_sign_off() {
    let mut w = World::new();
    let b = ops("job");
    let Routed::Opened { case_id: case } = w.arrive(&b, "crm", "job-1042", json!({"crm_id": "job-1042", "status": "booked"}), "s1") else { panic!() };
    w.turn(&case, &waits("booked", "tech assigned", "signal", "3d", "en route"));
    w.t += DAY;
    w.arrive(&b, "crm", "job-1042", json!({"crm_id": "job-1042", "status": "en_route"}), "s2");
    w.tick();
    let running = w.start(&w.queued_turn(&case).unwrap().id);
    w.arrive(&b, "crm", "job-1042", json!({"crm_id": "job-1042", "status": "signed_off"}), "s3");
    assert_eq!(w.tick().children_started, 0, "sign-off waits behind the running turn");
    w.finish(&running, &waits("en_route", "customer told the ETA", "signal", "1d", "sign-off"));
    let r = w.advance(db::EVENT_LEASE_SECS + 1);
    assert_eq!(r.children_started, 1, "the sign-off starts the next turn once the lease expires");
    w.turn(&case, &closes("signed_off", "job complete, signature on file"));
    let history = w.history(&case);
    let results: Vec<_> = history.iter().filter(|e| e.kind == "turn_result").map(|e| e.payload.clone()).collect();
    assert_eq!(results.len(), 3);
    assert!(results[0].contains("tech assigned") && results[1].contains("ETA") && results[2].contains("signature"));
    assert_eq!(w.s.engine_children(&case).unwrap().len(), 3);
    assert!(w.undelivered().is_empty());
}

/// uc33 — Compliance renewals such as licenses and certifications with
/// long horizons. A renewal wait two hundred days out is one row with
/// that deadline; ticks at day 100 and 199 wake nothing; the day itself
/// wakes it exactly once.
#[test]
fn uc33_a_renewal_wakes_on_the_day_and_not_before() {
    let mut w = World::new();
    let b = ops("renewal");
    let Routed::Opened { case_id: case } = w.arrive(&b, "crm", "lic-CA-88", json!({"crm_id": "lic-CA-88", "expires": "in 200 days"}), "r1") else { panic!() };
    let due = w.t + 200 * DAY;
    let rfc = chrono::DateTime::from_timestamp(due, 0).unwrap().to_rfc3339();
    w.turn(&case, &waits("tracked", "renewal date recorded", "signal", &rfc, "sixty days before expiry, start the renewal"));
    assert_eq!(w.wait(&case).unwrap().deadline, Some(due));

    for day in [1, 100, 199] {
        w.t = due - (200 - day) * DAY;
        assert_eq!(w.tick().children_started, 0, "day {day}: nothing");
    }
    w.t = due;
    assert_eq!(w.tick().children_started, 1, "the day itself");
    assert_eq!(w.advance(1).children_started, 0, "once");
    assert!(w.queued_turn(&case).unwrap().inputs.unwrap().contains("case.timer"));
}

/// uc34 — Contract milestones that wake a case months later. Milestone
/// one at ninety days wakes the case once; its turn declares milestone two
/// at a hundred and eighty; a restart between them changes nothing.
#[test]
fn uc34_milestones_wake_a_case_months_later_across_a_restart() {
    let mut w = World::new();
    let b = ops("contract");
    let Routed::Opened { case_id: case } = w.arrive(&b, "crm", "contract-7", json!({"crm_id": "contract-7"}), "c1") else { panic!() };
    let m1 = w.t + 90 * DAY;
    let m2 = w.t + 180 * DAY;
    let rfc = |t: i64| chrono::DateTime::from_timestamp(t, 0).unwrap().to_rfc3339();
    w.turn(&case, &waits("active", "milestone schedule recorded", "signal", &rfc(m1), "milestone 1: first delivery review"));

    // The machine restarts in month two: the case and its timer persist.
    w.t += 45 * DAY;
    suspend_for_shutdown(&w.s);
    recover(&w.s);
    assert_eq!(w.run(&case).state, "waiting");
    assert_eq!(w.s.engine_pending_timers("wait").unwrap().len(), 1);
    assert_eq!(w.tick().children_started, 0);

    w.t = m1;
    assert_eq!(w.tick().children_started, 1, "milestone one wakes the case");
    w.turn(&case, &waits("active", "milestone 1 reviewed", "signal", &rfc(m2), "milestone 2: final delivery"));
    assert_eq!(w.wait(&case).unwrap().deadline, Some(m2));
    w.t = m2 - DAY;
    assert_eq!(w.tick().children_started, 0);
    w.t = m2;
    assert_eq!(w.tick().children_started, 1, "milestone two, once");
    assert_eq!(w.advance(DAY).children_started, 0);
}

/// uc35 — Fleet or asset checks on interval with catch-up rules. A
/// six-hourly check sleeps three days: waking twenty minutes after an
/// occurrence runs one late check; the eleven missed ones are never
/// replayed; the next check is the next six-hour mark.
#[test]
fn uc35_interval_checks_catch_up_once_never_storm() {
    let s = World::new().s;
    let j = job(&s, "fleet-check", "0 0 */6 * * *", local(2026, 8, 1, 6, 0));
    let target = cron_target(&j);
    let r = tick(&s, local(2026, 8, 1, 6, 1), &idle, &no_steer);
    assert_eq!((r.armed, r.fired), (1, 0));
    assert_eq!(s.engine_pending_timers("binding").unwrap()[0].due_at, Some(local(2026, 8, 1, 12, 0)));

    // Down for three days; awake at 18:20 — twenty minutes after 18:00.
    let wake = local(2026, 8, 4, 18, 20);
    let r = tick(&s, wake, &idle, &no_steer);
    assert_eq!((r.skipped, r.fired), (1, 0), "the stale 12:00 of day one is skipped");
    let r = tick(&s, wake + 5, &idle, &no_steer);
    assert_eq!((r.armed, r.fired, r.skipped), (1, 1, 0), "18:00 today catches up once");
    assert_eq!(s.engine_count_runs_for_ref(&target).unwrap(), 1, "eleven missed checks are not replayed");
    let run = s.engine_queued_runs_of_kind("task", 10).unwrap().remove(0);
    s.engine_set_run_state(&run.id, "done", wake + 30, None).unwrap();
    let r = tick(&s, wake + 35, &idle, &no_steer);
    assert_eq!((r.armed, r.fired), (1, 0));
    assert_eq!(s.engine_pending_timers("binding").unwrap()[0].due_at, Some(local(2026, 8, 5, 0, 0)), "the next six-hour mark");
}
