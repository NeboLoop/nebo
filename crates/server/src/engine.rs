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
use workflow::cases::{settle_turn, start_child};

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
            Ok(false) => warn!(run = %run.id, "engine: run interrupted twice — poisoned, owner to decide"),
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
    report.poisoned = poisoned;
    if poisoned > 0 {
        warn!(poisoned, "engine: events poisoned after repeated delivery failure");
    }
    for event in &events {
        deliver(store, event, t, busy, steer, &mut report);
    }

    match store.engine_pending_effects() {
        Ok(pending) => {
            report.pending_effects = pending.len();
            // Reconciliation needs the providers behind the effect classes;
            // it lands with them. Until then a pending effect is visible in
            // the log every tick rather than silently assumed done.
            for e in pending.iter().take(5) {
                info!(effect = e.id, run = %e.run_id, class = %e.class, attempts = e.attempts, "engine: effect pending reconciliation");
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
/// that is not accepting steering still counts as live — a second turn
/// never starts beside it.
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
    let handed = match (child.state.as_str(), child.external_ref.as_deref()) {
        ("running", Some(wf_run_id)) => {
            // The session the turn's workflow run was created under is the
            // one the runner marks busy; read it from the run row rather
            // than recomputing it.
            let session_key = store
                .get_workflow_run(wf_run_id)
                .ok()
                .flatten()
                .and_then(|wf| wf.session_key)
                .unwrap_or_else(|| tools::workflow_session_key(&case.agent_id, wf_run_id));
            if busy(&session_key) {
                steer(&session_key, event);
                true
            } else {
                return LiveTurn::Deferred;
            }
        }
        ("running", None) => return LiveTurn::Deferred,
        ("queued", _) => match store.engine_append_pending_signal(&child.id, &event.payload) {
            Ok(()) => true,
            Err(e) => {
                warn!(child = %child.id, error = %e, "engine: could not append signal to the queued turn");
                return LiveTurn::Deferred;
            }
        },
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
fn schedule_floor(store: &Store, job: &CronJob, t: i64) -> i64 {
    let consumed = store.engine_last_timer_floor("binding", &cron_target(job)).ok().flatten();
    let created = job.created_at.as_deref().and_then(|s| {
        chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
            .ok()
            .map(|d| d.and_utc().timestamp())
    });
    consumed.or(created).unwrap_or(t).max(t - CATCH_UP_SECS)
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
    let pending = match store.engine_pending_timers("binding") {
        Ok(p) => p,
        Err(e) => {
            warn!(error = %e, "engine: could not read pending timers");
            return 0;
        }
    };
    let mut held: HashSet<String> = HashSet::new();
    for timer in &pending {
        let job = jobs.iter().find(|j| cron_target(j) == timer.target_id);
        match job {
            Some(j) if timer.schedule.as_deref() == Some(j.schedule.as_str()) => {
                held.insert(timer.target_id.clone());
            }
            _ => {
                if let Err(e) = store.engine_supersede_event(timer.id, t, "superseded: schedule changed or job gone") {
                    warn!(timer = timer.id, error = %e, "engine: could not drop a stale schedule timer");
                }
            }
        }
    }
    let mut armed = 0;
    for job in &jobs {
        let target = cron_target(job);
        if held.contains(&target) {
            continue;
        }
        let due = match next_occurrence(&job.schedule, schedule_floor(store, job, t)) {
            Ok(Some(due)) => due,
            Ok(None) => continue,
            Err(e) => {
                // Once per job per process, not once per tick.
                static WARNED: std::sync::Mutex<Vec<i64>> = std::sync::Mutex::new(Vec::new());
                let mut warned = WARNED.lock().unwrap_or_else(|p| p.into_inner());
                if !warned.contains(&job.id) {
                    warned.push(job.id);
                    warn!(job = job.name.as_str(), schedule = %job.schedule, error = %e, "invalid cron expression; this job will not fire");
                }
                continue;
            }
        };
        let idem = format!("cron:{}:{due}:{t}", job.id);
        match store.engine_enqueue_event(&NewEvent {
            kind: "timer",
            target_type: "binding",
            target_id: &target,
            idem_key: &idem,
            due_at: Some(due),
            schedule: Some(&job.schedule),
            ..Default::default()
        }) {
            Ok(db::Enqueued::Inserted(_)) => armed += 1,
            Ok(db::Enqueued::Duplicate) => {}
            Err(e) => warn!(job = job.name.as_str(), error = %e, "engine: could not arm schedule"),
        }
    }
    armed
}

/// A schedule's timer came due. Skip (and say why) when the job is gone or
/// disabled, when its last fire is still running, or when the occurrence
/// was missed by more than the catch-up window; otherwise queue ONE run.
/// The next occurrence is armed on the following tick from this consumed one.
fn fire_schedule(store: &Store, event: &EngineEvent, t: i64, report: &mut TickReport) {
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

/// Record a fire's outcome on its run and tell whoever is listening: the
/// desktop for scheduled fires that have no other surface, the UI for a
/// run-now, and always the desktop on failure.
fn settle_task(state: &AppState, run: &EngineRun, job: &CronJob, success: bool, output: String, err: Option<String>) {
    let t = now();
    let store = &state.store;
    if !output.is_empty() {
        let _ = store.engine_set_run_result(&run.id, &output, None);
    }
    let _ = store.engine_set_run_state(&run.id, if success { "done" } else { "failed" }, t, err.as_deref());

    let inputs: serde_json::Value = run.inputs.as_deref().and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default();
    let manual = inputs["manual"].as_bool().unwrap_or(false);
    if manual {
        state.hub.broadcast(
            "task_complete",
            serde_json::json!({
                "task": job.name,
                "success": success,
                "output": crate::truncate_str(if success { &output } else { err.as_deref().unwrap_or(&output) }, 500),
            }),
        );
    }
    // A channel-bound job already delivered its response where the owner
    // reads it; a desktop popup on top would be noise. Failures always
    // surface, because the channel delivery itself may be what failed.
    let channel_bound = job.agent_id.as_deref().is_some_and(|s| !s.is_empty())
        && job.channel_ctx_json.as_deref().is_some_and(|s| !s.is_empty());
    if success {
        info!(job = job.name.as_str(), "task completed");
        if !channel_bound && !manual {
            notify_crate::send("Nebo", &format!("{} completed", job.name));
        }
    } else {
        let e = err.as_deref().unwrap_or("unknown");
        warn!(job = job.name.as_str(), error = e, "task failed");
        notify_crate::send("Nebo", &format!("{} failed: {}", job.name, e));
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

    let tasks = store.engine_queued_runs_of_kind("task", TURNS_PER_TICK).unwrap_or_default();
    for (i, run) in tasks.into_iter().enumerate() {
        let inputs: serde_json::Value = run.inputs.as_deref().and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default();
        let job = inputs["job_id"].as_i64().and_then(|id| store.get_cron_job(id).ok().flatten());
        let Some(job) = job else {
            let _ = store.engine_set_run_state(&run.id, "failed", t, Some("scheduled job no longer exists"));
            continue;
        };
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
            let (success, output, err) = crate::scheduler::execute_job(&state, &job).await;
            settle_task(&state, &run, &job, success, output, err);
        });
    }

    let queued = store.engine_queued_runs_of_kind("case_turn", TURNS_PER_TICK).unwrap_or_default();
    for run in queued {
        let Some(definition) = run.definition.clone() else {
            let _ = store.engine_set_run_state(&run.id, "failed", t, Some("case turn has no definition"));
            continue;
        };
        let inputs: serde_json::Value = run.inputs.as_deref().and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default();
        let binding = inputs["_case"]["binding"].as_str().map(str::to_string);
        if let Err(e) = store.engine_set_run_state(&run.id, "running", t, None) {
            warn!(run = %run.id, error = %e, "engine: mark running failed");
            continue;
        }
        match state
            .workflow_manager
            .run_inline(definition, inputs, "case", binding, &run.agent_id, None)
            .await
        {
            Ok(wf_run_id) => {
                let _ = store.engine_set_external_ref(&run.id, &wf_run_id);
                info!(run = %run.id, workflow_run = %wf_run_id, "engine: case turn started");
            }
            Err(e) => {
                warn!(run = %run.id, error = %e, "engine: case turn failed to start");
                let _ = settle_turn(store, &run, Some(&format!("turn failed to start: {e}")), true, t);
            }
        }
    }

    let running = store.engine_running_runs_of_kind("case_turn").unwrap_or_default();
    for run in running {
        let Some(wf_run_id) = run.external_ref.as_deref() else { continue };
        let Ok(Some(wf)) = store.get_workflow_run(wf_run_id) else { continue };
        match wf.status.as_str() {
            "completed" => {
                if let Err(e) = settle_turn(store, &run, wf.output.as_deref(), false, t) {
                    warn!(run = %run.id, error = %e, "engine: settle failed");
                }
            }
            "failed" | "cancelled" => {
                let msg = wf.error.clone().unwrap_or_else(|| wf.status.clone());
                if let Err(e) = settle_turn(store, &run, Some(&msg), true, t) {
                    warn!(run = %run.id, error = %e, "engine: settle failed");
                }
            }
            _ => {}
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
        assert_eq!(children[0].kind, "case_turn");
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
        let turns = s.engine_queued_runs_of_kind("case_turn", 10).unwrap();
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
        s.engine_create_run(&NewRun { id: "turn-1", kind: "case_turn", session_key: "agent:a:case:k", agent_id: "a", lane: "main", parent_run_id: Some("case-1"), inputs: Some(r#"{"_case":{"key_type":"email","key_value":"x","default_wait_secs":86400}}"#), ..Default::default() }).unwrap();
        s.engine_set_run_state("turn-1", "running", 150, None).unwrap();
        s.engine_set_external_ref("turn-1", "wf-1").unwrap();
        s.engine_enqueue_event(&NewEvent { kind: "signal", target_type: "run", target_id: "email:x", payload: "again", idem_key: "s2", durable: true, ..Default::default() }).unwrap();

        let r = tick(&s, 200, &idle, &no_steer);
        assert_eq!(r.claimed, 1);
        assert_eq!(r.children_started, 0, "no second turn beside a running one");
        assert_eq!(r.steered, 0);
        assert_eq!(s.engine_queued_runs_of_kind("case_turn", 10).unwrap().len(), 0);

        // The turn settles; the parent's new wait carries the deferred event
        // once its lease has expired.
        let turn = s.engine_get_run("turn-1").unwrap().unwrap();
        settle_turn(&s, &turn, Some("sent first response"), false, 300).unwrap();
        let later = 200 + db::EVENT_LEASE_SECS + 1;
        let r = tick(&s, later, &idle, &no_steer);
        assert_eq!(r.children_started, 1, "now it starts the next turn");
        let next = s.engine_queued_runs_of_kind("case_turn", 10).unwrap();
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
        let turns = s.engine_queued_runs_of_kind("case_turn", 10).unwrap();
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
        assert_eq!(s.engine_queued_runs_of_kind("case_turn", 10).unwrap().len(), 1, "still exactly one turn");
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
        let turn = s.engine_queued_runs_of_kind("case_turn", 1).unwrap().remove(0);
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
        let turn2 = s.engine_queued_runs_of_kind("case_turn", 1).unwrap().remove(0);
        settle_turn(&s, &turn2, Some("Called, left a voicemail."), false, 5_000).unwrap();
        let case = s.engine_get_run(&case_id).unwrap().unwrap();
        let wait = s.engine_get_wait(case.current_wait_id.unwrap()).unwrap().unwrap();
        assert_eq!(wait.deadline, Some(5_000 + 3 * 86_400), "default three days");

        let (ev, _) = s.engine_claim_events(5_000 + 3 * 86_400, 10).unwrap();
        for e in &ev {
            deliver(&s, e, 5_000 + 3 * 86_400, &idle, &no_steer, &mut report);
        }
        let turn3 = s.engine_queued_runs_of_kind("case_turn", 1).unwrap().remove(0);
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
