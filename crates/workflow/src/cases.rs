//! Cases: how work for one person or thing enters the durable engine and
//! how a finished turn hands the case its next wait. Pure over the store,
//! so the webhook path, the event dispatcher, and the engine loop all call
//! the same code and tests need no runner. Design of record: "One Engine
//! for Durable Work" (2026-09-06).

use db::{EngineEvent, EngineRun, NewEvent, NewRun, NewWait, Store};
use types::NeboError;

/// A turn that ends without declaring a wait, on a binding that names none.
pub const DEFAULT_WAIT_SECS: i64 = 3 * 24 * 3600;
/// A failed turn is retried this many times, one minute apart at first and
/// doubling, never more than an hour apart; then the case waits its default.
pub const TURN_RETRY_ATTEMPTS: i64 = 3;
pub const TURN_RETRY_FIRST_SECS: i64 = 60;
pub const TURN_RETRY_MAX_SECS: i64 = 3600;

/// How many turns in a row have failed on this case, counting back from the
/// newest recorded turn (the one being settled is already recorded).
fn consecutive_failures(store: &Store, case_id: &str) -> i64 {
    store
        .engine_events_for("run", case_id, 50)
        .unwrap_or_default()
        .iter()
        .rev()
        .filter(|e| e.kind == "turn_failed" || e.kind == "turn_result")
        .take_while(|e| e.kind == "turn_failed")
        .count() as i64
}

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

impl<'a> CaseBinding<'a> {
    /// From a binding that declares `case`.
    pub fn from_binding(
        agent_id: &'a str,
        binding_name: &'a str,
        definition_json: &'a str,
        binding: &napp::agent::WorkflowBinding,
    ) -> Option<Self> {
        let case = binding.case.as_ref()?;
        let default_wait_secs = case
            .default_wait
            .as_deref()
            .and_then(relative_secs)
            .unwrap_or(DEFAULT_WAIT_SECS);
        let mut base_inputs = serde_json::to_value(&binding.inputs).unwrap_or_default();
        if !base_inputs.is_object() {
            base_inputs = serde_json::json!({});
        }
        Some(Self { agent_id, binding_name, definition_json, base_inputs, default_wait_secs })
    }
}

/// The person a payload names, per the binding's key spec. The spec is one
/// or more dotted paths, comma-separated, tried in order: the first present
/// wins, and its leaf becomes the key type (`customer.email` → `email`).
/// Values are trimmed and lowercased so `Alma@X.com` and `alma@x.com` are
/// one key. None means this payload does not name anyone.
pub fn resolve_key(payload: &serde_json::Value, spec: &str) -> Option<(String, String)> {
    for path in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        if let Some(value) = key_at(payload, path) {
            let key_type = path.rsplit('.').next().unwrap_or("key").to_string();
            return Some((key_type, value));
        }
    }
    None
}

fn key_at(payload: &serde_json::Value, path: &str) -> Option<String> {
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
    channel: &str,
    idem_key: &str,
    t: i64,
) -> Result<Routed, NeboError> {
    let key = format!("{key_type}:{key_value}");
    let event = NewEvent {
        kind: "signal",
        target_type: "run",
        target_id: &key,
        payload: &payload.to_string(),
        channel,
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
        external_ref: None,
    })?;
    if !store.engine_bind_key(&case_id, key_type, key_value)? {
        // Lost a race to another opener; that case owns the key now.
        store.engine_close_run(&case_id, "cancelled", t)?;
        if let Some(case) = store.engine_run_for_key(key_type, key_value)? {
            return Ok(Routed::Signaled { case_id: case.id });
        }
        return Err(NeboError::Internal("case key bound by nobody".into()));
    }
    // The case waits on its key from the start, so a second signal that
    // lands before the first turn ends is routed to it, not to nowhere.
    store.engine_declare_wait(
        &case_id,
        &NewWait { action: "trigger_child", on_kind: "signal", key: &key, deadline: None, parked: None, reason: "first contact" },
        t,
    )?;
    let case = store.engine_get_run(&case_id)?.ok_or(NeboError::NotFound)?;
    let claimed = store.engine_claim_events(t, 1)?; // the signal just written, in order
    if let Some(ev) = claimed.0.into_iter().find(|e| e.idem_key == idem_key) {
        start_child(store, &case, &ev)?;
        store.engine_complete_event(ev.id, t)?;
    }
    // Otherwise another claimer took it in between; the loop routes it.
    Ok(Routed::Opened { case_id })
}

