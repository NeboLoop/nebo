//! The ONE loop for durable work (design of record: "One Engine for Durable
//! Work", 2026-09-06). Every tick, in this order — earlier steps produce
//! what later steps consume:
//!
//! 1. claim deliverable events under a lease (timers that came due, signals,
//!    approvals);
//! 2. match each to the live wait it wakes: `resume` re-queues that run with
//!    its parked messages, `trigger_child` leaves the parent waiting and
//!    queues a child run carrying the event; an event for a superseded wait
//!    generation is dropped, never delivered; a signal for a case whose turn
//!    is live is steered into that turn (one live turn per case);
//! 3. start queued case turns through the workflow runner every existing
//!    workflow uses, and reconcile the running ones against their outcome —
//!    a finished turn's declared wait becomes the parent's next wait;
//! 4. reap: pending effects are surfaced, transient events past the TTL go.
//!
//! Schedules (cron jobs) are recurring timers: every enabled job holds ONE
//! pending timer aimed at binding `cron:<id>`; a due timer becomes a run of
//! kind `task` that `drive` executes; the next occurrence is armed from the
//! consumed one. A job's fire never overlaps its previous fire (skipped and
//! noted), and a fire missed by more than the catch-up window is skipped,
//! never replayed as a storm.
//!
//! Boot: runs the dead process left `running` are stamped `interrupted` and
//! given their ONE resume (I-3). Cases enter through
//! `workflow::cases::signal_or_open`, called by the webhook path and the
//! event dispatcher for bindings that declare `case`; every other binding
//! is untouched.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use chrono::{Local, TimeZone};
use tracing::{info, warn};

use crate::state::AppState;
use db::models::CronJob;
use db::{EngineEvent, EngineRun, NewEvent, Store};
use tools::workflows::WorkflowManager;
use workflow::cases::{needs_attention, settle_turn, start_child};

const TICK: Duration = Duration::from_secs(5);
const CLAIM_BATCH: i64 = 50;
/// Delivered transient events live this long, matching the task TTL today.
const TRANSIENT_TTL_SECS: i64 = 7 * 24 * 3600;
/// Turns started per tick, so one flood cannot starve everything else.
const TURNS_PER_TICK: i64 = 5;
/// A scheduled fire this late still runs (one missed occurrence, at most);
/// later than this it is skipped and noted. Design: catch-up window.
pub const CATCH_UP_SECS: i64 = 3600;

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// What one tick did — returned so tests and logs can say it in numbers.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct TickReport {
    pub claimed: usize,
    pub poisoned: usize,
    pub resumed: usize,
    pub children_started: usize,
    pub steered: usize,
    pub superseded: usize,
    pub unrouted: usize,
    pub pending_effects: usize,
    pub expired: usize,
    pub armed: usize,
    pub fired: usize,
    pub skipped: usize,
}

/// Boot sweep (I-3): nothing is left `running` by a process that is gone.
pub fn recover(store: &Store) -> usize {
    let t = now();
    let interrupted = match store.engine_mark_interrupted() {
        Ok(rows) => rows,
        Err(e) => {
            warn!(error = %e, "engine: boot sweep failed");
            return 0;
        }
    };
    let mut resumed = 0;
    for run in interrupted {
        match store.engine_resume_once(&run.id, t) {
            Ok(true) => resumed += 1,
            Ok(false) => {
                warn!(run = %run.id, "engine: run interrupted twice — failed, not retried");
                let case = run.parent_run_id.as_deref().and_then(|p| store.engine_get_run(p).ok().flatten());
                let reason = format!("{} run {} was interrupted by a restart twice and was not retried", run.kind, run.id);
                if let Err(e) = needs_attention(store, &run.agent_id, &run.id, &run.id, case.as_ref(), &reason, t) {
                    warn!(run = %run.id, error = %e, "engine: could not route the failed run for attention");
                }
            }
            Err(e) => warn!(run = %run.id, error = %e, "engine: resume failed"),
        }
    }
    if resumed > 0 {
        info!(resumed, "engine: resumed runs interrupted by restart");
    }
    resumed
}

/// One pass over the tables. Pure over the store so it can be tested without
/// an AppState. `busy` answers whether a session has a live turn; a matched
/// signal for a case whose turn is live is steered through `steer` instead
/// of starting another turn.
pub fn tick(store: &Store, t: i64, busy: &dyn Fn(&str) -> bool, steer: &dyn Fn(&str, &EngineEvent)) -> TickReport {
    let mut report = TickReport::default();
    report.armed = arm_schedules(store, t);
    let (events, poisoned) = match store.engine_claim_events(t, CLAIM_BATCH) {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, "engine: claim failed");
            return report;
        }
    };
    report.claimed = events.len();
    report.poisoned = poisoned.len();
    for p in &poisoned {
        warn!(event = p.id, kind = %p.kind, target = %p.target_id, "engine: event poisoned after repeated delivery failure");
        let (agent_id, run_id, case) = owner_of_event(store, p);
        let reason = format!("could not deliver {} to {} after {} attempts", p.kind, p.target_id, p.attempts);
        let subject = format!("event:{}", p.id);
        if let Err(e) = needs_attention(store, &agent_id, run_id.as_deref().unwrap_or(&subject), &subject, case.as_ref(), &reason, t) {
            warn!(event = p.id, error = %e, "engine: could not route the poisoned event for attention");
        }
    }
    for event in &events {
        deliver(store, event, t, busy, steer, &mut report);
    }

    match store.engine_pending_effects() {
        Ok(pending) => {
            report.pending_effects = pending.len();
            for e in &pending {
                info!(effect = e.id, run = %e.run_id, class = %e.class, attempts = e.attempts, "engine: effect pending reconciliation");
                // Money that was attempted and never confirmed is never
                // retried by the engine: whoever owns the run is told, once,
                // and reconciles by the provider's key.
                if e.class == "financial" && e.attempts > 0 {
                    let run = store.engine_get_run(&e.run_id).ok().flatten();
                    let agent_id = run.as_ref().map(|r| r.agent_id.clone()).unwrap_or_default();
                    let case = run.as_ref().and_then(|r| r.parent_run_id.as_deref()).and_then(|p| store.engine_get_run(p).ok().flatten());
                    let reason = format!("a {} charge could not be confirmed after {} attempt(s); it was not retried — confirm it with the provider under key {}", e.provider, e.attempts, e.idem_key);
                    if let Err(err) = needs_attention(store, &agent_id, &e.run_id, &format!("effect:{}", e.id), case.as_ref(), &reason, t) {
                        warn!(effect = e.id, error = %err, "engine: could not route the unconfirmed effect for attention");
                    }
                }
            }
        }
        Err(e) => warn!(error = %e, "engine: pending effects read failed"),
    }

    match store.engine_expire_transient_events(t - TRANSIENT_TTL_SECS) {
        Ok(n) => report.expired = n,
        Err(e) => warn!(error = %e, "engine: expiry failed"),
    }
    report
}

fn deliver(
    store: &Store,
    event: &EngineEvent,
    t: i64,
    busy: &dyn Fn(&str) -> bool,
    steer: &dyn Fn(&str, &EngineEvent),
    report: &mut TickReport,
) {
    if event.target_type == "binding" {
        fire_schedule(store, event, t, report);
        return;
    }
    if event.target_type == "entity" && event.kind == "timer" && event.target_id.starts_with("heartbeat:") {
        fire_heartbeat(store, event, t, report);
        return;
    }
    let wait = match store.engine_match_wait(event) {
        Ok(w) => w,
        Err(e) => {
            warn!(event = event.id, error = %e, "engine: match failed; lease will expire and retry");
            return;
        }
    };
    let Some(wait) = wait else {
        if event.target_type == "wait" {
            // I-9: an older generation. Dropped on purpose, recorded as such.
            if let Err(e) = store.engine_supersede_event(event.id, t, "superseded: wait generation replaced") {
                warn!(event = event.id, error = %e, "engine: supersede failed");
            } else {
                report.superseded += 1;
            }
            return;
        }
        // A signal for an open case that has no live wait — its first turn
        // is still queued or running. It reaches that turn, not a new one.
        if event.target_type == "run" && event.kind == "signal" {
            if let Some((kt, kv)) = event.target_id.split_once(':') {
                if let Ok(Some(case)) = store.engine_run_for_key(kt, kv) {
                    match live_turn(store, &case, event, t, busy, steer) {
                        LiveTurn::Handed => {
                            report.steered += 1;
                            return;
                        }
                        LiveTurn::Deferred => return,
                        LiveTurn::None => {}
                    }
                }
            }
        }
        // Nothing to wake. Completed with the fact recorded, so the row
        // neither loops nor pretends it did work.
        match store.engine_complete_event(event.id, t) {
            Ok(()) => {
                report.unrouted += 1;
                info!(event = event.id, kind = %event.kind, target = %event.target_id, "engine: event had no wait to wake");
            }
            Err(e) => warn!(event = event.id, error = %e, "engine: complete failed"),
        }
        return;
    };

    let outcome = match wait.action.as_str() {
        "resume" => store.engine_resume_from_wait(wait.id, event.id, t).map(|_| {
            report.resumed += 1;
        }),
        "trigger_child" => {
            let parent = match store.engine_get_run(&wait.run_id) {
                Ok(Some(p)) => p,
                Ok(None) => {
                    warn!(wait = wait.id, "engine: wait's run is gone; dropping the event");
                    let _ = store.engine_complete_event(event.id, t);
                    return;
                }
                Err(e) => {
                    warn!(wait = wait.id, error = %e, "engine: parent read failed");
                    return;
                }
            };
            // One live turn per case (design: concurrency). A running or
            // queued turn hears the signal; a second turn never starts.
            match live_turn(store, &parent, event, t, busy, steer) {
                LiveTurn::Handed => {
                    report.steered += 1;
                    return;
                }
                LiveTurn::Deferred => {
                    // A turn is running but cannot take the event right now
                    // (between provider retries, or finishing). The lease
                    // expires and the next tick routes it — by then the turn
                    // has settled and the parent's new wait carries it.
                    return;
                }
                LiveTurn::None => {}
            }
            start_child(store, &parent, event).map(|_| {
                report.children_started += 1;
            })
        }
        other => {
            warn!(wait = wait.id, action = other, "engine: unknown wait action; leaving the event to retry");
            return;
        }
    };
    match outcome.and_then(|_| store.engine_complete_event(event.id, t)) {
        Ok(()) => {}
        Err(e) => warn!(event = event.id, wait = wait.id, error = %e, "engine: delivery failed; lease will expire and retry"),
    }
}

/// Whose event this is: the employee, the run it was aimed at, and the
/// open case if it concerns one — so a give-up on it can be routed by that
/// employee's autonomy.
fn owner_of_event(store: &Store, e: &EngineEvent) -> (String, Option<String>, Option<EngineRun>) {
    let run = match e.target_type.as_str() {
        "run" => store
            .engine_get_run(&e.target_id)
            .ok()
            .flatten()
            .or_else(|| e.target_id.split_once(':').and_then(|(kt, kv)| store.engine_run_for_key(kt, kv).ok().flatten())),
        "wait" => e
            .target_id
            .parse::<i64>()
            .ok()
            .and_then(|id| store.engine_get_wait(id).ok().flatten())
            .and_then(|w| store.engine_get_run(&w.run_id).ok().flatten()),
        _ => None,
    };
    if let Some(run) = run {
        let case = if run.kind == "case" {
            Some(run.clone())
        } else {
            run.parent_run_id.as_deref().and_then(|p| store.engine_get_run(p).ok().flatten()).filter(|p| p.kind == "case")
        };
        return (run.agent_id.clone(), Some(run.id.clone()), case);
    }
    // Binding and entity timers name their employee in the target.
    let agent = match e.target_type.as_str() {
        "binding" => e
            .target_id
            .strip_prefix("hb:")
            .and_then(|s| s.split_once(':'))
            .map(|(a, _)| a.to_string())
            .or_else(|| {
                e.target_id
                    .strip_prefix("cron:")
                    .and_then(|s| s.parse::<i64>().ok())
                    .and_then(|id| store.get_cron_job(id).ok().flatten())
                    .and_then(|j| j.agent_id)
            }),
        "entity" => e.target_id.strip_prefix("heartbeat:agent:").map(str::to_string),
        _ => None,
    };
    (agent.unwrap_or_default(), None, None)
}

