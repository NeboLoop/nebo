//! Where an ask's one card goes and where its answer is delivered: the
//! Inbox (with a mobile push through the hub), the open chat, the wake
//! rail and the workflow engine.

use std::collections::HashMap;

use tracing::{info, warn};

use agent::harness::permissions::{Answer, Ask, AskSurfaces};

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

/// The answers a card offers, in order, with their labels.
fn offered(c: &PermissionAskCard) -> Vec<(&'static str, Answer)> {
    let mut answers = Vec::with_capacity(3);
    if c.allow_always {
        answers.push(("Allow always", Answer::AllowAlways));
    }
    if c.this_once {
        answers.push(("This once", Answer::ThisOnce));
    }
    answers.push(("No", Answer::No));
    answers
}

/// The card as a message in the owner's loop or phone conversation, which
/// can't show a card: what it wants, why it asked, and the answers it
/// takes, as reply options. It goes as `kind: ask` — the conversation's
/// question with answer chips, which the phone (as shipped, too) shows as
/// replies to tap; `ask_id` names the ask.
pub(crate) fn conversation_card(c: &PermissionAskCard) -> (String, HashMap<String, String>) {
    let labels: Vec<&str> = offered(c).iter().map(|(label, _)| *label).collect();
    let replies = match labels.as_slice() {
        [only] => only.to_string(),
        [rest @ .., last] => format!("{}, or {last}", rest.join(", ")),
        [] => String::new(),
    };
    let text = format!("{}: {} Reply {replies}.", title(c), body(c));
    let mut meta = HashMap::new();
    meta.insert("kind".to_string(), "ask".to_string());
    meta.insert("ask_id".to_string(), c.id.clone());
    meta.insert(
        "widgets".to_string(),
        serde_json::json!([{ "type": "options", "multiSelect": false, "options": labels }])
            .to_string(),
    );
    (text, meta)
}

/// The owner's reply in that conversation, read as one of the card's
/// answers: "Allow always" (or "always"), "This once" (or "allow", "approve",
/// "yes"), else No — an ask fails closed. An answer the card doesn't offer
/// becomes the nearest one it does: always → once, once → always (a card
/// for an employee's extra needs offers no "This once").
pub(crate) fn reply_answer(c: &PermissionAskCard, reply: &str) -> Answer {
    let words = reply.trim().trim_end_matches(['.', '!']).to_lowercase();
    let wanted = match words.as_str() {
        "allow always" | "approve always" | "always" => Answer::AllowAlways,
        "this once" | "once" | "allow once" | "approve once" | "allow" | "approve" | "yes" => {
            Answer::ThisOnce
        }
        _ => return Answer::No,
    };
    let answers: Vec<Answer> = offered(c).into_iter().map(|(_, a)| a).collect();
    let fallback = match wanted {
        Answer::AllowAlways => Answer::ThisOnce,
        _ => Answer::AllowAlways,
    };
    [wanted, fallback]
        .into_iter()
        .find(|a| answers.contains(a))
        .unwrap_or(Answer::No)
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

    fn remind(&self, ask: &Ask) {
        let state = &self.state;
        let c = card(state, ask);
        let (title, body) = (title(&c), body(&c));
        let id = inbox_id(&c.id);
        let user_id = state.store.ensure_local_user_id().unwrap_or_default();
        if let Err(e) = state.store.resurface_notification(&id, &user_id) {
            warn!(ask = %c.id, error = %e, "ask's Inbox row not brought back");
        }
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
        info!(ask = %c.id, "ask still open; the owner was reminded");
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

#[cfg(test)]
mod tests {
    use super::*;

    fn card(allow_always: bool, this_once: bool) -> PermissionAskCard {
        PermissionAskCard {
            id: "a1".into(),
            agent_id: "ava".into(),
            employee: "Ava".into(),
            session_key: "agent:ava:neboai".into(),
            sentence: "sending an email to pat@example.com".into(),
            reason: "It's the first time it would contact them.".into(),
            allow_always,
            this_once,
            status: "open".into(),
            answer: None,
            created_at: 0,
        }
    }

    #[test]
    fn a_reply_in_the_conversation_is_one_of_the_cards_answers() {
        let full = card(true, true);
        assert_eq!(reply_answer(&full, "Allow always"), Answer::AllowAlways);
        assert_eq!(reply_answer(&full, "approve always."), Answer::AllowAlways);
        assert_eq!(reply_answer(&full, "This once"), Answer::ThisOnce);
        assert_eq!(reply_answer(&full, "yes"), Answer::ThisOnce);
        assert_eq!(reply_answer(&full, "No"), Answer::No);
        assert_eq!(
            reply_answer(&full, "hmm, what is it for?"),
            Answer::No,
            "anything else fails closed"
        );
        // A widening card offers no Allow always; an employee's extra needs
        // offer no This once.
        assert_eq!(reply_answer(&card(false, true), "always"), Answer::ThisOnce);
        assert_eq!(reply_answer(&card(true, false), "yes"), Answer::AllowAlways);
    }

    #[test]
    fn the_card_as_a_message_offers_exactly_its_answers() {
        let (text, meta) = conversation_card(&card(true, true));
        assert_eq!(
            text,
            "Ava wants your OK: Sending an email to pat@example.com. It's the first time it would contact them. \
             Reply Allow always, This once, or No."
        );
        assert_eq!(
            meta["kind"], "ask",
            "the conversation's question with answer chips"
        );
        assert_eq!(meta["ask_id"], "a1");
        let widgets: serde_json::Value = serde_json::from_str(&meta["widgets"]).unwrap();
        assert_eq!(
            widgets[0]["options"],
            serde_json::json!(["Allow always", "This once", "No"])
        );
        let (text, _) = conversation_card(&card(false, true));
        assert!(text.ends_with("Reply This once, or No."), "{text}");
    }
}