/// `trigger_child`: the parent keeps waiting; a child run carries the event.
/// The child IS a workflow run — one engine row, queued here under its own
/// id with the case as parent, started by the engine loop through the same
/// `run_inline` every workflow uses, finished by the workflow itself. Its
/// inputs name the event that started it so the turn reads the signal and
/// the parent's history, not a guess.
pub fn start_child(store: &Store, parent: &EngineRun, event: &EngineEvent) -> Result<(), NeboError> {
    let mut inputs: serde_json::Value = parent
        .inputs
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    let payload: serde_json::Value = serde_json::from_str(&event.payload).unwrap_or_else(|_| serde_json::json!(event.payload));
    crate::events::insert_event_envelope(&mut inputs, &format!("case.{}", event.kind), payload, "case");
    inputs["_case"]["event_id"] = serde_json::json!(event.id);
    inputs["_case"]["history"] = serde_json::json!(history_lines(store, &parent.id));
    let binding = inputs["_case"]["binding"].as_str().unwrap_or("").to_string();
    let child_id = uuid::Uuid::new_v4().to_string();
    store.engine_create_run(&NewRun {
        id: &child_id,
        kind: "workflow",
        session_key: &tools::workflow_session_key(&parent.agent_id, &child_id),
        agent_id: &parent.agent_id,
        lane: &parent.lane,
        parent_run_id: Some(&parent.id),
        definition: parent.definition.as_deref(),
        inputs: Some(&inputs.to_string()),
        external_ref: None,
    })?;
    store.insert_workflow_run_detail(
        &child_id,
        &types::keyparser::agent_workflow_id(&parent.agent_id),
        "case",
        Some(&binding),
    )
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
    Some(t + relative_secs(s)?)
}

fn is_terminal(state: &str) -> bool {
    matches!(state, "booked" | "declined" | "opted_out" | "unresponsive" | "owner_takeover" | "closed" | "done")
}

/// A finished turn: apply what it declared to its case. A turn the
/// workflow already ended keeps its state and result; one that never
/// started (`failed` here) is ended here.
pub fn settle_turn(store: &Store, child: &EngineRun, output: Option<&str>, failed: bool, t: i64) -> Result<(), NeboError> {
    let ended = matches!(child.state.as_str(), "done" | "failed" | "cancelled");
    let Some(parent_id) = child.parent_run_id.as_deref() else {
        if !ended {
            store.engine_set_run_state(&child.id, if failed { "failed" } else { "done" }, t, None)?;
        }
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
    if !ended {
        if let Some(out) = output {
            store.engine_set_run_result(&child.id, out, None)?;
        }
        store.engine_set_run_state(&child.id, if failed { "failed" } else { "done" }, t, None)?;
    }
    // A case that already closed (a terminal state, or a merge) has no
    // next wait to declare; the turn's words are still recorded below.
    let parent = store.engine_get_run(parent_id)?;
    let parent_open = parent.as_ref().is_some_and(|p| matches!(p.state.as_str(), "waiting" | "queued" | "running"));

    // The turn's own words become the case's history.
    let summary = spec
        .as_ref()
        .map(|s| s.reason.clone())
        .filter(|r| !r.is_empty())
        .unwrap_or_else(|| if failed { "turn failed".to_string() } else { "turn finished without a declared wait".to_string() });
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
    if let Ok((claimed, _)) = store.engine_claim_events(t, 50) {
        for e in claimed.iter().filter(|e| e.r#ref == child.id && e.target_type == "run" && e.target_id == parent_id) {
            store.engine_complete_event(e.id, t)?;
        }
    }

    if !parent_open {
        return Ok(());
    }
    if let Some(state) = spec.as_ref().and_then(|s| s.state.as_deref()).filter(|s| is_terminal(s)) {
        store.engine_close_run(parent_id, "done", t)?;
        store.engine_set_run_result(parent_id, state, Some(&summary))?;
        return Ok(());
    }
    // Retry policy for a turn that failed (design: 1m, doubling, at most an
    // hour, three attempts): the case's next wait is a short timer, so the
    // next turn retries soon; after three failures in a row it waits the
    // binding's default like any other turn, and the owner sees the history.
    let (on_kind, deadline, reason) = match spec {
        Some(s) => (s.on_kind, s.deadline.or(Some(t + default_secs)), s.reason),
        None if failed => {
            let streak = consecutive_failures(store, parent_id);
            if streak <= TURN_RETRY_ATTEMPTS {
                let backoff = (TURN_RETRY_FIRST_SECS << (streak - 1)).min(TURN_RETRY_MAX_SECS);
                ("signal".to_string(), Some(t + backoff), format!("retry {streak} of {TURN_RETRY_ATTEMPTS}: {summary}"))
            } else {
                ("signal".to_string(), Some(t + default_secs), format!("gave up after {TURN_RETRY_ATTEMPTS} retries: {summary}"))
            }
        }
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
