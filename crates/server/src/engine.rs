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
//! Boot: runs the dead process left `running` are stamped `interrupted` and
//! given their ONE resume (I-3). Cases enter through [`signal_or_open`],
//! called by the webhook path for bindings that declare `case`; every other
//! binding is untouched.

use std::sync::Arc;
use std::time::Duration;

use tracing::{info, warn};

use crate::state::AppState;
use db::{EngineEvent, EngineRun, NewEvent, NewRun, NewWait, Store};
use tools::workflows::WorkflowManager;

const TICK: Duration = Duration::from_secs(5);
const CLAIM_BATCH: i64 = 50;
/// Delivered transient events live this long, matching the task TTL today.
const TRANSIENT_TTL_SECS: i64 = 7 * 24 * 3600;
/// A turn that ends without declaring a wait, on a binding that names none.
const DEFAULT_WAIT_SECS: i64 = 3 * 24 * 3600;
/// Turns started per tick, so one flood cannot starve everything else.
const TURNS_PER_TICK: i64 = 5;

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
        // A signal for an open case that has no live wait — its first turn
        // is still queued or running. It reaches that turn, not a new one.
        if event.target_type == "run" && event.kind == "signal" {
            if let Some((kt, kv)) = event.target_id.split_once(':') {
                if let Ok(Some(case)) = store.engine_run_for_key(kt, kv) {
                    if steer_into_live_turn(store, &case, event, t, busy, steer) {
                        report.steered += 1;
                        return;
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
            if steer_into_live_turn(store, &parent, event, t, busy, steer) {
                report.steered += 1;
                return;
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

/// If the case has a live turn, hand it the event: a running turn gets it
/// as steering; a queued one gets it appended to its inputs. Returns true
/// when the event was handed over (and completed).
fn steer_into_live_turn(
    store: &Store,
    case: &EngineRun,
    event: &EngineEvent,
    t: i64,
    busy: &dyn Fn(&str) -> bool,
    steer: &dyn Fn(&str, &EngineEvent),
) -> bool {
    let child = match store.engine_live_child(&case.id) {
        Ok(Some(c)) => c,
        _ => return false,
    };
    let handed = match (child.state.as_str(), child.external_ref.as_deref()) {
        ("running", Some(wf_run_id)) => {
            let session_key = tools::workflow_session_key(&case.agent_id, wf_run_id);
            if busy(&session_key) {
                steer(&session_key, event);
                true
            } else {
                // Between the runner finishing and reconciliation noticing:
                // let the lease expire and the next tick route it fresh.
                false
            }
        }
        ("queued", _) => store.engine_append_pending_signal(&child.id, &event.payload).is_ok(),
        _ => false,
    };
    if handed {
        if let Err(e) = store.engine_complete_event(event.id, t) {
            warn!(event = event.id, error = %e, "engine: complete after steer failed");
        }
    }
    handed
}

/// `trigger_child`: the parent keeps waiting; a child run carries the event.
/// The child's inputs name the event that started it so the turn reads the
/// signal and the parent's history, not a guess.
fn start_child(store: &Store, parent: &EngineRun, event: &EngineEvent) -> Result<(), types::NeboError> {
    let mut inputs: serde_json::Value = parent
        .inputs
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    let payload: serde_json::Value = serde_json::from_str(&event.payload).unwrap_or_else(|_| serde_json::json!(event.payload));
    workflow::events::insert_event_envelope(&mut inputs, &format!("case.{}", event.kind), payload, "case");
    inputs["_case"]["event_id"] = serde_json::json!(event.id);
    inputs["_case"]["history"] = serde_json::json!(history_lines(store, &parent.id));
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
        inputs: Some(&inputs.to_string()),
    })
}

/// The last durable events on a case, one line each, for the turn's prompt.
fn history_lines(store: &Store, case_id: &str) -> Vec<String> {
    store
        .engine_events_for("run", case_id, 50)
        .unwrap_or_default()
        .into_iter()
        .map(|e| format!("{} [{}] {}", e.id, e.kind, e.payload.chars().take(200).collect::<String>()))
        .collect()
}

// ── cases: how work enters ────────────────────────────────────────────────

/// Where a routed signal went.
#[derive(Debug, PartialEq, Eq)]
pub enum Routed {
    /// Already recorded under this idempotency key; nothing happened.
    Duplicate,
    /// Appended to an open case; the loop wakes it.
    Signaled { case_id: String },
    /// No open case for the key: one opened and its first turn queued.
    Opened { case_id: String },
}

/// Everything a binding brings to the router.
pub struct CaseBinding<'a> {
    pub agent_id: &'a str,
    pub binding_name: &'a str,
    pub definition_json: &'a str,
    pub base_inputs: serde_json::Value,
    pub default_wait_secs: i64,
}

/// Signal-with-start. The signal is recorded first (durable, idempotent);
/// then it either reaches the open case for the key or opens one. The
/// event's target is the key itself, `<type>:<value>`, which is also what
/// the case's wait matches on.
pub fn signal_or_open(
    store: &Store,
    b: &CaseBinding<'_>,
    key_type: &str,
    key_value: &str,
    payload: &serde_json::Value,
    idem_key: &str,
    t: i64,
) -> Result<Routed, types::NeboError> {
    let key = format!("{key_type}:{key_value}");
    let event = NewEvent {
        kind: "signal",
        target_type: "run",
        target_id: &key,
        payload: &payload.to_string(),
        channel: "webhook",
        r#ref: idem_key,
        idem_key,
        durable: true,
        ..Default::default()
    };
    if store.engine_enqueue_event(&event)? == db::Enqueued::Duplicate {
        return Ok(Routed::Duplicate);
    }
    if let Some(case) = store.engine_run_for_key(key_type, key_value)? {
        return Ok(Routed::Signaled { case_id: case.id });
    }

    let case_id = uuid::Uuid::new_v4().to_string();
    let session_key = format!("agent:{}:case:{}", b.agent_id, case_id);
    let mut inputs = b.base_inputs.clone();
    inputs["_case"] = serde_json::json!({
        "id": case_id,
        "key_type": key_type,
        "key_value": key_value,
        "binding": b.binding_name,
        "default_wait_secs": b.default_wait_secs,
    });
    store.engine_create_run(&NewRun {
        id: &case_id,
        kind: "case",
        session_key: &session_key,
        agent_id: b.agent_id,
        lane: "main",
        parent_run_id: None,
        definition: Some(b.definition_json),
        inputs: Some(&inputs.to_string()),
    })?;
    if !store.engine_bind_key(&case_id, key_type, key_value)? {
        // Lost a race to another opener; that case owns the key now.
        store.engine_close_run(&case_id, "cancelled", t)?;
        if let Some(case) = store.engine_run_for_key(key_type, key_value)? {
            return Ok(Routed::Signaled { case_id: case.id });
        }
        return Err(types::NeboError::Internal("case key bound by nobody".into()));
    }
    // The case waits on its key from the start, so a second signal that
    // lands before the first turn ends is routed to it, not to nowhere.
    store.engine_declare_wait(
        &case_id,
        &NewWait { action: "trigger_child", on_kind: "signal", key: &key, deadline: None, parked: None, reason: "first contact" },
        t,
    )?;
    let case = store.engine_get_run(&case_id)?.ok_or(types::NeboError::NotFound)?;
    let claimed = store.engine_claim_events(t, 1)?; // the signal just written, in order
    let first = claimed.0.into_iter().find(|e| e.idem_key == idem_key);
    match first {
        Some(ev) => {
            start_child(store, &case, &ev)?;
            store.engine_complete_event(ev.id, t)?;
        }
        None => {
            // Another claimer took it in between; the loop will route it.
        }
    }
    Ok(Routed::Opened { case_id })
}

/// `customer.email` → the string at that dotted path, trimmed and lowercased
/// so `Alma@X.com` and `alma@x.com` are one key.
pub fn key_at(payload: &serde_json::Value, path: &str) -> Option<String> {
    let mut cur = payload;
    for seg in path.split('.').filter(|s| !s.is_empty()) {
        cur = cur.get(seg)?;
    }
    let s = match cur {
        serde_json::Value::String(s) => s.trim().to_lowercase(),
        serde_json::Value::Number(n) => n.to_string(),
        _ => return None,
    };
    (!s.is_empty()).then_some(s)
}

// ── the turn contract ─────────────────────────────────────────────────────

/// What a finished turn asked to wait for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaitSpec {
    pub on_kind: String,
    pub deadline: Option<i64>,
    pub reason: String,
    pub state: Option<String>,
}

/// `{"wait": {...}}` anywhere in the turn's output, last occurrence wins.
/// `deadline` is RFC 3339 or a relative span (`3d`, `12h`, `45m`). A
/// terminal `state` (booked, declined, opted_out, unresponsive,
/// owner_takeover, closed) closes the case instead.
pub fn parse_wait(output: &str, t: i64) -> Option<WaitSpec> {
    let idx = output.rfind("\"wait\"")?;
    let bytes = output.as_bytes();
    // Walk outward to the enclosing object: the nearest '{' before "wait"
    // that parses together with some '}' after it.
    let mut starts: Vec<usize> = output[..idx].match_indices('{').map(|(i, _)| i).collect();
    starts.reverse();
    let ends: Vec<usize> = output[idx..].match_indices('}').map(|(i, _)| idx + i + 1).collect();
    for s in starts.iter().take(4) {
        for e in ends.iter().take(6) {
            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&bytes[*s..*e]) {
                if let Some(spec) = wait_from_value(&v, t) {
                    return Some(spec);
                }
            }
        }
    }
    None
}

