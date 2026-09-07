//! Cases: how work for one person or thing enters the durable engine and
//! how a finished turn hands the case its next wait. Pure over the store,
//! so the webhook path, the event dispatcher, and the engine loop all call
//! the same code and tests need no runner. Design of record: "One Engine
//! for Durable Work" (2026-09-06).

use db::{EngineEvent, EngineRun, Enqueued, NewEvent, NewRun, NewWait, Store};
use types::NeboError;

/// A turn that ends without declaring a wait, on a binding that names none.
pub const DEFAULT_WAIT_SECS: i64 = 3 * 24 * 3600;
/// A failed turn is retried this many times, one minute apart at first and
/// doubling, never more than an hour apart; then the case waits its default.
pub const TURN_RETRY_ATTEMPTS: i64 = 3;
pub const TURN_RETRY_FIRST_SECS: i64 = 60;
pub const TURN_RETRY_MAX_SECS: i64 = 3600;

/// The engine gave up on something: a poisoned event, a run interrupted
/// twice, a turn that failed past its retries, a money effect nobody can
/// confirm, a merge that left two open cases, two employees on one case.
/// These are ENGINE FAULTS, and an engine fault always goes to the owner —
/// a card in the Inbox — whatever the employee's autonomy. Autonomy governs
/// business decisions; it never means the model that just hit a fault is
/// asked to improvise infrastructure recovery. The fault is also recorded
/// on the case's history, so the employee's next legitimately triggered
/// turn sees it. Idempotent per subject: one give-up, one notice.
pub fn needs_attention(store: &Store, agent_id: &str, run_id: &str, subject: &str, case: Option<&EngineRun>, reason: &str, t: i64) -> Result<(), NeboError> {
    let idem = format!("attention:{subject}");
    let history_target = case.map(|c| c.id.clone()).unwrap_or_else(|| run_id.to_string());
    let recorded = store.engine_enqueue_event(&NewEvent {
        kind: "needs_attention",
        target_type: "run",
        target_id: &history_target,
        payload: reason,
        r#ref: subject,
        idem_key: &idem,
        durable: true,
        ..Default::default()
    })?;
    if recorded == db::Enqueued::Duplicate {
        return Ok(());
    }
    // History rows never wake anything.
    if let Ok((claimed, _)) = store.engine_claim_events(t, 50) {
        for e in claimed.iter().filter(|e| e.idem_key == idem) {
            store.engine_complete_event(e.id, t)?;
        }
    }

    let user_id = store.ensure_local_user_id().unwrap_or_default();
    let title = match case {
        Some(_) => "A case needs your attention",
        None => "A run needs your attention",
    };
    store.create_notification_if_not_exists(
        &idem,
        &user_id,
        "needs_attention",
        title,
        Some(reason),
        Some("/dashboard?inbox=1"),
        None,
        (!agent_id.is_empty()).then_some(agent_id),
    )
}

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
    /// No open case for the subject and case type: one opened and its
    /// first turn queued.
    Opened { case_id: String },
    /// An open case of this type exists for the subject and another
    /// employee owns it. Nothing was appended; the owner was told. Two
    /// employees on one case type is a handoff or a configuration error,
    /// never a silent merge.
    Conflict { case_id: String, owner: String },
    /// The person's last case of this type closed for inactivity and they
    /// wrote back: that same case is open again and the signal reaches it.
    Reopened { case_id: String },
    /// The person's last case of this type closed because they opted out
    /// or declined. Nothing reopens; the signal is recorded and the owner
    /// is told.
    Refused { case_id: String, reason: String },
}

/// Closure reasons a later signal reactivates the same case from — the
/// conversation simply went quiet — as opposed to a real ending.
pub fn reopens_on_reply(reason: &str) -> bool {
    matches!(reason, "unresponsive" | "inactive_timeout" | "inactive" | "no_reply")
}

/// Closure reasons that are the person's own word: nothing reopens.
pub fn never_reopens(reason: &str) -> bool {
    matches!(reason, "opted_out" | "declined" | "do_not_contact")
}