/// What happened to an event aimed at a case that may have a live turn.
enum LiveTurn {
    /// No queued or running turn: the caller may start one.
    None,
    /// The live turn took it (steered in, or appended to its inputs);
    /// the event is completed.
    Handed,
    /// A turn is live but cannot take it now. Nothing starts, nothing is
    /// completed; the lease expires and a later tick routes it.
    Deferred,
}

/// If the case has a live turn, hand it the event: a running turn gets it
/// as steering; a queued one gets it appended to its inputs. A running turn
/// that is not accepting steering, a turn parked on an approval, and a turn
/// interrupted by a restart all still count as live — a second turn never
/// starts beside one.
fn live_turn(
    store: &Store,
    case: &EngineRun,
    event: &EngineEvent,
    t: i64,
    busy: &dyn Fn(&str) -> bool,
    steer: &dyn Fn(&str, &EngineEvent),
) -> LiveTurn {
    let child = match store.engine_live_child(&case.id) {
        Ok(Some(c)) => c,
        _ => return LiveTurn::None,
    };
    let handed = match child.state.as_str() {
        "running" => {
            // The turn's own session is the one the runner marks busy.
            if busy(&child.session_key) {
                steer(&child.session_key, event);
                true
            } else {
                return LiveTurn::Deferred;
            }
        }
        "queued" => match store.engine_append_pending_signal(&child.id, &event.payload) {
            Ok(()) => true,
            Err(e) => {
                warn!(child = %child.id, error = %e, "engine: could not append signal to the queued turn");
                return LiveTurn::Deferred;
            }
        },
        "waiting" | "interrupted" => return LiveTurn::Deferred,
        _ => return LiveTurn::None,
    };
    if handed {
        if let Err(e) = store.engine_complete_event(event.id, t) {
            warn!(event = event.id, error = %e, "engine: complete after steer failed");
        }
        LiveTurn::Handed
    } else {
        LiveTurn::Deferred
    }
}

// ── schedules: one pending timer per enabled job ──────────────────────────

fn cron_target(job: &CronJob) -> String {
    db::cron_ref(job.id)
}

/// The next occurrence of a cron expression after `floor`, evaluated on the
/// machine's wall clock — the owner's clock. None: a one-shot whose moment
/// has passed. Err: the expression does not parse.
pub fn next_occurrence(schedule: &str, floor: i64) -> Result<Option<i64>, String> {
    let normalized = tools::PersonaTool::normalize_cron(schedule);
    let parsed: cron::Schedule = normalized.parse().map_err(|e: cron::error::Error| e.to_string())?;
    let floor = chrono::Utc
        .timestamp_opt(floor, 0)
        .single()
        .map(|d| d.with_timezone(&Local))
        .ok_or_else(|| "floor out of range".to_string())?;
    Ok(parsed.after(&floor).next().map(|d| d.timestamp()))
}

/// The floor the next occurrence is computed from: the last consumed timer,
/// else the job's creation, and never further back than the catch-up
/// window — so a job that slept through a hundred occurrences fires at most
/// one late and then its next real one.
fn schedule_floor(job: &CronJob, consumed: Option<i64>, t: i64) -> i64 {
    let created = job.created_at.as_deref().and_then(|s| {
        chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
            .ok()
            .map(|d| d.and_utc().timestamp())
    });
    consumed.or(created).unwrap_or(t).max(t - CATCH_UP_SECS)
}

// ── recurring timers: one pending timer per wanted target ─────────────────

/// A timer the engine should be holding: for what, carrying which config,
/// and — given the last consumed one — due when.
struct Wanted<'a> {
    target: String,
    /// The config the timer carries. A change replaces the pending timer.
    schedule: String,
    /// The next due moment from the floor (the last consumed timer's
    /// moment, if any). None: nothing to arm (a one-shot that passed).
    due: Box<dyn Fn(Option<i64>) -> Option<i64> + Send + Sync + 'a>,
}

/// The ONE reconciliation every recurring timer uses. Pending timers under
/// `prefix` are compared with `wanted`: one for a target no longer wanted,
/// or carrying a different config, is dropped with a note; every wanted
/// target without a pending timer gets one. Returns how many were armed.
fn reconcile_timers(store: &Store, t: i64, target_type: &str, prefix: &str, wanted: &[Wanted<'_>]) -> usize {
    let pending = match store.engine_pending_timers(target_type) {
        Ok(p) => p,
        Err(e) => {
            warn!(error = %e, target_type, "engine: could not read pending timers");
            return 0;
        }
    };
    let mut held: HashSet<String> = HashSet::new();
    for timer in pending.iter().filter(|e| e.target_id.starts_with(prefix)) {
        match wanted.iter().find(|w| w.target == timer.target_id) {
            Some(w) if timer.schedule.as_deref() == Some(w.schedule.as_str()) => {
                held.insert(timer.target_id.clone());
            }
            _ => {
                if let Err(e) = store.engine_supersede_event(timer.id, t, "superseded: schedule changed or target gone") {
                    warn!(timer = timer.id, error = %e, "engine: could not drop a stale timer");
                }
            }
        }
    }
    let mut armed = 0;
    for w in wanted {
        if held.contains(&w.target) {
            continue;
        }
        let floor = store.engine_last_timer_floor(target_type, &w.target).ok().flatten();
        let Some(due) = (w.due)(floor) else { continue };
        match store.engine_enqueue_event(&NewEvent {
            kind: "timer",
            target_type,
            target_id: &w.target,
            idem_key: &format!("{}:{due}:{t}", w.target),
            due_at: Some(due),
            schedule: Some(&w.schedule),
            ..Default::default()
        }) {
            Ok(db::Enqueued::Inserted(_)) => armed += 1,
            Ok(db::Enqueued::Duplicate) => {}
            Err(e) => warn!(target = %w.target, error = %e, "engine: could not arm timer"),
        }
    }
    armed
}

/// Reconcile pending timers with the enabled jobs: a timer whose job is
/// gone, disabled, or rescheduled is dropped; every enabled job without one
/// gets its next occurrence. Returns how many were armed.
pub fn arm_schedules(store: &Store, t: i64) -> usize {
    let jobs = match store.list_enabled_cron_jobs() {
        Ok(j) => j,
        Err(e) => {
            warn!(error = %e, "engine: could not read schedules");
            return 0;
        }
    };
    let wanted: Vec<Wanted<'_>> = jobs
        .iter()
        .filter(|job| {
            let ok = next_occurrence(&job.schedule, t).is_ok();
            if !ok {
                // Once per job per process, not once per tick.
                static WARNED: std::sync::Mutex<Vec<i64>> = std::sync::Mutex::new(Vec::new());
                let mut warned = WARNED.lock().unwrap_or_else(|p| p.into_inner());
                if !warned.contains(&job.id) {
                    warned.push(job.id);
                    warn!(job = job.name.as_str(), schedule = %job.schedule, "invalid cron expression; this job will not fire");
                }
            }
            ok
        })
        .map(|job| Wanted {
            target: cron_target(job),
            schedule: job.schedule.clone(),
            due: Box::new(move |consumed| next_occurrence(&job.schedule, schedule_floor(job, consumed, t)).ok().flatten()),
        })
        .collect();
    reconcile_timers(store, t, "binding", "cron:", &wanted)
}

/// A binding heartbeat's timer came due: queue ONE fire of the inline
/// workflow unless the binding is now off or its last run is still going.
/// Like an entity heartbeat, it is never "too late".
fn fire_binding_heartbeat(store: &Store, event: &EngineEvent, t: i64, report: &mut TickReport) {
    let skip = |store: &Store, note: &str, report: &mut TickReport| match store.engine_supersede_event(event.id, t, note) {
        Ok(()) => report.skipped += 1,
        Err(e) => warn!(event = event.id, error = %e, "engine: skip failed; lease will expire and retry"),
    };
    let Some((agent_id, binding)) = event.target_id.strip_prefix("hb:").and_then(|s| s.split_once(':')) else {
        skip(store, "skipped: malformed heartbeat binding target", report);
        return;
    };
    if !store.is_agent_workflow_active(agent_id, binding).unwrap_or(false) {
        skip(store, "skipped: binding inactive", report);
        return;
    }
    let wf_id = types::keyparser::agent_workflow_id(agent_id);
    if store.has_running_run(&wf_id, binding).unwrap_or(false) || store.engine_has_live_run_for_ref(&event.target_id).unwrap_or(false) {
        info!(agent = agent_id, binding, "engine: previous heartbeat run still active; this one skipped");
        skip(store, "skipped: previous run still active", report);
        return;
    }
    let id = uuid::Uuid::new_v4().to_string();
    let inputs = serde_json::json!({
        "command": format!("agent:{agent_id}:{binding}"),
        "trigger": "heartbeat",
        "label": format!("Heartbeat: {binding}"),
    })
    .to_string();
    let created = store.engine_create_run(&db::NewRun {
        id: &id,
        kind: "task",
        session_key: &format!("heartbeat-binding-{agent_id}-{binding}"),
        agent_id,
        lane: "main",
        inputs: Some(&inputs),
        external_ref: Some(&event.target_id),
        ..Default::default()
    });
    match created.and_then(|_| store.engine_complete_event(event.id, t)) {
        Ok(()) => {
            report.fired += 1;
            info!(agent = agent_id, binding, "engine: heartbeat binding fired");
        }
        Err(e) => warn!(agent = agent_id, binding, error = %e, "engine: could not queue the heartbeat fire; lease will expire and retry"),
    }
}

/// A schedule's timer came due. Skip (and say why) when the job is gone or
/// disabled, when its last fire is still running, or when the occurrence
/// was missed by more than the catch-up window; otherwise queue ONE run.
/// The next occurrence is armed on the following tick from this consumed one.
fn fire_schedule(store: &Store, event: &EngineEvent, t: i64, report: &mut TickReport) {
    if event.target_id.starts_with("hb:") {
        fire_binding_heartbeat(store, event, t, report);
        return;
    }
    let job = event
        .target_id
        .strip_prefix("cron:")
        .and_then(|s| s.parse::<i64>().ok())
        .and_then(|id| store.get_cron_job(id).ok().flatten())
        .filter(|j| j.enabled.unwrap_or(0) != 0);
    let skip = |store: &Store, note: &str, report: &mut TickReport| {
        match store.engine_supersede_event(event.id, t, note) {
            Ok(()) => report.skipped += 1,
            Err(e) => warn!(event = event.id, error = %e, "engine: skip failed; lease will expire and retry"),
        }
    };
    let Some(job) = job else {
        skip(store, "skipped: job gone or disabled", report);
        return;
    };
    let late = t - event.due_at.unwrap_or(t);
    if late > CATCH_UP_SECS {
        info!(job = job.name.as_str(), late_secs = late, "engine: scheduled fire missed the catch-up window; skipped");
        skip(store, &format!("skipped: missed by {late}s, beyond the catch-up window"), report);
        return;
    }
    match store.engine_has_live_run_for_ref(&cron_target(&job)) {
        Ok(true) => {
            info!(job = job.name.as_str(), "engine: previous fire still running; this occurrence skipped");
            skip(store, "skipped: previous fire still running", report);
            return;
        }
        Ok(false) => {}
        Err(e) => {
            warn!(job = job.name.as_str(), error = %e, "engine: overlap check failed; lease will expire and retry");
            return;
        }
    }
    match store.queue_cron_run(&job, false).and_then(|_| store.engine_complete_event(event.id, t)) {
        Ok(()) => {
            report.fired += 1;
            info!(job = job.name.as_str(), "dispatching scheduled task");
        }
        Err(e) => warn!(job = job.name.as_str(), error = %e, "engine: could not queue the scheduled run; lease will expire and retry"),
    }
}

/// What a settled fire should say, and to whom.
struct Settle<'a> {
    /// The name the owner knows it by.
    label: &'a str,
    /// The desktop line on success, if any: a fire that already delivered
    /// its result where the owner reads it (a channel post) says nothing.
    success_note: Option<String>,
    /// A run-now announces itself to the UI instead of the desktop.
    manual: bool,
}

