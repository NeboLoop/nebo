//! The ONE loop for durable work (design of record: "One Engine for Durable
//! Work", 2026-09-06). Every tick, in this order — earlier steps produce
//! what later steps consume:
//!
//! 1. claim deliverable events under a lease (timers that came due, signals,
//!    approvals);
//! 2. match each to the live wait it wakes: `resume` re-queues that run with
//!    its parked messages, `trigger_child` leaves the parent waiting and
//!    queues a child run carrying the event; an event for a superseded wait
//!    generation is dropped, never delivered;
//! 3. (busy-session steering and starting queued runs — the producers that
//!    put work into these tables land with the conversion migration; until
//!    then an unmatched event is completed with a note so nothing loops);
//! 4. reap: pending effects are surfaced, transient events past the TTL go.
//!
//! Boot: runs the dead process left `running` are stamped `interrupted` and
//! given their ONE resume (I-3). This runs dark today — the tables are empty
//! until the seven mechanisms are converted — so the loop's cost is a few
//! indexed reads every five seconds and its value is that it is real before
//! anything depends on it.

use std::sync::Arc;
use std::time::Duration;

use tracing::{info, warn};

use crate::state::AppState;
use db::{EngineEvent, NewRun, Store};

const TICK: Duration = Duration::from_secs(5);
const CLAIM_BATCH: i64 = 50;
/// Delivered transient events live this long, matching the task TTL today.
const TRANSIENT_TTL_SECS: i64 = 7 * 24 * 3600;

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
    pub superseded: usize,
    pub unrouted: usize,
    pub pending_effects: usize,
    pub expired: usize,
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
/// an AppState; `spawn` is the only caller that adds a clock.
pub fn tick(store: &Store, t: i64) -> TickReport {
    let mut report = TickReport::default();
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
        deliver(store, event, t, &mut report);
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

fn deliver(store: &Store, event: &EngineEvent, t: i64, report: &mut TickReport) {
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
            if let Err(e) = store.engine_supersede_event(event.id, t) {
                warn!(event = event.id, error = %e, "engine: supersede failed");
            } else {
                report.superseded += 1;
            }
            return;
        }
        // No live wait for it and no producer-side routing yet (that lands
        // with the conversion). Completed with the fact recorded, so the row
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
        "trigger_child" => start_child(store, &wait.run_id, event).map(|_| {
            report.children_started += 1;
        }),
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

/// `trigger_child`: the parent keeps waiting; a child run carries the event.
/// The child's inputs name the event that started it so the turn reads the
/// signal and the parent's history, not a guess.
fn start_child(store: &Store, parent_id: &str, event: &EngineEvent) -> Result<(), types::NeboError> {
    let parent = store
        .engine_get_run(parent_id)?
        .ok_or_else(|| types::NeboError::NotFound)?;
    let inputs = serde_json::json!({
        "event_id": event.id,
        "event_kind": event.kind,
        "payload": event.payload,
        "channel": event.channel,
        "ref": event.r#ref,
    })
    .to_string();
    let child_id = uuid::Uuid::new_v4().to_string();
    let kind = if parent.kind == "case" { "case_turn" } else { "task" };
    store.engine_create_run(&NewRun {
        id: &child_id,
        kind,
        session_key: &parent.session_key,
        agent_id: &parent.agent_id,
        lane: &parent.lane,
        parent_run_id: Some(&parent.id),
        definition: parent.definition.as_deref(),
        inputs: Some(&inputs),
    })
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
            let report = tokio::task::spawn_blocking(move || tick(&s, now())).await.unwrap_or_default();
            if report != TickReport::default() {
                info!(?report, "engine: tick");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use db::{NewEvent, NewWait};

    fn store() -> Store {
        let path = std::env::temp_dir().join(format!("nebo-engine-loop-{}.db", uuid::Uuid::new_v4()));
        Store::new(&path.to_string_lossy()).expect("store")
    }

    #[test]
    fn a_signal_for_a_waiting_case_starts_one_child_turn_and_the_parent_keeps_waiting() {
        let s = store();
        s.engine_create_run(&NewRun { id: "case-1", kind: "case", session_key: "agent:a:case:k", agent_id: "a", lane: "main", ..Default::default() }).unwrap();
        s.engine_declare_wait("case-1", &NewWait { action: "trigger_child", on_kind: "signal", key: "email:alma@x.com", deadline: Some(9_000), reason: "until Thu", ..Default::default() }, 100).unwrap();
        s.engine_enqueue_event(&NewEvent { kind: "signal", target_type: "run", target_id: "email:alma@x.com", payload: "form again", idem_key: "form-2", durable: true, ..Default::default() }).unwrap();

        let r = tick(&s, 200);
        assert_eq!(r.claimed, 1);
        assert_eq!(r.children_started, 1);
        assert_eq!(s.engine_get_run("case-1").unwrap().unwrap().state, "waiting", "parent still waits");
        let children = s.engine_queued_runs("main", 10).unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].kind, "case_turn");
        assert_eq!(children[0].parent_run_id.as_deref(), Some("case-1"));
        assert!(children[0].inputs.as_deref().unwrap().contains("form again"));
        // Delivered: a second tick finds nothing.
        assert_eq!(tick(&s, 300), TickReport { pending_effects: 0, ..Default::default() });
    }

    #[test]
    fn a_stale_deadline_is_superseded_and_a_current_one_starts_the_turn() {
        let s = store();
        s.engine_create_run(&NewRun { id: "case-1", kind: "case", session_key: "agent:a:case:k", agent_id: "a", lane: "main", ..Default::default() }).unwrap();
        s.engine_declare_wait("case-1", &NewWait { action: "trigger_child", on_kind: "signal", key: "k", deadline: Some(1_000), reason: "Wed", ..Default::default() }, 10).unwrap();
        s.engine_declare_wait("case-1", &NewWait { action: "trigger_child", on_kind: "signal", key: "k", deadline: Some(2_000), reason: "Fri", ..Default::default() }, 20).unwrap();
        let wed = tick(&s, 1_000);
        assert_eq!(wed.superseded, 1, "Wednesday's timer names the old generation");
        assert_eq!(wed.children_started, 0);
        let fri = tick(&s, 2_000);
        assert_eq!(fri.children_started, 1, "Friday's timer is the live one");
    }

    #[test]
    fn an_approval_resumes_the_parked_run_itself() {
        let s = store();
        s.engine_create_run(&NewRun { id: "wf-1", kind: "workflow", session_key: "agent:a:workflow:wf-1", agent_id: "a", lane: "main", ..Default::default() }).unwrap();
        s.engine_declare_wait("wf-1", &NewWait { action: "resume", on_kind: "approval", key: "approval:wf-1", parked: Some("[msgs]"), reason: "waiting for you", ..Default::default() }, 10).unwrap();
        s.engine_enqueue_event(&NewEvent { kind: "approval", target_type: "run", target_id: "approval:wf-1", payload: "approved", idem_key: "ok-1", ..Default::default() }).unwrap();
        let r = tick(&s, 50);
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
}
