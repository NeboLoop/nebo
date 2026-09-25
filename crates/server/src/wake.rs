//! Session wake rail — "the turn ends, the attention doesn't"
//! (docs/prd/session-wake-rail.md).
//!
//! Anything a session started or asked to be woken for reaches that session:
//! same session, full context, labeled as machine, exactly once, even across
//! a restart. Producers call [`enqueue`] — persist first (the write-ahead
//! queue), then attempt delivery. [`deliver`] is the ONE delivery: every
//! pending update becomes a notification row in the session's conversation
//! (`agent::harness::delegation::notify`). A running turn hears the rows at
//! its next step; an idle session gets a turn that starts from them. The
//! owner only ever sees the employee's resulting message.

use tracing::{info, warn};

use crate::chat_dispatch::{ChatConfig, run_chat};
use crate::reply_route::ReplyRoute;
use crate::state::AppState;
use agent::harness::delegation::notify;
use types::provenance::ProvenanceClass;

/// A single update is clipped to this many chars in its row — the full text
/// stays in the queue row / source thread.
const PAYLOAD_CLIP: usize = 2000;

/// Producer entry: persist the wake, then try to deliver it. Never blocks the
/// producer on the woken run. An update for a helper's session goes to the
/// helper registry, which holds the helper; one whose helper has finished
/// and been let go goes to that helper's parent, so it is never lost in a
/// thread nobody will read.
pub fn enqueue(
    state: &AppState,
    session_key: &str,
    kind: &str,
    payload: &str,
    provenance: &[ProvenanceClass],
    handoff_depth: u8,
) {
    let mut session_key = session_key;
    while let Some((parent, task_id)) = agent::harness::delegation::split_helper_key(session_key) {
        if state.helpers.notify(session_key, &row_text(kind, payload), provenance) {
            return;
        }
        info!(session = %session_key, task_id, "update for a finished helper goes to its parent");
        session_key = parent;
    }
    let prov = serde_json::to_string(provenance).unwrap_or_else(|_| "[]".to_string());
    if let Err(e) =
        state
            .store
            .engine_enqueue_wake(session_key, kind, payload, &prov, handoff_depth)
    {
        warn!(error = %e, session = %session_key, "wake: failed to persist — payload lost");
        return;
    }
    let state = state.clone();
    let key = session_key.to_string();
    tokio::spawn(async move { deliver(&state, &key).await });
}

/// Write a session's pending updates into its conversation as notification
/// rows, then, when no turn is running there, start one that hears them.
pub async fn deliver(state: &AppState, session_key: &str) {
    // Claiming, writing the rows and stamping them delivered is one step:
    // two deliveries racing (two replies landing together) would otherwise
    // both claim the same updates and write each row twice.
    let claim = claimed().lock().await;
    let (batch, poisoned) = match state.store.engine_claim_session_events(session_key, now()) {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, session = %session_key, "wake: claim failed");
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
        return;
    }

    // Union the batch's taint and take the deepest handoff chain: the turn
    // that hears them is decided at the gates exactly as their origins
    // demand, and further coworker sends stay bounded (R6).
    let mut seed_taint: Vec<ProvenanceClass> = Vec::new();
    let mut handoff_depth: u8 = 0;
    let mut written = Vec::new();
    for w in &batch {
        let taint = serde_json::from_str::<Vec<ProvenanceClass>>(&w.provenance).unwrap_or_default();
        match notify::append_row(state.harness.sessions(), session_key, &row_text(&w.kind, &w.payload), &taint) {
            Ok(()) => written.push(w.id),
            Err(e) => {
                warn!(error = %e, session = %session_key, kind = %w.kind, "wake: row not written; it redelivers");
                continue;
            }
        }
        handoff_depth = handoff_depth.max(w.handoff_depth.clamp(0, u8::MAX as i64) as u8);
        for class in taint {
            if !seed_taint.contains(&class) {
                seed_taint.push(class);
            }
        }
    }
    if written.is_empty() {
        return;
    }
    if let Err(e) = state.store.engine_complete_events(&written, now()) {
        warn!(error = %e, session = %session_key, "wake: failed to stamp delivered");
    }
    drop(claim);
    // A running turn hears the rows at its next step, or on the turn it
    // hands them to when they land after its last one. A turn already
    // closing has made its last check for input: the rows need a turn of
    // their own, which waits for its slot.
    if state.harness.hears_new_rows(session_key) {
        return;
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

    info!(session = %session_key, count = written.len(), "wake: waking session");
    // The woken turn replies where the session's work came from.
    let route = crate::reply_route::of(state, session_key);
    if let Some(ReplyRoute::Coworker(route)) = route {
        if let Err(e) = crate::coworker::run_in_thread(state, session_key, route, String::new(), None, seed_taint).await {
            warn!(error = %e, session = %session_key, "wake: coworker thread not woken; its rows wait for its next message");
        }
        return;
    }
    let config = ChatConfig {
        session_key: session_key.to_string(),
        // The conversation already holds what the turn hears.
        prompt: String::new(),
        user_id: String::new(),
        channel,
        origin: tools::Origin::System,
        door: types::permissions::Door::Chat,
        agent_id,
        cancel_token: tokio_util::sync::CancellationToken::new(),
        lane: types::constants::lanes::COMM.to_string(),
        comm_reply: route.as_ref().and_then(ReplyRoute::comm_reply),
        entity_config,
        images: vec![],
        attachments: vec![],
        entity_name: String::new(),
        origin_agent_id: None,
        mention_context: None,
        tool_scope: None,
        plan_mode: false,
        channel_ctx: None,
        handoff_depth,
        seed_taint,
        tool_allowlist: None,
        hidden_prompt: false,
        coworker: None,
        audience: None,
        cwd: None,
        model_override: None,
    };
    run_chat(state, config).await;
}