fn wait_from_value(v: &serde_json::Value, t: i64) -> Option<WaitSpec> {
    let w = v.get("wait")?;
    let state = v.get("state").and_then(|s| s.as_str()).map(str::to_string);
    let on_kind = w.get("on").and_then(|s| s.as_str()).unwrap_or("signal").to_string();
    let deadline = w.get("deadline").and_then(|d| d.as_str()).and_then(|d| parse_deadline(d, t));
    let reason = w
        .get("reason")
        .and_then(|s| s.as_str())
        .map(str::to_string)
        .or_else(|| v.get("outcome").and_then(|s| s.as_str()).map(str::to_string))
        .unwrap_or_default();
    Some(WaitSpec { on_kind, deadline, reason, state })
}

fn parse_deadline(s: &str, t: i64) -> Option<i64> {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s.trim()) {
        return Some(dt.timestamp());
    }
    let secs = relative_secs(s)?;
    Some(t + secs)
}

/// `3d`, `12h`, `45m`, `30s`, or combinations (`1d12h`).
pub fn relative_secs(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() || !s.chars().next()?.is_ascii_digit() {
        return None;
    }
    let mut total: i64 = 0;
    let mut n = String::new();
    for c in s.chars() {
        if c.is_ascii_digit() {
            n.push(c);
            continue;
        }
        let v: i64 = n.parse().ok()?;
        n.clear();
        total += match c {
            'd' => v * 86_400,
            'h' => v * 3_600,
            'm' => v * 60,
            's' => v,
            _ => return None,
        };
    }
    if !n.is_empty() {
        return None;
    }
    Some(total)
}

