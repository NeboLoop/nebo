//! Session wake rail — "the turn ends, the attention doesn't"
//! (docs/prd/session-wake-rail.md).
//!
//! Anything a session started or asked to be woken for re-invokes that
//! session: same session, full context, labeled as machine, exactly once,
//! even across a restart. Producers call [`enqueue`] — persist first (R1's
//! write-ahead queue), then attempt delivery. [`deliver`] is the ONE entry
//! that decides delivery from `run_registry` state: a busy session's wakes
//! stay queued and are drained by [`on_run_finished`] when its run ends; an
//! idle session gets a wake run whose hidden prompt (isMeta, invisible in
//! the owner's transcript) carries the payloads plus a directive to act and
//! report. The owner only ever sees the agent's resulting message.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use tracing::{info, warn};

use crate::chat_dispatch::{ChatConfig, run_chat};
use crate::state::AppState;
use types::provenance::ProvenanceClass;

/// R6 storm cap: a wake run carries at most this many payloads verbatim;
/// anything beyond degrades to one honest summary line, never a wake storm.
const STORM_CAP: usize = 20;
/// R2 payload bound: a single payload is clipped to this many chars in the
/// wake prompt — the full text stays in the queue row / source thread.
const PAYLOAD_CLIP: usize = 2000;

/// Sessions with a wake run dispatched but not yet finished, mapped to the
/// claimed wake ids that ride it. Stamped delivered when the run completes
/// ([`on_run_finished`]) — crash mid-run leaves the rows unstamped, so the
/// boot sweep redelivers (WS4's write-ahead discipline; poison cap bounds it).
static IN_FLIGHT: LazyLock<Mutex<HashMap<String, Vec<i64>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Producer entry: persist the wake, then try to deliver it. Never blocks the
/// producer on the woken run.
pub fn enqueue(
    state: &AppState,
    session_key: &str,
    kind: &str,
    payload: &str,
    provenance: &[ProvenanceClass],
    handoff_depth: u8,
) {
    let prov = serde_json::to_string(provenance).unwrap_or_else(|_| "[]".to_string());
    if let Err(e) =
        state
            .store
            .enqueue_session_wake(session_key, kind, payload, &prov, handoff_depth)
    {
        warn!(error = %e, session = %session_key, "wake: failed to persist — payload lost");
        return;
    }
    let state = state.clone();
    let key = session_key.to_string();
    tokio::spawn(async move { deliver(&state, &key).await });
}

/// Drain a session's pending wakes into ONE wake run. No-op when the session
/// is busy (its run's completion drains) or a wake run is already in flight.
pub async fn deliver(state: &AppState, session_key: &str) {
    if state.run_registry.find_by_session(session_key).await.is_some() {
        return; // busy — on_run_finished drains when the live run ends
    }
    {
        let mut in_flight = IN_FLIGHT.lock().expect("wake in-flight lock");
        if in_flight.contains_key(session_key) {
            return; // a wake run is already carrying this session's batch
        }
        // Reserve before claiming so a concurrent deliver can't double-claim.
        in_flight.insert(session_key.to_string(), Vec::new());
    }

    let (batch, poisoned) = match state.store.claim_session_wakes(session_key) {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, session = %session_key, "wake: claim failed");
            IN_FLIGHT.lock().expect("wake in-flight lock").remove(session_key);
            return;
        }
    };
    if poisoned > 0 {
        warn!(session = %session_key, poisoned, "wake: undeliverable wakes poisoned");
        let agent_id = types::keyparser::extract_agent_id(session_key);
        tools::owner_notify::emit(
            &state.store,
            Some(&|ev, payload| state.hub.broadcast(ev, payload)),
            &tools::owner_notify::OwnerNotification {
                id: &format!("wake-poisoned-{}", session_key),
                kind: "error",
                title: "A background update could not be delivered",
                body: Some(&format!(
                    "{poisoned} pending update(s) for this employee failed delivery repeatedly and were dropped."
                )),
                action_url: Some(&format!("/{}", agent_id)),
                agent_id: (!agent_id.is_empty()).then_some(agent_id.as_str()),
                loud: false,
            },
        );
    }
    if batch.is_empty() {
        IN_FLIGHT.lock().expect("wake in-flight lock").remove(session_key);
        return;
    }

    let ids: Vec<i64> = batch.iter().map(|w| w.id).collect();
    IN_FLIGHT
        .lock()
        .expect("wake in-flight lock")
        .insert(session_key.to_string(), ids);

    // Union the batch's taint and take the deepest handoff chain — the woken
    // run is decided at the WS2 gates exactly as the payloads' origins demand,
    // and further coworker sends stay bounded by MAX_HANDOFF_DEPTH (R6).
    let mut seed_taint: Vec<ProvenanceClass> = Vec::new();
    let mut handoff_depth: u8 = 0;
    for w in &batch {
        handoff_depth = handoff_depth.max(w.handoff_depth);
        for class in serde_json::from_str::<Vec<ProvenanceClass>>(&w.provenance).unwrap_or_default()
        {
            if !seed_taint.contains(&class) {
                seed_taint.push(class);
            }
        }
    }

    let agent_id = types::keyparser::extract_agent_id(session_key);
    let entity_config = if agent_id.is_empty() {
        crate::entity_config::resolve_for_chat(&state.store, "main", "main")
    } else {
        crate::entity_config::resolve_for_chat(&state.store, "agent", &agent_id)
    };
    let channel = {
        let info = types::keyparser::parse_session_key(session_key);
        if info.channel.is_empty() { "web".to_string() } else { info.channel }
    };

    info!(session = %session_key, count = batch.len(), "wake: waking session");
    let config = ChatConfig {
        session_key: session_key.to_string(),
        prompt: wake_prompt(&batch),
        system: String::new(),
        user_id: String::new(),
        channel,
        origin: tools::Origin::System,
        agent_id,
        cancel_token: tokio_util::sync::CancellationToken::new(),
        lane: types::constants::lanes::COMM.to_string(),
        comm_reply: None,
        entity_config,
        images: vec![],
        entity_name: String::new(),
        origin_agent_id: None,
        mention_context: None,
        tool_scope: None,
        plan_mode: false,
        channel_ctx: None,
        handoff_depth,
        seed_taint,
        tool_allowlist: None,
        hidden_prompt: true,
        audience: None,
    };
    run_chat(state, config).await;
}

