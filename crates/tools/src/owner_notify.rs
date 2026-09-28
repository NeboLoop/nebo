//! Owner notification emission — the ONE persist+broadcast sequence.
//!
//! Eleven hand-built copies existed (audit 2026-08-22) and had drifted on
//! three axes: payload field names (`actionUrl` vs `link`, `body` vs
//! `message`), payload completeness (`agentId`/`readAt`/`createdAt` present
//! at some sites only), and idempotency (deterministic ids written with the
//! non-deduped insert hard-error on retry). The comments at two of the copies
//! record the same empty-row bug being fixed one copy at a time.
//!
//! The TWO event names are deliberate and stay: they are the client's
//! loudness contract, not drift —
//! - `notification` (loud): Inbox row + toast + native OS banner
//!   (`listeners.ts` fires the Tauri notification client-side).
//! - `notification_created` (quiet): Inbox row only (the bell).

use serde_json::json;

/// One owner notification. `id` should be deterministic when the event is
/// retryable (e.g. `wf-approval:{run_id}`) — emission is idempotent.
pub struct OwnerNotification<'a> {
    pub id: &'a str,
    /// Inbox row type: "info" | "warning" | "error" | "approval" | ...
    pub kind: &'a str,
    pub title: &'a str,
    pub body: Option<&'a str>,
    pub action_url: Option<&'a str>,
    pub agent_id: Option<&'a str>,
    /// `true` = bell + toast + native OS banner; `false` = bell only.
    pub loud: bool,
}

/// Persist the Inbox row (idempotent) and broadcast the canonical payload.
/// Persistence is best-effort — the live broadcast is what surfaces the
/// notification, so a failed row write is logged, never fatal.
pub fn emit(
    store: &db::Store,
    broadcast: Option<&dyn Fn(&str, serde_json::Value)>,
    n: &OwnerNotification,
) {
    // notifications FK to users(id) — resolve the real local user ("" violates it).
    let user_id = store.ensure_local_user_id().unwrap_or_default();
    if let Err(e) = store.create_notification_if_not_exists(
        n.id,
        &user_id,
        n.kind,
        n.title,
        n.body,
        n.action_url,
        None,
        n.agent_id,
    ) {
        tracing::warn!(id = %n.id, error = %e, "owner notification row not persisted; broadcasting anyway");
    }

    if let Some(broadcast) = broadcast {
        broadcast(
            if n.loud { "notification" } else { "notification_created" },
            json!({
                "id": n.id,
                "type": n.kind,
                "title": n.title,
                "body": n.body,
                "actionUrl": n.action_url,
                "agentId": n.agent_id,
                "readAt": null,
                "createdAt": chrono::Utc::now().timestamp(),
            }),
        );
    }
}

/// Where an owner item opens: its Inbox row's `action_url`, and the `link`
/// of the item mirrored to the owner's hub Inbox and pushed to his phone.
/// A path in the app's own route space, so every surface reads the same
/// address: the desktop navigates to it, the web Inbox opens it through the
/// tunnel, and the phone maps it to its screen. Every item that is about a
/// place names that place here, never somewhere generic.
pub mod link {
    /// A conversation: `/{agent}/threads/{chat}` — where a card the owner
    /// answers sits. The employee's page when the chat is not known.
    pub fn chat(agent_id: &str, chat_id: &str) -> String {
        if chat_id.is_empty() {
            return format!("/{agent_id}");
        }
        format!("/{agent_id}/threads/{}", urlencoding::encode(chat_id))
    }

    /// The conversation a session is holding now (the session key is its
    /// name; see `db::Store::resolve_session_chat_id`).
    pub fn session_chat(store: &db::Store, agent_id: &str, session_key: &str) -> String {
        let chat_id = store
            .get_session_by_name(session_key)
            .ok()
            .flatten()
            .map(|s| store.resolve_session_chat_id(&s.id))
            .unwrap_or_default();
        chat(agent_id, &chat_id)
    }

    /// One run of an employee's work.
    pub fn run(agent_id: &str, run_id: &str) -> String {
        format!("/{agent_id}/runs/{}", urlencoding::encode(run_id))
    }

    /// An Inbox item opened in the Inbox's reader: a proposal the owner
    /// reads in full before he decides (`learn:<id>`).
    pub fn inbox_item(item_id: &str) -> String {
        format!("/inbox?m={}", urlencoding::encode(item_id))
    }

    /// An employee's connections, with `plugin` first when one is named:
    /// where "X needs Y connected" is fixed.
    pub fn accounts(agent_id: &str, plugin: Option<&str>) -> String {
        match plugin.filter(|p| !p.is_empty()) {
            Some(p) => format!("/{agent_id}/settings/accounts?plugin={}", urlencoding::encode(p)),
            None => format!("/{agent_id}/settings/accounts"),
        }
    }

    /// Settings → Updates, at one package.
    pub fn update(artifact_id: &str) -> String {
        format!("/settings/updates?artifact={}", urlencoding::encode(artifact_id))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn every_place_has_one_address() {
            assert_eq!(chat("emp", "c-1"), "/emp/threads/c-1");
            assert_eq!(chat("emp", ""), "/emp");
            assert_eq!(run("emp", "r 1"), "/emp/runs/r%201");
            assert_eq!(inbox_item("learn:abc"), "/inbox?m=learn%3Aabc");
            assert_eq!(accounts("emp", Some("gmail")), "/emp/settings/accounts?plugin=gmail");
            assert_eq!(accounts("emp", None), "/emp/settings/accounts");
            assert_eq!(update("google-sheets"), "/settings/updates?artifact=google-sheets");
        }

        #[test]
        fn a_session_opens_its_conversation() {
            let dir = tempfile::tempdir().unwrap();
            let store = db::Store::new(&dir.path().join("t.db").to_string_lossy()).unwrap();
            assert_eq!(session_chat(&store, "emp", "agent:emp:web"), "/emp", "no session: the employee");
            let s = store.create_session("s1", Some("agent:emp:web"), Some("agent"), Some("emp"), None).unwrap();
            let expected = format!("/emp/threads/{}", store.resolve_session_chat_id(&s.id));
            assert_eq!(session_chat(&store, "emp", "agent:emp:web"), expected);
        }
    }
}