fn is_terminal(state: &str) -> bool {
    matches!(state, "booked" | "declined" | "opted_out" | "unresponsive" | "owner_takeover" | "closed" | "done")
}

/// A finished turn: apply what it declared to its case. Pure over the store
/// so the reconciliation path is testable without a workflow runner.
pub fn settle_turn(store: &Store, child: &EngineRun, output: Option<&str>, failed: bool, t: i64) -> Result<(), types::NeboError> {
    let Some(parent_id) = child.parent_run_id.as_deref() else {
        store.engine_set_run_state(&child.id, if failed { "failed" } else { "done" }, t, None)?;
        return Ok(());
    };
    let inputs: serde_json::Value = child.inputs.as_deref().and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default();
    let key = format!(
        "{}:{}",
        inputs["_case"]["key_type"].as_str().unwrap_or(""),
        inputs["_case"]["key_value"].as_str().unwrap_or("")
    );
    let default_secs = inputs["_case"]["default_wait_secs"].as_i64().unwrap_or(DEFAULT_WAIT_SECS);

    let spec = output.and_then(|o| parse_wait(o, t));
    if let Some(out) = output {
        store.engine_set_run_result(&child.id, out, None)?;
    }
    store.engine_set_run_state(&child.id, if failed { "failed" } else { "done" }, t, None)?;

    // The turn's own words become the case's history.
    let summary = spec.as_ref().map(|s| s.reason.clone()).filter(|r| !r.is_empty()).unwrap_or_else(|| {
        if failed { "turn failed".to_string() } else { "turn finished without a declared wait".to_string() }
    });
    let _ = store.engine_enqueue_event(&NewEvent {
        kind: if failed { "turn_failed" } else { "turn_result" },
        target_type: "run",
        target_id: parent_id,
        payload: &summary,
        r#ref: &child.id,
        idem_key: &format!("turn:{}:result", child.id),
        durable: true,
        ..Default::default()
    });
    // History rows never wake anything; they are complete on arrival.
    if let Ok((claimed, _)) = store.engine_claim_events(t, CLAIM_BATCH) {
        for e in claimed.iter().filter(|e| e.r#ref == child.id && e.target_type == "run" && e.target_id == parent_id) {
            store.engine_complete_event(e.id, t)?;
        }
    }

    if let Some(state) = spec.as_ref().and_then(|s| s.state.as_deref()).filter(|s| is_terminal(s)) {
        store.engine_close_run(parent_id, "done", t)?;
        store.engine_set_run_result(parent_id, state, Some(&summary))?;
        return Ok(());
    }
    let (on_kind, deadline, reason) = match spec {
        Some(s) => (s.on_kind, s.deadline.or(Some(t + default_secs)), s.reason),
        None => ("signal".to_string(), Some(t + default_secs), summary.clone()),
    };
    store.engine_set_run_result(parent_id, "", Some(&summary))?;
    store.engine_declare_wait(
        parent_id,
        &NewWait { action: "trigger_child", on_kind: &on_kind, key: &key, deadline, parked: None, reason: &reason },
        t,
    )?;
    Ok(())
}

