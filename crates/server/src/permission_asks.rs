//! Where an ask's one card goes and where its answer is delivered: the
//! Inbox (with a mobile push through the hub), the open chat, the wake
//! rail and the workflow engine.

use tracing::{info, warn};

use agent::harness::permissions::{Ask, AskSurfaces};

use crate::handlers::permissions::{PermissionAskCard, card};
use crate::state::AppState;

/// The Inbox row's id for an ask.
pub(crate) fn inbox_id(ask_id: &str) -> String {
    format!("permission-ask:{ask_id}")
}

/// The card's headline: "{Employee} wants your OK".
fn title(c: &PermissionAskCard) -> String {
    format!("{} wants your OK", c.employee)
}

/// What it wants to do and why it asked.
fn body(c: &PermissionAskCard) -> String {
    let mut sentence = c.sentence.clone();
    if let Some(first) = sentence.get(..1) {
        sentence.replace_range(..1, &first.to_uppercase());
    }
    format!("{sentence}. {}", c.reason)
}

/// Push the card to the owner's hub Inbox, which reaches the phone. The
/// item carries its own answer calls; the hub relays them through the
/// tunnel. Idempotent on the item id.
pub(crate) fn push_to_inbox(state: &AppState, c: &PermissionAskCard) {
    let path = format!("/api/v1/permissions/asks/{}", c.id);
    let answer_path = format!("{path}/answer");
    let button = |label: &str, style: &str, answer: &str| {
        serde_json::json!({ "label": label, "style": style, "method": "POST",
            "path": answer_path, "body": { "answer": answer, "via": "mobile" } })
    };
    let mut buttons = Vec::with_capacity(3);
    if c.allow_always {
        buttons.push(button("Allow always", "primary", "allow_always"));
    }
    if c.this_once {
        buttons.push(button("This once", "default", "this_once"));
    }
    buttons.push(button("No", "danger", "no"));
    crate::codes::push_inbox(
        state,
        serde_json::json!({
            "id": inbox_id(&c.id),
            "type": "permission_ask",
            "title": title(c),
            "body": body(c),
            "link": format!("/{}", c.agent_id),
            "actions": { "buttons": buttons, "status": { "method": "GET", "path": path } },
        }),
    );
}

/// The server's surfaces for asks. Holds the app state: the hub, the
/// store, the wake rail.
pub(crate) struct OwnerSurfaces {
    pub state: AppState,
}

impl AskSurfaces for OwnerSurfaces {
    fn card(&self, ask: &Ask) {
        let state = &self.state;
        let c = card(state, ask);
        let (title, body) = (title(&c), body(&c));
        let id = inbox_id(&c.id);
        tools::owner_notify::emit(
            &state.store,
            Some(&|ev, payload| state.hub.broadcast(ev, payload)),
            &tools::owner_notify::OwnerNotification {
                id: &id,
                kind: "permission_ask",
                title: &title,
                body: Some(&body),
                action_url: None,
                agent_id: (!c.agent_id.is_empty()).then_some(c.agent_id.as_str()),
                loud: true,
            },
        );
        push_to_inbox(state, &c);
        state.hub.broadcast("permission_ask", serde_json::to_value(&c).unwrap_or_default());
    }

    fn resolved(&self, ask: &Ask) {
        let state = &self.state;
        let c = card(state, ask);
        let id = inbox_id(&c.id);
        let user_id = state.store.ensure_local_user_id().unwrap_or_default();
        if let Err(e) = state.store.mark_notification_read(&id, &user_id) {
            warn!(ask = %c.id, error = %e, "ask's Inbox row not marked read");
        }
        crate::codes::push_inbox(state, serde_json::json!({ "id": id, "resolved": true }));
        state.hub.broadcast("permission_ask_resolved", serde_json::to_value(&c).unwrap_or_default());
    }

    fn notify(&self, session_key: &str, text: &str) {
        crate::wake::enqueue(
            &self.state,
            session_key,
            agent::harness::delegation::notify::WAKE_KIND,
            text,
            &[],
            0,
        );
    }

    fn release_run(&self, run_id: &str, allowed: bool) {
        match self.state.store.engine_answer_wait(run_id, allowed) {
            Ok(_) => info!(run_id, allowed, "ask answered; parked workflow run released"),
            Err(e) => warn!(run_id, error = %e, "ask answered but the parked run could not be released"),
        }
    }
}

/// The asks still open, pushed again to the hub Inbox: pushes are
/// best-effort, so a reconnect is where a missed one heals.
pub(crate) fn reconcile_inbox(state: &AppState) {
    match state.permission_asks.open(None) {
        Ok(asks) => {
            for ask in &asks {
                push_to_inbox(state, &card(state, ask));
            }
        }
        Err(e) => warn!(error = %e, "owner inbox reconcile: asks unreadable"),
    }
}