/// Everything a binding brings to the router.
pub struct CaseBinding<'a> {
    pub agent_id: &'a str,
    pub binding_name: &'a str,
    /// The kind of case (`lead`, `support`). Bindings sharing it share the case.
    pub case_type: String,
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
        let case_type = case
            .case_type
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .unwrap_or(binding_name)
            .replace(':', "-");
        Some(Self { agent_id, binding_name, case_type, definition_json, base_inputs, default_wait_secs })
    }
}

/// Every alias a payload carries, per the binding's key spec: one or more
/// dotted paths, comma-separated; every path that is present contributes
/// an alias, typed by its leaf (`contact.email` → email, `phone` → phone,
/// `crm_id` → crm, anything else → the leaf itself) and normalized. Empty
/// means this payload does not name anyone.
pub fn resolve_aliases(payload: &serde_json::Value, spec: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for path in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let Some(raw) = value_at(payload, path) else { continue };
        let leaf = path.rsplit('.').next().unwrap_or("key").to_lowercase();
        let kind = if leaf.ends_with("email") {
            "email"
        } else if leaf.ends_with("phone") || leaf.ends_with("mobile") {
            "phone"
        } else if matches!(leaf.as_str(), "crm_id" | "crmid" | "customer_id" | "customerid" | "contact_id" | "contactid") {
            "crm"
        } else {
            leaf.as_str()
        };
        let Some(value) = normalize_alias(kind, &raw) else { continue };
        let kind = kind.to_string();
        if !out.iter().any(|(k, v)| *k == kind && *v == value) {
            out.push((kind, value));
        }
    }
    out
}

/// The first alias a payload carries, for callers that want one key.
pub fn resolve_key(payload: &serde_json::Value, spec: &str) -> Option<(String, String)> {
    resolve_aliases(payload, spec).into_iter().next()
}

/// One alias, the way it is stored: an email lowercased, a phone as E.164
/// digits, any other id trimmed. None when there is nothing usable.
pub fn normalize_alias(kind: &str, raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    let v = match kind {
        "email" => {
            let e = s.to_lowercase();
            if !e.contains('@') {
                return None;
            }
            e
        }
        "phone" => normalize_phone(s)?,
        _ => s.to_string(),
    };
    Some(v)
}

/// E.164 from what people type. Digits only; a leading `+` keeps the country
/// code; ten digits are read as North American; eleven digits starting with
/// 1 likewise. ponytail: no libphonenumber — anything else is kept as `+`
/// plus its digits, which is exact-match stable even when not canonical.
pub fn normalize_phone(s: &str) -> Option<String> {
    let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() < 7 {
        return None;
    }
    let plus = s.trim_start().starts_with('+');
    Some(match (plus, digits.len()) {
        (false, 10) => format!("+1{digits}"),
        (false, 11) if digits.starts_with('1') => format!("+{digits}"),
        _ => format!("+{digits}"),
    })
}