/// Run-completion hook — called at the end of every chat run's lane task:
/// anything still pending for the session (a row that could not be written)
/// is delivered again.
pub fn on_run_finished(state: &AppState, session_key: &str) {
    let has_pending = matches!(
        state.store.engine_sessions_with_pending(),
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
    let sessions = match state.store.engine_sessions_with_pending() {
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

/// The one lock over claiming a session's updates and writing them.
fn claimed() -> &'static tokio::sync::Mutex<()> {
    static CLAIM: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    CLAIM.get_or_init(|| tokio::sync::Mutex::new(()))
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// The row an update becomes. A helper's notification is already in the one
/// format and is never clipped (a helper's result is capped where it is
/// collected); any other update is labeled and clipped.
fn row_text(kind: &str, payload: &str) -> String {
    if kind == notify::WAKE_KIND {
        return payload.to_string();
    }
    notify::render_update(label(kind), &clip(payload))
}

fn label(kind: &str) -> &str {
    match kind {
        "coworker_reply" => "A coworker replied to your message",
        "team_reply" => "A teammate replied to your team post",
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

    fn wake(kind: &str, payload: &str) -> db::EngineEvent {
        db::EngineEvent {
            id: 1,
            kind: kind.into(),
            target_type: "session".into(),
            target_id: "agent:x:web".into(),
            payload: payload.into(),
            channel: String::new(),
            r#ref: String::new(),
            idem_key: "wake:test".into(),
            provenance: "[]".into(),
            handoff_depth: 0,
            retention: "transient".into(),
            due_at: None,
            schedule: None,
            attempts: 1,
            created_at: 0,
            delivered_at: None,
        }
    }

    #[test]
    fn an_update_is_a_labeled_notification() {
        let w = wake("coworker_reply", "[Reply from Billy]\nDone.");
        let row = row_text(&w.kind, &w.payload);
        assert!(row.starts_with("<system-reminder>\n[Notification: not a message from the owner]"), "{row}");
        assert!(row.contains("A coworker replied to your message:\n[Reply from Billy]\nDone."));
    }

    /// A helper's notification is written in its one format, unclipped, with
    /// no second header around it.
    #[test]
    fn a_helper_notification_is_written_as_it_is() {
        let n = format!(
            "<system-reminder>\n[Notification: not a message from the owner]\nhelper h-1 \"read logs\": done\n{}\n</system-reminder>",
            "x".repeat(5000)
        );
        assert_eq!(row_text(notify::WAKE_KIND, &n), n);
    }

    #[test]
    fn oversized_payload_clips_with_pointer() {
        let big = "x".repeat(5000);
        let row = row_text("coworker_reply", &big);
        assert!(row.contains("[clipped — read the source thread for the full text]"));
        assert!(row.len() < 3000);
    }

}