/// Record a fire's outcome on its run and tell whoever is listening.
/// Failures always reach the desktop, because the delivery itself may be
/// what failed.
fn settle_task(state: &AppState, run: &EngineRun, s: Settle<'_>, success: bool, output: String, err: Option<String>) {
    let t = now();
    let store = &state.store;
    if !output.is_empty() {
        let _ = store.engine_set_run_result(&run.id, &output, None);
    }
    let _ = store.engine_set_run_state(&run.id, if success { "done" } else { "failed" }, t, err.as_deref());

    if s.manual {
        state.hub.broadcast(
            "task_complete",
            serde_json::json!({
                "task": s.label,
                "success": success,
                "output": crate::truncate_str(if success { &output } else { err.as_deref().unwrap_or(&output) }, 500),
            }),
        );
    }
    if success {
        info!(task = s.label, "task completed");
        if let Some(note) = s.success_note.filter(|_| !s.manual) {
            notify_crate::send("Nebo", &note);
        }
    } else {
        let e = err.as_deref().unwrap_or("unknown");
        warn!(task = s.label, error = e, "task failed");
        notify_crate::send("Nebo", &format!("{} failed: {}", s.label, e));
    }
}

// ── heartbeats: one pending timer per enabled entity ──────────────────────

/// How often the enabled set is re-resolved against settings. Arming is
/// idempotent, so this only bounds how soon a settings change is noticed.
const HEARTBEAT_ARM_SECS: i64 = 60;

/// Reconcile pending heartbeat timers with the entities whose heartbeat is
/// on: a timer for an entity now off (or with a changed interval) is
/// dropped; every enabled entity without one gets its next fire — the last
/// consumed one plus the interval, no earlier than now, moved into the
/// entity's time window. An entity that has never fired is due now.
async fn arm_heartbeats(state: &AppState, t: i64) -> usize {
    let enabled = match crate::heartbeat::enabled_entities(state).await {
        Ok(e) => e,
        Err(e) => {
            warn!(error = %e, "engine: could not resolve heartbeat entities");
            return 0;
        }
    };
    let wanted: Vec<Wanted<'_>> = enabled
        .iter()
        .map(|e| Wanted {
            target: e.target(),
            schedule: e.interval_secs.to_string(),
            due: Box::new(move |consumed| {
                let due = consumed.or(e.last_fired_at).map(|f| f + e.interval_secs).unwrap_or(t).max(t);
                Some(crate::heartbeat::next_in_window(due, e.window.as_ref()))
            }),
        })
        .collect();
    reconcile_timers(&state.store, t, "entity", "heartbeat:", &wanted) + arm_binding_heartbeats(state, t).await
}

/// The same reconciliation for workflow bindings with a heartbeat trigger
/// (`"<duration>|HH:MM-HH:MM"`): one timer per active binding of a live
/// agent, aimed at binding `hb:<agent>:<binding>`, carrying the config so a
/// change replaces it. A binding that has never fired is due one interval
/// from now, as the old loop's first tick was.
async fn arm_binding_heartbeats(state: &AppState, t: i64) -> usize {
    let store = &state.store;
    let bindings = match store.list_active_heartbeat_workflows() {
        Ok(b) => b,
        Err(e) => {
            warn!(error = %e, "engine: could not read heartbeat bindings");
            return 0;
        }
    };
    let live: Vec<_> = {
        let registry = state.agent_registry.read().await;
        bindings.into_iter().filter(|b| registry.contains_key(&b.agent_id)).collect()
    };
    let wanted: Vec<Wanted<'_>> = live
        .iter()
        .filter_map(|b| {
            let (duration, window) = agent::agent_worker::parse_heartbeat(&b.trigger_config);
            if duration.is_zero() {
                warn!(agent = %b.agent_id, binding = %b.binding_name, config = %b.trigger_config, "invalid heartbeat config; this binding will not fire");
                return None;
            }
            let interval = duration.as_secs() as i64;
            let window = window.map(|(s, e)| (s.format("%H:%M").to_string(), e.format("%H:%M").to_string()));
            Some(Wanted {
                target: format!("hb:{}:{}", b.agent_id, b.binding_name),
                schedule: b.trigger_config.clone(),
                due: Box::new(move |consumed| {
                    let due = (consumed.unwrap_or(t) + interval).max(t);
                    Some(crate::heartbeat::next_in_window(due, window.as_ref()))
                }),
            })
        })
        .collect();
    reconcile_timers(store, t, "binding", "hb:", &wanted)
}