fn value_at(payload: &serde_json::Value, path: &str) -> Option<String> {
    let mut cur = payload;
    for seg in path.split('.').filter(|s| !s.is_empty()) {
        cur = cur.get(seg)?;
    }
    match cur {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// The open case of one type for whoever an alias names, if any.
pub fn open_case_for(store: &Store, case_type: &str, kind: &str, value: &str) -> Option<EngineRun> {
    let value = normalize_alias(kind, value)?;
    let subject = store.engine_subject_for_alias(kind, &value).ok().flatten()?;
    store.engine_run_for_key(&format!("case:{case_type}"), &subject).ok().flatten()
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

/// Signal-with-start for one alias. See `route_signal`.
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
    let value = normalize_alias(key_type, key_value).unwrap_or_else(|| key_value.to_string());
    route_signal(store, b, &[(key_type.to_string(), value)], payload, channel, idem_key, t)
}

/// Signal-with-start. The aliases name a subject (creating or, when they
/// were observed together, merging); the signal is recorded first (durable,
/// idempotent) against the case key `case:<type>:<subject>`; then it either
/// reaches the open case of that type for the subject or opens one. An
/// open case owned by another employee is a conflict, never a silent merge.
pub fn route_signal(
    store: &Store,
    b: &CaseBinding<'_>,
    aliases: &[(String, String)],
    payload: &serde_json::Value,
    channel: &str,
    idem_key: &str,
    t: i64,
) -> Result<Routed, NeboError> {
    if aliases.is_empty() {
        return Err(NeboError::Validation("a case signal names nobody".into()));
    }
    let source = format!("{}:{}:{}", b.agent_id, b.binding_name, channel);
    let (subject, merged) = store.engine_resolve_subject(aliases, &source, t)?;
    for loser in &merged {
        for conflicted in store.engine_rekey_open_runs(loser, &subject)? {
            let reason = format!("two open cases now name one person after a merge: {conflicted} and the case for subject {subject}; keep one");
            needs_attention(store, b.agent_id, &conflicted, &format!("merge:{loser}:{subject}"), None, &reason, t)?;
        }
    }
    let key_type = format!("case:{}", b.case_type);
    let key = format!("{key_type}:{subject}");
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
    // Recorded under this call's own lease: the loop cannot claim it while
    // the routing below decides who delivers it.
    let Enqueued::Inserted(event_id) = store.engine_enqueue_event_leased(&event, t)? else {
        return Ok(Routed::Duplicate);
    };
    route_recorded(store, b, &subject, event_id, idem_key, t)
}

/// Route a signal that is already on the books: from `route_signal` the
/// moment it is recorded, or from the engine loop when a claimed signal
/// outlived the case it was aimed at. The open case takes it; otherwise the
/// person's last case of this type decides (the reopen rules); otherwise a
/// new case, linked to the last.
pub fn route_recorded(store: &Store, b: &CaseBinding<'_>, subject: &str, event_id: i64, idem_key: &str, t: i64) -> Result<Routed, NeboError> {
    let key_type = format!("case:{}", b.case_type);
    let key = format!("{key_type}:{subject}");
    if let Some(case) = store.engine_run_for_key(&key_type, subject)? {
        if case.agent_id != b.agent_id {
            // Recorded, not appended: the signal is on the books with the
            // reason it went nowhere, and the owner decides the routing.
            store.engine_supersede_event(event_id, t, &format!("conflict: case owned by {}", case.agent_id))?;
            let reason = format!(
                "employees {} and {} both handle {} cases for the same person; the case belongs to {}. Route the source to one of them or hand the case off.",
                case.agent_id, b.agent_id, b.case_type, case.agent_id
            );
            needs_attention(store, "", &case.id, &format!("conflict:{}:{}", case.id, b.agent_id), None, &reason, t)?;
            return Ok(Routed::Conflict { case_id: case.id, owner: case.agent_id });
        }
        // The playbook is read fresh each turn: the case carries the
        // binding as it is NOW, so the turn this signal starts runs the
        // current definition, and the turn's governance record says which.
        if case.definition.as_deref() != Some(b.definition_json) {
            store.engine_set_run_definition(&case.id, b.definition_json)?;
        }
        // The case's wait delivers it: hand it to the loop.
        store.engine_release_event(event_id)?;
        return Ok(Routed::Signaled { case_id: case.id });
    }

    // No open case. What the person's LAST case of this type closed as
    // decides what happens now (owner's reopen rules, 2026-09-07).
    let previous = store.engine_last_closed_run_for_key(&key_type, &subject)?;
    if let Some(prev) = &previous {
        let reason = prev.result.clone().unwrap_or_default();
        if never_reopens(&reason) {
            store.engine_supersede_event(event_id, t, &format!("closed: {reason}; not reopened"))?;
            let why = format!("A message arrived from someone whose {} case closed as {reason}. Nothing was reopened and no reply was sent; decide whether to respond yourself.", b.case_type);
            needs_attention(store, &prev.agent_id, &prev.id, &format!("refused:{}:{}", prev.id, event_id), None, &why, t)?;
            return Ok(Routed::Refused { case_id: prev.id.clone(), reason });
        }
        if reopens_on_reply(&reason) && prev.agent_id == b.agent_id {
            // They wrote back: the same conversation continues.
            if store.engine_reopen_run(&prev.id, &key_type, &subject, t)? {
                store.engine_declare_wait(
                    &prev.id,
                    &NewWait { action: "trigger_child", on_kind: "signal", key: &key, deadline: None, parked: None, reason: "reopened: they wrote back" },
                    t,
                )?;
                // A history row: complete on arrival, it wakes nothing.
                if let Ok(Enqueued::Inserted(note)) = store.engine_enqueue_event(&NewEvent {
                    kind: "reopened",
                    target_type: "run",
                    target_id: &prev.id,
                    payload: &format!("reopened after closing as {reason}"),
                    r#ref: idem_key,
                    idem_key: &format!("reopen:{}:{event_id}", prev.id),
                    durable: true,
                    ..Default::default()
                }) {
                    store.engine_complete_event(note, t)?;
                }
                // The signal that reopened it starts the next turn now.
                let case = store.engine_get_run(&prev.id)?.ok_or(NeboError::NotFound)?;
                if let Some(ev) = store.engine_get_event(event_id)? {
                    start_child(store, &case, &ev)?;
                    store.engine_complete_event(ev.id, t)?;
                }
                return Ok(Routed::Reopened { case_id: prev.id.clone() });
            }
        }
        // Won, completed, or closed for any other reason: a new case, linked.
    }

    let case_id = uuid::Uuid::new_v4().to_string();
    let session_key = format!("agent:{}:case:{}", b.agent_id, case_id);
    let mut inputs = b.base_inputs.clone();
    inputs["_case"] = serde_json::json!({
        "id": case_id,
        "case_type": b.case_type,
        "subject_id": subject,
        "key": key,
        "aliases": store.engine_subject_aliases(&subject)?.into_iter().map(|(k, v)| serde_json::json!({"kind": k, "value": v})).collect::<Vec<_>>(),
        "binding": b.binding_name,
        "default_wait_secs": b.default_wait_secs,
        "previous_case": previous.as_ref().map(|p| serde_json::json!({"id": p.id, "closed_as": p.result, "summary": p.summary})),
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
    if !store.engine_bind_key(&case_id, &key_type, &subject)? {
        // Lost a race to another opener; that case owns the key now.
        store.engine_close_run(&case_id, "cancelled", t)?;
        if let Some(case) = store.engine_run_for_key(&key_type, &subject)? {
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
    // The signal just written starts the first turn here, so the case is
    // never open with nothing queued.
    // The signal is still under the recorder's lease (seen under contention:
    // without it the loop claimed it here and started a second first turn —
    // forty people, forty-three first turns), so this is the one hand-off.
    if let Some(ev) = store.engine_get_event(event_id)? {
        start_child(store, &case, &ev)?;
        store.engine_complete_event(ev.id, t)?;
    }
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
    // What governs this turn, recorded with it: the playbook is read fresh
    // each turn on purpose, so the record says which one this turn ran
    // under and what the employee's policy was at the time.
    inputs["_case"]["governance"] = serde_json::json!({
        "definition_hash": parent.definition.as_deref().map(fingerprint).unwrap_or_default(),
        "policy_default": store
            .get_entity_config("agent", &parent.agent_id)
            .ok()
            .flatten()
            .and_then(|c| c.operation_policy)
            .map(|j| tools::policy::OperationPolicy::from_json(Some(&j)))
            .map(|p| format!("{:?}", p.default).to_lowercase())
            .unwrap_or_else(|| "approval".to_string()),
        "queued_at": chrono::Utc::now().timestamp(),
    });
    let binding = inputs["_case"]["binding"].as_str().unwrap_or("").to_string();
    // Signals parked on the case while it waited on something else ride
    // this turn (they are in the copied inputs), and only this turn.
    let carried_parked = inputs["_case"]["pending_signals"].as_array().is_some_and(|a| !a.is_empty());
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
    )?;
    if carried_parked {
        store.engine_clear_pending_signals(&parent.id)?;
    }
    Ok(())
}

/// A short stable fingerprint of a definition, for the governance record.
fn fingerprint(s: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    format!("{:016x}", h.finish())
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

/// The turn contract. A case step ENDS with one JSON object, and nothing
/// after it: `result` is the employee's business state (the playbook owns
/// it; the engine only records it), `next` is the engine command.
///
/// ```json
/// {"result": {"status": "awaiting_documents", "summary": "Requested W-2 and two bank statements"},
///  "next":   {"action": "wait", "on": "signal", "deadline": "2026-09-09T18:00:00Z", "reason": "documents"}}
/// ```
///
/// `next.action` is `wait` (with `on`, an optional `deadline` as RFC 3339
/// or a relative span such as `3d`, and an optional `reason`) or `close`.
/// Anything else — prose after the object, a missing `next`, an unknown
/// action, an unreadable deadline — is an invalid turn, never interpreted.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TurnEnvelope {
    #[serde(default)]
    pub result: Option<TurnResult>,
    pub next: TurnNext,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Deserialize)]
pub struct TurnResult {
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TurnNext {
    pub action: String,
    #[serde(default = "default_on")]
    pub on: String,
    #[serde(default)]
    pub deadline: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
}

fn default_on() -> String {
    "signal".to_string()
}

/// What the engine does with a valid turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turn {
    /// `Some(status)`: close the case in this business state.
    pub close: Option<String>,
    pub on_kind: String,
    pub deadline: Option<i64>,
    pub reason: String,
    pub summary: String,
}

/// Strict read of a case step's output: the LAST top-level JSON object,
/// which must be the last thing in the output (a closing code fence and
/// whitespace excepted), validated as a `TurnEnvelope`. Prose before the
/// object is the employee's own words and is fine; anything after it, or
/// no object at all, is an invalid turn.
pub fn parse_turn(output: &str, t: i64) -> Result<Turn, String> {
    let (start, end) = last_json_object(output).ok_or("no JSON object at the end of the turn")?;
    let tail = output[end..].trim().trim_end_matches("```").trim();
    if !tail.is_empty() {
        return Err(format!("text after the turn's JSON object: {:?}", tail.chars().take(40).collect::<String>()));
    }
    let env: TurnEnvelope = serde_json::from_str(&output[start..end]).map_err(|e| format!("turn object does not match the contract: {e}"))?;
    let result = env.result.unwrap_or_default();
    let reason = env.next.reason.clone().filter(|r| !r.is_empty()).unwrap_or_else(|| result.summary.clone());
    match env.next.action.as_str() {
        "close" => Ok(Turn {
            close: Some(if result.status.is_empty() { "closed".to_string() } else { result.status.clone() }),
            on_kind: env.next.on,
            deadline: None,
            reason,
            summary: result.summary,
        }),
        "wait" => {
            let deadline = match env.next.deadline.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
                Some(d) => Some(parse_deadline(d, t).ok_or_else(|| format!("unreadable deadline {d:?}"))?),
                None => None,
            };
            Ok(Turn { close: None, on_kind: env.next.on, deadline, reason, summary: result.summary })
        }
        other => Err(format!("unknown next.action {other:?}")),
    }
}

/// Byte span of the last top-level `{…}` in `s`, string-aware.
fn last_json_object(s: &str) -> Option<(usize, usize)> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    let mut start = None;
    let mut last = None;
    for (i, c) in s.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' => {
                if depth == 0 {
                    start = Some(i);
                }
                depth += 1;
            }
            '}' => {
                if depth > 0 {
                    depth -= 1;
                    if depth == 0 {
                        if let Some(st) = start {
                            last = Some((st, i + c.len_utf8()));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    last
}

fn parse_deadline(s: &str, t: i64) -> Option<i64> {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s.trim()) {
        return Some(dt.timestamp());
    }
    Some(t + relative_secs(s)?)
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
    let key = inputs["_case"]["key"].as_str().unwrap_or("").to_string();
    let default_secs = inputs["_case"]["default_wait_secs"].as_i64().unwrap_or(DEFAULT_WAIT_SECS);

    // A turn that ran to the end must have ended with the contract. One
    // that did not is recorded as such and the case waits its default —
    // it is NOT retried, because the turn may have done real work.
    let contract = if failed { None } else { Some(parse_turn(output.unwrap_or(""), t)) };
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
    let summary = match &contract {
        Some(Ok(turn)) => [turn.summary.as_str(), turn.reason.as_str()]
            .into_iter()
            .find(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| "turn finished".to_string()),
        Some(Err(why)) => format!("turn ended without a valid next: {why}"),
        // The reason is the audit line, not just the fact.
        None => match output.map(str::trim).filter(|o| !o.is_empty()) {
            Some(why) => format!("turn failed: {}", why.chars().take(300).collect::<String>()),
            None => "turn failed".to_string(),
        },
    };
    // The receipts behind the words: what the ledger saw this turn. Seen
    // live: a turn closed a case as booked saying "calendar invite sent to
    // both" — every calendar call had failed and nothing was sent. The
    // history carries the fact beside the claim, so the next turn and the
    // owner read both.
    let receipts: Vec<_> = store.engine_effects_for_run(&child.id)?.into_iter().filter(|e| e.state == "completed").collect();
    let receipt_line = if receipts.is_empty() {
        "no send on the ledger this turn".to_string()
    } else {
        let list = receipts
            .iter()
            .map(|e| format!("{}#{}{}", e.provider, e.id, e.provider_ref.as_deref().map(|r| format!(" ({r})")).unwrap_or_default()))
            .collect::<Vec<_>>()
            .join(", ");
        format!("{} send(s) on the ledger this turn: {list}", receipts.len())
    };
    let _ = store.engine_enqueue_event(&NewEvent {
        kind: if failed { "turn_failed" } else { "turn_result" },
        target_type: "run",
        target_id: parent_id,
        payload: &format!("{summary} — {receipt_line}"),
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
    if let Some(Ok(Turn { close: Some(status), .. })) = &contract {
        store.engine_close_run(parent_id, "done", t)?;
        store.engine_set_run_result(parent_id, status, Some(&summary))?;
        // Closed on an inbound turn with nothing on the ledger: the person
        // wrote, the turn says the case is settled, and no receipt backs
        // it. The decision stands — the owner is told, with the fact. A
        // close that needs no answer (they opted out, they declined) is
        // not that.
        let inbound = inputs["_case"]["event_id"]
            .as_i64()
            .and_then(|id| store.engine_get_event(id).ok().flatten())
            .is_some_and(|e| e.kind == "signal");
        if inbound && receipts.is_empty() && !never_reopens(status) {
            let why = format!(
                "The turn closed this {} case as '{status}' saying \"{summary}\", but the ledger shows no message sent this turn. The person may be waiting on an answer that never went out; check before trusting the summary.",
                inputs["_case"]["case_type"].as_str().unwrap_or("open")
            );
            needs_attention(store, &child.agent_id, &child.id, &format!("unreceipted:{}", child.id), parent.as_ref(), &why, t)?;
        }
        return Ok(());
    }
    // Retry policy for a turn that failed (design: 1m, doubling, at most an
    // hour, three attempts): the case's next wait is a short timer, so the
    // next turn retries soon; after three failures in a row it waits the
    // binding's default like any other turn, and the owner sees the history.
    let (on_kind, deadline, reason) = match contract {
        Some(Ok(turn)) => (turn.on_kind, turn.deadline.or(Some(t + default_secs)), if turn.reason.is_empty() { summary.clone() } else { turn.reason }),
        Some(Err(_)) => ("signal".to_string(), Some(t + default_secs), summary.clone()),
        None => {
            let streak = consecutive_failures(store, parent_id);
            if streak <= TURN_RETRY_ATTEMPTS {
                let backoff = (TURN_RETRY_FIRST_SECS << (streak - 1)).min(TURN_RETRY_MAX_SECS);
                ("signal".to_string(), Some(t + backoff), format!("retry {streak} of {TURN_RETRY_ATTEMPTS}: {summary}"))
            } else {
                let reason = format!("gave up after {TURN_RETRY_ATTEMPTS} retries: {summary}");
                needs_attention(store, &child.agent_id, &child.id, &child.id, parent.as_ref(), &reason, t)?;
                ("signal".to_string(), Some(t + default_secs), reason)
            }
        }
    };
    store.engine_set_run_result(parent_id, "", Some(&summary))?;
    store.engine_declare_wait(
        parent_id,
        &NewWait { action: "trigger_child", on_kind: &on_kind, key: &key, deadline, parked: None, reason: &reason },
        t,
    )?;
    Ok(())
}