// ── driving turns through the workflow runner ─────────────────────────────

/// Start queued case turns and reconcile running ones. This is the only
/// place the engine touches the runner, and it does so through the same
/// `run_inline` every webhook and scheduled workflow uses.
async fn drive(state: &AppState) {
    let store = &state.store;
    let t = now();
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
        let first = signal_or_open(&s, &b, "email", "alma@aboundinggoods.com", &payload, "sub-aug18", 1_000).unwrap();
        let Routed::Opened { case_id } = first else { panic!("first submission opens a case") };
        // The first turn is queued with the payload and the case context.
        let turns = s.engine_queued_runs_of_kind("case_turn", 10).unwrap();
        assert_eq!(turns.len(), 1);
        let inputs = turns[0].inputs.as_deref().unwrap();
        assert!(inputs.contains("aboundinggoods"));
        assert!(inputs.contains("\"binding\":\"work-lead\""));

        // Aug 19, Sep 4, Sep 6: three more submissions, same person.
        for (i, idem) in ["sub-aug19", "sub-sep4", "sub-sep6"].iter().enumerate() {
            let r = signal_or_open(&s, &b, "email", "alma@aboundinggoods.com", &payload, idem, 2_000 + i as i64).unwrap();
            assert_eq!(r, Routed::Signaled { case_id: case_id.clone() }, "{idem} reaches the same case");
        }
        // A replayed webhook is a duplicate, never a second case or turn.
        assert_eq!(signal_or_open(&s, &b, "email", "alma@aboundinggoods.com", &payload, "sub-sep6", 3_000).unwrap(), Routed::Duplicate);

        // The loop routes the three signals into the one queued turn's
        // inputs — one live turn per case — and starts nothing else.
        let r = tick(&s, 4_000, &idle, &no_steer);
        assert_eq!(r.steered, 3);
        assert_eq!(r.children_started, 0);
        assert_eq!(s.engine_queued_runs_of_kind("case_turn", 10).unwrap().len(), 1, "still exactly one turn");
        let refreshed = s.engine_get_run(&turns[0].id).unwrap().unwrap();
        assert_eq!(refreshed.inputs.as_deref().unwrap().matches("aboundinggoods").count() >= 4, true, "the later submissions rode along");

        // One case, one key, still open, still waiting on the same person.
        assert_eq!(s.engine_run_for_key("email", "alma@aboundinggoods.com").unwrap().unwrap().id, case_id);
        assert_eq!(s.engine_get_run(&case_id).unwrap().unwrap().state, "waiting");
    }

    #[test]
    fn a_finished_turn_declares_the_parents_next_wait_or_falls_back_to_the_default() {
        let s = store();
        let b = binding();
        let payload = serde_json::json!({"email": "a@b.c"});
        let Routed::Opened { case_id } = signal_or_open(&s, &b, "email", "a@b.c", &payload, "s1", 1_000).unwrap() else { panic!() };
        let turn = s.engine_queued_runs_of_kind("case_turn", 1).unwrap().remove(0);
        s.engine_set_run_state(&turn.id, "running", 1_001, None).unwrap();
        let turn = s.engine_get_run(&turn.id).unwrap().unwrap();

        // Declared: wait on a reply, or Thursday.
        let out = r#"Sent the day-1 email. {"outcome":"sent day-1 follow-up","state":"waiting_on_customer","wait":{"on":"signal","deadline":"3d","reason":"follow up if no reply by Thursday"}}"#;
        settle_turn(&s, &turn, Some(out), false, 2_000).unwrap();
        assert_eq!(s.engine_get_run(&turn.id).unwrap().unwrap().state, "done");
        let case = s.engine_get_run(&case_id).unwrap().unwrap();
        assert_eq!(case.state, "waiting");
        assert_eq!(case.summary, "follow up if no reply by Thursday");
        let wait = s.engine_get_wait(case.current_wait_id.unwrap()).unwrap().unwrap();
        assert_eq!(wait.deadline, Some(2_000 + 3 * 86_400));
        assert_eq!(wait.key, "email:a@b.c");
        // History has the turn's result.
        let hist = s.engine_events_for("run", &case_id, 50).unwrap();
        assert!(hist.iter().any(|e| e.kind == "turn_result" && e.payload.contains("Thursday")));

        // Undeclared: the binding's default wait applies.
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

        // Terminal: the case closes and releases its key.
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