/// A heartbeat's timer came due: queue ONE run of kind `heartbeat` unless
/// the previous one is still going. A heartbeat is never "too late" — an
/// entity that slept through its interval simply gets its turn now, once.
fn fire_heartbeat(store: &Store, event: &EngineEvent, t: i64, report: &mut TickReport) {
    let Some(rest) = event.target_id.strip_prefix("heartbeat:") else { return };
    let Some((entity_type, entity_id)) = rest.split_once(':') else {
        let _ = store.engine_supersede_event(event.id, t, "skipped: malformed heartbeat target");
        report.skipped += 1;
        return;
    };
    match store.engine_has_live_run_for_ref(&event.target_id) {
        Ok(true) => {
            info!(entity = %event.target_id, "engine: previous heartbeat still running; this one skipped");
            match store.engine_supersede_event(event.id, t, "skipped: previous heartbeat still running") {
                Ok(()) => report.skipped += 1,
                Err(e) => warn!(event = event.id, error = %e, "engine: skip failed; lease will expire and retry"),
            }
            return;
        }
        Ok(false) => {}
        Err(e) => {
            warn!(entity = %event.target_id, error = %e, "engine: overlap check failed; lease will expire and retry");
            return;
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    let inputs = serde_json::json!({ "entity_type": entity_type, "entity_id": entity_id }).to_string();
    let created = store.engine_create_run(&db::NewRun {
        id: &id,
        kind: "heartbeat",
        session_key: &format!("heartbeat-{entity_type}-{entity_id}"),
        agent_id: if entity_type == "agent" { entity_id } else { "" },
        lane: "heartbeat",
        inputs: Some(&inputs),
        external_ref: Some(&event.target_id),
        ..Default::default()
    });
    match created.and_then(|_| store.engine_complete_event(event.id, t)) {
        Ok(()) => report.fired += 1,
        Err(e) => warn!(entity = %event.target_id, error = %e, "engine: could not queue the heartbeat; lease will expire and retry"),
    }
}

// ── driving turns through the workflow runner ─────────────────────────────

/// Start queued case turns and scheduled fires, and reconcile running
/// turns. This is the only place the engine touches the runner, and turns
/// go through the same `run_inline` every webhook and scheduled workflow uses.
async fn drive(state: &AppState) {
    let store = &state.store;
    let t = now();
    if crate::DRAINING.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }

    {
        use std::sync::atomic::{AtomicI64, Ordering};
        static LAST_ARM: AtomicI64 = AtomicI64::new(0);
        let last = LAST_ARM.load(Ordering::Relaxed);
        if t - last >= HEARTBEAT_ARM_SECS && LAST_ARM.compare_exchange(last, t, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
            let armed = arm_heartbeats(state, t).await;
            if armed > 0 {
                info!(armed, "engine: heartbeat timers armed");
            }
        }
    }

    let beats = store.engine_queued_runs_of_kind("heartbeat", TURNS_PER_TICK).unwrap_or_default();
    for run in beats {
        let inputs: serde_json::Value = run.inputs.as_deref().and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default();
        let (Some(entity_type), Some(entity_id)) = (inputs["entity_type"].as_str(), inputs["entity_id"].as_str()) else {
            let _ = store.engine_set_run_state(&run.id, "failed", t, Some("heartbeat run has no entity"));
            continue;
        };
        let (entity_type, entity_id) = (entity_type.to_string(), entity_id.to_string());
        if let Err(e) = store.engine_set_run_state(&run.id, "running", t, None) {
            warn!(run = %run.id, error = %e, "engine: mark running failed");
            continue;
        }
        let state = state.clone();
        tokio::spawn(async move {
            let outcome = crate::heartbeat::fire(&state, &entity_type, &entity_id).await;
            let t = now();
            match outcome {
                Ok(true) => {
                    let _ = state.store.engine_set_run_state(&run.id, "done", t, None);
                }
                Ok(false) => {
                    let _ = state.store.engine_set_run_result(&run.id, "not fired: heartbeat off or empty by the time it came due", None);
                    let _ = state.store.engine_set_run_state(&run.id, "done", t, None);
                }
                Err(e) => {
                    warn!(run = %run.id, error = %e, "engine: heartbeat failed");
                    let _ = state.store.engine_set_run_state(&run.id, "failed", t, Some(&e));
                }
            }
        });
    }

    let tasks = store.engine_queued_runs_of_kind("task", TURNS_PER_TICK).unwrap_or_default();
    for (i, run) in tasks.into_iter().enumerate() {
        let inputs: serde_json::Value = run.inputs.as_deref().and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default();
        // A fire is either a scheduled job (by id) or a binding heartbeat
        // (by command); both execute through the scheduler's executors.
        let job = inputs["job_id"].as_i64().and_then(|id| store.get_cron_job(id).ok().flatten());
        let command = inputs["command"].as_str().map(str::to_string);
        if job.is_none() && command.is_none() {
            let _ = store.engine_set_run_state(&run.id, "failed", t, Some("scheduled job no longer exists"));
            continue;
        }
        if let Err(e) = store.engine_set_run_state(&run.id, "running", t, None) {
            warn!(run = %run.id, error = %e, "engine: mark running failed");
            continue;
        }
        let state = state.clone();
        tokio::spawn(async move {
            // Same-tick starts ramp at ~1/sec so a 9:00 herd never spikes
            // the provider all at once.
            if i > 0 {
                tokio::time::sleep(Duration::from_secs(i as u64)).await;
            }
            match (job, command) {
                (Some(job), _) => {
                    let (success, output, err) = crate::scheduler::execute_job(&state, &job).await;
                    // A channel-bound job already delivered its response where
                    // the owner reads it; a desktop popup on top would be noise.
                    let channel_bound = job.agent_id.as_deref().is_some_and(|s| !s.is_empty())
                        && job.channel_ctx_json.as_deref().is_some_and(|s| !s.is_empty());
                    let settle = Settle {
                        label: &job.name,
                        success_note: (!channel_bound).then(|| format!("{} completed", job.name)),
                        manual: inputs["manual"].as_bool().unwrap_or(false),
                    };
                    settle_task(&state, &run, settle, success, output, err);
                }
                (None, Some(command)) => {
                    let trigger = inputs["trigger"].as_str().unwrap_or("heartbeat");
                    let label = inputs["label"].as_str().unwrap_or(&command).to_string();
                    let (success, output, err) =
                        crate::scheduler::execute_agent_workflow_task(&*state.workflow_manager, &state.store, &command, trigger).await;
                    let settle = Settle { label: &label, success_note: Some(label.clone()), manual: false };
                    settle_task(&state, &run, settle, success, output, err);
                }
                (None, None) => unreachable!("checked above"),
            }
        });
    }

    // Queued workflow runs are the engine's to start: a run an approval
    // just woke resumes (or is denied) at the parked call; a case turn is
    // relaunched under its own id. Both go through the same `run_inline`
    // every workflow uses; a run the boot sweep resumed is taken by
    // whichever of this loop and the manager's recovery sees it first — the
    // call is the same.
    let queued = store.engine_queued_runs_of_kind("workflow", TURNS_PER_TICK).unwrap_or_default();
    for run in queued {
        if let Some(event_id) = run.woken_by() {
            resume_after_approval(state, &run, event_id, t).await;
        } else if run.parent_run_id.is_some() {
            start_turn(state, &run, t).await;
        }
    }

    time_out_turns(state, t).await;

    // A turn the workflow ended, whose case has not heard it: the turn's
    // declared wait (or the default) becomes the case's next wait.
    for turn in store.engine_unsettled_turns(TURNS_PER_TICK).unwrap_or_default() {
        let failed = turn.state != "done";
        let output = if failed { turn.error.clone().or(turn.result.clone()).or_else(|| Some(turn.state.clone())) } else { turn.result.clone() };
        if let Err(e) = settle_turn(store, &turn, output.as_deref(), failed, t) {
            warn!(run = %turn.id, error = %e, "engine: settle failed");
        }
    }
}

/// A turn may run this long from start to close; longer is cancelled and
/// settled as a failure (the retry policy takes it from there).
const TURN_START_TO_CLOSE_SECS: i64 = 3600;
/// A running turn that shows no activity for this long is stuck, not slow.
const TURN_IDLE_SECS: u64 = 600;
/// A turn queued this long without starting is worth a loud line.
const TURN_QUEUED_ALERT_SECS: i64 = 600;

/// Turn timeouts (design: start-to-close, heartbeat, queued-too-long). A
/// cancelled turn ends as cancelled; the next tick settles it as a failed
/// turn and the case retries on the policy's schedule.
async fn time_out_turns(state: &AppState, t: i64) {
    let store = &state.store;
    let mut stuck: Vec<(EngineRun, String)> = Vec::new();
    for turn in store.engine_turns_in_state_since("running", t - TURN_START_TO_CLOSE_SECS).unwrap_or_default() {
        stuck.push((turn, format!("timed out: running for more than {} minutes", TURN_START_TO_CLOSE_SECS / 60)));
    }
    for turn in store.engine_turns_in_state_since("running", t).unwrap_or_default() {
        if stuck.iter().any(|(s, _)| s.id == turn.id) {
            continue;
        }
        if let Some(snap) = state.run_registry.find_by_session(&turn.session_key).await {
            if snap.idle_secs > TURN_IDLE_SECS {
                stuck.push((turn, format!("timed out: no activity for {} minutes", snap.idle_secs / 60)));
            }
        }
    }
    for (turn, reason) in stuck {
        warn!(run = %turn.id, reason, "engine: turn timed out; cancelling");
        let _ = store.update_workflow_run(&turn.id, None, None, None, Some(&reason), None);
        if state.workflow_manager.cancel_run(&turn.id).await.is_err() {
            // Not registered as live (between start and register, or the
            // runner already let go): end the row ourselves.
            let _ = store.update_workflow_run(&turn.id, Some("cancelled"), None, None, None, None);
        }
    }

    for turn in store.engine_turns_in_state_since("queued", t - TURN_QUEUED_ALERT_SECS).unwrap_or_default() {
        static WARNED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
        let mut warned = WARNED.lock().unwrap_or_else(|p| p.into_inner());
        if !warned.contains(&turn.id) {
            warned.push(turn.id.clone());
            warn!(run = %turn.id, case = ?turn.parent_run_id, "engine: turn queued for more than {} minutes without starting", TURN_QUEUED_ALERT_SECS / 60);
        }
    }
}

/// Relaunch a queued case turn under its own id.
async fn start_turn(state: &AppState, run: &EngineRun, t: i64) {
    let store = &state.store;
    let Some(definition) = run.definition.clone() else {
        let _ = store.engine_set_run_state(&run.id, "failed", t, Some("case turn has no definition"));
        return;
    };
    let mut inputs: serde_json::Value = run.inputs.as_deref().and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default();
    let binding = inputs["_case"]["binding"].as_str().map(str::to_string);
    inputs["_relaunch_run"] = serde_json::json!(run.id);
    match state
        .workflow_manager
        .run_inline(definition, inputs, "case", binding, &run.agent_id, None)
        .await
    {
        Ok(_) => info!(run = %run.id, "engine: case turn started"),
        Err(e) => {
            warn!(run = %run.id, error = %e, "engine: case turn failed to start");
            let _ = settle_turn(store, run, Some(&format!("turn failed to start: {e}")), true, t);
        }
    }
}

/// An approval event resumed a parked run: continue it at the approved
/// call, or end it as denied. The event's payload is the owner's answer.
async fn resume_after_approval(state: &AppState, run: &EngineRun, event_id: i64, t: i64) {
    let store = &state.store;
    let approved = store
        .engine_get_event(event_id)
        .ok()
        .flatten()
        .and_then(|e| serde_json::from_str::<serde_json::Value>(&e.payload).ok())
        .and_then(|p| p["approved"].as_bool());
    let Some(approved) = approved else {
        // Woken by something that is not an answer; nothing to continue.
        let _ = store.engine_set_run_state(&run.id, "failed", t, Some("woken without an approval decision"));
        return;
    };
    let suspension = store.get_workflow_suspension(&run.id).ok().flatten();
    let (agent_id, binding, display) = match &suspension {
        Some((a, b, _, _, _, _, _, _, d)) => (a.clone(), b.clone(), d.clone()),
        None => {
            let _ = store.engine_set_run_state(&run.id, "failed", t, Some("resumed with no parked state to continue from"));
            return;
        }
    };
    if !approved {
        let _ = store.delete_workflow_suspension(&run.id);
        let _ = store.update_workflow_run(&run.id, Some("denied"), None, None, Some(&format!("Owner denied: {display}")), None);
        state.hub.broadcast("workflow_run_denied", serde_json::json!({ "runId": run.id, "agentId": agent_id }));
        info!(run = %run.id, "engine: approval denied; run ended");
        return;
    }
    let Some(definition) = run.definition.clone() else {
        let _ = store.engine_set_run_state(&run.id, "failed", t, Some("parked run has no definition snapshot to resume"));
        return;
    };
    let mut inputs: serde_json::Value = run.inputs.as_deref().and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default();
    inputs.as_object_mut().map(|m| m.remove("woken_by"));
    inputs["_resume_run"] = serde_json::json!(run.id);
    match state
        .workflow_manager
        .run_inline(definition, inputs, "approval", Some(binding), &agent_id, None)
        .await
    {
        Ok(_) => info!(run = %run.id, "engine: approval accepted; parked run resumed"),
        Err(e) => {
            warn!(run = %run.id, error = %e, "engine: parked run failed to resume");
            let _ = store.update_workflow_run(&run.id, Some("failed"), None, None, Some(&format!("resume after approval failed: {e}")), None);
        }
    }
}

/// The loop. Boot sweep first, then a tick every five seconds for the life
/// of the process.
pub fn spawn(state: AppState) {
    let store: Arc<Store> = state.store.clone();
    tokio::spawn(async move {
        recover(&store);
        let mut interval = tokio::time::interval(TICK);
        loop {
            interval.tick().await;
            let s = store.clone();
            let runner = state.runner.clone();
            let report = tokio::task::spawn_blocking(move || {
                let busy = |session: &str| runner.is_session_busy(session);
                let steer = |session: &str, event: &EngineEvent| {
                    let content = agent::steering::wrap_system_reminder(&format!(
                        "[Case event — not an owner message]\n{}:\n{}\n\nHandle this alongside your current work, and include the outcome in your report.",
                        event.kind,
                        event.payload.chars().take(2_000).collect::<String>()
                    ));
                    let taint = serde_json::from_str(&event.provenance).unwrap_or_default();
                    agent::steering::push_wake(session, agent::steering::WakeEntry { wake_id: event.id, content, taint });
                };
                tick(&s, now(), &busy, &steer)
            })
            .await
            .unwrap_or_default();
            if report != TickReport::default() {
                info!(?report, "engine: tick");
            }
            drive(&state).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use db::{NewEvent, NewRun, NewWait};
    use workflow::cases::{parse_wait, relative_secs, signal_or_open, CaseBinding, Routed};

    fn store() -> Store {
        let path = std::env::temp_dir().join(format!("nebo-engine-loop-{}.db", uuid::Uuid::new_v4()));
        Store::new(&path.to_string_lossy()).expect("store")
    }

    fn idle(_: &str) -> bool {
        false
    }
    fn no_steer(_: &str, _: &EngineEvent) {}

    fn binding<'a>() -> CaseBinding<'a> {
        CaseBinding {
            agent_id: "ic",
            binding_name: "work-lead",
            definition_json: r#"{"activities":[{"id":"run","intent":"work the lead"}]}"#,
            base_inputs: serde_json::json!({"tone": "warm"}),
            default_wait_secs: 3 * 86_400,
        }
    }

    #[test]
    fn a_signal_for_a_waiting_case_starts_one_child_turn_and_the_parent_keeps_waiting() {
        let s = store();
        s.engine_create_run(&NewRun { id: "case-1", kind: "case", session_key: "agent:a:case:k", agent_id: "a", lane: "main", ..Default::default() }).unwrap();
        s.engine_declare_wait("case-1", &NewWait { action: "trigger_child", on_kind: "signal", key: "email:alma@x.com", deadline: Some(9_000), reason: "until Thu", ..Default::default() }, 100).unwrap();
        s.engine_enqueue_event(&NewEvent { kind: "signal", target_type: "run", target_id: "email:alma@x.com", payload: "form again", idem_key: "form-2", durable: true, ..Default::default() }).unwrap();

        let r = tick(&s, 200, &idle, &no_steer);
        assert_eq!(r.claimed, 1);
        assert_eq!(r.children_started, 1);
        assert_eq!(s.engine_get_run("case-1").unwrap().unwrap().state, "waiting", "parent still waits");
        let children = s.engine_queued_runs("main", 10).unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].kind, "workflow", "a turn is one run: the workflow run itself");
        assert_eq!(children[0].session_key, format!("agent:a:workflow:{}", children[0].id));
        assert!(s.get_workflow_run(&children[0].id).unwrap().is_some(), "with its workflow detail row");
        assert_eq!(children[0].parent_run_id.as_deref(), Some("case-1"));
        assert!(children[0].inputs.as_deref().unwrap().contains("form again"));
        assert_eq!(tick(&s, 300, &idle, &no_steer), TickReport::default(), "delivered: a second tick finds nothing");
    }

    /// Seen live: after a turn settled, three signals claimed in ONE tick
    /// each started a child. The second and third must ride the first.
    #[test]
    fn three_signals_in_one_tick_start_one_turn_and_ride_the_rest() {
        let s = store();
        s.engine_create_run(&NewRun { id: "case-1", kind: "case", session_key: "agent:a:case:k", agent_id: "a", lane: "main", ..Default::default() }).unwrap();
        s.engine_declare_wait("case-1", &NewWait { action: "trigger_child", on_kind: "signal", key: "email:x", deadline: Some(9_000), reason: "after a failed turn", ..Default::default() }, 100).unwrap();
        for i in 1..=3 {
            s.engine_enqueue_event(&NewEvent { kind: "signal", target_type: "run", target_id: "email:x", payload: &format!("sub-{i}"), idem_key: &format!("s{i}"), durable: true, ..Default::default() }).unwrap();
        }
        let r = tick(&s, 200, &idle, &no_steer);
        assert_eq!(r.claimed, 3);
        assert_eq!(r.children_started, 1, "one turn");
        assert_eq!(r.steered, 2, "the other two ride it");
        let turns = s.engine_queued_runs_of_kind("workflow", 10).unwrap();
        assert_eq!(turns.len(), 1);
        let inputs = turns[0].inputs.as_deref().unwrap();
        assert!(inputs.contains("sub-1") && inputs.contains("sub-2") && inputs.contains("sub-3"));
    }

    /// Seen live: a turn was running but its session was between provider
    /// retries, so it did not read as busy — and a second turn started
    /// beside it. A running turn blocks a second one, busy or not; the event
    /// waits, and rides the parent's next wait once the turn settles.
    #[test]
    fn a_running_turn_that_is_not_busy_defers_the_signal_instead_of_starting_another() {
        let s = store();
        s.engine_create_run(&NewRun { id: "case-1", kind: "case", session_key: "agent:a:case:k", agent_id: "a", lane: "main", ..Default::default() }).unwrap();
        s.engine_declare_wait("case-1", &NewWait { action: "trigger_child", on_kind: "signal", key: "email:x", deadline: None, reason: "first contact", ..Default::default() }, 100).unwrap();
        s.engine_create_run(&NewRun { id: "turn-1", kind: "workflow", session_key: "agent:a:workflow:turn-1", agent_id: "a", lane: "main", parent_run_id: Some("case-1"), inputs: Some(r#"{"_case":{"key_type":"email","key_value":"x","default_wait_secs":86400}}"#), ..Default::default() }).unwrap();
        s.engine_set_run_state("turn-1", "running", 150, None).unwrap();
        s.engine_enqueue_event(&NewEvent { kind: "signal", target_type: "run", target_id: "email:x", payload: "again", idem_key: "s2", durable: true, ..Default::default() }).unwrap();

        let r = tick(&s, 200, &idle, &no_steer);
        assert_eq!(r.claimed, 1);
        assert_eq!(r.children_started, 0, "no second turn beside a running one");
        assert_eq!(r.steered, 0);
        assert_eq!(s.engine_queued_runs_of_kind("workflow", 10).unwrap().len(), 0);

        // The turn settles; the parent's new wait carries the deferred event
        // once its lease has expired.
        let turn = s.engine_get_run("turn-1").unwrap().unwrap();
        settle_turn(&s, &turn, Some("sent first response"), false, 300).unwrap();
        let later = 200 + db::EVENT_LEASE_SECS + 1;
        let r = tick(&s, later, &idle, &no_steer);
        assert_eq!(r.children_started, 1, "now it starts the next turn");
        let next = s.engine_queued_runs_of_kind("workflow", 10).unwrap();
        assert!(next[0].inputs.as_deref().unwrap().contains("again"));
    }

    #[test]
    fn a_stale_deadline_is_superseded_and_a_current_one_starts_the_turn() {
        let s = store();
        s.engine_create_run(&NewRun { id: "case-1", kind: "case", session_key: "agent:a:case:k", agent_id: "a", lane: "main", ..Default::default() }).unwrap();
        s.engine_declare_wait("case-1", &NewWait { action: "trigger_child", on_kind: "signal", key: "k", deadline: Some(1_000), reason: "Wed", ..Default::default() }, 10).unwrap();
        s.engine_declare_wait("case-1", &NewWait { action: "trigger_child", on_kind: "signal", key: "k", deadline: Some(2_000), reason: "Fri", ..Default::default() }, 20).unwrap();
        let wed = tick(&s, 1_000, &idle, &no_steer);
        assert_eq!(wed.superseded, 1, "Wednesday's timer names the old generation");
        assert_eq!(wed.children_started, 0);
        let fri = tick(&s, 2_000, &idle, &no_steer);
        assert_eq!(fri.children_started, 1, "Friday's timer is the live one");
    }

    #[test]
    fn an_approval_resumes_the_parked_run_itself() {
        let s = store();
        s.engine_create_run(&NewRun { id: "wf-1", kind: "workflow", session_key: "agent:a:workflow:wf-1", agent_id: "a", lane: "main", ..Default::default() }).unwrap();
        s.engine_declare_wait("wf-1", &NewWait { action: "resume", on_kind: "approval", key: "approval:wf-1", parked: Some("[msgs]"), reason: "waiting for you", ..Default::default() }, 10).unwrap();
        s.engine_enqueue_event(&NewEvent { kind: "approval", target_type: "run", target_id: "approval:wf-1", payload: "approved", idem_key: "ok-1", ..Default::default() }).unwrap();
        let r = tick(&s, 50, &idle, &no_steer);
        assert_eq!(r.resumed, 1);
        assert_eq!(s.engine_get_run("wf-1").unwrap().unwrap().state, "queued");
    }

    #[test]
    fn boot_recovery_resumes_once() {
        let s = store();
        s.engine_create_run(&NewRun { id: "r", kind: "task", session_key: "agent:a:web", agent_id: "a", lane: "main", ..Default::default() }).unwrap();
        s.engine_set_run_state("r", "running", 1, None).unwrap();
        assert_eq!(recover(&s), 1);
        s.engine_set_run_state("r", "running", 2, None).unwrap();
        assert_eq!(recover(&s), 0, "second death is poison");
        assert_eq!(s.engine_get_run("r").unwrap().unwrap().state, "failed");
    }

    #[test]
    fn the_assessment_thread_four_submissions_one_case_one_first_turn() {
        let s = store();
        let b = binding();
        let payload = serde_json::json!({"email": "alma@aboundinggoods.com", "hours": "27-56"});
        let first = signal_or_open(&s, &b, "email", "alma@aboundinggoods.com", &payload, "webhook", "sub-aug18", 1_000).unwrap();
        let Routed::Opened { case_id } = first else { panic!("first submission opens a case") };
        let turns = s.engine_queued_runs_of_kind("workflow", 10).unwrap();
        assert_eq!(turns.len(), 1);
        let inputs = turns[0].inputs.as_deref().unwrap();
        assert!(inputs.contains("aboundinggoods"));
        assert!(inputs.contains("\"binding\":\"work-lead\""));

        for (i, idem) in ["sub-aug19", "sub-sep4", "sub-sep6"].iter().enumerate() {
            let r = signal_or_open(&s, &b, "email", "alma@aboundinggoods.com", &payload, "webhook", idem, 2_000 + i as i64).unwrap();
            assert_eq!(r, Routed::Signaled { case_id: case_id.clone() }, "{idem} reaches the same case");
        }
        assert_eq!(signal_or_open(&s, &b, "email", "alma@aboundinggoods.com", &payload, "webhook", "sub-sep6", 3_000).unwrap(), Routed::Duplicate);

        let r = tick(&s, 4_000, &idle, &no_steer);
        assert_eq!(r.steered, 3);
        assert_eq!(r.children_started, 0);
        assert_eq!(s.engine_queued_runs_of_kind("workflow", 10).unwrap().len(), 1, "still exactly one turn");
        let refreshed = s.engine_get_run(&turns[0].id).unwrap().unwrap();
        assert!(refreshed.inputs.as_deref().unwrap().matches("aboundinggoods").count() >= 4, "the later submissions rode along");
        assert_eq!(s.engine_run_for_key("email", "alma@aboundinggoods.com").unwrap().unwrap().id, case_id);
        assert_eq!(s.engine_get_run(&case_id).unwrap().unwrap().state, "waiting");
    }

    #[test]
    fn a_finished_turn_declares_the_parents_next_wait_or_falls_back_to_the_default() {
        let s = store();
        let b = binding();
        let payload = serde_json::json!({"email": "a@b.c"});
        let Routed::Opened { case_id } = signal_or_open(&s, &b, "email", "a@b.c", &payload, "webhook", "s1", 1_000).unwrap() else { panic!() };
        let turn = s.engine_queued_runs_of_kind("workflow", 1).unwrap().remove(0);
        s.engine_set_run_state(&turn.id, "running", 1_001, None).unwrap();
        let turn = s.engine_get_run(&turn.id).unwrap().unwrap();

        let out = r#"Sent the day-1 email. {"outcome":"sent day-1 follow-up","state":"waiting_on_customer","wait":{"on":"signal","deadline":"3d","reason":"follow up if no reply by Thursday"}}"#;
        settle_turn(&s, &turn, Some(out), false, 2_000).unwrap();
        assert_eq!(s.engine_get_run(&turn.id).unwrap().unwrap().state, "done");
        let case = s.engine_get_run(&case_id).unwrap().unwrap();
        assert_eq!(case.state, "waiting");
        assert_eq!(case.summary, "follow up if no reply by Thursday");
        let wait = s.engine_get_wait(case.current_wait_id.unwrap()).unwrap().unwrap();
        assert_eq!(wait.deadline, Some(2_000 + 3 * 86_400));
        assert_eq!(wait.key, "email:a@b.c");
        let hist = s.engine_events_for("run", &case_id, 50).unwrap();
        assert!(hist.iter().any(|e| e.kind == "turn_result" && e.payload.contains("Thursday")));

        let (ev, _) = s.engine_claim_events(2_000 + 3 * 86_400, 10).unwrap();
        let mut report = TickReport::default();
        for e in &ev {
            deliver(&s, e, 2_000 + 3 * 86_400, &idle, &no_steer, &mut report);
        }
        assert_eq!(report.children_started, 1, "the deadline started the next turn");
        let turn2 = s.engine_queued_runs_of_kind("workflow", 1).unwrap().remove(0);
        settle_turn(&s, &turn2, Some("Called, left a voicemail."), false, 5_000).unwrap();
        let case = s.engine_get_run(&case_id).unwrap().unwrap();
        let wait = s.engine_get_wait(case.current_wait_id.unwrap()).unwrap().unwrap();
        assert_eq!(wait.deadline, Some(5_000 + 3 * 86_400), "default three days");

        let (ev, _) = s.engine_claim_events(5_000 + 3 * 86_400, 10).unwrap();
        for e in &ev {
            deliver(&s, e, 5_000 + 3 * 86_400, &idle, &no_steer, &mut report);
        }
        let turn3 = s.engine_queued_runs_of_kind("workflow", 1).unwrap().remove(0);
        settle_turn(&s, &turn3, Some(r#"{"outcome":"booked for Tuesday","state":"booked","wait":{}}"#), false, 9_000).unwrap();
        let case = s.engine_get_run(&case_id).unwrap().unwrap();
        assert_eq!(case.state, "done");
        assert_eq!(case.result.as_deref(), Some("booked"));
        assert!(s.engine_run_for_key("email", "a@b.c").unwrap().is_none(), "key released");
    }

    // ── schedules ────────────────────────────────────────────────────────

    fn local(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> i64 {
        Local.with_ymd_and_hms(y, mo, d, h, mi, s).single().unwrap().timestamp()
    }

    /// A job whose floor is `floor`: one consumed timer at that moment, the
    /// way a job that has fired before carries its floor.
    fn job(s: &Store, name: &str, schedule: &str, floor: i64) -> CronJob {
        let j = s.create_cron_job(name, schedule, "echo hi", "shell", None, None, None, true, None, None).unwrap();
        let target = cron_target(&j);
        let db::Enqueued::Inserted(id) = s
            .engine_enqueue_event(&NewEvent { kind: "timer", target_type: "binding", target_id: &target, idem_key: &format!("{target}:floor"), due_at: Some(floor), ..Default::default() })
            .unwrap()
        else {
            panic!()
        };
        s.engine_complete_event(id, floor).unwrap();
        j
    }

    #[test]
    fn next_occurrence_reads_recurring_one_shot_five_field_and_weekday_schedules() {
        // Every 30 minutes after 9:00 → 9:30.
        assert_eq!(next_occurrence("0 0,30 * * * *", local(2026, 8, 23, 9, 0, 0)).unwrap(), Some(local(2026, 8, 23, 9, 30, 0)));
        // A year-pinned one-shot is found from before its moment, and gone after it.
        assert_eq!(next_occurrence("0 5 10 23 8 * 2026", local(2026, 8, 23, 10, 0, 0)).unwrap(), Some(local(2026, 8, 23, 10, 5, 0)));
        assert_eq!(next_occurrence("0 5 10 23 8 * 2026", local(2026, 8, 23, 10, 8, 0)).unwrap(), None);
        // Stale five-field crons normalize instead of erroring.
        assert_eq!(next_occurrence("0 7 * * *", local(2026, 8, 23, 6, 0, 0)).unwrap(), Some(local(2026, 8, 23, 7, 0, 0)));
        // Weekdays-at-7 skips the weekend: Friday's run → Monday.
        assert_eq!(next_occurrence("0 0 7 * * Mon-Fri", local(2026, 8, 21, 7, 0, 30)).unwrap(), Some(local(2026, 8, 24, 7, 0, 0)));
        assert!(next_occurrence("not a cron", 0).is_err());
    }

    /// The old scheduler's whole contract, on the engine: due fires once
    /// and only once; a disabled job's timer goes; a rescheduled job's
    /// timer is replaced; a fire while the last one runs is skipped.
    #[test]
    fn a_schedule_holds_one_timer_fires_once_when_due_and_re_arms_from_the_consumed_one() {
        let s = store();
        let created = local(2026, 8, 23, 8, 0, 0);
        let j = job(&s, "briefing", "0 0 9 * * *", created);
        let t0 = created + 60;

        let r = tick(&s, t0, &idle, &no_steer);
        assert_eq!(r.armed, 1);
        let pending = s.engine_pending_timers("binding").unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].due_at, Some(local(2026, 8, 23, 9, 0, 0)));
        assert_eq!(tick(&s, t0 + 5, &idle, &no_steer).armed, 0, "one timer per job, not one per tick");

        // 8:59: nothing. 9:00:05: fires once, queues one task run.
        assert_eq!(tick(&s, local(2026, 8, 23, 8, 59, 0), &idle, &no_steer).fired, 0);
        let r = tick(&s, local(2026, 8, 23, 9, 0, 5), &idle, &no_steer);
        assert_eq!(r.fired, 1);
        let runs = s.engine_queued_runs_of_kind("task", 10).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].external_ref.as_deref(), Some("cron:1"));
        assert!(runs[0].inputs.as_deref().unwrap().contains("\"job_id\":1"));

        // The next tick arms tomorrow's from the consumed occurrence.
        let r = tick(&s, local(2026, 8, 23, 9, 0, 10), &idle, &no_steer);
        assert_eq!(r.armed, 1);
        assert_eq!(r.fired, 0);
        assert_eq!(s.engine_pending_timers("binding").unwrap()[0].due_at, Some(local(2026, 8, 24, 9, 0, 0)));

        // Tomorrow 9:00 while yesterday's fire is still running: skipped, noted.
        s.engine_set_run_state(&runs[0].id, "running", local(2026, 8, 23, 9, 0, 6), None).unwrap();
        let r = tick(&s, local(2026, 8, 24, 9, 0, 1), &idle, &no_steer);
        assert_eq!(r.skipped, 1);
        assert_eq!(r.fired, 0);
        assert_eq!(s.engine_queued_runs_of_kind("task", 10).unwrap().len(), 0);

        // Rescheduled: the pending timer is replaced by one on the new schedule.
        tick(&s, local(2026, 8, 24, 9, 0, 6), &idle, &no_steer);
        s.upsert_cron_job("briefing", "0 30 9 * * *", "echo hi", "shell", None, None, None, true, None, None).unwrap();
        let r = tick(&s, local(2026, 8, 24, 9, 0, 11), &idle, &no_steer);
        assert_eq!(r.armed, 1);
        let pending = s.engine_pending_timers("binding").unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].due_at, Some(local(2026, 8, 24, 9, 30, 0)));

        // Disabled: the timer goes and nothing fires.
        s.set_cron_job_enabled(j.id, false).unwrap();
        let r = tick(&s, local(2026, 8, 24, 9, 30, 1), &idle, &no_steer);
        assert_eq!(r.fired, 0);
        assert!(s.engine_pending_timers("binding").unwrap().is_empty());
    }

    /// A job that slept through many occurrences (laptop closed) fires at
    /// most one late, and only inside the catch-up window — never a storm.
    #[test]
    fn a_missed_schedule_is_caught_up_once_inside_the_window_and_skipped_beyond_it() {
        let s = store();
        let created = local(2026, 6, 1, 8, 0, 0);
        job(&s, "hourly", "0 0 * * * *", created);

        // Months later: the floor is clamped to the window, so the first
        // timer is 9:00 today — 20 minutes ago, inside the window — and it
        // fires late in the same tick. June's occurrences are not replayed.
        let t = local(2026, 8, 23, 9, 20, 0);
        let r = tick(&s, t, &idle, &no_steer);
        assert_eq!((r.armed, r.fired, r.skipped), (1, 1, 0));
        let fires = s.engine_queued_runs_of_kind("task", 10).unwrap();
        assert_eq!(fires.len(), 1, "one late fire, not a storm");
        s.engine_set_run_state(&fires[0].id, "done", t + 1, None).unwrap();
        let r = tick(&s, t + 5, &idle, &no_steer);
        assert_eq!((r.armed, r.fired), (1, 0));
        assert_eq!(s.engine_pending_timers("binding").unwrap()[0].due_at, Some(local(2026, 8, 23, 10, 0, 0)));

        // Process down from 10:00 to 15:10: the 10:00 timer is far past the
        // window when claimed — skipped. 15:00 is inside the window, so it
        // fires once, late; 11:00–14:00 are never replayed. Then 16:00.
        let r = tick(&s, local(2026, 8, 23, 15, 10, 0), &idle, &no_steer);
        assert_eq!((r.skipped, r.fired), (1, 0));
        let r = tick(&s, local(2026, 8, 23, 15, 10, 5), &idle, &no_steer);
        assert_eq!((r.armed, r.fired, r.skipped), (1, 1, 0), "15:00 catches up once");
        let r = tick(&s, local(2026, 8, 23, 15, 10, 10), &idle, &no_steer);
        assert_eq!((r.armed, r.fired), (1, 0));
        assert_eq!(s.engine_pending_timers("binding").unwrap()[0].due_at, Some(local(2026, 8, 23, 16, 0, 0)));
        assert_eq!(s.engine_count_runs_for_ref("cron:1").unwrap(), 2, "9:00 and 15:00 ran; nothing else");
    }

    /// A heartbeat timer fires ONE run of kind `heartbeat` on the heartbeat
    /// lane; while that run is live, the next timer is skipped, not stacked.
    #[test]
    fn a_heartbeat_timer_fires_one_run_and_never_stacks_on_a_live_one() {
        let s = store();
        let target = "heartbeat:agent:ic";
        s.engine_enqueue_event(&NewEvent { kind: "timer", target_type: "entity", target_id: target, idem_key: "hb-1", due_at: Some(1_000), schedule: Some("1800"), ..Default::default() }).unwrap();
        let r = tick(&s, 1_000, &idle, &no_steer);
        assert_eq!((r.fired, r.skipped), (1, 0));
        let beats = s.engine_queued_runs_of_kind("heartbeat", 10).unwrap();
        assert_eq!(beats.len(), 1);
        assert_eq!(beats[0].lane, "heartbeat");
        assert_eq!(beats[0].agent_id, "ic");
        assert_eq!(beats[0].session_key, "heartbeat-agent-ic");
        assert_eq!(beats[0].external_ref.as_deref(), Some(target));
        assert_eq!(s.engine_last_timer_floor("entity", target).unwrap(), Some(1_000), "the consumed timer is the floor for the next");

        s.engine_set_run_state(&beats[0].id, "running", 1_001, None).unwrap();
        s.engine_enqueue_event(&NewEvent { kind: "timer", target_type: "entity", target_id: target, idem_key: "hb-2", due_at: Some(2_800), schedule: Some("1800"), ..Default::default() }).unwrap();
        let r = tick(&s, 2_800, &idle, &no_steer);
        assert_eq!((r.fired, r.skipped), (0, 1));
        assert_eq!(s.engine_queued_runs_of_kind("heartbeat", 10).unwrap().len(), 0);
    }

    // ── approvals, parked turns, settling, and the arming helper ────────

    /// The owner's answer is an event aimed at the run's live wait. One
    /// answer per wait generation: a second click is a duplicate, and an
    /// answer for a run that is not waiting wakes nothing.
    #[test]
    fn an_approval_event_resumes_the_parked_run_once_and_a_second_answer_is_a_duplicate() {
        let s = store();
        s.create_workflow_run("wf-1", "agent:a", "watch", Some("intake:x"), Some("{}"), Some("agent:a:workflow:wf-1"), Some("{}")).unwrap();
        s.create_workflow_suspension("wf-1", "a", "intake", "act-2", "", None, "[msgs]", "{}", "crm.write", "Create invoice").unwrap();
        s.update_workflow_run("wf-1", Some("awaiting_approval"), None, None, None, None).unwrap();
        let wait_id = s.engine_get_run("wf-1").unwrap().unwrap().current_wait_id.unwrap();
        let answer = |approved: bool| NewEvent {
            kind: "approval",
            target_type: "run",
            target_id: "approval:wf-1",
            payload: if approved { r#"{"approved":true}"# } else { r#"{"approved":false}"# },
            channel: "owner",
            idem_key: "approval:wf-1:1",
            durable: true,
            ..Default::default()
        };
        assert!(matches!(s.engine_enqueue_event(&answer(true)).unwrap(), db::Enqueued::Inserted(_)));
        assert_eq!(s.engine_enqueue_event(&answer(false)).unwrap(), db::Enqueued::Duplicate, "the same wait answered twice is one answer");

        let r = tick(&s, 100, &idle, &no_steer);
        assert_eq!((r.resumed, r.unrouted), (1, 0));
        let run = s.engine_get_run("wf-1").unwrap().unwrap();
        assert_eq!(run.state, "queued", "re-queued for the loop to resume at the parked call");
        assert!(run.woken_by().is_some(), "the answer is on the run");
        assert!(s.get_workflow_suspension("wf-1").unwrap().is_some(), "the parked state is still readable for the resume");
        assert!(s.list_workflow_suspensions().unwrap().is_empty(), "but nothing is pending");
        let _ = wait_id;

        // A later answer for a run that is no longer waiting reaches no wait.
        s.engine_enqueue_event(&NewEvent { idem_key: "approval:wf-1:late", ..answer(true) }).unwrap();
        let r = tick(&s, 200, &idle, &no_steer);
        assert_eq!((r.resumed, r.unrouted), (0, 1));
        assert_eq!(s.engine_get_run("wf-1").unwrap().unwrap().state, "queued", "untouched");
    }

    /// A turn parked on an approval is still the case's live turn: a signal
    /// that arrives meanwhile waits for it, and never starts a second turn.
    #[test]
    fn a_signal_for_a_case_whose_turn_is_parked_on_approval_waits_its_turn() {
        let s = store();
        s.engine_create_run(&NewRun { id: "case-1", kind: "case", session_key: "agent:a:case:k", agent_id: "a", lane: "main", ..Default::default() }).unwrap();
        s.engine_declare_wait("case-1", &NewWait { action: "trigger_child", on_kind: "signal", key: "email:x", deadline: None, reason: "first contact", ..Default::default() }, 100).unwrap();
        s.engine_create_run(&NewRun { id: "turn-1", kind: "workflow", session_key: "agent:a:workflow:turn-1", agent_id: "a", lane: "main", parent_run_id: Some("case-1"), inputs: Some(r#"{"_case":{"key_type":"email","key_value":"x"}}"#), ..Default::default() }).unwrap();
        s.engine_declare_wait("turn-1", &NewWait { action: "resume", on_kind: "approval", key: "approval:turn-1", parked: Some("{}"), reason: "Create invoice", ..Default::default() }, 150).unwrap();
        s.engine_enqueue_event(&NewEvent { kind: "signal", target_type: "run", target_id: "email:x", payload: "again", idem_key: "s2", durable: true, ..Default::default() }).unwrap();
        let r = tick(&s, 200, &idle, &no_steer);
        assert_eq!((r.claimed, r.children_started, r.steered), (1, 0, 0), "deferred: the parked turn is the live one");
        assert_eq!(s.engine_live_child("case-1").unwrap().unwrap().id, "turn-1");
    }

    /// A turn is the workflow run itself. When the workflow ends it, the
    /// next tick finds it unsettled and hands the case its declared wait;
    /// once settled it is never found again. A cancelled turn settles as a
    /// failure with the default wait. A DAG child is not a turn.
    #[test]
    fn a_turn_the_workflow_ended_is_settled_once_by_the_next_tick() {
        let s = store();
        let b = binding();
        let payload = serde_json::json!({"email": "a@b.c"});
        let Routed::Opened { case_id } = signal_or_open(&s, &b, "email", "a@b.c", &payload, "webhook", "s1", 1_000).unwrap() else { panic!() };
        let turn = s.engine_queued_runs_of_kind("workflow", 1).unwrap().remove(0);
        assert_eq!(turn.parent_run_id.as_deref(), Some(case_id.as_str()));
        assert!(s.engine_unsettled_turns(10).unwrap().is_empty(), "a queued turn is not finished");

        // The workflow ends it, as the manager does: result, then done.
        s.engine_set_run_state(&turn.id, "running", 1_001, None).unwrap();
        s.complete_workflow_run(&turn.id, "completed", 10, None, None, Some(r#"{"outcome":"sent day-1","wait":{"deadline":"2d","reason":"follow up Wednesday"}}"#)).unwrap();
        let unsettled = s.engine_unsettled_turns(10).unwrap();
        assert_eq!(unsettled.len(), 1);
        let t = &unsettled[0];
        settle_turn(&s, t, t.result.as_deref(), false, 2_000).unwrap();
        assert!(s.engine_unsettled_turns(10).unwrap().is_empty(), "settled once");
        let case = s.engine_get_run(&case_id).unwrap().unwrap();
        assert_eq!(case.state, "waiting");
        assert_eq!(s.engine_get_wait(case.current_wait_id.unwrap()).unwrap().unwrap().deadline, Some(2_000 + 2 * 86_400));
        assert_eq!(s.get_workflow_run(&turn.id).unwrap().unwrap().status, "completed", "the turn keeps the workflow's own outcome");

        // The deadline starts the next turn; the owner cancels it.
        let r = tick(&s, 2_000 + 2 * 86_400, &idle, &no_steer);
        assert_eq!(r.children_started, 1);
        let turn2 = s.engine_queued_runs_of_kind("workflow", 1).unwrap().remove(0);
        s.complete_workflow_run(&turn2.id, "cancelled", 0, Some("owner cancelled"), None, None).unwrap();
        let unsettled = s.engine_unsettled_turns(10).unwrap();
        assert_eq!(unsettled.len(), 1);
        settle_turn(&s, &unsettled[0], Some("owner cancelled"), true, 5_000).unwrap();
        assert_eq!(s.get_workflow_run(&turn2.id).unwrap().unwrap().status, "cancelled", "settling does not rewrite the outcome");
        let case = s.engine_get_run(&case_id).unwrap().unwrap();
        assert_eq!(s.engine_get_wait(case.current_wait_id.unwrap()).unwrap().unwrap().deadline, Some(5_000 + 60), "a failed turn retries in a minute");
        assert!(s.engine_events_for("run", &case_id, 50).unwrap().iter().any(|e| e.kind == "turn_failed"));

        // A DAG child that finishes is not a turn.
        s.create_pending_task("dag-1", "dag", "k", None, "plan", None, None, None, 0, None).unwrap();
        s.create_pending_task("dag-1-a", "subagent", "k", None, "do", None, None, None, 0, Some("dag-1")).unwrap();
        s.update_task_completed("dag-1-a", Some("ok")).unwrap();
        assert!(s.engine_unsettled_turns(10).unwrap().is_empty());
    }

    /// Failed turns retry on the policy's clock — one minute, then two, then
    /// four — and after three in a row the case waits its default; the first
    /// success resets the streak. A turn that declared its own wait is not
    /// second-guessed.
    #[test]
    fn failed_turns_retry_with_backoff_then_give_up_and_a_success_resets_the_streak() {
        let s = store();
        let b = binding();
        let payload = serde_json::json!({"email": "a@b.c"});
        let Routed::Opened { case_id } = signal_or_open(&s, &b, "email", "a@b.c", &payload, "webhook", "s1", 1_000).unwrap() else { panic!() };
        let mut t = 1_000;
        let fail_turn = |s: &Store, t: i64| {
            let turn = s.engine_queued_runs_of_kind("workflow", 1).unwrap().remove(0);
            s.complete_workflow_run(&turn.id, "failed", 0, Some("provider down"), None, None).unwrap();
            let turn = s.engine_unsettled_turns(1).unwrap().remove(0);
            settle_turn(s, &turn, Some("provider down"), true, t).unwrap();
            let case = s.engine_get_run(&case_id).unwrap().unwrap();
            let wait = s.engine_get_wait(case.current_wait_id.unwrap()).unwrap().unwrap();
            (wait.deadline.unwrap() - t, wait.reason)
        };
        for (n, expected) in [(1, 60), (2, 120), (3, 240)] {
            let (delay, reason) = fail_turn(&s, t);
            assert_eq!(delay, expected, "retry {n}");
            assert!(reason.starts_with(&format!("retry {n} of 3")), "{reason}");
            t += delay;
            let r = tick(&s, t, &idle, &no_steer);
            assert_eq!(r.children_started, 1, "the retry timer starts the next turn");
        }
        let (delay, reason) = fail_turn(&s, t);
        assert_eq!(delay, 3 * 86_400, "fourth failure: the binding's default wait");
        assert!(reason.starts_with("gave up after 3 retries"), "{reason}");
        t += delay;
        assert_eq!(tick(&s, t, &idle, &no_steer).children_started, 1);

        // A success resets the streak; the next failure retries at one minute again.
        let turn = s.engine_queued_runs_of_kind("workflow", 1).unwrap().remove(0);
        s.complete_workflow_run(&turn.id, "completed", 5, None, None, Some(r#"{"wait":{"deadline":"5m","reason":"sent"}}"#)).unwrap();
        let turn = s.engine_unsettled_turns(1).unwrap().remove(0);
        settle_turn(&s, &turn, turn.result.as_deref(), false, t).unwrap();
        t += 300;
        assert_eq!(tick(&s, t, &idle, &no_steer).children_started, 1);
        let (delay, _) = fail_turn(&s, t);
        assert_eq!(delay, 60, "streak reset by the success");
    }

    /// When the engine gives up, the employee's autonomy decides who hears
    /// it: an autonomous employee gets the give-up as a turn on the case,
    /// right away; anyone else gets a card in the owner's Inbox and the case
    /// waits its default. Either way the case history says what happened.
    #[test]
    fn a_give_up_reaches_an_autonomous_employee_as_a_turn_and_the_owner_otherwise() {
        let s = store();
        let user = s.ensure_local_user_id().unwrap();
        let fail_four = |s: &Store, b: &CaseBinding<'_>, idem: &str, key: &str| -> (String, i64) {
            let payload = serde_json::json!({"email": key});
            let Routed::Opened { case_id } = signal_or_open(s, b, "email", key, &payload, "webhook", idem, 1_000).unwrap() else { panic!() };
            let mut t = 1_000;
            for _ in 0..4 {
                let turn = s.engine_queued_runs_of_kind("workflow", 1).unwrap().remove(0);
                s.complete_workflow_run(&turn.id, "failed", 0, Some("provider down"), None, None).unwrap();
                let turn = s.engine_unsettled_turns(1).unwrap().remove(0);
                settle_turn(s, &turn, Some("provider down"), true, t).unwrap();
                let case = s.engine_get_run(&case_id).unwrap().unwrap();
                let wait = s.engine_get_wait(case.current_wait_id.unwrap()).unwrap().unwrap();
                if wait.reason.starts_with("gave up") {
                    return (case_id, t);
                }
                t = wait.deadline.unwrap();
                assert_eq!(tick(s, t, &idle, &no_steer).children_started, 1);
            }
            panic!("never gave up");
        };

        // Autonomous employee: the give-up becomes the case's next turn now.
        s.upsert_entity_config("agent", "ic", &serde_json::json!({"operationPolicy": {"default": "always"}})).unwrap();
        let b = binding();
        let (case_id, t) = fail_four(&s, &b, "s-auto", "auto@x.com");
        assert!(s.engine_events_for("run", &case_id, 50).unwrap().iter().any(|e| e.kind == "needs_attention"), "recorded on the case");
        let r = tick(&s, t + 1, &idle, &no_steer);
        assert_eq!(r.children_started, 1, "the employee gets the give-up as a turn, not in three days");
        let turn = s.engine_queued_runs_of_kind("workflow", 1).unwrap().remove(0);
        assert!(turn.inputs.as_deref().unwrap().contains("needs_attention"), "the turn reads what broke");
        assert!(s.get_notification(&format!("attention:{}", turn.id), &user).unwrap().is_none());

        // Owner-consulted employee: a card, and the case waits its default.
        // (Its own store: the autonomous case above left a queued turn behind.)
        let s = store();
        let user = s.ensure_local_user_id().unwrap();
        let mut owner_b = binding();
        owner_b.agent_id = "careful";
        let (case_id2, t2) = fail_four(&s, &owner_b, "s-owner", "owner@x.com");
        assert_eq!(tick(&s, t2 + 1, &idle, &no_steer).children_started, 0, "nothing starts on its own");
        let last_turn = s.engine_events_for("run", &case_id2, 50).unwrap().into_iter().filter(|e| e.kind == "needs_attention").last().unwrap();
        let card = s.get_notification(&format!("attention:{}", last_turn.r#ref), &user).unwrap().expect("an Inbox card for the owner");
        assert_eq!(card.notification_type, "needs_attention");
        assert_eq!(card.agent_id.as_deref(), Some("careful"));
    }

    /// A poisoned event is routed to whoever owns what it was aimed at: for
    /// a signal on a case key, the case's employee — and an autonomous one
    /// takes it as a turn.
    #[test]
    fn a_poisoned_event_reaches_the_case_it_was_aimed_at() {
        let s = store();
        s.upsert_entity_config("agent", "a", &serde_json::json!({"operationPolicy": {"default": "always"}})).unwrap();
        s.engine_create_run(&NewRun { id: "case-1", kind: "case", session_key: "agent:a:case:k", agent_id: "a", lane: "main", inputs: Some(r#"{"_case":{"key_type":"email","key_value":"x"}}"#), ..Default::default() }).unwrap();
        s.engine_bind_key("case-1", "email", "x").unwrap();
        s.engine_declare_wait("case-1", &NewWait { action: "trigger_child", on_kind: "signal", key: "email:x", deadline: None, reason: "waiting", ..Default::default() }, 100).unwrap();
        // A signal that is claimed five times and never completed.
        s.engine_enqueue_event(&NewEvent { kind: "signal", target_type: "run", target_id: "email:x", payload: "cursed", idem_key: "cursed", durable: true, ..Default::default() }).unwrap();
        let mut t = 200;
        for _ in 0..db::EVENT_MAX_ATTEMPTS {
            assert_eq!(s.engine_claim_events(t, 10).unwrap().0.len(), 1);
            t += db::EVENT_LEASE_SECS + 1;
        }
        let r = tick(&s, t, &idle, &no_steer);
        assert_eq!(r.poisoned, 1);
        assert!(s.engine_events_for("run", "case-1", 50).unwrap().iter().any(|e| e.kind == "needs_attention" && e.payload.contains("email:x")));
        // The attention signal is delivered in the same tick or the next.
        let started = r.children_started + tick(&s, t + 5, &idle, &no_steer).children_started;
        assert_eq!(started, 1, "the autonomous employee takes it as a turn");
    }

    /// Money that was attempted and never confirmed is never retried by the
    /// engine, whatever the employee's autonomy: it stays pending and the
    /// owner is told once. A messaging effect in the same state is not
    /// escalated — its class is loose.
    #[test]
    fn an_unconfirmed_charge_is_never_retried_and_the_owner_is_told_once() {
        let s = store();
        let user = s.ensure_local_user_id().unwrap();
        s.upsert_entity_config("agent", "a", &serde_json::json!({"operationPolicy": {"default": "always"}})).unwrap();
        s.engine_create_run(&NewRun { id: "wf-1", kind: "workflow", session_key: "agent:a:workflow:wf-1", agent_id: "a", lane: "main", ..Default::default() }).unwrap();
        let charge = s.engine_effect_pending("wf-1", "financial", "charge:inv-1042", "stripe", "pi_1042").unwrap();
        let note = s.engine_effect_pending("wf-1", "messaging", "email:inv-1042", "smtp", "").unwrap();
        s.engine_effect_attempted(charge).unwrap();
        s.engine_effect_attempted(note).unwrap();

        let r = tick(&s, 100, &idle, &no_steer);
        assert_eq!(r.pending_effects, 2);
        let card = s.get_notification(&format!("attention:effect:{charge}"), &user).unwrap().expect("the owner is told about the charge");
        assert!(card.body.as_deref().unwrap().contains("pi_1042") || card.body.as_deref().unwrap().contains("charge:inv-1042"), "with the key to reconcile by");
        assert!(s.get_notification(&format!("attention:effect:{note}"), &user).unwrap().is_none(), "a message is not money");

        tick(&s, 200, &idle, &no_steer);
        assert_eq!(s.engine_get_effect(charge).unwrap().unwrap().attempts, 1, "never retried");
        assert_eq!(s.engine_get_effect(charge).unwrap().unwrap().state, "pending", "still pending until confirmed by the provider");
        assert_eq!(s.engine_events_for("run", "wf-1", 50).unwrap().iter().filter(|e| e.kind == "needs_attention").count(), 1, "told once");
    }

    /// The timeout worklists: a turn running since before the cutoff, or
    /// queued since before it, and nothing else.
    #[test]
    fn timeout_worklists_find_only_turns_past_the_cutoff() {
        let s = store();
        s.engine_create_run(&NewRun { id: "case-1", kind: "case", session_key: "k", agent_id: "a", lane: "main", ..Default::default() }).unwrap();
        for (id, state, at) in [("old-run", "running", 100), ("new-run", "running", 900), ("old-q", "queued", 100), ("new-q", "queued", 900)] {
            s.engine_create_run(&NewRun { id, kind: "workflow", session_key: id, agent_id: "a", lane: "main", parent_run_id: Some("case-1"), ..Default::default() }).unwrap();
            s.conn_exec_for_test(&format!("UPDATE engine_runs SET created_at = {at} WHERE id = '{id}'"));
            if state == "running" {
                s.engine_set_run_state(id, "running", at, None).unwrap();
            }
        }
        // A plain workflow run (no case) is never a turn.
        s.engine_create_run(&NewRun { id: "plain", kind: "workflow", session_key: "p", agent_id: "a", lane: "main", ..Default::default() }).unwrap();
        s.engine_set_run_state("plain", "running", 100, None).unwrap();
        let ids = |v: Vec<EngineRun>| v.into_iter().map(|r| r.id).collect::<Vec<_>>();
        assert_eq!(ids(s.engine_turns_in_state_since("running", 500).unwrap()), vec!["old-run"]);
        assert_eq!(ids(s.engine_turns_in_state_since("queued", 500).unwrap()), vec!["old-q"]);
        assert_eq!(s.engine_turns_in_state_since("running", 1_000).unwrap().len(), 2);
    }

    /// A turn whose case already closed is recorded and declares nothing:
    /// the case stays closed and its key stays released.
    #[test]
    fn a_turn_of_a_closed_case_is_recorded_but_declares_no_wait() {
        let s = store();
        let b = binding();
        let payload = serde_json::json!({"email": "a@b.c"});
        let Routed::Opened { case_id } = signal_or_open(&s, &b, "email", "a@b.c", &payload, "webhook", "s1", 1_000).unwrap() else { panic!() };
        let turn = s.engine_queued_runs_of_kind("workflow", 1).unwrap().remove(0);
        // The owner closes the case by hand while the turn runs.
        s.engine_close_run(&case_id, "done", 1_500).unwrap();
        s.engine_set_run_state(&turn.id, "running", 1_001, None).unwrap();
        s.complete_workflow_run(&turn.id, "completed", 1, None, None, Some(r#"{"wait":{"deadline":"1d"}}"#)).unwrap();
        let t = s.engine_unsettled_turns(10).unwrap().remove(0);
        settle_turn(&s, &t, t.result.as_deref(), false, 2_000).unwrap();
        let case = s.engine_get_run(&case_id).unwrap().unwrap();
        assert_eq!(case.state, "done");
        assert!(case.current_wait_id.is_none() || s.engine_get_wait(case.current_wait_id.unwrap()).unwrap().unwrap().deadline != Some(2_000 + 86_400), "no new wait");
        assert!(s.engine_run_for_key("email", "a@b.c").unwrap().is_none(), "key stays released");
        assert!(s.engine_unsettled_turns(10).unwrap().is_empty(), "recorded once");
    }

    /// The ONE reconciliation: unchanged timers are held, a changed config
    /// replaces its timer, an unwanted target loses its timer, a target with
    /// nothing due is not armed, and the floor is the last consumed one.
    #[test]
    fn reconcile_timers_holds_replaces_drops_and_arms_from_the_consumed_floor() {
        let s = store();
        let want = |target: &str, schedule: &str, step: i64| Wanted {
            target: target.to_string(),
            schedule: schedule.to_string(),
            due: Box::new(move |floor| if step == 0 { None } else { Some(floor.unwrap_or(1_000) + step) }),
        };
        let wanted = vec![want("x:a", "1", 10), want("x:b", "2", 20), want("x:never", "0", 0)];
        assert_eq!(reconcile_timers(&s, 100, "entity", "x:", &wanted), 2, "two armed; nothing due for the third");
        assert_eq!(reconcile_timers(&s, 105, "entity", "x:", &wanted), 0, "held");
        let pending = s.engine_pending_timers("entity").unwrap();
        assert_eq!(pending.iter().map(|e| e.due_at.unwrap()).collect::<Vec<_>>(), vec![1_010, 1_020]);

        // b's config changes: its timer is replaced; a is untouched.
        let wanted = vec![want("x:a", "1", 10), want("x:b", "3", 30)];
        assert_eq!(reconcile_timers(&s, 110, "entity", "x:", &wanted), 1);
        let pending = s.engine_pending_timers("entity").unwrap();
        assert_eq!(pending.len(), 2);
        let b = pending.iter().find(|e| e.target_id == "x:b").unwrap();
        assert_eq!(b.schedule.as_deref(), Some("3"));
        assert_eq!(b.due_at, Some(110 + 30), "a replaced timer floors at the moment it was replaced, never at its old future due");

        // a is no longer wanted: its timer goes. A timer under another prefix is not touched.
        s.engine_enqueue_event(&NewEvent { kind: "timer", target_type: "entity", target_id: "y:other", idem_key: "y", due_at: Some(9_999), ..Default::default() }).unwrap();
        let wanted = vec![want("x:b", "3", 30)];
        assert_eq!(reconcile_timers(&s, 120, "entity", "x:", &wanted), 0);
        let pending = s.engine_pending_timers("entity").unwrap();
        assert_eq!(pending.iter().map(|e| e.target_id.as_str()).collect::<Vec<_>>(), vec!["x:b", "y:other"]);

        // b's timer is consumed at its due moment: the next is armed from it.
        let b_timer = pending.iter().find(|e| e.target_id == "x:b").unwrap().id;
        s.engine_complete_event(b_timer, 145).unwrap();
        assert_eq!(reconcile_timers(&s, 146, "entity", "x:", &wanted), 1);
        let next = s.engine_pending_timers("entity").unwrap().into_iter().find(|e| e.target_id == "x:b").unwrap();
        assert_eq!(next.due_at, Some(140 + 30), "from the consumed due moment, not from when it was delivered");
    }

    #[test]
    fn parse_wait_reads_the_last_declaration_and_relative_or_absolute_deadlines() {
        let t = 1_000;
        assert_eq!(parse_wait("no json here", t), None);
        let w = parse_wait(r#"{"wait":{"on":"signal","deadline":"12h","reason":"r"}}"#, t).unwrap();
        assert_eq!(w.deadline, Some(t + 12 * 3600));
        assert_eq!(w.on_kind, "signal");
        let w = parse_wait(r#"prose {"wait":{"deadline":"2026-09-09T09:00:00-06:00"}} trailing"#, t).unwrap();
        let expected = chrono::DateTime::parse_from_rfc3339("2026-09-09T09:00:00-06:00").unwrap().timestamp();
        assert_eq!(w.deadline, Some(expected));
        let w = parse_wait(r#"{"wait":{"deadline":"1d"}} then later {"state":"booked","wait":{}}"#, t).unwrap();
        assert_eq!(w.state.as_deref(), Some("booked"));
        assert_eq!(w.deadline, None);
        assert_eq!(relative_secs("1d12h"), Some(129_600));
        assert_eq!(relative_secs("soon"), None);
    }
}