/// Run-completion hook — called at the end of every chat run's lane task.
/// Stamps the batch the finished run carried (if it was a wake run), then
/// drains anything that queued while the session was busy.
pub fn on_run_finished(state: &AppState, session_key: &str) {
    let ids = IN_FLIGHT.lock().expect("wake in-flight lock").remove(session_key);
    if let Some(ids) = ids
        && !ids.is_empty()
    {
        if let Err(e) = state.store.mark_session_wakes_delivered(&ids) {
            warn!(error = %e, session = %session_key, "wake: failed to stamp delivered");
        }
    }
    let has_pending = matches!(
        state.store.sessions_with_pending_wakes(),
        Ok(keys) if keys.iter().any(|k| k == session_key)
    );
    if has_pending {
        let state = state.clone();
        let key = session_key.to_string();
        tokio::spawn(async move { deliver(&state, &key).await });
    }
}

/// Boot sweep — same recovery moment as `recover_interrupted_runs` (R1):
/// wakes persisted before a crash deliver on the next boot.
pub async fn recover_pending_wakes(state: &AppState) {
    let sessions = match state.store.sessions_with_pending_wakes() {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "wake: boot sweep query failed");
            return;
        }
    };
    if sessions.is_empty() {
        return;
    }
    info!(sessions = sessions.len(), "wake: boot sweep delivering pending wakes");
    for key in sessions {
        deliver(state, &key).await;
    }
}

/// The hidden wake context (R2). The agent sees this; the transcript never
/// does — what the owner sees is only the agent's resulting report.
fn wake_prompt(batch: &[db::SessionWake]) -> String {
    let mut out = String::from("[Background event — not an owner message]\n");
    let shown = batch.len().min(STORM_CAP);
    if batch.len() == 1 {
        out.push_str(&format!("{}:\n{}\n", label(&batch[0].kind), clip(&batch[0].payload)));
    } else {
        out.push_str(&format!("{} events arrived while you were idle, in order:\n\n", batch.len()));
        for (i, w) in batch[..shown].iter().enumerate() {
            out.push_str(&format!("{}. {}:\n{}\n\n", i + 1, label(&w.kind), clip(&w.payload)));
        }
        if batch.len() > shown {
            out.push_str(&format!(
                "…plus {} more not shown — check your threads and tasks for the rest.\n",
                batch.len() - shown
            ));
        }
    }
    out.push_str(
        "\nAct on this now: continue what you were doing with it, and report the outcome \
         to the owner in your own words. If it changes nothing, a short note is enough.",
    );
    out
}

fn label(kind: &str) -> &str {
    match kind {
        "coworker_reply" => "A coworker replied to your message",
        "task_done" => "A background task you started finished",
        other => other,
    }
}

fn clip(payload: &str) -> String {
    if payload.chars().count() <= PAYLOAD_CLIP {
        return payload.to_string();
    }
    let cut: String = payload.chars().take(PAYLOAD_CLIP).collect();
    format!("{cut}… [clipped — read the source thread for the full text]")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wake(kind: &str, payload: &str) -> db::SessionWake {
        db::SessionWake {
            id: 1,
            session_key: "agent:x:web".into(),
            kind: kind.into(),
            payload: payload.into(),
            provenance: "[]".into(),
            handoff_depth: 0,
            attempts: 1,
        }
    }

    #[test]
    fn single_wake_prompt_is_direct() {
        let p = wake_prompt(&[wake("coworker_reply", "[Reply from Billy]\nDone.")]);
        assert!(p.starts_with("[Background event — not an owner message]"));
        assert!(p.contains("A coworker replied to your message"));
        assert!(p.contains("[Reply from Billy]\nDone."));
        assert!(p.contains("report the outcome"));
    }

    #[test]
    fn storm_degrades_to_summary() {
        let batch: Vec<_> = (0..30).map(|i| wake("task_done", &format!("t{i}"))).collect();
        let p = wake_prompt(&batch);
        assert!(p.contains("30 events arrived"));
        assert!(p.contains("t19"), "first 20 shown verbatim");
        assert!(!p.contains("t20\n"), "beyond the cap is summarized");
        assert!(p.contains("plus 10 more not shown"));
    }

    #[test]
    fn oversized_payload_clips_with_pointer() {
        let big = "x".repeat(5000);
        let p = wake_prompt(&[wake("coworker_reply", &big)]);
        assert!(p.contains("[clipped — read the source thread for the full text]"));
        assert!(p.len() < 3000);
    }
}
