//! Where an ask's one card goes and where its answer is delivered: the
//! Inbox (with a mobile push through the hub), the open chat, the wake
//! rail and the workflow engine.

use std::collections::HashMap;

use tracing::{info, warn};

use agent::harness::permissions::{Answer, Ask, AskKind, AskSurfaces};

use crate::handlers::permissions::{PermissionAskCard, card};
use crate::state::AppState;

/// The Inbox row's id for an ask.
pub(crate) fn inbox_id(ask_id: &str) -> String {
    format!("permission-ask:{ask_id}")
}

/// The card's headline: "{Employee} wants your OK", or for a held send
/// "Did {Employee}'s message go out?".
fn title(c: &PermissionAskCard) -> String {
    if c.kind == AskKind::SendCheck.as_str() {
        return format!("Did {}'s message go out?", c.employee);
    }
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
pub(crate) fn offered(c: &PermissionAskCard) -> Vec<(&'static str, Answer)> {
    if c.kind == AskKind::SendCheck.as_str() {
        return vec![("It went out", Answer::Sent), ("It didn't go out", Answer::NotSent)];
    }
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

/// The card as a question the owner answers in words, on his call or in
/// a pinned bar: "OK to go ahead with sending an email to …? It's the first
/// time it would contact them." — or, for a held send, "Did … go out?".
pub(crate) fn question(c: &PermissionAskCard) -> String {
    let sentence = c.sentence.trim().trim_end_matches('.');
    if c.kind == AskKind::SendCheck.as_str() {
        return format!("Did {sentence} go out?");
    }
    format!("OK to go ahead with {sentence}? {}", c.reason.trim())
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

/// The card as the owner's item, the ONE place its Inbox row and its hub
/// copy (which the phone is pushed) are built: `f` gets the item — the same
/// id, words and link on every surface — and what only the hub copy
/// carries: the card's own answer calls, which the hub relays through the
/// tunnel, and the conversation that asked, so an answer by email (the hub
/// mails the owner every new item) comes back into it.
fn with_item<R>(
    c: &PermissionAskCard,
    f: impl FnOnce(&tools::owner_notify::OwnerNotification, serde_json::Value) -> R,
) -> R {
    let path = format!("/api/v1/permissions/asks/{}", c.id);
    let answer_path = format!("{path}/answer");
    let button = |label: &str, style: &str, answer: &str| {
        serde_json::json!({ "label": label, "style": style, "method": "POST",
            "path": answer_path, "body": { "answer": answer, "via": "mobile" } })
    };
    // The card's own answers: the same list the conversation card offers.
    let buttons: Vec<serde_json::Value> = offered(c)
        .into_iter()
        .map(|(label, answer)| {
            let style = match answer {
                Answer::AllowAlways | Answer::Sent => "primary",
                Answer::No | Answer::NotSent => "danger",
                Answer::ThisOnce => "default",
            };
            button(label, style, answer.as_str())
        })
        .collect();
    // The conversation whose own flow raised the ask: where its card sits,
    // and where a tap on the item opens. An ask no chat raised (a schedule,
    // a workflow, another employee's run) opens in the Inbox.
    let (id, title, body) = (inbox_id(&c.id), title(c), body(c));
    let chat_id = c.chat_id.clone();
    let link = if chat_id.is_empty() {
        tools::owner_notify::link::inbox_item(&id)
    } else {
        tools::owner_notify::link::chat(&c.agent_id, &chat_id)
    };
    let n = tools::owner_notify::OwnerNotification {
        id: &id,
        kind: "permission_ask",
        title: &title,
        body: Some(&body),
        action_url: Some(&link),
        agent_id: (!c.agent_id.is_empty()).then_some(c.agent_id.as_str()),
        loud: true,
    };
    f(
        &n,
        serde_json::json!({
            "chatId": chat_id,
            "actions": { "buttons": buttons, "status": { "method": "GET", "path": path } },
        }),
    )
}

/// Put the card before the owner: its Inbox row in this Inbox at once, and
/// its hub copy (the phone's push) gathered with the rest of its burst
/// ([`crate::ask_push`]). Idempotent on the item id.
fn surface(state: &AppState, c: &PermissionAskCard) {
    let queued = with_item(c, |n, hub_only| {
        tools::owner_notify::emit(&state.store, Some(&|ev, payload| state.hub.broadcast(ev, payload)), n);
        queued(state, c, n.hub_item(hub_only))
    });
    crate::ask_push::global(state).queue(vec![queued]);
}

/// The ask as it waits for its push.
fn queued(state: &AppState, c: &PermissionAskCard, item: serde_json::Value) -> crate::ask_push::Queued {
    crate::ask_push::Queued {
        ask_id: c.id.clone(),
        employee: c.employee.clone(),
        bot: crate::handlers::permissions::employee_name(state, ""),
        sentence: c.sentence.clone(),
        item,
    }
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
        surface(state, &c);
        state.hub.broadcast("permission_ask", serde_json::to_value(&c).unwrap_or_default());
        crate::handlers::asks::permission_raised(state, &c);
    }

    fn remind(&self, ask: &Ask) {
        let state = &self.state;
        let c = card(state, ask);
        let id = inbox_id(&c.id);
        let user_id = state.store.ensure_local_user_id().unwrap_or_default();
        if let Err(e) = state.store.resurface_notification(&id, &user_id) {
            warn!(ask = %c.id, error = %e, "ask's Inbox row not brought back");
        }
        surface(state, &c);
        state.hub.broadcast("permission_ask", serde_json::to_value(&c).unwrap_or_default());
        crate::handlers::asks::permission_raised(state, &c);
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
        crate::ask_push::global(state).settled(&c.id);
        state.hub.broadcast("permission_ask_resolved", serde_json::to_value(&c).unwrap_or_default());
        crate::handlers::asks::spawn_changed(state);
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
/// best-effort, so a reconnect is where a missed one heals. All of them go
/// out together, as ONE hub item when there are several: a bot that comes
/// up with a backlog (29 asks open across an upgrade) pushes the owner's
/// phone once, and the same backlog on the next reconnect is the same item
/// and pushes nothing. Nothing here answers or withdraws an ask.
pub(crate) fn reconcile_inbox(state: &AppState) {
    match state.permission_asks.open(None) {
        Ok(asks) => {
            let queued = asks
                .iter()
                .map(|ask| {
                    let c = card(state, ask);
                    with_item(&c, |n, hub_only| queued(state, &c, n.hub_item(hub_only)))
                })
                .collect();
            crate::ask_push::global(state).queue(queued);
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
            kind: "permission".into(),
            agent_id: "ava".into(),
            employee: "Ava".into(),
            session_key: "agent:ava:neboai".into(),
            sentence: "sending an email to pat@example.com".into(),
            reason: "It's the first time it would contact them.".into(),
            allow_always,
            this_once,
            status: "open".into(),
            answer: None,
            chat_id: String::new(),
            created_at: 0,
        }
    }

    /// A tap on an ask, anywhere — the Inbox row here, the hub row, the
    /// phone's push — opens the conversation whose own flow raised it, where
    /// its card waits; an ask no chat raised opens in the Inbox, never in a
    /// chat. One link, carried by the row and the hub copy alike.
    #[test]
    fn an_ask_opens_the_chat_that_raised_it_or_the_inbox() {
        let mut c = card(true, true);
        let (link, item) = with_item(&c, |n, extra| (n.link(), n.hub_item(extra)));
        assert_eq!(link, "/inbox?m=permission-ask%3Aa1");
        assert_eq!(item["link"], link);
        assert_eq!(item["chatId"], "", "no chat to open");

        c.chat_id = "c-9".into();
        let (link, item) = with_item(&c, |n, extra| (n.link(), n.hub_item(extra)));
        assert_eq!(link, "/ava/threads/c-9");
        assert_eq!(item["link"], link, "the hub copy opens where the row opens");
        assert_eq!(item["id"], "permission-ask:a1");
        assert_eq!(item["type"], "permission_ask");
        assert_eq!(item["agentId"], "ava");
        assert_eq!(item["chatId"], "c-9");
        assert_eq!(item["actions"]["buttons"].as_array().map(Vec::len), Some(3));
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
